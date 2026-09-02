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
use crate::model::{
    Background, BackgroundImageFit, CornerRadius, Document, GradientKind, ScreenshotElement, ShadowParams, TextAlign, TextBackground,
    TextElement,
};
use crate::shadow_cache::{ShadowCache, MAX_SHADOW_SURFACE_DIM};

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("cairo error: {0}")]
    Cairo(#[from] cairo::Error),
    #[error("could not read shadow pixels for blurring: {0}")]
    Borrow(#[from] cairo::BorrowError),
    #[error("missing decoded image for element {0}")]
    MissingImage(Uuid),
    #[error("missing decoded background image")]
    MissingBackgroundImage,
}

/// Renders `doc` onto `target`, at `scale` (1.0 = document pixels map 1:1 to
/// surface pixels; use a smaller scale for a downsized preview and 1.0 for a
/// full-resolution export of the same document). `resolved_images` must
/// contain a decoded [`cairo::ImageSurface`] for every visible element,
/// keyed by element id — decoding is the app layer's job, this function
/// only composites already-decoded pixels. `background_image` is likewise a
/// pre-decoded surface, needed only when `doc.background` is
/// [`Background::Image`]. `shadow_cache` lets repeated calls against the
/// same document (the interactive preview) reuse already-rendered shadow
/// bitmaps instead of re-blurring them every time — see
/// [`crate::shadow_cache`]; a caller that renders only once (export) can
/// just pass a fresh, empty `ShadowCache` and pay a one-time miss per
/// shadow.
pub fn compose(
    doc: &Document,
    target: &cairo::ImageSurface,
    scale: f64,
    resolved_images: &HashMap<Uuid, cairo::ImageSurface>,
    background_image: Option<&cairo::ImageSurface>,
    shadow_cache: &ShadowCache,
) -> Result<(), RenderError> {
    shadow_cache.begin_frame();
    let ctx = Context::new(target)?;
    ctx.scale(scale, scale);

    let visible: Vec<ScreenshotElement> = doc.elements.iter().filter(|e| e.visible).cloned().collect();
    let placements = compute_layout(doc.layout.mode, &visible, doc.layout.spacing_px, doc.layout.margin_px);
    let screenshot_regions: Vec<crate::generator::ScreenshotRegion> = placements
        .iter()
        .map(|p| crate::generator::ScreenshotRegion { x: p.x, y: p.y, width: p.width, height: p.height })
        .collect();

    draw_background(
        &ctx,
        &doc.background,
        doc.canvas.export_width as f64,
        doc.canvas.export_height as f64,
        background_image,
        &screenshot_regions,
    )?;

    for (el, placement) in visible.iter().zip(placements.iter()) {
        let image = resolved_images.get(&el.id).ok_or(RenderError::MissingImage(el.id))?;

        ctx.save()?;
        let cx = placement.x + placement.width / 2.0;
        let cy = placement.y + placement.height / 2.0;
        ctx.translate(cx, cy);
        ctx.rotate(el.transform.rotation_deg.to_radians());
        ctx.translate(-placement.width / 2.0, -placement.height / 2.0);

        if el.shadow.enabled {
            draw_shadow(&ctx, placement.width, placement.height, &el.corner_radius, &el.shadow, scale, shadow_cache)?;
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

        // Drawn screenshot-relative, still inside this element's own
        // translate/rotate block (so the label moves and rotates with its
        // screenshot — spec §11), but after `reset_clip` since a label
        // commonly sits outside the screenshot's own rounded-rect bounds
        // (e.g. a caption below it) and must not be clipped away.
        if el.label.enabled && !el.label.content.is_empty() {
            draw_text_element(&ctx, &el.label, placement.width, placement.height, scale, shadow_cache)?;
        }

        ctx.restore()?;
    }

    if doc.title.enabled && !doc.title.content.is_empty() {
        draw_text_element(&ctx, &doc.title, doc.canvas.export_width as f64, doc.canvas.export_height as f64, scale, shadow_cache)?;
    }

    Ok(())
}

/// Draws one [`TextElement`] — its box shadow, background, and the text
/// itself, in that order — within a `ref_w`×`ref_h` reference rect. `ctx`
/// is expected to already be at that rect's own origin (`(0, 0)` is the
/// rect's top-left) — the whole canvas for a composition title, or one
/// screenshot's own placement for a label; see `compose`'s two call sites.
/// A no-op when `text` is disabled or empty (checked by the caller too, so
/// this never runs Pango layout on nothing).
///
/// Uses Pango (via `pangocairo`), the GNOME stack's own text layout engine
/// — not Cairo's "toy" text API — so font family/weight/italic/alignment/
/// wrapping/letter- and line-spacing all come from the same fontconfig-
/// backed shaping every other GTK app on the system uses, rather than a
/// second, ad-hoc text system.
fn draw_text_element(ctx: &Context, text: &TextElement, ref_w: f64, ref_h: f64, scale: f64, cache: &ShadowCache) -> Result<(), RenderError> {
    let layout = pangocairo::functions::create_layout(ctx);

    let mut font_desc = pango::FontDescription::new();
    font_desc.set_family(&text.typography.font_family);
    // Absolute (device-pixel) sizing, not points -- this renders onto a
    // raw image surface with no independent screen-DPI to account for, so
    // `font_size` should mean exactly what it says: pixels.
    font_desc.set_absolute_size(text.typography.font_size.max(0.1) * pango::SCALE as f64);
    font_desc.set_weight(pango::Weight::__Unknown(text.typography.weight));
    font_desc.set_style(if text.typography.italic { pango::Style::Italic } else { pango::Style::Normal });
    layout.set_font_description(Some(&font_desc));

    layout.set_alignment(match text.typography.alignment {
        TextAlign::Left => pango::Alignment::Left,
        TextAlign::Center => pango::Alignment::Center,
        TextAlign::Right => pango::Alignment::Right,
    });
    layout.set_line_spacing(text.typography.line_spacing.max(0.1) as f32);

    if text.typography.letter_spacing != 0.0 {
        let attrs = pango::AttrList::new();
        attrs.insert(pango::AttrInt::new_letter_spacing((text.typography.letter_spacing * pango::SCALE as f64) as i32));
        layout.set_attributes(Some(&attrs));
    }

    if text.typography.wrap {
        layout.set_width((text.wrap_width(ref_w) * pango::SCALE as f64) as i32);
        layout.set_wrap(pango::WrapMode::Word);
    }

    layout.set_text(&text.content);

    let (_, logical) = layout.pixel_extents();
    let content_w = logical.width() as f64;
    let content_h = logical.height() as f64;
    let box_w = content_w + 2.0 * text.background_padding;
    let box_h = content_h + 2.0 * text.background_padding;

    let (box_x, box_y) = text.position.resolve_box_origin(ref_w, ref_h, box_w, box_h);

    ctx.save()?;
    ctx.translate(box_x, box_y);

    if text.shadow.enabled {
        draw_shadow(ctx, box_w, box_h, &text.corner_radius, &text.shadow, scale, cache)?;
    }

    match &text.background {
        TextBackground::None => {}
        TextBackground::Solid(color) => {
            rounded_rect_path(ctx, 0.0, 0.0, box_w, box_h, &text.corner_radius);
            ctx.set_source_rgba(color.r, color.g, color.b, color.a);
            ctx.fill()?;
        }
        TextBackground::Gradient(spec) => {
            ctx.save()?;
            rounded_rect_path(ctx, 0.0, 0.0, box_w, box_h, &text.corner_radius);
            ctx.clip();
            paint_gradient(ctx, spec, box_w, box_h)?;
            ctx.restore()?;
        }
    }

    let c = text.typography.color;
    ctx.set_source_rgba(c.r, c.g, c.b, c.a * text.typography.opacity.clamp(0.0, 1.0));
    ctx.move_to(text.background_padding, text.background_padding);
    pangocairo::functions::show_layout(ctx, &layout);

    ctx.restore()?;
    Ok(())
}

/// Draws one element's shadow: a rounded rect matching the element's own
/// shape, offset by `shadow.offset_x`/`offset_y` and optionally blurred by
/// `shadow.blur` pixels, reusing an already-rendered bitmap from
/// `cache` whenever one matching this shape already exists (see
/// [`crate::shadow_cache`] — notably, moving an element never changes its
/// cache key, so a move-drag reuses the same bitmap on every frame instead
/// of re-blurring it).
///
/// The bitmap itself is rendered at `scale`'s *output* resolution (capped
/// by [`MAX_SHADOW_SURFACE_DIM`]) rather than always at full document
/// resolution, then painted back through a matching inverse scale — see
/// `shadow_render_scale`. That keeps a zoomed-out preview's shadows both
/// cheap to generate (a smaller bitmap to blur) and cheap to keep cached
/// (a smaller bitmap to hold onto), while a full-resolution export still
/// gets a full-resolution shadow, all from one code path. Cairo has no
/// native blur filter, so the bitmap is built by rendering the *unshifted*
/// shape onto a separate, padded offscreen surface and blurring its raw
/// pixels in place (`render_shadow_bitmap`); only the offset — a pure
/// translation, which commutes with blur — is applied here, at paint time.
fn draw_shadow(
    ctx: &Context,
    width: f64,
    height: f64,
    corner_radius: &CornerRadius,
    shadow: &ShadowParams,
    scale: f64,
    cache: &ShadowCache,
) -> Result<(), RenderError> {
    let render_scale = shadow_render_scale(width, height, scale);
    let surface = cache.get_or_render(width, height, corner_radius, shadow, render_scale, || {
        render_shadow_bitmap(width, height, corner_radius, shadow, render_scale)
    })?;

    let pad = shadow_pad(shadow.blur, render_scale);
    ctx.save()?;
    // Cancels just this block's share of the outer `ctx.scale(scale, ...)`
    // (plus whatever extra reduction `shadow_render_scale` applied), so a
    // bitmap authored at `render_scale`-resolution pixels lands back at
    // its correct *document*-space size and position — see this
    // function's doc comment.
    ctx.scale(1.0 / render_scale, 1.0 / render_scale);
    ctx.set_source_surface(&*surface, shadow.offset_x * render_scale - pad as f64, shadow.offset_y * render_scale - pad as f64)?;
    ctx.paint()?;
    ctx.restore()?;
    Ok(())
}

/// The resolution to actually render a shadow bitmap at: `scale` (the
/// caller's document-to-device pixel ratio), reduced further if needed so
/// neither dimension of the (unpadded) bitmap exceeds
/// [`MAX_SHADOW_SURFACE_DIM`].
fn shadow_render_scale(width: f64, height: f64, scale: f64) -> f64 {
    if width <= 0.0 || height <= 0.0 || scale <= 0.0 {
        return scale.max(0.0);
    }
    let longest = width.max(height) * scale;
    if longest <= MAX_SHADOW_SURFACE_DIM as f64 {
        scale
    } else {
        scale * (MAX_SHADOW_SURFACE_DIM as f64 / longest)
    }
}

/// Padding (in `render_scale`-resolution pixels) around the shape so a
/// blurred edge never reaches the bitmap's own boundary — three box-blur
/// passes each spread roughly `blur` pixels further, so 3x (plus a small
/// margin) comfortably covers it.
fn shadow_pad(blur: f64, render_scale: f64) -> i32 {
    ((blur * render_scale).max(0.0) * 3.0 + 4.0).ceil() as i32
}

/// Renders the *unshifted* shadow shape — a rounded rect matching
/// `corner_radius`, filled with `shadow.color` at `shadow.opacity` and
/// blurred by `shadow.blur` — onto a freshly padded surface at
/// `render_scale`-resolution. Pure function of its inputs, which is what
/// makes it safe to call only on a `ShadowCache` miss.
fn render_shadow_bitmap(
    width: f64,
    height: f64,
    corner_radius: &CornerRadius,
    shadow: &ShadowParams,
    render_scale: f64,
) -> Result<cairo::ImageSurface, RenderError> {
    let pad = shadow_pad(shadow.blur, render_scale);
    let surface_w = ((width * render_scale).ceil() as i32 + 2 * pad).max(1);
    let surface_h = ((height * render_scale).ceil() as i32 + 2 * pad).max(1);

    let mut shadow_surface = cairo::ImageSurface::create(cairo::Format::ARgb32, surface_w, surface_h)?;
    {
        let shadow_ctx = Context::new(&shadow_surface)?;
        shadow_ctx.translate(pad as f64, pad as f64);
        shadow_ctx.scale(render_scale, render_scale);
        rounded_rect_path(&shadow_ctx, 0.0, 0.0, width, height, corner_radius);
        let c = shadow.color;
        shadow_ctx.set_source_rgba(c.r, c.g, c.b, c.a * shadow.opacity);
        shadow_ctx.fill()?;
    }

    if shadow.blur > 0.0 {
        let stride = shadow_surface.stride();
        let mut data = shadow_surface.data()?;
        crate::blur::box_blur(&mut data, surface_w, surface_h, stride, shadow.blur * render_scale);
    }

    Ok(shadow_surface)
}

/// Fills the `width`×`height` rect at the current origin with `spec` —
/// shared between the canvas background and a [`TextElement`]'s own
/// gradient background (the latter clips to its rounded box first, then
/// calls this the same way `draw_background` clips to nothing/the whole
/// canvas).
fn paint_gradient(ctx: &Context, spec: &crate::model::GradientSpec, width: f64, height: f64) -> Result<(), RenderError> {
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
    Ok(())
}

fn draw_background(
    ctx: &Context,
    background: &Background,
    width: f64,
    height: f64,
    background_image: Option<&cairo::ImageSurface>,
    screenshot_regions: &[crate::generator::ScreenshotRegion],
) -> Result<(), RenderError> {
    match background {
        Background::Solid(color) => {
            ctx.set_source_rgba(color.r, color.g, color.b, color.a);
            ctx.rectangle(0.0, 0.0, width, height);
            ctx.fill()?;
        }
        Background::Gradient(spec) => paint_gradient(ctx, spec, width, height)?,
        Background::Image(spec) => {
            let image = background_image.ok_or(RenderError::MissingBackgroundImage)?;
            let (img_w, img_h) = (image.width() as f64, image.height() as f64);
            if img_w > 0.0 && img_h > 0.0 {
                ctx.save()?;
                ctx.rectangle(0.0, 0.0, width, height);
                ctx.clip();
                match spec.fit {
                    BackgroundImageFit::Cover => {
                        let s = (width / img_w).max(height / img_h);
                        ctx.translate((width - img_w * s) / 2.0, (height - img_h * s) / 2.0);
                        ctx.scale(s, s);
                        ctx.set_source_surface(image, 0.0, 0.0)?;
                    }
                    BackgroundImageFit::Contain => {
                        let s = (width / img_w).min(height / img_h);
                        ctx.translate((width - img_w * s) / 2.0, (height - img_h * s) / 2.0);
                        ctx.scale(s, s);
                        ctx.set_source_surface(image, 0.0, 0.0)?;
                    }
                    BackgroundImageFit::Fill => {
                        ctx.scale(width / img_w, height / img_h);
                        ctx.set_source_surface(image, 0.0, 0.0)?;
                    }
                    BackgroundImageFit::Tile => {
                        let pattern = cairo::SurfacePattern::create(image);
                        pattern.set_extend(cairo::Extend::Repeat);
                        ctx.set_source(&pattern)?;
                    }
                }
                ctx.paint_with_alpha(spec.opacity)?;
                ctx.restore()?;
            }
        }
        Background::Generated(generated) => {
            crate::generator::render(ctx, generated, width, height, screenshot_regions)?;
        }
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
        compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new()).unwrap();

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
        compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new()).unwrap();

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
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new()).unwrap();

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

        let err = compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new()).unwrap_err();
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
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new()).unwrap();

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
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new()).unwrap();

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
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new()).unwrap();

        assert_close(read_pixel(&mut target, 10, 50), (1.0, 0.0, 0.0, 1.0));
        assert_close(read_pixel(&mut target, 90, 50), (0.0, 0.0, 1.0, 1.0));
    }

    fn image_background(fit: crate::model::BackgroundImageFit, opacity: f64) -> Background {
        Background::Image(crate::model::ImageBackgroundSpec {
            source: ImageSource::Path(PathBuf::from("bg.png")),
            fit,
            opacity,
        })
    }

    #[test]
    fn image_background_without_a_decoded_surface_is_an_error() {
        let mut doc = Document::new();
        doc.background = image_background(crate::model::BackgroundImageFit::Cover, 1.0);
        let target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();

        let err = compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new()).unwrap_err();
        assert!(matches!(err, RenderError::MissingBackgroundImage));
    }

    #[test]
    fn cover_scales_up_to_fill_the_canvas_with_no_gaps() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 100, export_height: 100, ..CanvasSettings::default() };
        doc.background = image_background(crate::model::BackgroundImageFit::Cover, 1.0);
        let bg = solid_surface(200, 100, Rgba::new(0.0, 1.0, 0.0, 1.0));

        let mut target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        compose(&doc, &target, 1.0, &HashMap::new(), Some(&bg), &ShadowCache::new()).unwrap();

        // A wider-than-tall image under "cover" scales until height matches,
        // overflowing left/right — every corner should be fully painted.
        assert_close(read_pixel(&mut target, 1, 1), (0.0, 1.0, 0.0, 1.0));
        assert_close(read_pixel(&mut target, 98, 98), (0.0, 1.0, 0.0, 1.0));
    }

    #[test]
    fn contain_leaves_letterbox_gaps_transparent() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 100, export_height: 100, ..CanvasSettings::default() };
        doc.background = image_background(crate::model::BackgroundImageFit::Contain, 1.0);
        let bg = solid_surface(200, 100, Rgba::new(0.0, 1.0, 0.0, 1.0));

        let mut target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        compose(&doc, &target, 1.0, &HashMap::new(), Some(&bg), &ShadowCache::new()).unwrap();

        // Scaled to 100x50, centered: covers y in [25, 75), leaves top/bottom empty.
        assert_close(read_pixel(&mut target, 50, 50), (0.0, 1.0, 0.0, 1.0));
        assert_close(read_pixel(&mut target, 50, 5), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn fill_stretches_to_the_canvas_exactly() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 100, export_height: 100, ..CanvasSettings::default() };
        doc.background = image_background(crate::model::BackgroundImageFit::Fill, 1.0);
        let bg = solid_surface(200, 100, Rgba::new(0.0, 1.0, 0.0, 1.0));

        let mut target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        compose(&doc, &target, 1.0, &HashMap::new(), Some(&bg), &ShadowCache::new()).unwrap();

        assert_close(read_pixel(&mut target, 1, 1), (0.0, 1.0, 0.0, 1.0));
        assert_close(read_pixel(&mut target, 98, 98), (0.0, 1.0, 0.0, 1.0));
    }

    #[test]
    fn tile_repeats_the_image_across_the_canvas() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 100, export_height: 100, ..CanvasSettings::default() };
        doc.background = image_background(crate::model::BackgroundImageFit::Tile, 1.0);
        let bg = solid_surface(10, 10, Rgba::new(0.0, 1.0, 0.0, 1.0));

        let mut target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        compose(&doc, &target, 1.0, &HashMap::new(), Some(&bg), &ShadowCache::new()).unwrap();

        assert_close(read_pixel(&mut target, 5, 5), (0.0, 1.0, 0.0, 1.0));
        assert_close(read_pixel(&mut target, 95, 95), (0.0, 1.0, 0.0, 1.0));
    }

    #[test]
    fn opacity_is_applied_to_the_background_image() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 100, export_height: 100, ..CanvasSettings::default() };
        doc.background = image_background(crate::model::BackgroundImageFit::Fill, 0.5);
        let bg = solid_surface(100, 100, Rgba::new(0.0, 1.0, 0.0, 1.0));

        let mut target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        compose(&doc, &target, 1.0, &HashMap::new(), Some(&bg), &ShadowCache::new()).unwrap();

        assert_close(read_pixel(&mut target, 50, 50), (0.0, 1.0, 0.0, 0.5));
    }

    fn shadow_test_doc(shadow: ShadowParams) -> (Document, uuid::Uuid) {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 200, export_height: 200, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_px: 50.0 };

        let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 100.0);
        el.shadow = shadow;
        let id = el.id;
        doc.elements = vec![el];
        (doc, id)
    }

    #[test]
    fn shadow_offset_moves_it_away_from_the_element() {
        let shadow = ShadowParams {
            enabled: true,
            offset_x: 20.0,
            offset_y: 20.0,
            blur: 0.0,
            opacity: 1.0,
            color: Rgba::new(0.0, 0.0, 0.0, 1.0),
        };
        let (doc, id) = shadow_test_doc(shadow);
        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(100, 100, Rgba::new(0.0, 1.0, 0.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 200, 200).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new()).unwrap();

        // Element at [50,150)x[50,150), shadow shifted by (20, 20) to
        // [70,170)x[70,170). (160, 160) is inside the shadow but past the
        // element's own edge, so only the shadow (black) shows there.
        assert_close(read_pixel(&mut target, 160, 160), (0.0, 0.0, 0.0, 1.0));
        // The element itself still draws on top of its own shadow.
        assert_close(read_pixel(&mut target, 100, 100), (0.0, 1.0, 0.0, 1.0));
        // Clearly outside both, the white background is untouched.
        assert_close(read_pixel(&mut target, 190, 190), (1.0, 1.0, 1.0, 1.0));
    }

    /// End-to-end regression test for the perf fix: re-composing the same
    /// document after only its *position* changed (here, a different
    /// margin — the same effect a Free-mode move drag has on `placement.x`/
    /// `y`) must reuse the already-rendered shadow bitmap rather than
    /// blurring a new one, when the two calls share one `ShadowCache`.
    #[test]
    fn recomposing_after_a_move_reuses_the_cached_shadow_bitmap() {
        let (mut doc, id) = shadow_test_doc(ShadowParams::standard());
        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(100, 100, Rgba::new(0.0, 1.0, 0.0, 1.0)));
        let cache = ShadowCache::new();

        let target = ImageSurface::create(Format::ARgb32, 200, 200).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &cache).unwrap();
        assert_eq!(cache.len(), 1);

        // "Move" the element without changing its size/shape/shadow — a
        // different margin shifts every placement exactly like a drag
        // would, with nothing the shadow's own bitmap depends on changed.
        doc.layout.margin_px = 65.0;
        compose(&doc, &target, 1.0, &resolved, None, &cache).unwrap();
        assert_eq!(cache.len(), 1, "moving the element should not have minted a second cached shadow bitmap");
    }

    /// The mirror case: changing something the shadow bitmap actually
    /// depends on (here, size, via a wider layout margin change is not
    /// enough -- use a second, larger element) does invalidate the cache.
    #[test]
    fn two_elements_with_matching_shadows_share_one_cached_bitmap() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 400, export_height: 200, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_px: 20.0 };

        let mut a = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 100.0);
        a.shadow = ShadowParams::standard();
        let mut b = ScreenshotElement::new(ImageSource::Path(PathBuf::from("b.png")), 100.0, 100.0);
        b.shadow = ShadowParams::standard();
        let (id_a, id_b) = (a.id, b.id);
        doc.elements = vec![a, b];

        let mut resolved = HashMap::new();
        resolved.insert(id_a, solid_surface(100, 100, Rgba::new(0.0, 1.0, 0.0, 1.0)));
        resolved.insert(id_b, solid_surface(100, 100, Rgba::new(0.0, 0.0, 1.0, 1.0)));

        let cache = ShadowCache::new();
        let target = ImageSurface::create(Format::ARgb32, 400, 200).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &cache).unwrap();

        assert_eq!(cache.len(), 1, "two elements with identical size/shape/shadow should share one cached bitmap");
    }

    /// Stability regression test: a pathologically large screenshot (spec's
    /// "large screenshots" + shadow combination) must render without
    /// erroring and without the shadow bitmap growing past
    /// `MAX_SHADOW_SURFACE_DIM` in either dimension — before
    /// `shadow_render_scale` capped it, this shape would have asked Cairo
    /// to allocate a multi-gigabyte surface.
    #[test]
    fn a_very_large_shadowed_screenshot_does_not_blow_up_the_shadow_surface() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 20_100, export_height: 20_100, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_px: 50.0 };

        let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("huge.png")), 20_000.0, 20_000.0);
        el.shadow = ShadowParams::floating();
        let id = el.id;
        doc.elements = vec![el];

        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(1, 1, Rgba::new(0.0, 1.0, 0.0, 1.0)));

        // A tiny target surface (well below the document's own huge
        // export_width/height) mimics a downscaled interactive preview --
        // exactly the case `shadow_render_scale` optimizes for.
        let scale = 100.0 / 20_100.0;
        let target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        let cache = ShadowCache::new();

        compose(&doc, &target, scale, &resolved, None, &cache).expect("a huge element's shadow must render, not error or abort");
    }

    /// Same shape, but at `scale = 1.0` (a full-resolution export of a huge
    /// canvas, rather than a downscaled preview) — this is what actually
    /// exercises `MAX_SHADOW_SURFACE_DIM` rather than an already-small
    /// preview scale doing the capping on its own. Without the cap, this
    /// would ask Cairo for a ~20000x20000 ARGB32 surface (~1.6GB) per
    /// shadow.
    #[test]
    fn a_full_resolution_export_of_a_huge_shadowed_screenshot_stays_bounded() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 20_100, export_height: 20_100, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_px: 50.0 };

        let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("huge.png")), 20_000.0, 20_000.0);
        el.shadow = ShadowParams::floating();
        let id = el.id;
        doc.elements = vec![el];

        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(1, 1, Rgba::new(0.0, 1.0, 0.0, 1.0)));

        // The target itself is downscaled from the document's own huge
        // export size purely so this test doesn't need to allocate a
        // multi-gigabyte *target* surface too -- `scale` (not the target's
        // own size) is what `shadow_render_scale` bases its cap on, so
        // this still exercises the same code path a real 1:1 export would.
        let target = ImageSurface::create(Format::ARgb32, 500, 500).unwrap();
        let cache = ShadowCache::new();

        compose(&doc, &target, 1.0, &resolved, None, &cache)
            .expect("a full-resolution shadow on a huge element must still render, not error or abort");
    }

    #[test]
    fn shadow_without_blur_has_a_sharp_edge() {
        let shadow =
            ShadowParams { enabled: true, offset_x: 0.0, offset_y: 0.0, blur: 0.0, opacity: 1.0, color: Rgba::new(0.0, 0.0, 0.0, 1.0) };
        let (doc, id) = shadow_test_doc(shadow);
        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(100, 100, Rgba::new(0.0, 1.0, 0.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 200, 200).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new()).unwrap();

        // Element/shadow both at [50,150)x[50,150) (zero offset) -- just
        // past the shared edge, the unblurred shadow leaves pure background.
        assert_close(read_pixel(&mut target, 160, 100), (1.0, 1.0, 1.0, 1.0));
    }

    #[test]
    fn shadow_blur_softens_and_extends_beyond_the_sharp_edge() {
        let shadow = ShadowParams {
            enabled: true,
            offset_x: 0.0,
            offset_y: 0.0,
            blur: 15.0,
            opacity: 1.0,
            color: Rgba::new(0.0, 0.0, 0.0, 1.0),
        };
        let (doc, id) = shadow_test_doc(shadow);
        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(100, 100, Rgba::new(0.0, 1.0, 0.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 200, 200).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new()).unwrap();

        // Same point that stayed pure white with no blur now has some
        // shadow darkness bled into it.
        let just_outside = read_pixel(&mut target, 160, 100);
        assert!(just_outside.0 < 0.99, "expected blur to darken the background past the sharp edge, got {just_outside:?}");
    }

    fn text_test_doc(title: crate::model::TextElement) -> Document {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 200, export_height: 100, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.title = title;
        doc
    }

    /// A title/label with absolute placement at `(10, 10)`, black text,
    /// no background/shadow -- the minimal shape most of these tests only
    /// need to vary `content`/`enabled` on.
    fn absolute_title(enabled: bool, content: &str) -> crate::model::TextElement {
        crate::model::TextElement {
            enabled,
            content: content.to_string(),
            position: crate::model::TextPosition::Absolute { x: 10.0, y: 10.0 },
            typography: crate::model::Typography { font_size: 24.0, color: Rgba::BLACK, ..crate::model::Typography::title_default() },
            ..crate::model::TextElement::title_default()
        }
    }

    /// Any non-background pixel within the caption's expected bounding
    /// area -- exact glyph shapes depend on the system's installed fonts,
    /// so this only checks that *some* ink landed roughly where expected,
    /// not a pixel-perfect match.
    fn any_ink_in_region(target: &mut ImageSurface, x0: i32, y0: i32, x1: i32, y1: i32) -> bool {
        for y in y0..y1 {
            for x in x0..x1 {
                let (r, g, b, a) = read_pixel(target, x, y);
                if a > 0.0 && (r, g, b) != (1.0, 1.0, 1.0) {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn disabled_title_draws_nothing() {
        let doc = text_test_doc(absolute_title(false, "Hallo"));
        let mut target = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
        compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new()).unwrap();

        assert!(!any_ink_in_region(&mut target, 0, 0, 200, 100), "a disabled title should draw no ink at all");
    }

    #[test]
    fn empty_content_draws_nothing_even_when_enabled() {
        let doc = text_test_doc(absolute_title(true, ""));
        let mut target = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
        compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new()).unwrap();

        assert!(!any_ink_in_region(&mut target, 0, 0, 200, 100), "empty content should draw no ink");
    }

    #[test]
    fn enabled_title_draws_ink_near_its_position() {
        let doc = text_test_doc(absolute_title(true, "Hallo"));
        let mut target = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
        compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new()).unwrap();

        assert!(any_ink_in_region(&mut target, 5, 5, 120, 45), "expected the caption's glyphs somewhere near (10, 10)");
        assert!(!any_ink_in_region(&mut target, 0, 60, 200, 100), "far from the caption, the background should stay untouched");
    }

    /// Regression test for spec §16 "responsive positioning": a
    /// canvas-relative, semantically top-centered title must land at a
    /// *different* horizontal position when the export width changes,
    /// rather than staying at whatever pixel a fixed x/y would have used.
    #[test]
    fn semantic_title_position_follows_a_changed_canvas_size() {
        let title = crate::model::TextElement {
            enabled: true,
            content: "Title".to_string(),
            position: crate::model::TextPosition::Semantic {
                horizontal: crate::model::HorizontalAnchor::Center,
                vertical: crate::model::VerticalAnchor::Top,
                padding: 4.0,
            },
            typography: crate::model::Typography { font_size: 16.0, color: Rgba::BLACK, ..crate::model::Typography::title_default() },
            ..crate::model::TextElement::title_default()
        };

        // The 200px-wide canvas's own horizontal center is x=100 — check
        // a fixed region around *that* absolute point in both renders. A
        // title that's still tracking "centered" after the canvas widens
        // to 800 (center x=400) should have moved well clear of it.
        let ink_near_x100_at_200_wide = {
            let mut doc = text_test_doc(title.clone());
            doc.canvas = CanvasSettings { export_width: 200, export_height: 100, ..CanvasSettings::default() };
            let mut target = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
            compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new()).unwrap();
            any_ink_in_region(&mut target, 70, 0, 130, 40)
        };

        let ink_near_x100_at_800_wide = {
            let mut doc = text_test_doc(title);
            doc.canvas = CanvasSettings { export_width: 800, export_height: 100, ..CanvasSettings::default() };
            let mut target = ImageSurface::create(Format::ARgb32, 800, 100).unwrap();
            compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new()).unwrap();
            any_ink_in_region(&mut target, 70, 0, 130, 40)
        };

        assert!(ink_near_x100_at_200_wide, "expected the centered title near x=100, the 200px canvas's own center");
        assert!(
            !ink_near_x100_at_800_wide,
            "expected the title to have moved away from x=100 once the canvas widened and recentered around x=400"
        );
    }

    #[test]
    fn title_with_a_solid_background_paints_a_filled_box_behind_the_text() {
        let title = crate::model::TextElement {
            enabled: true,
            content: "Hi".to_string(),
            position: crate::model::TextPosition::Absolute { x: 20.0, y: 20.0 },
            background: crate::model::TextBackground::Solid(Rgba::new(0.0, 0.0, 1.0, 1.0)),
            background_padding: 10.0,
            typography: crate::model::Typography { font_size: 16.0, color: Rgba::WHITE, ..crate::model::Typography::title_default() },
            ..crate::model::TextElement::title_default()
        };
        let doc = text_test_doc(title);
        let mut target = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
        compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new()).unwrap();

        // Just inside the box's padding (before any glyph ink starts),
        // the background's blue fill should show through untouched.
        let (r, g, b, a) = read_pixel(&mut target, 22, 22);
        assert!(a > 0.0 && b > r && b > g, "expected the blue background box to be visible near its corner, got ({r}, {g}, {b}, {a})");
    }

    /// End-to-end regression test for spec §11: a screenshot's own label
    /// must render, positioned relative to *that screenshot's* placement
    /// rect, not the whole canvas.
    #[test]
    fn screenshot_label_renders_relative_to_its_own_screenshot() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 300, export_height: 200, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_px: 50.0 };

        let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 100.0);
        el.label = crate::model::TextElement {
            enabled: true,
            content: "Label".to_string(),
            position: crate::model::TextPosition::Absolute { x: 5.0, y: 5.0 },
            typography: crate::model::Typography { font_size: 16.0, color: Rgba::BLACK, ..crate::model::Typography::label_default() },
            ..crate::model::TextElement::label_default()
        };
        let id = el.id;
        doc.elements = vec![el];

        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(100, 100, Rgba::new(0.0, 1.0, 0.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 300, 200).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new()).unwrap();

        // The screenshot sits at [50,150)x[50,150) (margin 50); its label
        // is placed at a local (5, 5) offset, i.e. near document (55, 55).
        assert!(any_ink_in_region(&mut target, 50, 50, 90, 80), "expected the label's glyphs near the screenshot's own top-left");
    }

    #[test]
    fn disabled_screenshot_label_draws_nothing() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 200, export_height: 200, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_px: 50.0 };

        let el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 100.0);
        let id = el.id;
        doc.elements = vec![el]; // label left at its disabled default

        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(100, 100, Rgba::new(0.0, 1.0, 0.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 200, 200).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new()).unwrap();

        // Below the screenshot (where a bottom-anchored label would land
        // if it were somehow enabled) should stay pure background.
        assert!(!any_ink_in_region(&mut target, 50, 150, 150, 200), "a disabled label should draw no ink at all");
    }
}
