//! Cairo composition function shared between the interactive preview and
//! full-resolution export — the same `compose()` call renders either, only
//! the target surface's size and `scale` differ, which is how preview and
//! export stay pixel-identical (mod resolution/antialiasing).

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI};
use std::rc::Rc;

use cairo::{Context, LinearGradient, RadialGradient};
use thiserror::Error;
use uuid::Uuid;

use crate::background_cache::BackgroundCache;
use crate::layout::{compute_layout, Placement};
use crate::model::{
    Background, BackgroundImageFit, Callout, CornerRadius, Document, GeneratedBackground, GradientKind, ScreenshotElement,
    ShadowParams, TextAlign, TextBackground, TextElement,
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
    background_cache: &BackgroundCache,
) -> Result<(), RenderError> {
    shadow_cache.begin_frame();
    let ctx = Context::new(target)?;
    ctx.scale(scale, scale);

    let visible: Vec<ScreenshotElement> = doc.elements.iter().filter(|e| e.visible).cloned().collect();
    let placements = compute_layout(doc.layout.mode, &visible, doc.layout.spacing_px, doc.layout.margin_x, doc.layout.margin_y);
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
        scale,
        background_cache,
    )?;

    draw_elements(&ctx, doc, &visible, &placements, resolved_images, scale, shadow_cache)
}

/// Renders every visible element (screenshots, their shadows, labels, and
/// callouts) onto `target` — everything [`compose`] draws *except* the
/// background — so a caller that can prove the background hasn't changed
/// can composite the two independently instead of paying for both every
/// frame. `target` should start fully transparent (a fresh `ImageSurface`
/// already is), since this never paints anything outside each element's
/// own footprint. Used by the interactive canvas while a wallpaper drag is
/// in progress: it renders this once at drag-start (see
/// `app/src/canvas/mod.rs`'s `try_begin_wallpaper_drag`) and reuses the
/// same bitmap, unmoved, for the whole drag, since dragging the background
/// never moves any element. Shares its actual drawing code with `compose`
/// (via `draw_elements`), so the two-layer composite this enables is
/// pixel-identical to `compose`'s own single-pass output.
pub fn compose_elements(
    doc: &Document,
    target: &cairo::ImageSurface,
    scale: f64,
    resolved_images: &HashMap<Uuid, cairo::ImageSurface>,
    shadow_cache: &ShadowCache,
) -> Result<(), RenderError> {
    shadow_cache.begin_frame();
    let ctx = Context::new(target)?;
    ctx.scale(scale, scale);

    let visible: Vec<ScreenshotElement> = doc.elements.iter().filter(|e| e.visible).cloned().collect();
    let placements = compute_layout(doc.layout.mode, &visible, doc.layout.spacing_px, doc.layout.margin_x, doc.layout.margin_y);

    draw_elements(&ctx, doc, &visible, &placements, resolved_images, scale, shadow_cache)
}

/// The shared drawing loop behind both [`compose`] and [`compose_elements`]
/// — every visible element's screenshot, shadow, label, and callouts, onto
/// `ctx` (already scaled by the caller's `scale`).
fn draw_elements(
    ctx: &Context,
    doc: &Document,
    visible: &[ScreenshotElement],
    placements: &[Placement],
    resolved_images: &HashMap<Uuid, cairo::ImageSurface>,
    scale: f64,
    shadow_cache: &ShadowCache,
) -> Result<(), RenderError> {
    // Every element is placed in "nominal" layout space by `compute_layout`
    // (which knows nothing about negative-padding labels/callouts pushing
    // content past the canvas's own edge) — `content_offset_x/y` (set by
    // `crate::layout::fit_canvas_to_content`) shifts that whole space into
    // its correct final position in one step, applied only here and never
    // to the background itself, which must always fill the canvas's own
    // full, unshifted bounds.
    ctx.save()?;
    ctx.translate(doc.canvas.content_offset_x, doc.canvas.content_offset_y);

    for (el, placement) in visible.iter().zip(placements.iter()) {
        let image = resolved_images.get(&el.id).ok_or(RenderError::MissingImage(el.id))?;

        ctx.save()?;
        let cx = placement.x + placement.width / 2.0;
        let cy = placement.y + placement.height / 2.0;
        ctx.translate(cx, cy);
        ctx.rotate(el.transform.rotation_deg.to_radians());
        ctx.translate(-placement.width / 2.0, -placement.height / 2.0);

        if el.shadow.enabled {
            draw_shadow(ctx, placement.width, placement.height, &el.corner_radius, &el.shadow, scale, shadow_cache)?;
        }

        rounded_rect_path(ctx, 0.0, 0.0, placement.width, placement.height, &el.corner_radius);
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
            let resolved_label = el.label.resolve(&doc.label_defaults);
            draw_text_element(ctx, &resolved_label, placement.width, placement.height, scale, shadow_cache)?;
        }

        // Drawn after the label, in the same screenshot-relative space, so
        // a callout's arrow can point anywhere within (or, via a bubble
        // placed near an edge, right up against) the screenshot regardless
        // of where the label itself sits.
        for callout in &el.callouts {
            if callout.enabled && !callout.text.content.is_empty() {
                draw_callout(ctx, callout, placement.width, placement.height, scale, shadow_cache)?;
            }
        }

        ctx.restore()?;
    }

    ctx.restore()?;
    Ok(())
}

