//! Procedural background generation: turns a [`GeneratedBackground`]'s
//! seed + parameters + palette into Cairo drawing directly on the
//! composition's own context — no intermediate raster image. Because
//! every element is a vector path (`line_to`/`arc`/...) drawn straight
//! onto whatever `Context` the caller hands in, the same generator call
//! produces a crisp result whether that context is a small interactive
//! preview or a 4K export (the vector/procedural scene is rendered
//! directly at the requested export resolution).
//!
//! Determinism is the core contract: the same `seed` + parameters +
//! canvas size + screenshot layout must always produce the identical
//! scene, which is what makes storing just those inputs in the project
//! file (rather than a rendered image, or a serialized scene graph)
//! enough to reproduce a background exactly — see
//! [`crate::model::GeneratedBackground`]'s own doc comment.
//!
//! The scene itself is one algorithm, [`draw_wave_layers`]: a fan of
//! nested, wave-perturbed arc layers around a single focus point. Whether
//! that reads as flat wave bands or as nested arcs tucked into a canvas
//! corner is controlled entirely by `GeneratedBackground::corner_bias` —
//! see that function's doc comment for the geometry.

use cairo::{Context, Format, ImageSurface, LinearGradient};

use crate::model::{GeneratedBackground, GeneratorStyle, Rgba};
use crate::palette::{oklab_to_rgb, rgb_to_oklab};
use crate::render::RenderError;
use crate::rng::Rng;

/// One screenshot's placement, in the same document-pixel space as the
/// canvas being drawn on — enough for the generator to keep its focus
/// point clear of actual content. A minimal local shape rather than
/// reusing `crate::layout::Placement` directly, since the generator has
/// no use for that type's element id.
#[derive(Debug, Clone, Copy)]
pub struct ScreenshotRegion {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ScreenshotRegion {
    fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// `0.0` at the region's own center, growing to `1.0` at its edge and
    /// beyond — a soft falloff (not a hard boolean) is what lets corner
    /// selection prefer a corner a little further from a region without
    /// abruptly favoring anything a single pixel further out.
    fn proximity(&self, x: f64, y: f64) -> f64 {
        let (cx, cy) = self.center();
        let dx = (x - cx) / (self.width / 2.0).max(1.0);
        let dy = (y - cy) / (self.height / 2.0).max(1.0);
        (1.0 - (dx * dx + dy * dy).sqrt()).max(0.0)
    }
}

/// How strongly `(x, y)` overlaps any screenshot region — `0.0` means
/// clear of all of them. Used to steer the generator's focus point toward
/// empty canvas space when `GeneratedBackground::adapt_to_screenshots` is
/// set.
fn occupancy(x: f64, y: f64, regions: &[ScreenshotRegion]) -> f64 {
    regions.iter().map(|r| r.proximity(x, y)).fold(0.0, f64::max)
}

/// Converts `palette` to Oklab once per render (so `layer_color` never
/// reconverts on every call), plus its mean lightness — the pivot
/// `layer_color`'s contrast boost pushes every layer's lightness/chroma
/// away from. Falls back to a single neutral-gray stop when `palette` is
/// empty, converted the same way so `layer_color` needs no separate
/// empty-palette case.
fn oklab_palette(palette: &[Rgba]) -> (Vec<(f64, f64, f64)>, f64) {
    let stops: Vec<(f64, f64, f64)> =
        if palette.is_empty() { vec![rgb_to_oklab(Rgba::new(0.5, 0.5, 0.5, 1.0))] } else { palette.iter().map(|&c| rgb_to_oklab(c)).collect() };
    let mean_l = stops.iter().map(|s| s.0).sum::<f64>() / stops.len() as f64;
    (stops, mean_l)
}

/// Interpolates Oklab `stops` at fractional position `t` (`0.0..=1.0`)
/// between its two nearest entries — the same ramp-across-the-whole-
/// palette shape the old RGB-space `palette_lerp` had, but in a
/// perceptually uniform space. Plain RGB interpolation between two very
/// different hues passes through a muddy, desaturated midpoint; Oklab
/// doesn't, which is a big part of why adjacent layers used to read as
/// too similar to tell apart (spec: "stärkere Farbunterschiede").
fn oklab_lerp(stops: &[(f64, f64, f64)], t: f64) -> (f64, f64, f64) {
    if stops.len() == 1 {
        return stops[0];
    }
    let t = t.clamp(0.0, 1.0);
    let scaled = t * (stops.len() - 1) as f64;
    let i0 = scaled.floor() as usize;
    let i1 = (i0 + 1).min(stops.len() - 1);
    let frac = scaled - i0 as f64;
    let (l0, a0, b0) = stops[i0];
    let (l1, a1, b1) = stops[i1];
    (l0 + (l1 - l0) * frac, a0 + (a1 - a0) * frac, b0 + (b1 - b0) * frac)
}

/// One wave layer's flat fill color: `oklab_lerp(stops, t)`, then pushed
/// away from the palette's own mean lightness `mean_l` — and its chroma
/// scaled the same way — by `contrast`. At `contrast = 0` this is just
/// the plain interpolated color; at `contrast = 1` adjacent layers pull
/// apart much more in both lightness and saturation. That's the
/// difference between "clearly distinct regions" and a single smooth
/// wash: boosting contrast globally (e.g. an S-curve on `t` itself) would
/// only widen the *overall* range, but every layer sits at its own fixed
/// `t` regardless — pushing each color away from the *palette's* pivot
/// widens the gap between every pair of neighbors at once.
fn layer_color(stops: &[(f64, f64, f64)], mean_l: f64, t: f64, contrast: f64) -> Rgba {
    let (l, a, b) = oklab_lerp(stops, t);
    let gain = 1.0 + contrast.clamp(0.0, 1.0) * 1.4;
    let boosted_l = (mean_l + (l - mean_l) * gain).clamp(0.0, 1.0);
    oklab_to_rgb(boosted_l, a * gain, b * gain)
}

/// Bundles the canvas/screenshot context [`draw_wave_layers`] needs beyond
/// its own RNG stream and the resolved palette.
struct Scene<'a> {
    width: f64,
    height: f64,
    regions: &'a [ScreenshotRegion],
    avoid: bool,
}

