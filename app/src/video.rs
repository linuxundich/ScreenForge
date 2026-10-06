//! Animated export as WebM (VP9): the background stands still while the
//! screenshots fly in one after another, rising a little and fading in,
//! then the finished composition holds. Frames are composited from
//! layers rendered once (`screenforge_core::render::compose_layers`) and
//! encoded with GStreamer (appsrc → videoconvert → vp9enc → webmmux).
//! Plain data in, file out — runs on a background thread like the image
//! export.

use std::collections::HashMap;
use std::path::Path;

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gtk4::cairo;
use screenforge_core::model::Document;
use uuid::Uuid;

use crate::export::ExportError;
use crate::import::{self, DecodedImage};

const FPS: i32 = 30;
/// When the first screenshot starts moving, the delay between screenshots,
/// how long each one takes, and how long the finished image holds.
const START: f64 = 0.3;
const STAGGER: f64 = 0.35;
const DURATION: f64 = 0.7;
const HOLD: f64 = 1.5;

fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn gst_error(err: impl std::fmt::Display) -> ExportError {
    ExportError::Video(err.to_string())
}

/// Renders the animation of `doc` to `path`, at the document's export width.
pub fn render_and_write(
    doc: &Document,
    decoded_images: &HashMap<Uuid, DecodedImage>,
    background_image: Option<&DecodedImage>,
    path: &Path,
) -> Result<(), ExportError> {
    gst::init().map_err(gst_error)?;
    let surfaces: HashMap<Uuid, cairo::ImageSurface> = decoded_images
        .iter()
        .map(|(id, image)| Ok((*id, import::surface_from_decoded(image)?)))
        .collect::<Result<_, import::ImportError>>()?;
    let background_surface = background_image.map(import::surface_from_decoded).transpose()?;

    // VP9 in I420 needs even dimensions.
    let scale = doc.canvas.export_target_width as f64 / doc.canvas.export_width.max(1) as f64;
    let width = ((doc.canvas.export_target_width as i32) / 2 * 2).max(2);
    let height = (((doc.canvas.export_height as f64 * scale).round() as i32) / 2 * 2).max(2);
    let (background, layers) =
        screenforge_core::render::compose_layers(doc, scale, &surfaces, background_surface.as_ref(), width, height)?;

    let total = START + STAGGER * layers.len().saturating_sub(1) as f64 + DURATION + HOLD;
    let frames = (total * FPS as f64).ceil() as u64;

    let pipeline = gst::Pipeline::new();
    let info = gstreamer_video::VideoInfo::builder(gstreamer_video::VideoFormat::Bgra, width as u32, height as u32)
        .fps(gst::Fraction::new(FPS, 1))
        .build()
        .map_err(gst_error)?;
    let src = gst_app::AppSrc::builder().caps(&info.to_caps().map_err(gst_error)?).format(gst::Format::Time).build();
    let convert = gst::ElementFactory::make("videoconvert").build().map_err(gst_error)?;
    let encoder = gst::ElementFactory::make("vp9enc")
        .property("deadline", 1i64)
        .property("cpu-used", 6i32)
        .property("target-bitrate", width * height * 4)
        .build()
        .map_err(gst_error)?;
    let mux = gst::ElementFactory::make("webmmux").build().map_err(gst_error)?;
    let sink = gst::ElementFactory::make("filesink").property("location", path.to_string_lossy().to_string()).build().map_err(gst_error)?;
    pipeline.add_many([src.upcast_ref(), &convert, &encoder, &mux, &sink]).map_err(gst_error)?;
    gst::Element::link_many([src.upcast_ref(), &convert, &encoder, &mux, &sink]).map_err(gst_error)?;
    pipeline.set_state(gst::State::Playing).map_err(gst_error)?;

    let rise = height as f64 * 0.06;
    let frame = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let mut frame = Some(frame);
    for n in 0..frames {
        let t = n as f64 / FPS as f64;
        let mut surface = frame.take().expect("frame surface");
        {
            let ctx = cairo::Context::new(&surface)?;
            ctx.set_operator(cairo::Operator::Source);
            ctx.set_source_surface(&background, 0.0, 0.0)?;
            ctx.paint()?;
            ctx.set_operator(cairo::Operator::Over);
            for (i, layer) in layers.iter().enumerate() {
                let progress = ease_out_cubic((t - START - i as f64 * STAGGER) / DURATION);
                if progress <= 0.0 {
                    continue;
                }
                ctx.set_source_surface(layer, 0.0, (1.0 - progress) * rise)?;
                ctx.paint_with_alpha(progress)?;
            }
        }
        surface.flush();
        let data = surface.data()?.to_vec();
        frame = Some(surface);
        let mut buffer = gst::Buffer::from_mut_slice(data);
        {
            let buffer = buffer.get_mut().expect("fresh buffer");
            buffer.set_pts(gst::ClockTime::from_nseconds(n * 1_000_000_000 / FPS as u64));
            buffer.set_duration(gst::ClockTime::from_nseconds(1_000_000_000 / FPS as u64));
        }
        src.push_buffer(buffer).map_err(gst_error)?;
    }
    src.end_of_stream().map_err(gst_error)?;

    let bus = pipeline.bus().ok_or_else(|| gst_error("no pipeline bus"))?;
    let result = loop {
        let Some(message) = bus.timed_pop(gst::ClockTime::from_seconds(120)) else {
            break Err(gst_error("encoding timed out"));
        };
        match message.view() {
            gst::MessageView::Eos(..) => break Ok(()),
            gst::MessageView::Error(err) => break Err(gst_error(err.error())),
            _ => {}
        }
    };
    pipeline.set_state(gst::State::Null).map_err(gst_error)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_short_webm_file() {
        let mut doc = Document::new();
        doc.canvas.export_width = 64;
        doc.canvas.export_height = 48;
        doc.canvas.export_target_width = 64;
        let path = std::env::temp_dir().join("screenforge-video-test.webm");
        render_and_write(&doc, &HashMap::new(), None, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        // WebM is EBML: starts with 0x1A45DFA3.
        assert_eq!(&bytes[..4], &[0x1a, 0x45, 0xdf, 0xa3]);
        std::fs::remove_file(path).ok();
    }
}
