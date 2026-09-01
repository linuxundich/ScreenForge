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

/// Renders `doc` at its configured export size and writes it to `path` in
/// its configured format. Intended to run off the main thread — takes only
/// `Send` types (see [`DecodedImage`]) and builds every `cairo::ImageSurface`
/// it needs internally, so none has to be shared with the caller's thread.
pub fn render_and_write(doc: &Document, decoded_images: &HashMap<Uuid, DecodedImage>, path: &Path) -> Result<(), ExportError> {
    let surfaces: HashMap<Uuid, cairo::ImageSurface> = decoded_images
        .iter()
        .map(|(id, image)| Ok((*id, import::surface_from_decoded(image)?)))
        .collect::<Result<_, import::ImportError>>()?;

    let mut target =
        cairo::ImageSurface::create(cairo::Format::ARgb32, doc.canvas.export_width as i32, doc.canvas.export_height as i32)?;
    screenforge_core::render::compose(doc, &target, 1.0, &surfaces)?;

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