/// Draws one [`Callout`] — its text bubble (via `draw_text_element`) plus a
/// straight arrow from the bubble's edge to `callout.target_x/target_y`
/// (a fraction of `ref_w`×`ref_h`). The arrow starts at whichever point on
/// the bubble's own rectangle faces the target, rather than its center, so
/// the line never gets drawn underneath the text itself.
fn draw_callout(ctx: &Context, callout: &Callout, ref_w: f64, ref_h: f64, scale: f64, cache: &ShadowCache) -> Result<(), RenderError> {
    let (box_x, box_y, box_w, box_h) = measure_text_box(&callout.text, ref_w, ref_h)?;
    let target = (callout.target_x.clamp(0.0, 1.0) * ref_w, callout.target_y.clamp(0.0, 1.0) * ref_h);
    let start = box_edge_toward((box_x, box_y, box_w, box_h), target);

    let c = callout.arrow_color;
    ctx.set_source_rgba(c.r, c.g, c.b, c.a);
    ctx.set_line_width(callout.arrow_width.max(0.1));
    ctx.set_line_cap(cairo::LineCap::Round);
    ctx.move_to(start.0, start.1);
    ctx.line_to(target.0, target.1);
    ctx.stroke()?;
    draw_arrowhead(ctx, start, target, (callout.arrow_width * 3.0).max(4.0));

    // A small filled marker at the exact target point, on top of the line
    // (which runs to the same point, so it ends up hidden under the dot) —
    // makes the thing the callout is actually pointing at unambiguous, the
    // way an on-screen annotation tool's marker dot does. Reuses the
    // arrow's own color for the fill rather than adding a separate one,
    // since the dot and arrow read as one "pointer" unit; `dot_radius` is
    // the fill's own radius only. Ringed with a white border whose width
    // is the arrow's own line width rather than a separate setting (spec:
    // "der Rand um den Punkt soll sich aus der Linienstärke des Pfeils
    // ergeben") — keeps the dot legible against a same-colored background
    // the way the arrow's white-free line alone wouldn't be.
    if callout.dot_radius > 0.0 {
        ctx.set_source_rgba(c.r, c.g, c.b, c.a);
        ctx.arc(target.0, target.1, callout.dot_radius, 0.0, 2.0 * PI);
        ctx.fill()?;

        if callout.arrow_width > 0.0 {
            ctx.set_source_rgba(1.0, 1.0, 1.0, 1.0);
            ctx.set_line_width(callout.arrow_width);
            ctx.arc(target.0, target.1, callout.dot_radius + callout.arrow_width / 2.0, 0.0, 2.0 * PI);
            ctx.stroke()?;
        }
    }

    draw_text_element(ctx, &callout.text, ref_w, ref_h, scale, cache)
}

/// The point on a `box_w`×`box_h` rectangle's own perimeter (at `box_x`,
/// `box_y`) that faces `target` — found by projecting the center-to-target
/// direction out to whichever axis (horizontal or vertical) the box's own
/// half-extent is reached first. Falls back to the box's center if `target`
/// coincides with it (a zero-length direction has no "facing" side).
fn box_edge_toward(box_rect: (f64, f64, f64, f64), target: (f64, f64)) -> (f64, f64) {
    let (box_x, box_y, box_w, box_h) = box_rect;
    let (cx, cy) = (box_x + box_w / 2.0, box_y + box_h / 2.0);
    let (dx, dy) = (target.0 - cx, target.1 - cy);
    if dx == 0.0 && dy == 0.0 {
        return (cx, cy);
    }
    let t_x = if dx != 0.0 { (box_w / 2.0) / dx.abs() } else { f64::INFINITY };
    let t_y = if dy != 0.0 { (box_h / 2.0) / dy.abs() } else { f64::INFINITY };
    let t = t_x.min(t_y);
    (cx + dx * t, cy + dy * t)
}

/// Fills a small triangular arrowhead at `to`, oriented along the
/// `from`→`to` direction — drawn as a separate filled path rather than a
/// stroke join, so its size doesn't depend on `arrow_width`'s own line-cap
/// rendering.
fn draw_arrowhead(ctx: &Context, from: (f64, f64), to: (f64, f64), size: f64) {
    let angle = (to.1 - from.1).atan2(to.0 - from.0);
    const SPREAD: f64 = 0.45;
    let p1 = (to.0 - size * (angle - SPREAD).cos(), to.1 - size * (angle - SPREAD).sin());
    let p2 = (to.0 - size * (angle + SPREAD).cos(), to.1 - size * (angle + SPREAD).sin());
    ctx.move_to(to.0, to.1);
    ctx.line_to(p1.0, p1.1);
    ctx.line_to(p2.0, p2.1);
    ctx.close_path();
    let _ = ctx.fill();
}

