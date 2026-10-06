//! Full-resolution render + encode. Runs on a background thread
//! ([`gio::spawn_blocking`] in `main.rs`) so the UI never blocks on export —
//! everything here is plain data in and a file on disk out, no GTK types
//! cross the thread boundary (see `import.rs` for why that matters).

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

use gtk4::cairo;
use image::{ImageEncoder, RgbaImage};
use screenforge_core::model::{Document, ExportFormat};
use thiserror::Error;
use uuid::Uuid;

use crate::import::{self, DecodedImage};

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("render error: {0}")]
    Render(#[from] screenforge_core::render::RenderError),
    #[error("cairo error: {0}")]
    Cairo(#[from] cairo::Error),
    #[error("could not read rendered pixels: {0}")]
    Borrow(#[from] cairo::BorrowError),
    #[error("could not write file: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not write PNG: {0}")]
    Png(#[from] cairo::IoError),
    #[error("could not encode image: {0}")]
    Encode(#[from] image::ImageError),
    #[error("could not build source surface: {0}")]
    Import(#[from] import::ImportError),
    #[error("video encoding failed: {0}")]
    Video(String),
}

/// Renders `doc` scaled to its configured target export width (height
/// following proportionally — see `CanvasSettings::export_target_width`)
/// and writes it to `path` in its configured format. Intended to run off
/// the main thread — takes only `Send` types (see [`DecodedImage`]) and
/// builds every `cairo::ImageSurface` it needs internally, so none has to
/// be shared with the caller's thread. `background_image` is only
/// consulted when `doc.background` is `Background::Image`.
pub fn render_and_write(
    doc: &Document,
    decoded_images: &HashMap<Uuid, DecodedImage>,
    background_image: Option<&DecodedImage>,
    path: &Path,
) -> Result<(), ExportError> {
    if doc.canvas.export_format == ExportFormat::WebM {
        return crate::video::render_and_write(doc, decoded_images, background_image, path);
    }
    let target = render_surface(doc, decoded_images, background_image)?;
    let slices = doc.canvas.slices.max(1);
    let parts = split(target, slices)?;

    if doc.canvas.export_format == ExportFormat::Pdf {
        // One page per slice, each the size of its slice in points.
        let (w, h) = (parts[0].width() as f64, parts[0].height() as f64);
        let pdf = cairo::PdfSurface::new(w, h, path)?;
        let ctx = cairo::Context::new(&pdf)?;
        for part in &parts {
            pdf.set_size(part.width() as f64, part.height() as f64)?;
            ctx.set_source_surface(part, 0.0, 0.0)?;
            ctx.paint()?;
            ctx.show_page()?;
        }
        drop(ctx);
        pdf.finish();
        return Ok(());
    }

    for (index, mut part) in parts.into_iter().enumerate() {
        let part_path = if slices > 1 { numbered_path(path, index + 1) } else { path.to_path_buf() };
        write_image(&mut part, doc.canvas.export_format, doc.canvas.export_quality, &part_path)?;
    }
    Ok(())
}

/// The sizes [`render_store_set`] writes: (file name part, width, height)
/// of one image; with "Split Into" each becomes that many images.
pub const STORE_SET: [(&str, u32, u32); 3] = [("iphone-6.9", 1320, 2868), ("ipad-13", 2064, 2752), ("google-play", 1080, 1920)];

/// Writes `doc` into `dir` once per [`STORE_SET`] size, named
/// `{name}-{size}.{ext}` (numbered when split). PDF and WebM fall back to
/// PNG. Returns how many image files were written.
pub fn render_store_set(
    doc: &Document,
    decoded_images: &HashMap<Uuid, DecodedImage>,
    background_image: Option<&DecodedImage>,
    dir: &Path,
    name: &str,
) -> Result<u32, ExportError> {
    let format = match doc.canvas.export_format {
        ExportFormat::Pdf | ExportFormat::WebM => ExportFormat::Png,
        other => other,
    };
    let extension = match format {
        ExportFormat::Jpeg => "jpg",
        ExportFormat::WebP => "webp",
        ExportFormat::Avif => "avif",
        _ => "png",
    };
    let mut written = 0;
    for (tag, w, h) in STORE_SET {
        let mut sized = doc.clone();
        let slices = sized.canvas.slices.max(1);
        sized.canvas.aspect = Some((w, h));
        sized.canvas.export_target_width = w * slices;
        sized.canvas.export_format = format;
        screenforge_core::layout::fit_canvas_to_content(&mut sized);
        render_and_write(&sized, decoded_images, background_image, &dir.join(format!("{name}-{tag}.{extension}")))?;
        written += slices;
    }
    Ok(written)
}

/// `path` with `-n` appended to the file stem: `shots.png` → `shots-2.png`.
pub fn numbered_path(path: &Path, n: usize) -> std::path::PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = match path.extension() {
        Some(ext) => format!("{stem}-{n}.{}", ext.to_string_lossy()),
        None => format!("{stem}-{n}"),
    };
    path.with_file_name(name)
}

/// Cuts `surface` into `n` equally wide vertical strips (the last one takes
/// the rounding remainder). `n == 1` returns the surface itself.
fn split(surface: cairo::ImageSurface, n: u32) -> Result<Vec<cairo::ImageSurface>, ExportError> {
    if n <= 1 {
        return Ok(vec![surface]);
    }
    let (w, h) = (surface.width(), surface.height());
    let mut parts = Vec::new();
    for k in 0..n as i32 {
        let x0 = (w as f64 * k as f64 / n as f64).round() as i32;
        let x1 = (w as f64 * (k + 1) as f64 / n as f64).round() as i32;
        let part = cairo::ImageSurface::create(cairo::Format::ARgb32, (x1 - x0).max(1), h)?;
        let ctx = cairo::Context::new(&part)?;
        ctx.set_source_surface(&surface, -x0 as f64, 0.0)?;
        ctx.paint()?;
        drop(ctx);
        parts.push(part);
    }
    Ok(parts)
}

fn write_image(target: &mut cairo::ImageSurface, format: ExportFormat, quality: u8, path: &Path) -> Result<(), ExportError> {
    match format {
        ExportFormat::Png => {
            let mut file = File::create(path)?;
            target.write_to_png(&mut file)?;
        }
        ExportFormat::Jpeg => {
            // JPEG has no alpha: flatten a transparent export onto white.
            let rgba = surface_to_rgba_image(&mut flattened_on_white(target)?)?;
            let mut file = File::create(path)?;
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut file, quality);
            encoder.encode_image(&rgba)?;
        }
        ExportFormat::WebP => {
            let rgba = surface_to_rgba_image(target)?;
            let file = File::create(path)?;
            let encoder = image::codecs::webp::WebPEncoder::new_lossless(file);
            encoder.write_image(rgba.as_raw(), rgba.width(), rgba.height(), image::ExtendedColorType::Rgba8)?;
        }
        ExportFormat::Avif => {
            let rgba = surface_to_rgba_image(target)?;
            let file = File::create(path)?;
            let encoder = image::codecs::avif::AvifEncoder::new_with_speed_quality(file, 4, quality.clamp(1, 100));
            encoder.write_image(rgba.as_raw(), rgba.width(), rgba.height(), image::ExtendedColorType::Rgba8)?;
        }
        ExportFormat::Pdf | ExportFormat::WebM => unreachable!("written by render_and_write"),
    }
    Ok(())
}

/// A copy of `surface` composited over opaque white.
fn flattened_on_white(surface: &cairo::ImageSurface) -> Result<cairo::ImageSurface, ExportError> {
    let out = cairo::ImageSurface::create(cairo::Format::ARgb32, surface.width(), surface.height())?;
    let ctx = cairo::Context::new(&out)?;
    ctx.set_source_rgb(1.0, 1.0, 1.0);
    ctx.paint()?;
    ctx.set_source_surface(surface, 0.0, 0.0)?;
    ctx.paint()?;
    drop(ctx);
    Ok(out)
}

/// Renders the export at its target size and returns the raw premultiplied
/// BGRA bytes (Cairo's ARGB32 on little-endian), width, height and stride —
/// plain data, so it can cross from a background thread to the UI thread
/// for the clipboard.
pub fn render_pixels(
    doc: &Document,
    decoded_images: &HashMap<Uuid, DecodedImage>,
    background_image: Option<&DecodedImage>,
) -> Result<(Vec<u8>, i32, i32, usize), ExportError> {
    let mut surface = render_surface(doc, decoded_images, background_image)?;
    surface.flush();
    let (width, height, stride) = (surface.width(), surface.height(), surface.stride() as usize);
    let data = surface.data()?.to_vec();
    Ok((data, width, height, stride))
}

/// Writes the export as PNG to `path` regardless of the chosen format —
/// what drag-and-drop out of the window hands over.
pub fn render_png(
    doc: &Document,
    decoded_images: &HashMap<Uuid, DecodedImage>,
    background_image: Option<&DecodedImage>,
    path: &Path,
) -> Result<(), ExportError> {
    let surface = render_surface(doc, decoded_images, background_image)?;
    let mut file = File::create(path)?;
    surface.write_to_png(&mut file)?;
    Ok(())
}

fn render_surface(
    doc: &Document,
    decoded_images: &HashMap<Uuid, DecodedImage>,
    background_image: Option<&DecodedImage>,
) -> Result<cairo::ImageSurface, ExportError> {
    let surfaces: HashMap<Uuid, cairo::ImageSurface> = decoded_images
        .iter()
        .map(|(id, image)| Ok((*id, import::surface_from_decoded(image)?)))
        .collect::<Result<_, import::ImportError>>()?;
    let background_surface = background_image.map(import::surface_from_decoded).transpose()?;

    // The canvas's own width/height are the composition's native, content-
    // fitted size (see `screenforge_core::layout::fit_canvas_to_content`);
    // scaling to the user-chosen target width — instead of rendering
    // straight at export_width/export_height — is what lets that target be
    // freely edited without ever cropping or distorting the content.
    let scale = doc.canvas.export_target_width as f64 / doc.canvas.export_width.max(1) as f64;
    let out_width = doc.canvas.export_target_width.max(1);
    // With a fixed format the height follows the exact aspect ratio (app
    // stores reject a pixel off); otherwise the content-fitted canvas.
    let out_height = match doc.canvas.aspect {
        Some((aw, ah)) if aw > 0 && ah > 0 => {
            (out_width as f64 * ah as f64 / (aw * doc.canvas.slices.max(1)) as f64).round().max(1.0) as u32
        }
        _ => ((doc.canvas.export_height as f64) * scale).round().max(1.0) as u32,
    };
    let scale_y = out_height as f64 / doc.canvas.export_height.max(1) as f64;

    let target = cairo::ImageSurface::create(cairo::Format::ARgb32, out_width as i32, out_height as i32)?;
    // A fresh, one-shot cache: export renders this document exactly once,
    // so there's nothing to gain from reusing shadow bitmaps across calls
    // the way the interactive preview does (see `Canvas`'s long-lived
    // one) — every shadow just misses once and renders at full quality.
    let shadow_cache = screenforge_core::shadow_cache::ShadowCache::new();
    let background_cache = screenforge_core::background_cache::BackgroundCache::new();
    screenforge_core::render::compose_stretched(doc, &target, scale, scale_y, &surfaces, background_surface.as_ref(), &shadow_cache, &background_cache)?;

    Ok(target)
}

/// Unpremultiplies and channel-swaps a rendered ARGB32 surface into an
/// `image`-crate RGBA buffer — Cairo has no JPEG/WebP encoder, so this is
/// the handoff point to the `image` crate for those two formats (PNG uses
/// Cairo's own writer directly and never needs this).
fn surface_to_rgba_image(surface: &mut cairo::ImageSurface) -> Result<RgbaImage, cairo::BorrowError> {
    let width = surface.width() as u32;
    let height = surface.height() as u32;
    let stride = surface.stride();
    let data = surface.data()?;

    let mut out = RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let idx = (y as i32 * stride + x as i32 * 4) as usize;
            let b = data[idx] as f64;
            let g = data[idx + 1] as f64;
            let r = data[idx + 2] as f64;
            let a = data[idx + 3] as f64;
            let (ur, ug, ub) = if a > 0.0 {
                ((r * 255.0 / a).round() as u8, (g * 255.0 / a).round() as u8, (b * 255.0 / a).round() as u8)
            } else {
                (0, 0, 0)
            };
            out.put_pixel(x, y, image::Rgba([ur, ug, ub, a as u8]));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenforge_core::model::Document;

    /// Each format's encoder path, exercised end-to-end against an empty
    /// (background-only) document — enough to catch an encoder call that
    /// panics or a file that never gets written, without needing any real
    /// screenshots decoded.
    #[test]
    fn a_store_set_writes_every_size_with_its_aspect() {
        let dir = std::env::temp_dir().join("screenforge-store-set-test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut doc = Document::new();
        let image = DecodedImage { bytes: vec![255u8; 40 * 80 * 4].into(), width: 40, height: 80 };
        let element = screenforge_core::model::ScreenshotElement::new(screenforge_core::model::ImageSource::Path("x.png".into()), 40.0, 80.0);
        let mut images = HashMap::new();
        images.insert(element.id, image);
        doc.elements.push(element);
        let count = render_store_set(&doc, &images, None, &dir, "shots").unwrap();
        assert_eq!(count, 3);
        for (tag, w, h) in STORE_SET {
            let mut file = File::open(dir.join(format!("shots-{tag}.png"))).unwrap();
            let surface = cairo::ImageSurface::create_from_png(&mut file).unwrap();
            assert_eq!(surface.width() as u32, w);
            assert_eq!(surface.height() as u32, h, "{tag}");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn slices_are_written_as_numbered_files() {
        let mut doc = Document::new();
        doc.canvas.export_width = 30;
        doc.canvas.export_height = 20;
        doc.canvas.export_target_width = 30;
        doc.canvas.slices = 3;
        let path = std::env::temp_dir().join("screenforge-slices-test.png");
        render_and_write(&doc, &HashMap::new(), None, &path).unwrap();
        for n in 1..=3 {
            let part = numbered_path(&path, n);
            let mut file = File::open(&part).unwrap();
            let surface = cairo::ImageSurface::create_from_png(&mut file).unwrap();
            assert_eq!((surface.width(), surface.height()), (10, 20));
            std::fs::remove_file(part).ok();
        }
    }

    #[test]
    fn every_export_format_writes_a_non_empty_file() {
        for format in [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::WebP, ExportFormat::Avif, ExportFormat::Pdf] {
            let mut doc = Document::new();
            doc.canvas.export_width = 32;
            doc.canvas.export_height = 24;
            doc.canvas.export_target_width = 32; // no scaling -- keep the test surfaces tiny
            doc.canvas.export_format = format;

            let path = std::env::temp_dir().join(format!("screenforge-export-test-{format:?}.bin"));
            render_and_write(&doc, &HashMap::new(), None, &path).unwrap_or_else(|err| panic!("{format:?} export failed: {err}"));

            let bytes = std::fs::read(&path).unwrap();
            assert!(!bytes.is_empty(), "{format:?} export wrote an empty file");
            let _ = std::fs::remove_file(&path);
        }
    }
}
