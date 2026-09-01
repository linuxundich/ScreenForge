//! Cairo composition function shared between the interactive preview and
//! full-resolution export — the same `compose()` call renders either, only
//! the target surface's size and `scale` differ, which is how preview and
//! export stay pixel-identical (mod resolution/antialiasing).

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI};

use cairo::{Context, LinearGradient, RadialGradient};
use thiserror::Error;
use uuid::Uuid;

use crate::layout::compute_layout;
use crate::model::{Background, CornerRadius, Document, GradientKind, ScreenshotElement};

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("cairo error: {0}")]
    Cairo(#[from] cairo::Error),
    #[error("missing decoded image for element {0}")]
    MissingImage(Uuid),
}

/// Renders `doc` onto `target`, at `scale` (1.0 = document pixels map 1:1 to
/// surface pixels; use a smaller scale for a downsized preview and 1.0 for a
/// full-resolution export of the same document). `resolved_images` must
/// contain a decoded [`cairo::ImageSurface`] for every visible element,
/// keyed by element id — decoding is the app layer's job, this function
/// only composites already-decoded pixels.
pub fn compose(
    doc: &Document,
    target: &cairo::ImageSurface,
    scale: f64,
    resolved_images: &HashMap<Uuid, cairo::ImageSurface>,
) -> Result<(), RenderError> {
    let ctx = Context::new(target)?;
    ctx.scale(scale, scale);

    draw_background(
        &ctx,
        &doc.background,
        doc.canvas.export_width as f64,
        doc.canvas.export_height as f64,
    )?;

    let visible: Vec<ScreenshotElement> = doc.elements.iter().filter(|e| e.visible).cloned().collect();
    let placements = compute_layout(doc.layout.mode, &visible, doc.layout.spacing_px, doc.layout.margin_px);

    for (el, placement) in visible.iter().zip(placements.iter()) {
        let image = resolved_images.get(&el.id).ok_or(RenderError::MissingImage(el.id))?;

        ctx.save()?;
        let cx = placement.x + placement.width / 2.0;
        let cy = placement.y + placement.height / 2.0;
        ctx.translate(cx, cy);
        ctx.rotate(el.transform.rotation_deg.to_radians());
        ctx.translate(-placement.width / 2.0, -placement.height / 2.0);

        if el.shadow.enabled {
            ctx.save()?;
            ctx.translate(el.shadow.offset_x, el.shadow.offset_y);
            rounded_rect_path(&ctx, 0.0, 0.0, placement.width, placement.height, &el.corner_radius);
            let c = el.shadow.color;
            ctx.set_source_rgba(c.r, c.g, c.b, c.a * el.shadow.opacity);
            ctx.fill()?;
            ctx.restore()?;
        }

        rounded_rect_path(&ctx, 0.0, 0.0, placement.width, placement.height, &el.corner_radius);
        ctx.clip();

        ctx.save()?;
        if el.transform.flip_horizontal {
            ctx.translate(placement.width, 0.0);
            ctx.scale(-1.0, 1.0);
        }
        if el.transform.flip_vertical {
            ctx.translate(0.0, placement.height);
            ctx.scale(1.0, -1.0);
        }
        let sx = placement.width / image.width() as f64;
        let sy = placement.height / image.height() as f64;
        ctx.scale(sx, sy);
        ctx.set_source_surface(image, 0.0, 0.0)?;
        ctx.paint()?;
        ctx.restore()?;

        ctx.reset_clip();
        ctx.restore()?;
    }

    Ok(())
}

fn draw_background(ctx: &Context, background: &Background, width: f64, height: f64) -> Result<(), RenderError> {
    match background {
        Background::Solid(color) => {
            ctx.set_source_rgba(color.r, color.g, color.b, color.a);
            ctx.rectangle(0.0, 0.0, width, height);
            ctx.fill()?;
        }
        Background::Gradient(spec) => {
            match spec.kind {
                GradientKind::Linear { angle_deg } => {
                    let rad = angle_deg.to_radians();
                    let (dx, dy) = (rad.cos(), rad.sin());
                    let (cx, cy) = (width / 2.0, height / 2.0);
                    let len = (width.powi(2) + height.powi(2)).sqrt() / 2.0;
                    let gradient = LinearGradient::new(cx - dx * len, cy - dy * len, cx + dx * len, cy + dy * len);
                    for (pos, color) in &spec.stops {
                        gradient.add_color_stop_rgba(*pos, color.r, color.g, color.b, color.a);
                    }
                    ctx.set_source(&gradient)?;
                    ctx.rectangle(0.0, 0.0, width, height);
                    ctx.fill()?;
                }
                GradientKind::Radial { center_x, center_y } => {
                    let radius = width.max(height) / 2.0;
                    let (px, py) = (center_x * width, center_y * height);
                    let gradient = RadialGradient::new(px, py, 0.0, px, py, radius);
                    for (pos, color) in &spec.stops {
                        gradient.add_color_stop_rgba(*pos, color.r, color.g, color.b, color.a);
                    }
                    ctx.set_source(&gradient)?;
                    ctx.rectangle(0.0, 0.0, width, height);
                    ctx.fill()?;
                }
            }
        }
        Background::Image(_) => todo!("image backgrounds are not implemented yet"),
        Background::Decoration(_) => todo!("vector decoration backgrounds are not implemented yet"),
    }
    Ok(())
}