/// Builds the Pango layout for `text` — font, alignment, line spacing,
/// letter spacing, wrapping — and returns it alongside its measured content
/// size in pixels. Shared by `draw_text_element` (which also shows the
/// layout) and `measure_text_box` (which only needs the size), so the two
/// never risk disagreeing about how big a label's text actually is.
fn build_text_layout(ctx: &Context, text: &TextElement, ref_w: f64) -> (pango::Layout, f64, f64) {
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

    // Pango already treats a literal `\n` in `content` as a hard line
    // break, so multi-line text needs no special handling beyond this
    // `set_text` call — the manual/automatic distinction below is purely
    // about whether *long* lines also get word-wrapped.
    layout.set_text(&text.content);

    // A label/callout must never grow wider than its own screenshot
    // (`ref_w`, see `TextElement::wrap_width`'s own doc comment) —
    // `typography.wrap` on proactively word-wraps to fill that width;
    // off, manual line breaks are respected as typed and width-wrapping
    // only kicks in as a fallback for a single line that's too long on
    // its own, so short manually-broken text isn't needlessly reflowed.
    let max_width = text.wrap_width(ref_w);
    let needs_width_constraint = if text.typography.wrap {
        true
    } else {
        let (_, natural) = layout.pixel_extents();
        natural.width() as f64 > max_width
    };
    if needs_width_constraint {
        layout.set_width((max_width * pango::SCALE as f64) as i32);
        layout.set_wrap(pango::WrapMode::Word);
    }

    let (_, logical) = layout.pixel_extents();
    (layout, logical.width() as f64, logical.height() as f64)
}

/// The `(box_x, box_y, box_w, box_h)` a [`TextElement`] would occupy within
/// a `ref_w`×`ref_h` reference rect, without drawing anything — pure
/// measurement, for hit-testing/dragging a label on the canvas widget
/// (which needs to know where it is without re-running the whole
/// composition). Pango layout needs a `cairo::Context` to measure against
/// even when nothing is painted, hence the scratch 1x1 surface.
pub fn measure_text_box(text: &TextElement, ref_w: f64, ref_h: f64) -> Result<(f64, f64, f64, f64), RenderError> {
    let scratch = cairo::ImageSurface::create(cairo::Format::ARgb32, 1, 1)?;
    let ctx = Context::new(&scratch)?;
    let (_, content_w, content_h) = build_text_layout(&ctx, text, ref_w);
    let box_w = content_w + 2.0 * text.padding_x;
    let box_h = content_h + 2.0 * text.padding_y;
    let (box_x, box_y) = text.position.resolve_box_origin(ref_w, ref_h, box_w, box_h);
    Ok((box_x, box_y, box_w, box_h))
}

/// Draws one [`TextElement`] — its box shadow, background, and the text
/// itself, in that order — within a `ref_w`×`ref_h` reference rect. `ctx`
/// is expected to already be at that rect's own origin (`(0, 0)` is the
/// rect's top-left) — one screenshot's own placement for its label; see
/// `compose`'s call site. A no-op when `text` is disabled or empty (checked
/// by the caller too, so this never runs Pango layout on nothing).
///
/// Uses Pango (via `pangocairo`), the GNOME stack's own text layout engine
/// — not Cairo's "toy" text API — so font family/weight/italic/alignment/
/// wrapping/letter- and line-spacing all come from the same fontconfig-
/// backed shaping every other GTK app on the system uses, rather than a
/// second, ad-hoc text system.
fn draw_text_element(ctx: &Context, text: &TextElement, ref_w: f64, ref_h: f64, scale: f64, cache: &ShadowCache) -> Result<(), RenderError> {
    let (layout, content_w, content_h) = build_text_layout(ctx, text, ref_w);
    let box_w = content_w + 2.0 * text.padding_x;
    let box_h = content_h + 2.0 * text.padding_y;

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
    ctx.move_to(text.padding_x, text.padding_y);
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
    scale: f64,
    background_cache: &BackgroundCache,
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
            let surface = generated_background_bitmap(generated, width, height, scale, screenshot_regions, background_cache)?;
            ctx.save()?;
            ctx.scale(1.0 / scale, 1.0 / scale);
            paint_generated_background_bitmap(ctx, &surface, 0.0, 0.0)?;
            ctx.restore()?;
        }
    }
    Ok(())
}

/// Returns the cached (or freshly rendered + cached) bitmap for a
/// `Background::Generated` background on its own, without painting it —
/// shared by `draw_background`'s own `Generated` arm above and directly by
/// the interactive canvas (`app/src/canvas/mod.rs`'s
/// `try_begin_wallpaper_drag`), which grabs this exact bitmap once when a
/// wallpaper drag begins so the whole drag can reuse it (via
/// [`paint_generated_background_bitmap`]) instead of re-rasterizing the
/// wave layers on every mouse-move. That reuse only works because the
/// document's `GeneratedBackground` itself is never mutated until the drag
/// commits — see `crate::background_cache`.
pub fn generated_background_bitmap(
    bg: &GeneratedBackground,
    width: f64,
    height: f64,
    scale: f64,
    regions: &[crate::generator::ScreenshotRegion],
    cache: &BackgroundCache,
) -> Result<Rc<cairo::ImageSurface>, RenderError> {
    cache.get_or_render(bg, width, height, regions, || render_generated_background_2x(bg, width, height, scale, regions))
}

