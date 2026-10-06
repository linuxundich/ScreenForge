//! Generic device frames drawn around a screenshot: a phone with thin
//! bezels and a punch-hole camera, a tablet with wider even bezels, and a
//! browser window with a title bar. All vector, sized relative to the
//! element, so there are no manufacturer images to maintain.
//!
//! The layout reserves the frame's outer size ([`outer_size`]), so the
//! screenshot keeps its exact aspect ratio inside [`screen_area`].

use cairo::Context;

use crate::model::{CornerRadius, DeviceFrame, DeviceKind, FrameTone};
use crate::render::{rounded_rect_path, RenderError};

/// Bezel width as a fraction of the frame's outer width.
const PHONE_BEZEL: f64 = 0.035;
const TABLET_BEZEL: f64 = 0.05;
/// Browser title bar height as a fraction of the window width.
const BROWSER_BAR: f64 = 0.065;

/// The outer size of `frame` around a `width`×`height` screenshot shown at
/// natural scale.
pub fn outer_size(frame: DeviceFrame, width: f64, height: f64) -> (f64, f64) {
    match frame.kind {
        DeviceKind::None => (width, height),
        DeviceKind::Phone | DeviceKind::Tablet => {
            let bezel = bezel_fraction(frame.kind);
            let outer_w = width / (1.0 - 2.0 * bezel);
            (outer_w, height + 2.0 * bezel * outer_w)
        }
        DeviceKind::Browser => (width, height + BROWSER_BAR * width),
    }
}

fn bezel_fraction(kind: DeviceKind) -> f64 {
    match kind {
        DeviceKind::Tablet => TABLET_BEZEL,
        _ => PHONE_BEZEL,
    }
}

/// Where the screenshot goes inside a frame of outer size `w`×`h`, and the
/// corner radius it is clipped to.
#[derive(Debug, Clone, Copy)]
pub struct ScreenArea {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub radius: CornerRadius,
}

/// The frame's own outer corner radius; without a frame, the element's
/// corner radius setting.
pub fn outer_radius(frame: DeviceFrame, w: f64, corner_radius: &CornerRadius) -> CornerRadius {
    match frame.kind {
        DeviceKind::None => *corner_radius,
        DeviceKind::Phone => CornerRadius::uniform(w * 0.12),
        DeviceKind::Tablet => CornerRadius::uniform(w * 0.05),
        DeviceKind::Browser => CornerRadius::uniform(w * 0.018),
    }
}

pub fn screen_area(frame: DeviceFrame, w: f64, h: f64, corner_radius: &CornerRadius) -> ScreenArea {
    match frame.kind {
        DeviceKind::None => ScreenArea { x: 0.0, y: 0.0, width: w, height: h, radius: *corner_radius },
        DeviceKind::Phone | DeviceKind::Tablet => {
            let bezel = bezel_fraction(frame.kind) * w;
            let outer = outer_radius(frame, w, corner_radius).top_left;
            let inner = (outer - bezel * 0.8).max(0.0);
            ScreenArea { x: bezel, y: bezel, width: w - 2.0 * bezel, height: h - 2.0 * bezel, radius: CornerRadius::uniform(inner) }
        }
        DeviceKind::Browser => {
            let bar = BROWSER_BAR * w;
            let outer = outer_radius(frame, w, corner_radius).top_left;
            ScreenArea {
                x: 0.0,
                y: bar,
                width: w,
                height: h - bar,
                radius: CornerRadius { top_left: 0.0, top_right: 0.0, bottom_right: outer, bottom_left: outer },
            }
        }
    }
}

fn tone_colors(tone: FrameTone) -> ((f64, f64, f64), (f64, f64, f64)) {
    // (body, edge highlight)
    match tone {
        FrameTone::Dark => ((0.10, 0.10, 0.11), (0.32, 0.32, 0.34)),
        FrameTone::Light => ((0.91, 0.91, 0.93), (0.76, 0.76, 0.79)),
    }
}

