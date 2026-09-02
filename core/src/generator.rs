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

use cairo::{Context, LinearGradient, RadialGradient};

use crate::model::{GeneratedBackground, Rgba};
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

/// Picks a color at fractional position `t` (`0.0..=1.0`) along `palette`
/// by linearly interpolating between its two nearest entries — a smooth
/// ramp across the whole palette, rather than `generator`'s old
/// cycle-with-occasional-random-jump behavior. Falls back to a neutral
/// gray if `palette` is empty.
fn palette_lerp(palette: &[Rgba], t: f64) -> Rgba {
    if palette.is_empty() {
        return Rgba::new(0.5, 0.5, 0.5, 1.0);
    }
    if palette.len() == 1 {
        return palette[0];
    }
    let t = t.clamp(0.0, 1.0);
    let scaled = t * (palette.len() - 1) as f64;
    let i0 = scaled.floor() as usize;
    let i1 = (i0 + 1).min(palette.len() - 1);
    let frac = scaled - i0 as f64;
    let a = palette[i0];
    let b = palette[i1];
    Rgba::new(
        a.r + (b.r - a.r) * frac,
        a.g + (b.g - a.g) * frac,
        a.b + (b.b - a.b) * frac,
        a.a + (b.a - a.a) * frac,
    )
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
fn choose_corner(rng: &mut Rng, width: f64, height: f64, regions: &[ScreenshotRegion], avoid: bool) -> (f64, f64) {
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
/// Each layer's fill color comes from `palette_lerp` at its position in
/// the stack, so the palette reads as one smooth ramp from the outermost
/// to the innermost layer, darkest/lightest end depending on how the
/// caller resolved the palette.
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
    let segments = 24;

    for i in 0..layer_count {
        let t = i as f64 / (layer_count - 1).max(1) as f64;
        let r_base = max_radius + (min_radius - max_radius) * t;
        let phase = rng.range(0.0, std::f64::consts::PI * 2.0);
        let layer_amplitude = amplitude * rng.range(0.7, 1.3);

        ctx.new_path();
        ctx.move_to(focus.0, focus.1);
        for s in 0..=segments {
            let frac = s as f64 / segments as f64;
            let angle = base_angle - sweep_rad / 2.0 + frac * sweep_rad;
            let wobble = (frac * freq * std::f64::consts::PI * 2.0 + phase).sin() * layer_amplitude;
            let r = (r_base + wobble).max(1.0);
            ctx.line_to(focus.0 + angle.cos() * r, focus.1 + angle.sin() * r);
        }
        ctx.close_path();

        let color = palette_lerp(palette, t);
        let alpha = color.a * (1.0 - 0.3 * bg.contrast.clamp(0.0, 1.0) * (1.0 - t));

        if bg.softness < 0.5 {
            ctx.set_source_rgba(color.r, color.g, color.b, alpha);
        } else {
            let gradient = RadialGradient::new(focus.0, focus.1, 0.0, focus.0, focus.1, r_base + layer_amplitude);
            gradient.add_color_stop_rgba(0.0, color.r, color.g, color.b, alpha);
            gradient.add_color_stop_rgba(1.0, color.r, color.g, color.b, alpha * 0.6);
            ctx.set_source(&gradient)?;
        }
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

    #[test]
    fn palette_lerp_of_empty_palette_is_neutral_gray() {
        let c = palette_lerp(&[], 0.5);
        assert_eq!(c, Rgba::new(0.5, 0.5, 0.5, 1.0));
    }

    #[test]
    fn palette_lerp_at_the_ends_returns_the_end_colors_exactly() {
        let palette = vec![Rgba::new(0.0, 0.0, 0.0, 1.0), Rgba::new(1.0, 1.0, 1.0, 1.0)];
        assert_eq!(palette_lerp(&palette, 0.0), palette[0]);
        assert_eq!(palette_lerp(&palette, 1.0), palette[1]);
    }

    #[test]
    fn palette_lerp_at_the_midpoint_blends_the_two_nearest_colors() {
        let palette = vec![Rgba::new(0.0, 0.0, 0.0, 1.0), Rgba::new(1.0, 1.0, 1.0, 1.0)];
        let mid = palette_lerp(&palette, 0.5);
        assert!((mid.r - 0.5).abs() < 1e-9);
    }
}