/// Renders `bg` directly onto `ctx`, filling the `width`×`height` canvas.
/// `regions` are the currently visible screenshots' placements, consulted
/// only when `bg.adapt_to_screenshots` is set. Deterministic in every
/// input — the same arguments always draw the identical scene.
pub fn render(ctx: &Context, bg: &GeneratedBackground, width: f64, height: f64, regions: &[ScreenshotRegion]) -> Result<(), RenderError> {
    if width <= 0.0 || height <= 0.0 {
        return Ok(());
    }
    if bg.style == GeneratorStyle::Waves {
        render_waves(ctx, bg, width, height, regions)?;
    } else {
        crate::styles::render(ctx, bg, width, height, regions)?;
    }
    crate::styles::apply_grain(ctx, width, height, bg.grain, bg.seed)
}

/// Variant number `index` of `base`, for offering several alternatives at
/// once: a new seed and fresh `density`/`flow`/`variation`/`softness`,
/// all derived deterministically from `base.seed` and `index`, so the same
/// base always yields the same set. Everything the user set deliberately
/// (color strategy, mood, contrast, grain, scale, offset) is kept. With
/// `mix_styles`, the variants walk through all modern styles from a seeded
/// starting point; otherwise they keep `base.style`.
///
/// `palette` is copied unchanged — callers re-resolve it for the new seed
/// (`crate::palette::resolve_palette_for`) when the strategy derives it.
pub fn variant(base: &GeneratedBackground, index: u64, mix_styles: bool) -> GeneratedBackground {
    let mut rng = Rng::new(base.seed ^ 0x7661_7269_616E_7400 ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let seed = rng.next_u64() % 1_000_000_000;
    let style = if mix_styles {
        let start = (base.seed % GeneratorStyle::MODERN.len() as u64) as usize;
        GeneratorStyle::MODERN[(start + index as usize) % GeneratorStyle::MODERN.len()]
    } else {
        base.style
    };
    GeneratedBackground {
        seed,
        style,
        density: rng.range(0.0, 1.0),
        flow: rng.range(0.2, 0.8),
        variation: rng.range(0.0, 1.0),
        softness: rng.range(0.2, 1.0),
        ..base.clone()
    }
}

/// The classic wave-layer scene (`GeneratorStyle::Waves`), unchanged since
/// before styles existed so old projects render identically.
fn render_waves(ctx: &Context, bg: &GeneratedBackground, width: f64, height: f64, regions: &[ScreenshotRegion]) -> Result<(), RenderError> {
    let mut rng = Rng::new(bg.seed);
    let palette = if bg.palette.is_empty() { &[Rgba::new(0.9, 0.9, 0.92, 1.0)][..] } else { &bg.palette[..] };

    base_fill(ctx, &mut rng.fork(1), palette, width, height)?;

    let scene = Scene { width, height, regions, avoid: bg.adapt_to_screenshots };
    draw_wave_layers(ctx, &mut rng.fork(2), bg, palette, &scene)?;
    Ok(())
}

/// A subtle full-canvas gradient (or solid, for a single-color palette)
/// the wave layers are drawn over — keeps the composition visually
/// grounded in the palette even where the layers themselves don't reach.
fn base_fill(ctx: &Context, rng: &mut Rng, palette: &[Rgba], width: f64, height: f64) -> Result<(), RenderError> {
    ctx.save()?;
    if palette.len() < 2 {
        let c = palette.first().copied().unwrap_or(Rgba::new(0.9, 0.9, 0.92, 1.0));
        ctx.set_source_rgba(c.r, c.g, c.b, c.a);
    } else {
        let angle = rng.range(0.0, std::f64::consts::PI * 2.0);
        let (dx, dy) = (angle.cos(), angle.sin());
        let len = (width.powi(2) + height.powi(2)).sqrt() / 2.0;
        let (cx, cy) = (width / 2.0, height / 2.0);
        let gradient = LinearGradient::new(cx - dx * len, cy - dy * len, cx + dx * len, cy + dy * len);
        gradient.add_color_stop_rgba(0.0, palette[0].r, palette[0].g, palette[0].b, palette[0].a);
        gradient.add_color_stop_rgba(1.0, palette[1].r, palette[1].g, palette[1].b, palette[1].a);
        ctx.set_source(&gradient)?;
    }
    ctx.rectangle(0.0, 0.0, width, height);
    ctx.fill()?;
    ctx.restore()?;
    Ok(())
}

/// Wraps `angle` (radians) into `(-PI, PI]` — the signed-shortest-turn form
/// needed to compare an arbitrary angle against `base_angle` regardless of
/// which side of the 0/2π seam either one falls on.
fn wrap_to_pi(angle: f64) -> f64 {
    let two_pi = std::f64::consts::PI * 2.0;
    let mut a = angle % two_pi;
    if a > std::f64::consts::PI {
        a -= two_pi;
    } else if a < -std::f64::consts::PI {
        a += two_pi;
    }
    a
}

/// The angular sweep, centered on `base_angle`, that fully contains every
/// point in `corners` as seen from `focus` — plus a small margin so the
/// sweep's own edge clears the canvas rather than grazing it.
///
/// Each wave layer is a "pie slice" with two dead-straight radial sides;
/// without this, a sweep width tuned for one `corner_bias`/canvas-size
/// combination would leave those straight sides cutting visibly across the
/// canvas as a stray diagonal line for another — the one hard, unwavy edge
/// in an otherwise all-organic scene. Sizing the sweep from the actual
/// canvas geometry instead guarantees both straight sides always fall
/// outside the visible canvas, however far `focus` sits from it.
fn required_sweep(focus: (f64, f64), base_angle: f64, corners: &[(f64, f64)]) -> f64 {
    let margin = 8.0_f64.to_radians();
    let max_offset =
        corners.iter().map(|&(x, y)| wrap_to_pi((y - focus.1).atan2(x - focus.0) - base_angle).abs()).fold(0.0_f64, f64::max);
    (max_offset * 2.0 + margin * 2.0).min(std::f64::consts::PI * 2.0 - 0.01)
}

/// Picks the focus point's canvas corner: one of the 4 corners, biased
/// toward whichever is least covered by `regions` when `avoid` is set
/// (ties broken by the RNG stream, keeping it deterministic), otherwise
/// picked uniformly at random.
pub(crate) fn choose_corner(rng: &mut Rng, width: f64, height: f64, regions: &[ScreenshotRegion], avoid: bool) -> (f64, f64) {
    let corners = [(0.0, 0.0), (width, 0.0), (0.0, height), (width, height)];
    if !avoid || regions.is_empty() {
        return corners[rng.index(corners.len())];
    }
    let mut best = corners[0];
    let mut best_occupancy = occupancy(best.0, best.1, regions);
    for &corner in &corners[1..] {
        let o = occupancy(corner.0, corner.1, regions);
        if o < best_occupancy {
            best = corner;
            best_occupancy = o;
        }
    }
    best
}

/// Casts `points`' silhouette (the exact wobbly wedge a layer is about to
/// fill) as a soft, semi-transparent shadow offset by `offset`, painted
/// onto `ctx` *before* that layer's own opaque fill. Drawing it this way
/// — as part of generation, not a raster post-process over the finished
/// image — is what gives each layer a visible "contact shadow" rim
/// exactly where the *next*, smaller layer's boundary doesn't quite
/// reach: that later layer's own fill covers the un-shifted footprint,
/// leaving only the sliver the offset pushed outside it. See
/// `draw_wave_layers` for how `offset`/`blur`/`alpha` are chosen.
///
/// Renders into a scratch surface capped at `SHADOW_RENDER_MAX_DIM` in its
/// longest dimension, *not* the shape's true document-space size —
/// shadows are blurred by construction, so the resolution this trades
/// away is imperceptible, and it's what keeps this cheap however large
/// the canvas or an outer layer's own footprint gets: without it, a wide
/// outer layer's shadow on a multi-thousand-pixel export canvas blurs a
/// multi-megapixel surface, once per layer, on every single render —
/// exactly what made a generated background slow enough to look hung.
/// The shape is drawn scaled down to fit, blurred at that resolution,
/// then stretched back out over its true footprint when composited
/// (mirroring `render::shadow_render_scale`'s own reasoning, just with a
/// fixed cap rather than one derived from the caller's device scale,
/// since nothing here has that scale threaded through it).
///
/// Deliberately does *not* clamp the padded bounding box to the canvas
/// edges: cropping away the margin a blurred edge needs right at the
/// canvas boundary — which every outer layer's shadow used to hit, since
/// the fan reaches into a canvas corner by design — left a hard, visible
/// seam exactly where the crop happened, a stray straight line rather
/// than the intended soft fade-out.
const SHADOW_RENDER_MAX_DIM: f64 = 256.0;

fn draw_layer_shadow(ctx: &Context, points: &[(f64, f64)], offset: (f64, f64), blur: f64, alpha: f64) -> Result<(), RenderError> {
    if alpha <= 0.0 || points.is_empty() {
        return Ok(());
    }
    // Deliberately traces just `points` — the arc boundary itself — closed
    // directly rather than the true fan-shaped fill (which also has a
    // vertex at the shared focus point, however far outside the canvas
    // that sits for a low `corner_bias`). Every layer shares that same
    // focus and roughly the same angular sweep, so the sliver between the
    // arc and the focus is common ground every layer overpaints anyway —
    // a shadow there would never be visible. Leaving it out of the
    // silhouette keeps the shadow's own bounding box close to the arc's
    // actual on-canvas size instead of ballooning out to the focus point
    // (which, for a wide/flat wave-band read, may sit many canvas-
    // diagonals away) — that ballooning was the actual reason a generated
    // background could take seconds per layer and look hung.
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (points[0].0, points[0].0, points[0].1, points[0].1);
    for &(x, y) in points {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    // Widen by the offset so the shifted copy's own extent is covered too.
    min_x = min_x.min(min_x + offset.0);
    min_y = min_y.min(min_y + offset.1);
    max_x = max_x.max(max_x + offset.0);
    max_y = max_y.max(max_y + offset.1);

    let bbox_w = (max_x - min_x).max(1.0);
    let bbox_h = (max_y - min_y).max(1.0);
    // The blur's own padding (~3x its radius per side, see `pad` below)
    // has to count toward the size being capped too — a small shape with
    // a large blur radius needs just as much downscaling as a large shape
    // would, otherwise the padding alone (not the traced shape) is what
    // balloons the surface past the cap.
    let unscaled_extent = bbox_w.max(bbox_h) + blur * 6.0 + 8.0;
    let render_scale = (SHADOW_RENDER_MAX_DIM / unscaled_extent).min(1.0);
    let render_blur = blur * render_scale;
    let pad = (render_blur * 3.0 + 4.0).max(4.0);

    let surface_w = (bbox_w * render_scale + pad * 2.0).ceil() as i32;
    let surface_h = (bbox_h * render_scale + pad * 2.0).ceil() as i32;
    if surface_w <= 0 || surface_h <= 0 {
        return Ok(());
    }

    let mut shadow_surface = ImageSurface::create(Format::ARgb32, surface_w, surface_h)?;
    {
        let shadow_ctx = Context::new(&shadow_surface)?;
        shadow_ctx.translate(pad, pad);
        shadow_ctx.scale(render_scale, render_scale);
        shadow_ctx.translate(-min_x, -min_y);
        shadow_ctx.set_source_rgba(0.0, 0.0, 0.0, alpha);
        shadow_ctx.new_path();
        shadow_ctx.move_to(points[0].0, points[0].1);
        for &(x, y) in &points[1..] {
            shadow_ctx.line_to(x, y);
        }
        shadow_ctx.close_path();
        shadow_ctx.fill()?;
    }
    if render_blur > 0.0 {
        let stride = shadow_surface.stride();
        let mut data = shadow_surface.data()?;
        crate::blur::box_blur(&mut data, surface_w, surface_h, stride, render_blur);
    }

    ctx.save()?;
    ctx.scale(1.0 / render_scale, 1.0 / render_scale);
    ctx.set_source_surface(&shadow_surface, (min_x + offset.0) * render_scale - pad, (min_y + offset.1) * render_scale - pad)?;
    ctx.paint()?;
    ctx.restore()?;
    Ok(())
}

/// Draws a fan of nested, wave-perturbed arc layers around a single focus
/// point — the one procedural style this generator produces. Everything
/// about *where the layers sit* and *how the fan spreads* is derived from
/// `bg.corner_bias` (`0.0..=1.0`):
///
/// - The focus point sits on the diagonal through a chosen canvas corner
///   (`choose_corner`), at a distance from that corner that shrinks to
///   zero as `corner_bias` rises to `1.0` — so at `1.0` the focus point
///   *is* the corner, and the fan's sweep reads as nested arcs tucked into
///   it. At `0.0` the focus point sits many canvas-diagonals further out
///   along the same line — from that distance the same arcs read as
///   nearly-flat, gently wavy bands crossing the canvas.
/// - Every value in between is a continuous morph between those two
///   reads, rather than a switch between unrelated algorithms.
///
/// `bg.offset_x`/`bg.offset_y` then shift that focus point further in
/// plain canvas pixels (independent of `corner_bias`), and `bg.scale`
/// zooms the whole fan in or out around it — together, letting the user
/// move and resize the pattern without touching its wave-vs-arc read. The
/// fan's angular sweep is always sized from the actual canvas geometry
/// (`required_sweep`) rather than a fixed width, so it stays wide enough
/// to fully cover the canvas from wherever the focus point ends up.
///
/// Layers are drawn outermost (largest radius) first and innermost last,
/// so the innermost layer — closest to the focus point — paints on top.
/// Every layer is an opaque, hard-edged flat fill (`layer_color`, Oklab
/// space) rather than a per-layer gradient — the earlier per-layer
/// `RadialGradient` fade was the main reason generated backgrounds read
/// as one soft, homogeneous wash instead of distinct regions. `bg.contrast`
/// now does two jobs at once, matching how the reference wallpapers this
/// algorithm chases actually read: it widens the color gap between
/// neighboring layers (`layer_color`'s contrast boost) *and* strengthens
/// the contact shadow each layer casts on the one behind it
/// (`draw_layer_shadow`), so "more contrast" reads as "more separated,
/// more dimensional" rather than just "more saturated". `bg.softness`
/// controls that same shadow's blur radius — a layer's own fill is always
/// crisp, only its cast shadow is ever soft.
///
/// The shadow direction/length is fixed per render (derived from
/// `outward`, scaled to a fraction of one layer band's own thickness) so
/// every layer casts its shadow the same way — a consistent "light
/// direction" across the whole composition, rather than a jumble of
/// differently-angled shadows.
fn draw_wave_layers(ctx: &Context, rng: &mut Rng, bg: &GeneratedBackground, palette: &[Rgba], scene: &Scene) -> Result<(), RenderError> {
    let (width, height, regions, avoid) = (scene.width, scene.height, scene.regions, scene.avoid);
    let corner = choose_corner(rng, width, height, regions, avoid);
    let center = (width / 2.0, height / 2.0);

    let outward = {
        let (dx, dy) = (corner.0 - center.0, corner.1 - center.1);
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        (dx / len, dy / len)
    };
    let half_diag = ((width / 2.0).powi(2) + (height / 2.0).powi(2)).sqrt();
    let corner_bias = bg.corner_bias.clamp(0.0, 1.0);

    let extra_distance = half_diag * 6.0 * (1.0 - corner_bias);
    let offset = (bg.offset_x.clamp(-1.0, 1.0) * width * 0.5, bg.offset_y.clamp(-1.0, 1.0) * height * 0.5);
    let focus = (corner.0 + outward.0 * extra_distance + offset.0, corner.1 + outward.1 * extra_distance + offset.1);

    let base_angle = (center.1 - focus.1).atan2(center.0 - focus.0);
    let all_corners = [(0.0, 0.0), (width, 0.0), (0.0, height), (width, height)];
    let sweep_rad = required_sweep(focus, base_angle, &all_corners);

    let layer_count = 4 + (6.0 * bg.density.clamp(0.0, 1.0)).round() as usize;

    let scale = bg.scale.max(0.05);
    let max_radius = all_corners
        .iter()
        .map(|&(x, y)| ((x - focus.0).powi(2) + (y - focus.1).powi(2)).sqrt())
        .fold(0.0_f64, f64::max)
        .max(1.0)
        * scale;
    let min_radius = max_radius * 0.15;

    let amplitude = max_radius * 0.05 * (0.3 + bg.flow.clamp(0.0, 1.0) * 0.7);
    let freq = 1.0 + (bg.variation.clamp(0.0, 1.0) * 3.0).round();
    // Enough points for a smooth curve through them (`styles::smooth`); the
    // old 24-point polygon showed visible corners on large canvases.
    let segments = 120;

    let contrast = bg.contrast.clamp(0.0, 1.0);
    let (oklab_stops, mean_l) = oklab_palette(palette);

    let band_thickness = (max_radius - min_radius) / layer_count.max(1) as f64;
    let shadow_offset = {
        let len = band_thickness * (0.25 + 0.35 * contrast);
        (outward.0 * len, outward.1 * len)
    };
    let shadow_blur = (band_thickness * (0.15 + 0.5 * bg.softness.clamp(0.0, 1.0))).max(0.5);
    let shadow_alpha = 0.16 + 0.34 * contrast;

    for i in 0..layer_count {
        let t = i as f64 / (layer_count - 1).max(1) as f64;
        let r_base = max_radius + (min_radius - max_radius) * t;
        let phase = rng.range(0.0, std::f64::consts::PI * 2.0);
        let layer_amplitude = amplitude * rng.range(0.7, 1.3);

        let points: Vec<(f64, f64)> = (0..=segments)
            .map(|s| {
                let frac = s as f64 / segments as f64;
                let angle = base_angle - sweep_rad / 2.0 + frac * sweep_rad;
                let wobble = (frac * freq * std::f64::consts::PI * 2.0 + phase).sin() * layer_amplitude;
                let r = (r_base + wobble).max(1.0);
                (focus.0 + angle.cos() * r, focus.1 + angle.sin() * r)
            })
            .collect();

        // Cast onto whatever's already painted (the base fill, or the
        // previous, larger layer) before this layer's own opaque fill —
        // see this function's doc comment for why that's what produces a
        // visible rim rather than a fully hidden or fully detached blob.
        draw_layer_shadow(ctx, &points, shadow_offset, shadow_blur, shadow_alpha)?;

        ctx.new_path();
        ctx.move_to(focus.0, focus.1);
        crate::styles::smooth(ctx, &points, false);
        ctx.close_path();

        let color = layer_color(&oklab_stops, mean_l, t, contrast);
        ctx.set_source_rgba(color.r, color.g, color.b, color.a);
        ctx.fill()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ColorStrategy, GeneratedBackground};
    use cairo::{Format, ImageSurface};

    fn checksum(surface: &mut ImageSurface) -> u64 {
        let data = surface.data().unwrap();
        let mut hash: u64 = 1469598103934665603;
        for &byte in data.iter() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(1099511628211);
        }
        hash
    }

    fn sample_background(seed: u64, corner_bias: f64) -> GeneratedBackground {
        GeneratedBackground {
            palette: vec![Rgba::new(0.2, 0.3, 0.6, 1.0), Rgba::new(0.8, 0.5, 0.2, 1.0), Rgba::new(0.9, 0.9, 0.9, 1.0)],
            color_strategy: ColorStrategy::FromScreenshots,
            corner_bias,
            style: GeneratorStyle::Waves,
            ..GeneratedBackground::new(seed)
        }
    }

    fn render_to_checksum(bg: &GeneratedBackground, regions: &[ScreenshotRegion]) -> u64 {
        let mut surface = ImageSurface::create(Format::ARgb32, 300, 200).unwrap();
        {
            let ctx = Context::new(&surface).unwrap();
            render(&ctx, bg, 300.0, 200.0, regions).unwrap();
        }
        checksum(&mut surface)
    }

    #[test]
    fn renders_without_error_and_paints_something_at_both_extremes() {
        for corner_bias in [0.0, 0.5, 1.0] {
            let bg = sample_background(1, corner_bias);
            let mut surface = ImageSurface::create(Format::ARgb32, 300, 200).unwrap();
            let ctx = Context::new(&surface).unwrap();
            render(&ctx, &bg, 300.0, 200.0, &[]).expect("should render without error");
            drop(ctx);
            let data = surface.data().unwrap();
            assert!(data.iter().any(|&b| b != 0), "corner_bias={corner_bias} painted nothing at all");
        }
    }

    /// Regression test for a real perf incident: a low `corner_bias` puts
    /// the focus point many canvas-diagonals away, and a large `contrast`/
    /// `softness` combination drives a correspondingly large document-
    /// space shadow blur radius — together, every per-layer shadow's
    /// scratch surface ballooned to a multi-megapixel blur, once per
    /// layer, making a single render slow enough that GNOME's own "not
    /// responding" watchdog fired for what should be a routine slider
    /// tweak. `SHADOW_RENDER_MAX_DIM` (and folding the blur radius itself
    /// into the size that gets capped, not just the traced shape) is what
    /// keeps this bounded regardless of canvas size or how far outside it
    /// the focus point sits — this asserts that bound holds at every
    /// `corner_bias` extreme, with the highest layer count and the
    /// highest contrast/softness this generator allows.
    #[test]
    fn rendering_stays_fast_even_at_the_most_expensive_shadow_settings() {
        for corner_bias in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let bg = GeneratedBackground {
                density: 1.0,
                contrast: 1.0,
                softness: 1.0,
                corner_bias,
                ..sample_background(1, corner_bias)
            };
            let start = std::time::Instant::now();
            let _ = render_to_checksum(&bg, &[]);
            let elapsed = start.elapsed();
            assert!(
                elapsed.as_millis() < 2000,
                "corner_bias={corner_bias} took {elapsed:?} — expected well under a second even in a debug build"
            );
        }
    }

    #[test]
    fn the_same_seed_and_parameters_render_identically() {
        let bg = sample_background(12345, 0.5);
        let a = render_to_checksum(&bg, &[]);
        let b = render_to_checksum(&bg, &[]);
        assert_eq!(a, b, "identical inputs must render pixel-identical output");
    }

    #[test]
    fn a_different_seed_renders_a_visibly_different_scene() {
        let a = render_to_checksum(&sample_background(1, 0.5), &[]);
        let b = render_to_checksum(&sample_background(2, 0.5), &[]);
        assert_ne!(a, b, "different seeds should not coincidentally render the same scene");
    }

    #[test]
    fn different_corner_bias_renders_differently_for_the_same_seed() {
        let a = render_to_checksum(&sample_background(7, 0.0), &[]);
        let b = render_to_checksum(&sample_background(7, 1.0), &[]);
        assert_ne!(a, b);
    }

    #[test]
    fn offset_changes_the_rendered_scene() {
        let mut bg = sample_background(3, 0.7);
        let unmoved = render_to_checksum(&bg, &[]);
        bg.offset_x = 0.4;
        bg.offset_y = -0.3;
        let moved = render_to_checksum(&bg, &[]);
        assert_ne!(unmoved, moved, "a nonzero offset should visibly shift the pattern");
    }

    #[test]
    fn scale_changes_the_rendered_scene() {
        let mut bg = sample_background(3, 0.7);
        let unscaled = render_to_checksum(&bg, &[]);
        bg.scale = 1.8;
        let scaled = render_to_checksum(&bg, &[]);
        assert_ne!(unscaled, scaled, "a nonzero scale change should visibly resize the pattern");
    }

    /// Regression test for the "diagonal cut across the canvas" artifact:
    /// each wave layer's two dead-straight radial sides must always fall
    /// outside the visible canvas, however far or close `focus` sits to
    /// it, otherwise they show up as a hard, unwavy edge slicing through
    /// the pattern. `required_sweep` must widen enough to keep both sides
    /// clear of every canvas corner in every case, including when `focus`
    /// sits exactly on the canvas boundary (`corner_bias = 1.0`).
    #[test]
    fn variants_are_deterministic_distinct_and_keep_user_settings() {
        let base = GeneratedBackground { grain: 0.7, contrast: 0.9, mood: crate::model::Mood::Light, ..GeneratedBackground::new(99) };
        let a: Vec<_> = (0..6).map(|i| variant(&base, i, false)).collect();
        let b: Vec<_> = (0..6).map(|i| variant(&base, i, false)).collect();
        assert_eq!(a, b);
        let seeds: std::collections::HashSet<u64> = a.iter().map(|v| v.seed).collect();
        assert_eq!(seeds.len(), 6);
        for v in &a {
            assert_eq!((v.style, v.grain, v.contrast, v.mood), (base.style, base.grain, base.contrast, base.mood));
        }
    }

    #[test]
    fn mixed_variants_cover_every_modern_style() {
        let base = GeneratedBackground::new(5);
        let styles: std::collections::HashSet<GeneratorStyle> = (0..6).map(|i| variant(&base, i, true).style).collect();
        assert_eq!(styles.len(), GeneratorStyle::MODERN.len());
    }

    #[test]
    fn a_project_saved_before_styles_existed_loads_as_classic_waves_without_grain() {
        let mut json = serde_json::to_value(GeneratedBackground::new(3)).unwrap();
        let fields = json.as_object_mut().unwrap();
        fields.remove("style");
        fields.remove("mood");
        fields.remove("grain");
        let loaded: GeneratedBackground = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.style, GeneratorStyle::Waves);
        assert_eq!(loaded.grain, 0.0);
    }

    #[test]
    fn required_sweep_always_clears_every_canvas_corner() {
        let corners = [(0.0, 0.0), (400.0, 0.0), (0.0, 300.0), (400.0, 300.0)];
        let focus_points: [(f64, f64); 5] = [(0.0, 0.0), (400.0, 300.0), (-500.0, -400.0), (200.0, 150.0), (900.0, 150.0)];
        for &focus in &focus_points {
            let base_angle = (150.0 - focus.1).atan2(200.0 - focus.0);
            let sweep = required_sweep(focus, base_angle, &corners);
            for &(x, y) in &corners {
                let offset = wrap_to_pi((y - focus.1).atan2(x - focus.0) - base_angle).abs();
                assert!(offset <= sweep / 2.0 + 1e-9, "corner ({x}, {y}) at offset {offset} not covered by sweep {sweep} from focus {focus:?}");
            }
        }
    }

    #[test]
    fn zero_size_canvas_does_not_panic() {
        let bg = sample_background(1, 0.5);
        let surface = ImageSurface::create(Format::ARgb32, 1, 1).unwrap();
        let ctx = Context::new(&surface).unwrap();
        render(&ctx, &bg, 0.0, 0.0, &[]).unwrap();
    }

    #[test]
    fn empty_palette_falls_back_to_a_neutral_color_without_panicking() {
        let mut bg = sample_background(1, 0.5);
        bg.palette.clear();
        let surface = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        let ctx = Context::new(&surface).unwrap();
        render(&ctx, &bg, 100.0, 100.0, &[]).unwrap();
    }

    #[test]
    fn adapt_to_screenshots_changes_output_versus_not_adapting() {
        let mut bg = sample_background(42, 0.8);
        let regions = [ScreenshotRegion { x: 50.0, y: 50.0, width: 200.0, height: 100.0 }];

        bg.adapt_to_screenshots = false;
        let not_avoiding = render_to_checksum(&bg, &regions);
        bg.adapt_to_screenshots = true;
        let avoiding = render_to_checksum(&bg, &regions);

        assert_ne!(not_avoiding, avoiding, "avoiding screenshot regions should change the chosen focus corner");
    }

    fn assert_close(a: Rgba, b: Rgba, tol: f64) {
        assert!((a.r - b.r).abs() < tol, "r: {a:?} vs {b:?}");
        assert!((a.g - b.g).abs() < tol, "g: {a:?} vs {b:?}");
        assert!((a.b - b.b).abs() < tol, "b: {a:?} vs {b:?}");
    }

    #[test]
    fn layer_color_of_empty_palette_is_neutral_gray_regardless_of_contrast() {
        let (stops, mean_l) = oklab_palette(&[]);
        for contrast in [0.0, 0.5, 1.0] {
            let c = layer_color(&stops, mean_l, 0.5, contrast);
            assert_close(c, Rgba::new(0.5, 0.5, 0.5, 1.0), 0.02);
        }
    }

    #[test]
    fn layer_color_at_the_ends_approximates_the_end_colors_at_zero_contrast() {
        let palette = vec![Rgba::new(0.0, 0.0, 0.0, 1.0), Rgba::new(1.0, 1.0, 1.0, 1.0)];
        let (stops, mean_l) = oklab_palette(&palette);
        assert_close(layer_color(&stops, mean_l, 0.0, 0.0), palette[0], 0.02);
        assert_close(layer_color(&stops, mean_l, 1.0, 0.0), palette[1], 0.02);
    }

    #[test]
    fn higher_contrast_widens_the_gap_between_two_layers_colors() {
        let palette = vec![Rgba::new(0.2, 0.3, 0.6, 1.0), Rgba::new(0.8, 0.5, 0.2, 1.0)];
        let (stops, mean_l) = oklab_palette(&palette);
        let gap_at = |contrast: f64| {
            let a = layer_color(&stops, mean_l, 0.25, contrast);
            let b = layer_color(&stops, mean_l, 0.75, contrast);
            ((a.r - b.r).powi(2) + (a.g - b.g).powi(2) + (a.b - b.b).powi(2)).sqrt()
        };
        assert!(gap_at(1.0) > gap_at(0.0), "higher contrast should pull neighboring layers' colors further apart");
    }

    #[test]
    fn oklab_lerp_of_a_single_stop_is_that_stop_at_every_t() {
        let stops = [(0.4, 0.1, -0.05)];
        assert_eq!(oklab_lerp(&stops, 0.0), stops[0]);
        assert_eq!(oklab_lerp(&stops, 1.0), stops[0]);
    }
}