/// Paints a bitmap previously obtained from [`generated_background_bitmap`]
/// onto `ctx` at `(x, y)`, top-left aligned, undoing exactly the
/// supersampling `render_generated_background_2x` baked into its pixel
/// dimensions — nothing else. Callers are responsible for any additional
/// transform of their own: `draw_background` above first undoes its
/// context's document-to-device `scale` (since the bitmap is already at
/// device resolution), while the interactive canvas's drag-preview path
/// (`app/src/canvas/mod.rs`) calls this directly on an unscaled,
/// device-pixel context and passes `(x, y)` as a device-pixel offset.
pub fn paint_generated_background_bitmap(ctx: &Context, surface: &cairo::ImageSurface, x: f64, y: f64) -> Result<(), RenderError> {
    ctx.save()?;
    ctx.translate(x, y);
    ctx.scale(1.0 / BACKGROUND_SUPERSAMPLE, 1.0 / BACKGROUND_SUPERSAMPLE);
    let pattern = cairo::SurfacePattern::create(surface);
    pattern.set_filter(cairo::Filter::Good);
    ctx.set_source(&pattern)?;
    ctx.paint()?;
    ctx.restore()?;
    Ok(())
}

/// How much finer than the canvas's own render resolution the generated
/// background is rasterized at — a hand-drawn reference wallpaper looks
/// crisp because it's vector art; rendering the procedural wave layers at
/// the canvas's exact pixel resolution can leave thin bands looking slightly
/// soft after Cairo's antialiasing, which supersampling then downscaling
/// (via `Filter::Good`) fixes without changing the canvas/export size
/// itself (spec: "Die Größe des Canvas darf sich dadurch nicht verändern").
const BACKGROUND_SUPERSAMPLE: f64 = 2.0;