/// Builds a rounded-rectangle path at `(x, y)` sized `w`×`h` with the given
/// per-corner radii. Cairo has no native rounded-rect primitive, so this is
/// four `arc()` calls joined by implicit `line_to`s (cairo draws a
/// straight line to the next arc's start automatically).
fn rounded_rect_path(ctx: &Context, x: f64, y: f64, w: f64, h: f64, r: &CornerRadius) {
    let tl = r.top_left.max(0.0).min(w / 2.0).min(h / 2.0);
    let tr = r.top_right.max(0.0).min(w / 2.0).min(h / 2.0);
    let br = r.bottom_right.max(0.0).min(w / 2.0).min(h / 2.0);
    let bl = r.bottom_left.max(0.0).min(w / 2.0).min(h / 2.0);

    ctx.new_sub_path();
    ctx.arc(x + w - tr, y + tr, tr, -FRAC_PI_2, 0.0);
    ctx.arc(x + w - br, y + h - br, br, 0.0, FRAC_PI_2);
    ctx.arc(x + bl, y + h - bl, bl, FRAC_PI_2, PI);
    ctx.arc(x + tl, y + tl, tl, PI, 3.0 * FRAC_PI_2);
    ctx.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Background, CanvasSettings, Document, GradientKind, GradientSpec, ImageSource, LayoutSettings, Rgba, ScreenshotElement};
    use cairo::{Format, ImageSurface};
    use std::path::PathBuf;

    /// A solid-color `w`x`h` surface, standing in for a decoded screenshot
    /// without needing an actual image file.
    fn solid_surface(w: i32, h: i32, color: Rgba) -> ImageSurface {
        let surface = ImageSurface::create(Format::ARgb32, w, h).unwrap();
        let ctx = Context::new(&surface).unwrap();
        ctx.set_source_rgba(color.r, color.g, color.b, color.a);
        ctx.paint().unwrap();
        drop(ctx);
        surface
    }

    /// Reads one pixel as (r, g, b, a) in `0.0..=1.0`, unpremultiplying
    /// cairo's premultiplied-alpha ARGB32 storage. Format is native-endian
    /// 0xAARRGGBB, i.e. byte order B, G, R, A on this little-endian target.
    fn read_pixel(surface: &mut ImageSurface, x: i32, y: i32) -> (f64, f64, f64, f64) {
        let stride = surface.stride();
        let data = surface.data().unwrap();
        let offset = (y * stride + x * 4) as usize;
        let b = data[offset] as f64;
        let g = data[offset + 1] as f64;
        let r = data[offset + 2] as f64;
        let a = data[offset + 3] as f64;
        if a == 0.0 {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            (r / a, g / a, b / a, a / 255.0)
        }
    }

    fn assert_close(actual: (f64, f64, f64, f64), expected: (f64, f64, f64, f64)) {
        let tol = 0.02;
        assert!((actual.0 - expected.0).abs() < tol, "r: {actual:?} vs {expected:?}");
        assert!((actual.1 - expected.1).abs() < tol, "g: {actual:?} vs {expected:?}");
        assert!((actual.2 - expected.2).abs() < tol, "b: {actual:?} vs {expected:?}");
        assert!((actual.3 - expected.3).abs() < tol, "a: {actual:?} vs {expected:?}");
    }

    #[test]
    fn solid_background_fills_the_canvas() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 200, export_height: 100, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::new(0.2, 0.4, 0.6, 1.0));

        let mut target = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
        compose(&doc, &target, 1.0, &HashMap::new()).unwrap();

        assert_close(read_pixel(&mut target, 5, 5), (0.2, 0.4, 0.6, 1.0));
        assert_close(read_pixel(&mut target, 195, 95), (0.2, 0.4, 0.6, 1.0));
    }

    #[test]
    fn linear_gradient_background_varies_across_the_canvas() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 200, export_height: 100, ..CanvasSettings::default() };
        doc.background = Background::Gradient(GradientSpec {
            kind: GradientKind::Linear { angle_deg: 0.0 },
            stops: vec![(0.0, Rgba::new(0.0, 0.0, 0.0, 1.0)), (1.0, Rgba::new(1.0, 1.0, 1.0, 1.0))],
        });

        let mut target = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
        compose(&doc, &target, 1.0, &HashMap::new()).unwrap();

        let left = read_pixel(&mut target, 2, 50);
        let right = read_pixel(&mut target, 197, 50);
        assert!(left.0 < 0.3, "left edge should be near black: {left:?}");
        assert!(right.0 > 0.7, "right edge should be near white: {right:?}");
    }

    #[test]
    fn two_elements_are_placed_side_by_side_and_composited() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 300, export_height: 200, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 10.0, margin_px: 20.0 };

        let red = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 160.0);
        let blue = ScreenshotElement::new(ImageSource::Path(PathBuf::from("b.png")), 100.0, 160.0);
        let (red_id, blue_id) = (red.id, blue.id);
        doc.elements = vec![red, blue];

        let mut resolved = HashMap::new();
        resolved.insert(red_id, solid_surface(100, 160, Rgba::new(1.0, 0.0, 0.0, 1.0)));
        resolved.insert(blue_id, solid_surface(100, 160, Rgba::new(0.0, 0.0, 1.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 300, 200).unwrap();
        compose(&doc, &target, 1.0, &resolved).unwrap();

        // First element: x in [20, 120), y in [20, 180).
        assert_close(read_pixel(&mut target, 60, 100), (1.0, 0.0, 0.0, 1.0));
        // Second element starts at 120 + 10 = 130.
        assert_close(read_pixel(&mut target, 160, 100), (0.0, 0.0, 1.0, 1.0));
        // Background is visible in the gap between the two elements.
        assert_close(read_pixel(&mut target, 122, 100), (1.0, 1.0, 1.0, 1.0));
    }

    #[test]
    fn missing_decoded_image_is_reported_as_an_error() {
        let mut doc = Document::new();
        doc.elements = vec![ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 100.0)];
        let target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();

        let err = compose(&doc, &target, 1.0, &HashMap::new()).unwrap_err();
        assert!(matches!(err, RenderError::MissingImage(_)));
    }

    #[test]
    fn rounded_corners_clip_the_element_at_the_corner_pixel() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 100, export_height: 100, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_px: 0.0 };

        let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 100.0);
        el.corner_radius = CornerRadius::uniform(20.0);
        let id = el.id;
        doc.elements = vec![el];

        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(100, 100, Rgba::new(0.0, 0.0, 0.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        compose(&doc, &target, 1.0, &resolved).unwrap();

        // The very corner pixel is outside the rounded-rect clip, so the
        // white canvas background should show through.
        assert_close(read_pixel(&mut target, 1, 1), (1.0, 1.0, 1.0, 1.0));
        // The center is well inside the clip and should be the element's color.
        assert_close(read_pixel(&mut target, 50, 50), (0.0, 0.0, 0.0, 1.0));
    }

    /// A surface whose left half is `left` and right half is `right`, so a
    /// horizontal flip is unambiguous to detect by sampling either side.
    fn split_surface(w: i32, h: i32, left: Rgba, right: Rgba) -> ImageSurface {
        let surface = ImageSurface::create(Format::ARgb32, w, h).unwrap();
        let ctx = Context::new(&surface).unwrap();
        ctx.set_source_rgba(left.r, left.g, left.b, left.a);
        ctx.rectangle(0.0, 0.0, w as f64 / 2.0, h as f64);
        ctx.fill().unwrap();
        ctx.set_source_rgba(right.r, right.g, right.b, right.a);
        ctx.rectangle(w as f64 / 2.0, 0.0, w as f64 / 2.0, h as f64);
        ctx.fill().unwrap();
        drop(ctx);
        surface
    }

    #[test]
    fn flip_horizontal_mirrors_the_element() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 100, export_height: 100, ..CanvasSettings::default() };
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_px: 0.0 };

        let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 100.0);
        el.transform.flip_horizontal = true;
        let id = el.id;
        doc.elements = vec![el];

        let mut resolved = HashMap::new();
        resolved.insert(id, split_surface(100, 100, Rgba::new(1.0, 0.0, 0.0, 1.0), Rgba::new(0.0, 0.0, 1.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        compose(&doc, &target, 1.0, &resolved).unwrap();

        // Unflipped this would be red-left/blue-right; flipped it's reversed.
        assert_close(read_pixel(&mut target, 10, 50), (0.0, 0.0, 1.0, 1.0));
        assert_close(read_pixel(&mut target, 90, 50), (1.0, 0.0, 0.0, 1.0));
    }

    #[test]
    fn no_flip_keeps_original_orientation() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 100, export_height: 100, ..CanvasSettings::default() };
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_px: 0.0 };

        let el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 100.0);
        let id = el.id;
        doc.elements = vec![el];

        let mut resolved = HashMap::new();
        resolved.insert(id, split_surface(100, 100, Rgba::new(1.0, 0.0, 0.0, 1.0), Rgba::new(0.0, 0.0, 1.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        compose(&doc, &target, 1.0, &resolved).unwrap();

        assert_close(read_pixel(&mut target, 10, 50), (1.0, 0.0, 0.0, 1.0));
        assert_close(read_pixel(&mut target, 90, 50), (0.0, 0.0, 1.0, 1.0));
    }
}
