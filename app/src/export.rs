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
    let out_height = ((doc.canvas.export_height as f64) * scale).round().max(1.0) as u32;

    let mut target = cairo::ImageSurface::create(cairo::Format::ARgb32, out_width as i32, out_height as i32)?;
    // A fresh, one-shot cache: export renders this document exactly once,
    // so there's nothing to gain from reusing shadow bitmaps across calls
    // the way the interactive preview does (see `Canvas`'s long-lived
    // one) — every shadow just misses once and renders at full quality.
    let shadow_cache = screenforge_core::shadow_cache::ShadowCache::new();
    screenforge_core::render::compose(doc, &target, scale, &surfaces, background_surface.as_ref(), &shadow_cache)?;

    match doc.canvas.export_format {
        ExportFormat::Png => {
            let mut file = File::create(path)?;
            target.write_to_png(&mut file)?;
        }
        ExportFormat::Jpeg => {
            let rgba = surface_to_rgba_image(&mut target)?;
            let mut file = File::create(path)?;
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut file, doc.canvas.export_quality);
            encoder.encode_image(&rgba)?;
        }
        ExportFormat::WebP => {
            let rgba = surface_to_rgba_image(&mut target)?;
            let file = File::create(path)?;
            let encoder = image::codecs::webp::WebPEncoder::new_lossless(file);
            encoder.write_image(rgba.as_raw(), rgba.width(), rgba.height(), image::ExtendedColorType::Rgba8)?;
        }
        ExportFormat::Avif => {
            let rgba = surface_to_rgba_image(&mut target)?;
            let file = File::create(path)?;
            let quality = doc.canvas.export_quality.clamp(1, 100);
            let encoder = image::codecs::avif::AvifEncoder::new_with_speed_quality(file, 4, quality);
            encoder.write_image(rgba.as_raw(), rgba.width(), rgba.height(), image::ExtendedColorType::Rgba8)?;
        }
    }
    Ok(())
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
    fn every_export_format_writes_a_non_empty_file() {
        for format in [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::WebP, ExportFormat::Avif] {
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