/// Draws the frame body (bezel or title bar) for an element of outer size
/// `w`×`h`, before the screenshot is painted into its screen area.
pub fn draw_body(ctx: &Context, frame: DeviceFrame, w: f64, h: f64, corner_radius: &CornerRadius) -> Result<(), RenderError> {
    if frame.kind == DeviceKind::None {
        return Ok(());
    }
    let (body, edge) = tone_colors(frame.tone);
    let radius = outer_radius(frame, w, corner_radius);
    ctx.save()?;
    rounded_rect_path(ctx, 0.0, 0.0, w, h, &radius);
    ctx.set_source_rgb(body.0, body.1, body.2);
    ctx.fill_preserve()?;
    ctx.set_source_rgb(edge.0, edge.1, edge.2);
    ctx.set_line_width((w * 0.004).max(0.5));
    ctx.stroke()?;

    if frame.kind == DeviceKind::Browser {
        let bar = BROWSER_BAR * w;
        let (dot, pill) = match frame.tone {
            FrameTone::Dark => ((0.42, 0.42, 0.45), (0.22, 0.22, 0.24)),
            FrameTone::Light => ((0.70, 0.70, 0.73), (1.0, 1.0, 1.0)),
        };
        ctx.set_source_rgb(dot.0, dot.1, dot.2);
        for i in 0..3 {
            ctx.new_path();
            ctx.arc(bar * 0.55 + i as f64 * bar * 0.42, bar / 2.0, bar * 0.13, 0.0, std::f64::consts::TAU);
            ctx.fill()?;
        }
        let pill_x = bar * 1.9;
        let pill_w = (w - pill_x - bar * 0.6).max(0.0);
        rounded_rect_path(ctx, pill_x, bar * 0.24, pill_w, bar * 0.52, &CornerRadius::uniform(bar * 0.26));
        ctx.set_source_rgb(pill.0, pill.1, pill.2);
        ctx.fill()?;
    }
    ctx.restore()?;
    Ok(())
}

/// Draws what sits on top of the screenshot: the phone's punch-hole camera
/// or the tablet's camera in the top bezel.
pub fn draw_overlay(ctx: &Context, frame: DeviceFrame, w: f64, area: &ScreenArea) -> Result<(), RenderError> {
    ctx.save()?;
    match frame.kind {
        DeviceKind::Phone => {
            let r = area.width * 0.022;
            ctx.new_path();
            ctx.arc(area.x + area.width / 2.0, area.y + area.width * 0.045, r, 0.0, std::f64::consts::TAU);
            ctx.set_source_rgb(0.03, 0.03, 0.04);
            ctx.fill()?;
        }
        DeviceKind::Tablet => {
            let bezel = TABLET_BEZEL * w;
            ctx.new_path();
            ctx.arc(w / 2.0, bezel / 2.0, bezel * 0.16, 0.0, std::f64::consts::TAU);
            ctx.set_source_rgb(0.2, 0.2, 0.22);
            ctx.fill()?;
        }
        DeviceKind::Browser | DeviceKind::None => {}
    }
    ctx.restore()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(kind: DeviceKind) -> DeviceFrame {
        DeviceFrame { kind, tone: FrameTone::Dark }
    }

    #[test]
    fn the_screen_area_keeps_the_screenshots_aspect_ratio() {
        let (nw, nh) = (1080.0, 2400.0);
        for kind in [DeviceKind::None, DeviceKind::Phone, DeviceKind::Tablet, DeviceKind::Browser] {
            let (w, h) = outer_size(frame(kind), nw, nh);
            // Shown at any scale, e.g. half size.
            let area = screen_area(frame(kind), w * 0.5, h * 0.5, &CornerRadius::none());
            let ratio = area.width / area.height;
            assert!((ratio - nw / nh).abs() < 1e-9, "{kind:?}: {ratio}");
        }
    }

    #[test]
    fn frames_add_space_and_no_frame_adds_none() {
        assert_eq!(outer_size(frame(DeviceKind::None), 100.0, 200.0), (100.0, 200.0));
        let (w, h) = outer_size(frame(DeviceKind::Phone), 100.0, 200.0);
        assert!(w > 100.0 && h > 200.0);
        let (w, h) = outer_size(frame(DeviceKind::Browser), 100.0, 200.0);
        assert!(w == 100.0 && h > 200.0);
    }
}
