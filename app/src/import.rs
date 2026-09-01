//! File-dialog and drag-and-drop import, sharing one decode path:
//! `gdk_pixbuf::Pixbuf::from_file` → premultiplied ARGB32 bytes.
//!
//! Decoded images are kept as plain byte buffers ([`DecodedImage`]), not as
//! `cairo::ImageSurface`s: cairo surfaces are not `Send` (`ImageSurface::data`
//! refuses to borrow once the surface's reference count is above 1, which it
//! always is once both the canvas and an export snapshot hold a clone), so a
//! surface can't be handed to the background thread the export pipeline runs
//! on. Keeping the canonical decoded form as `Send`-safe bytes and building a
//! fresh, single-owner `cairo::ImageSurface` from them wherever one is
//! actually needed (preview render, export render) sidesteps that entirely.

use std::path::Path;
use std::sync::Arc;

use gtk4::cairo;
use gtk4::gdk_pixbuf;
use gtk4::glib;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("could not read image: {0}")]
    Decode(#[from] glib::Error),
    #[error("could not build surface: {0}")]
    Surface(#[from] cairo::Error),
    #[error("could not access pixel data: {0}")]
    Borrow(#[from] cairo::BorrowError),
}

/// Premultiplied ARGB32 pixel data, tightly packed (`stride == width * 4`),
/// plus its size. Cheap to clone (`Arc`), safe to send across threads.
#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub bytes: Arc<[u8]>,
    pub width: i32,
    pub height: i32,
}

/// Decodes an image file into premultiplied ARGB32 bytes.
///
/// gdk-pixbuf stores non-premultiplied RGB(A) with a possibly-padded row
/// stride; Cairo's `ARgb32` wants *premultiplied* alpha, native-endian
/// 0xAARRGGBB — i.e. byte order B, G, R, A on this little-endian target.
/// Getting the premultiply step wrong is the single most common bug in this
/// kind of conversion, so it's kept in one place and documented here rather
/// than inlined at each call site.
pub fn decode_image(path: &Path) -> Result<DecodedImage, ImportError> {
    let pixbuf = gdk_pixbuf::Pixbuf::from_file(path)?;
    let width = pixbuf.width();
    let height = pixbuf.height();
    let n_channels = pixbuf.n_channels();
    let src_stride = pixbuf.rowstride();
    let has_alpha = pixbuf.has_alpha();
    let src = pixbuf.read_pixel_bytes();

    let dst_stride = width * 4;
    let mut out = vec![0u8; (dst_stride * height) as usize];
    for y in 0..height {
        for x in 0..width {
            let s = (y * src_stride + x * n_channels) as usize;
            let r = src[s] as f64;
            let g = src[s + 1] as f64;
            let b = src[s + 2] as f64;
            let a = if has_alpha { src[s + 3] as f64 } else { 255.0 };
            let alpha_frac = a / 255.0;

            let d = (y * dst_stride + x * 4) as usize;
            out[d] = (b * alpha_frac).round() as u8;
            out[d + 1] = (g * alpha_frac).round() as u8;
            out[d + 2] = (r * alpha_frac).round() as u8;
            out[d + 3] = a as u8;
        }
    }

    Ok(DecodedImage { bytes: Arc::from(out), width, height })
}

/// Builds a fresh, single-owner `cairo::ImageSurface` from decoded bytes.
/// Always succeeds in borrowing its own data (`data()` never sees another
/// reference), unlike a surface fetched from a shared cache would.
pub fn surface_from_decoded(image: &DecodedImage) -> Result<cairo::ImageSurface, ImportError> {
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, image.width, image.height)?;
    let dst_stride = surface.stride();
    let src_stride = image.width * 4;
    {
        let mut data = surface.data()?;
        if dst_stride == src_stride {
            data.copy_from_slice(&image.bytes);
        } else {
            for y in 0..image.height {
                let s = (y * src_stride) as usize;
                let d = (y * dst_stride) as usize;
                data[d..d + src_stride as usize].copy_from_slice(&image.bytes[s..s + src_stride as usize]);
            }
        }
    }
    Ok(surface)
}