/// Renders `generated` onto a fresh, standalone `ImageSurface` at
/// `BACKGROUND_SUPERSAMPLE` times the effective render resolution
/// (`scale * BACKGROUND_SUPERSAMPLE` device pixels per document pixel) —
/// the whole "2x resolution" implementation, since `generator::render`
/// already draws pure vector geometry in document-pixel coordinates and
/// needs nothing else to benefit from a finer target. Cached by
/// `BackgroundCache` (see `draw_background`'s `Generated` arm) so this only
/// runs again when the background's own parameters, the canvas size, or the
/// screenshot layout actually change — never on a label/callout drag.
fn render_generated_background_2x(
    generated: &GeneratedBackground,
    width: f64,
    height: f64,
    scale: f64,
    screenshot_regions: &[crate::generator::ScreenshotRegion],
) -> Result<cairo::ImageSurface, RenderError> {
    let device_scale = scale * BACKGROUND_SUPERSAMPLE;
    let surf_w = (width * device_scale).round().max(1.0) as i32;
    let surf_h = (height * device_scale).round().max(1.0) as i32;
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, surf_w, surf_h)?;
    let scratch_ctx = Context::new(&surface)?;
    scratch_ctx.scale(device_scale, device_scale);
    crate::generator::render(&scratch_ctx, generated, width, height, screenshot_regions)?;
    Ok(surface)
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
    use crate::model::{
        Background, CanvasSettings, Document, GradientKind, GradientSpec, ImageSource, Label, LabelStyle, LayoutSettings, Rgba,
        ScreenshotElement,
    };
    use cairo::{Format, ImageSurface};
    use std::path::PathBuf;

    /// Splits a fully-specified `TextElement` (as most of these tests
    /// already build one, via `absolute_title`/inline construction) into
    /// the shared `LabelStyle` it implies plus the plain `Label` for one
    /// element — `resolve()`ing that `Label` against the returned style
    /// then reproduces the input `TextElement` unchanged. Lets every
    /// existing "build a `TextElement`, render it" test keep working
    /// unmodified now that a label's look always comes from
    /// `Document::label_defaults` rather than anything per-label.
    fn split_label_style(text: crate::model::TextElement) -> (LabelStyle, Label) {
        let style = LabelStyle {
            position: text.position,
            typography: text.typography.clone(),
            background: text.background.clone(),
            corner_radius: text.corner_radius,
            padding_x: text.padding_x,
            padding_y: text.padding_y,
            shadow: text.shadow,
        };
        (style, Label { enabled: text.enabled, content: text.content })
    }

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
        compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

        assert_close(read_pixel(&mut target, 5, 5), (0.2, 0.4, 0.6, 1.0));
        assert_close(read_pixel(&mut target, 195, 95), (0.2, 0.4, 0.6, 1.0));
    }

    #[test]
    fn generated_background_paints_something_and_leaves_the_canvas_size_unchanged() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 200, export_height: 100, ..CanvasSettings::default() };
        doc.background = Background::Generated(crate::model::GeneratedBackground::new(7));

        let mut target = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
        let cache = BackgroundCache::new();
        compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new(), &cache).unwrap();

        assert_eq!(target.width(), 200);
        assert_eq!(target.height(), 100);
        assert!(cache.is_cached());
        let painted = (0..100)
            .flat_map(|y| (0..200).map(move |x| (x, y)))
            .any(|(x, y)| read_pixel(&mut target, x, y).3 > 0.0);
        assert!(painted, "the 2x-supersampled background should still paint visible pixels onto the 1x canvas");
    }

    /// Regression test for the interactive canvas's wallpaper-drag preview
    /// (`app/src/canvas/mod.rs`'s `WallpaperPreview`): painting
    /// `generated_background_bitmap`'s bitmap via
    /// `paint_generated_background_bitmap`, then `compose_elements` on top,
    /// must land on the exact same pixels as a single `compose()` call —
    /// otherwise the live preview (background + elements painted as two
    /// separate blits) would visibly snap to a different image the moment
    /// the real `compose()` takes over on drag release.
    #[test]
    fn two_layer_composite_matches_a_single_compose_call() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 200, export_height: 100, ..CanvasSettings::default() };
        doc.background = Background::Generated(crate::model::GeneratedBackground::new(11));
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 10.0, margin_x: 20.0, margin_y: 20.0 };
        let el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 60.0, 60.0);
        let el_id = el.id;
        doc.elements = vec![el];
        let mut resolved = HashMap::new();
        resolved.insert(el_id, solid_surface(60, 60, Rgba::new(1.0, 0.0, 0.0, 1.0)));

        let mut single_pass = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
        compose(&doc, &single_pass, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

        let cache = BackgroundCache::new();
        let bg_surface = generated_background_bitmap(
            match &doc.background {
                Background::Generated(g) => g,
                _ => unreachable!(),
            },
            200.0,
            100.0,
            1.0,
            &[crate::generator::ScreenshotRegion { x: 20.0, y: 20.0, width: 60.0, height: 60.0 }],
            &cache,
        )
        .unwrap();

        let mut two_layer = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
        {
            let ctx = Context::new(&two_layer).unwrap();
            paint_generated_background_bitmap(&ctx, &bg_surface, 0.0, 0.0).unwrap();
        }
        compose_elements(&doc, &two_layer, 1.0, &resolved, &ShadowCache::new()).unwrap();

        for (x, y) in [(5, 5), (100, 50), (195, 95), (50, 50), (140, 50)] {
            assert_close(read_pixel(&mut single_pass, x, y), read_pixel(&mut two_layer, x, y));
        }
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
        compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

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
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 10.0, margin_x: 20.0, margin_y: 20.0 };

        let red = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 160.0);
        let blue = ScreenshotElement::new(ImageSource::Path(PathBuf::from("b.png")), 100.0, 160.0);
        let (red_id, blue_id) = (red.id, blue.id);
        doc.elements = vec![red, blue];

        let mut resolved = HashMap::new();
        resolved.insert(red_id, solid_surface(100, 160, Rgba::new(1.0, 0.0, 0.0, 1.0)));
        resolved.insert(blue_id, solid_surface(100, 160, Rgba::new(0.0, 0.0, 1.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 300, 200).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

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

        let err = compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new(), &BackgroundCache::new()).unwrap_err();
        assert!(matches!(err, RenderError::MissingImage(_)));
    }

    #[test]
    fn rounded_corners_clip_the_element_at_the_corner_pixel() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 100, export_height: 100, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_x: 0.0, margin_y: 0.0 };

        let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 100.0);
        el.corner_radius = CornerRadius::uniform(20.0);
        let id = el.id;
        doc.elements = vec![el];

        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(100, 100, Rgba::new(0.0, 0.0, 0.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

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
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_x: 0.0, margin_y: 0.0 };

        let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 100.0);
        el.transform.flip_horizontal = true;
        let id = el.id;
        doc.elements = vec![el];

        let mut resolved = HashMap::new();
        resolved.insert(id, split_surface(100, 100, Rgba::new(1.0, 0.0, 0.0, 1.0), Rgba::new(0.0, 0.0, 1.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

        // Unflipped this would be red-left/blue-right; flipped it's reversed.
        assert_close(read_pixel(&mut target, 10, 50), (0.0, 0.0, 1.0, 1.0));
        assert_close(read_pixel(&mut target, 90, 50), (1.0, 0.0, 0.0, 1.0));
    }

    #[test]
    fn no_flip_keeps_original_orientation() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 100, export_height: 100, ..CanvasSettings::default() };
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_x: 0.0, margin_y: 0.0 };

        let el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 100.0);
        let id = el.id;
        doc.elements = vec![el];

        let mut resolved = HashMap::new();
        resolved.insert(id, split_surface(100, 100, Rgba::new(1.0, 0.0, 0.0, 1.0), Rgba::new(0.0, 0.0, 1.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

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

        let err = compose(&doc, &target, 1.0, &HashMap::new(), None, &ShadowCache::new(), &BackgroundCache::new()).unwrap_err();
        assert!(matches!(err, RenderError::MissingBackgroundImage));
    }

    #[test]
    fn cover_scales_up_to_fill_the_canvas_with_no_gaps() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 100, export_height: 100, ..CanvasSettings::default() };
        doc.background = image_background(crate::model::BackgroundImageFit::Cover, 1.0);
        let bg = solid_surface(200, 100, Rgba::new(0.0, 1.0, 0.0, 1.0));

        let mut target = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        compose(&doc, &target, 1.0, &HashMap::new(), Some(&bg), &ShadowCache::new(), &BackgroundCache::new()).unwrap();

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
        compose(&doc, &target, 1.0, &HashMap::new(), Some(&bg), &ShadowCache::new(), &BackgroundCache::new()).unwrap();

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
        compose(&doc, &target, 1.0, &HashMap::new(), Some(&bg), &ShadowCache::new(), &BackgroundCache::new()).unwrap();

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
        compose(&doc, &target, 1.0, &HashMap::new(), Some(&bg), &ShadowCache::new(), &BackgroundCache::new()).unwrap();

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
        compose(&doc, &target, 1.0, &HashMap::new(), Some(&bg), &ShadowCache::new(), &BackgroundCache::new()).unwrap();

        assert_close(read_pixel(&mut target, 50, 50), (0.0, 1.0, 0.0, 0.5));
    }

    fn shadow_test_doc(shadow: ShadowParams) -> (Document, uuid::Uuid) {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 200, export_height: 200, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_x: 50.0, margin_y: 50.0 };

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
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

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
        compose(&doc, &target, 1.0, &resolved, None, &cache, &BackgroundCache::new()).unwrap();
        assert_eq!(cache.len(), 1);

        // "Move" the element without changing its size/shape/shadow — a
        // different margin shifts every placement exactly like a drag
        // would, with nothing the shadow's own bitmap depends on changed.
        doc.layout.margin_x = 65.0;
        doc.layout.margin_y = 65.0;
        compose(&doc, &target, 1.0, &resolved, None, &cache, &BackgroundCache::new()).unwrap();
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
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_x: 20.0, margin_y: 20.0 };

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
        compose(&doc, &target, 1.0, &resolved, None, &cache, &BackgroundCache::new()).unwrap();

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
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_x: 50.0, margin_y: 50.0 };

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

        compose(&doc, &target, scale, &resolved, None, &cache, &BackgroundCache::new()).expect("a huge element's shadow must render, not error or abort");
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
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_x: 50.0, margin_y: 50.0 };

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

        compose(&doc, &target, 1.0, &resolved, None, &cache, &BackgroundCache::new())
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
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

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
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

        // Same point that stayed pure white with no blur now has some
        // shadow darkness bled into it.
        let just_outside = read_pixel(&mut target, 160, 100);
        assert!(just_outside.0 < 0.99, "expected blur to darken the background past the sharp edge, got {just_outside:?}");
    }

    /// A document with one screenshot placed exactly at the canvas origin
    /// (margin/spacing 0, natural size == canvas size), so the screenshot's
    /// own local coordinate space lines up 1:1 with document space —
    /// letting these tests use the same absolute coordinates a canvas-wide
    /// element would have used, while actually exercising a screenshot's
    /// own label.
    fn text_test_doc(label: crate::model::TextElement) -> Document {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 200, export_height: 100, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_x: 0.0, margin_y: 0.0 };
        let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 200.0, 100.0);
        let (style, plain_label) = split_label_style(label);
        doc.label_defaults = style;
        el.label = plain_label;
        doc.elements = vec![el];
        doc
    }

    fn resolved_for(doc: &Document) -> HashMap<Uuid, ImageSurface> {
        let mut resolved = HashMap::new();
        let el = &doc.elements[0];
        resolved.insert(el.id, solid_surface(el.natural_width as i32, el.natural_height as i32, Rgba::WHITE));
        resolved
    }

    /// A label with absolute placement at `(10, 10)`, black text, no
    /// background/shadow -- the minimal shape most of these tests only
    /// need to vary `content`/`enabled` on.
    fn absolute_title(enabled: bool, content: &str) -> crate::model::TextElement {
        crate::model::TextElement {
            enabled,
            content: content.to_string(),
            position: crate::model::TextPosition::Absolute { x: 10.0, y: 10.0 },
            typography: crate::model::Typography { font_size: 24.0, color: Rgba::BLACK, ..crate::model::Typography::label_default() },
            ..crate::model::TextElement::label_default()
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
        let resolved = resolved_for(&doc);
        let mut target = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

        assert!(!any_ink_in_region(&mut target, 0, 0, 200, 100), "a disabled title should draw no ink at all");
    }

    #[test]
    fn empty_content_draws_nothing_even_when_enabled() {
        let doc = text_test_doc(absolute_title(true, ""));
        let resolved = resolved_for(&doc);
        let mut target = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

        assert!(!any_ink_in_region(&mut target, 0, 0, 200, 100), "empty content should draw no ink");
    }

    #[test]
    fn enabled_title_draws_ink_near_its_position() {
        let doc = text_test_doc(absolute_title(true, "Hallo"));
        let resolved = resolved_for(&doc);
        let mut target = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

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
            typography: crate::model::Typography { font_size: 16.0, color: Rgba::BLACK, ..crate::model::Typography::label_default() },
            ..crate::model::TextElement::label_default()
        };

        // The 200px-wide canvas's own horizontal center is x=100 — check
        // a fixed region around *that* absolute point in both renders. A
        // title that's still tracking "centered" after the canvas widens
        // to 800 (center x=400) should have moved well clear of it.
        let ink_near_x100_at_200_wide = {
            let mut doc = text_test_doc(title.clone());
            doc.canvas = CanvasSettings { export_width: 200, export_height: 100, ..CanvasSettings::default() };
            doc.elements[0].natural_width = 200.0;
            doc.elements[0].natural_height = 100.0;
            let resolved = resolved_for(&doc);
            let mut target = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
            compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();
            any_ink_in_region(&mut target, 70, 0, 130, 40)
        };

        let ink_near_x100_at_800_wide = {
            let mut doc = text_test_doc(title);
            doc.canvas = CanvasSettings { export_width: 800, export_height: 100, ..CanvasSettings::default() };
            doc.elements[0].natural_width = 800.0;
            doc.elements[0].natural_height = 100.0;
            let resolved = resolved_for(&doc);
            let mut target = ImageSurface::create(Format::ARgb32, 800, 100).unwrap();
            compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();
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
            padding_x: 10.0,
            padding_y: 10.0,
            typography: crate::model::Typography { font_size: 16.0, color: Rgba::WHITE, ..crate::model::Typography::label_default() },
            ..crate::model::TextElement::label_default()
        };
        let doc = text_test_doc(title);
        let resolved = resolved_for(&doc);
        let mut target = ImageSurface::create(Format::ARgb32, 200, 100).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

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
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_x: 50.0, margin_y: 50.0 };

        let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 100.0);
        let (style, plain_label) = split_label_style(crate::model::TextElement {
            enabled: true,
            content: "Label".to_string(),
            position: crate::model::TextPosition::Absolute { x: 5.0, y: 5.0 },
            typography: crate::model::Typography { font_size: 16.0, color: Rgba::BLACK, ..crate::model::Typography::label_default() },
            ..crate::model::TextElement::label_default()
        });
        doc.label_defaults = style;
        el.label = plain_label;
        let id = el.id;
        doc.elements = vec![el];

        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(100, 100, Rgba::new(0.0, 1.0, 0.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 300, 200).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

        // The screenshot sits at [50,150)x[50,150) (margin 50); its label
        // is placed at a local (5, 5) offset, i.e. near document (55, 55).
        assert!(any_ink_in_region(&mut target, 50, 50, 90, 80), "expected the label's glyphs near the screenshot's own top-left");
    }

    #[test]
    fn measure_text_box_matches_where_the_label_actually_renders() {
        let label = crate::model::TextElement {
            enabled: true,
            content: "Hi".to_string(),
            position: crate::model::TextPosition::Absolute { x: 20.0, y: 20.0 },
            padding_x: 10.0,
            padding_y: 5.0,
            typography: crate::model::Typography { font_size: 16.0, ..crate::model::Typography::label_default() },
            ..crate::model::TextElement::label_default()
        };
        let (box_x, box_y, box_w, box_h) = measure_text_box(&label, 200.0, 100.0).unwrap();
        assert_eq!((box_x, box_y), (20.0, 20.0), "absolute position should be the box's own top-left, unchanged");
        assert!(box_w > 20.0 && box_h > 10.0, "expected the measured box to be at least as big as its own padding, got {box_w}x{box_h}");
    }

    fn plain_label(content: &str) -> crate::model::TextElement {
        crate::model::TextElement {
            enabled: true,
            content: content.to_string(),
            position: crate::model::TextPosition::Absolute { x: 0.0, y: 0.0 },
            padding_x: 5.0,
            padding_y: 5.0,
            typography: crate::model::Typography { font_size: 16.0, ..crate::model::Typography::label_default() },
            ..crate::model::TextElement::label_default()
        }
    }

    #[test]
    fn a_manual_newline_produces_a_taller_multi_line_box() {
        let one_line = measure_text_box(&plain_label("Linux"), 400.0, 200.0).unwrap();
        let two_lines = measure_text_box(&plain_label("Linux\nfür alle"), 400.0, 200.0).unwrap();
        assert!(two_lines.3 > one_line.3 * 1.5, "expected a manual line break to noticeably grow the box height, got {one_line:?} vs {two_lines:?}");
    }

    #[test]
    fn automatic_wrap_keeps_the_box_within_the_screenshot_width() {
        let mut label = plain_label("a rather long single line of text with no manual breaks at all");
        label.typography.wrap = true;
        let (_, _, box_w, _) = measure_text_box(&label, 120.0, 200.0).unwrap();
        assert!(box_w <= 120.0, "expected wrapping to keep the box at or under the screenshot's width, got {box_w}");
    }

    #[test]
    fn manual_mode_still_force_wraps_a_single_line_that_would_exceed_the_screenshot_width() {
        // typography.wrap is left at its default (false/manual) here --
        // the hard "never wider than the screenshot" ceiling must still
        // apply as a fallback, per spec.
        let label = plain_label("a rather long single line of text with no manual breaks at all");
        let (_, _, box_w, _) = measure_text_box(&label, 120.0, 200.0).unwrap();
        assert!(box_w <= 120.0, "expected the manual-mode safety net to still cap width at the screenshot's own size, got {box_w}");
    }

    #[test]
    fn manual_mode_leaves_short_text_unwrapped_and_uncentered_in_extra_width() {
        // A short manual line well within the screenshot's width should
        // NOT be forced to reflow/fill the whole available width.
        let label = plain_label("Hi");
        let (_, _, box_w, _) = measure_text_box(&label, 1000.0, 200.0).unwrap();
        assert!(box_w < 200.0, "expected a short line's box to stay tight around its own content, got {box_w}");
    }

    #[test]
    fn disabled_screenshot_label_draws_nothing() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 200, export_height: 200, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_x: 50.0, margin_y: 50.0 };

        let el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 100.0);
        let id = el.id;
        doc.elements = vec![el]; // label left at its disabled default

        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(100, 100, Rgba::new(0.0, 1.0, 0.0, 1.0)));

        let mut target = ImageSurface::create(Format::ARgb32, 200, 200).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

        // Below the screenshot (where a bottom-anchored label would land
        // if it were somehow enabled) should stay pure background.
        assert!(!any_ink_in_region(&mut target, 50, 150, 150, 200), "a disabled label should draw no ink at all");
    }

    #[test]
    fn enabled_callout_draws_a_bubble_and_an_arrow_to_its_target() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 200, export_height: 200, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_x: 0.0, margin_y: 0.0 };

        let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 200.0, 200.0);
        let mut callout = crate::model::Callout::new_for_width(200.0);
        callout.text.typography.color = Rgba::BLACK;
        callout.arrow_color = Rgba::BLACK;
        callout.target_x = 0.9;
        callout.target_y = 0.9;
        el.callouts = vec![callout];
        let id = el.id;
        doc.elements = vec![el];

        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(200, 200, Rgba::WHITE));

        let mut target = ImageSurface::create(Format::ARgb32, 200, 200).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

        // The bubble sits near the top-left corner (its default position).
        assert!(any_ink_in_region(&mut target, 0, 0, 60, 60), "expected the callout's text bubble near the top-left corner");
        // The arrow's target is near the bottom-right corner.
        assert!(any_ink_in_region(&mut target, 170, 170, 190, 190), "expected the arrow to reach near its target point");
    }

    #[test]
    fn callout_dot_radius_controls_the_size_of_the_marker_at_the_target() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 200, export_height: 200, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_x: 0.0, margin_y: 0.0 };

        let render_with_dot_radius = |dot_radius: f64| {
            let mut doc = doc.clone();
            let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 200.0, 200.0);
            let mut callout = crate::model::Callout::new_for_width(200.0);
            callout.arrow_color = Rgba::BLACK;
            // A near-zero arrow width keeps the arrowhead itself tiny (its
            // size floors at 4px regardless), so ink well outside that
            // reach can only have come from the dot.
            callout.arrow_width = 0.1;
            callout.dot_radius = dot_radius;
            callout.target_x = 0.5;
            callout.target_y = 0.5;
            el.callouts = vec![callout];
            let id = el.id;
            doc.elements = vec![el];

            let mut resolved = HashMap::new();
            resolved.insert(id, solid_surface(200, 200, Rgba::WHITE));
            let mut target = ImageSurface::create(Format::ARgb32, 200, 200).unwrap();
            compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();
            // A band well clear of the tiny arrowhead but inside a 20px-radius dot.
            any_ink_in_region(&mut target, 85, 95, 90, 105)
        };

        assert!(!render_with_dot_radius(0.0), "expected no ink from a zero-radius dot");
        assert!(render_with_dot_radius(20.0), "expected the enlarged dot to paint ink well beyond the tiny arrowhead");
    }

    #[test]
    fn disabled_callout_draws_nothing() {
        let mut doc = Document::new();
        doc.canvas = CanvasSettings { export_width: 200, export_height: 200, ..CanvasSettings::default() };
        doc.background = Background::Solid(Rgba::WHITE);
        doc.layout = LayoutSettings { mode: crate::model::LayoutMode::Horizontal, spacing_px: 0.0, margin_x: 0.0, margin_y: 0.0 };

        let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 200.0, 200.0);
        let mut callout = crate::model::Callout::new_for_width(200.0);
        callout.enabled = false;
        el.callouts = vec![callout];
        let id = el.id;
        doc.elements = vec![el];

        let mut resolved = HashMap::new();
        resolved.insert(id, solid_surface(200, 200, Rgba::WHITE));

        let mut target = ImageSurface::create(Format::ARgb32, 200, 200).unwrap();
        compose(&doc, &target, 1.0, &resolved, None, &ShadowCache::new(), &BackgroundCache::new()).unwrap();

        assert!(!any_ink_in_region(&mut target, 0, 0, 200, 200), "a disabled callout should draw no ink at all");
    }

    #[test]
    fn box_edge_toward_picks_the_side_of_the_box_facing_the_target() {
        let box_rect = (50.0, 50.0, 100.0, 40.0); // spans x:[50,150], y:[50,90]
        // Target far to the right, roughly level with the box's own
        // vertical center -- should land on the box's right edge (x=150).
        let (x, y) = box_edge_toward(box_rect, (500.0, 70.0));
        assert!((x - 150.0).abs() < 1e-9, "expected the right edge, got x={x}");
        assert!((50.0..=90.0).contains(&y));

        // Target straight below -- should land on the box's bottom edge (y=90).
        let (x, y) = box_edge_toward(box_rect, (100.0, 500.0));
        assert!((y - 90.0).abs() < 1e-9, "expected the bottom edge, got y={y}");
        assert!((50.0..=150.0).contains(&x));
    }
}
