//! Generated backgrounds. The generator itself is Prospekt, the background
//! library shared with Kulisse; this module only adds what ScreenForge
//! decides on top of it.
//!
//! ScreenForge draws the classic waves as smooth curves since 0.34.1, so
//! [`render`] uses at least algorithm version 2 (Prospekt's smooth waves),
//! also for scenes saved before the version existed. Everything else
//! renders exactly as Prospekt does.

use cairo::Context;

use crate::model::GeneratedBackground;
pub use prospekt::generator::{variant, Region as ScreenshotRegion};

/// Renders `bg` onto `ctx`, filling the `width`×`height` canvas, keeping
/// clear of `regions` where the background asks for it.
pub fn render(ctx: &Context, bg: &GeneratedBackground, width: f64, height: f64, regions: &[ScreenshotRegion]) -> Result<(), prospekt::RenderError> {
    let bg = GeneratedBackground { version: bg.version.max(2), ..bg.clone() };
    prospekt::render(ctx, &bg, width, height, regions)
}
