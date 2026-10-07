//! The modern generator styles (every [`GeneratorStyle`] except the legacy
//! `Waves`, which stays in `crate::generator`). Same contract as the
//! classic generator: pure vector drawing straight onto the caller's
//! context, deterministic in the seed and every parameter, so storing the
//! inputs in the project file is enough to reproduce a background.
//!
//! What all styles share, and what makes them read as designed rather
//! than random:
//!
//! - A palette resampled into a dark-to-light ramp plus one complementary
//!   accent ([`crate::palette::ramp_and_accent`]), so colors are always
//!   ordered and harmonious whatever strategy produced them.
//! - Smooth curves (Catmull-Rom converted to cubic Béziers) instead of
//!   polylines, so no visible kinks at any export size.
//! - Soft shadows cast from one light direction, and a gentle gradient
//!   inside every shape, which together give depth.
//! - The same knobs everywhere: `density` sets how many elements, `variation`
//!   how much they curve, `flow` the rotation, `contrast` shadow strength,
//!   `softness` shadow blur; `scale`/`offset_*` zoom and move the scene.
//!
//! Film grain is added afterwards by `crate::generator::render` for every
//! style.

use std::f64::consts::PI;

use cairo::{Context, Format, ImageSurface, LinearGradient, RadialGradient};

use crate::generator::ScreenshotRegion;
use crate::model::{GeneratedBackground, GeneratorStyle, Rgba};
use crate::palette::ramp_and_accent;
use crate::render::RenderError;
use crate::rng::Rng;

/// Longest side of the scratch surface a shadow is rasterized and blurred
/// on. Shadows are blurry by construction, so this resolution is plenty
/// even for a 4K export, and it keeps every shadow cheap.
const SHADOW_MAX_DIM: f64 = 384.0;

/// Canvas geometry plus the scene transform: points are given in canvas
/// coordinates, rotated by `rot` around the canvas center, then zoomed by
/// `scale` and moved by the offset (the same meaning `offset_x`/`offset_y`
/// and `scale` have for the classic generator).
struct Geo {
    w: f64,
    h: f64,
    /// The shorter canvas side — the unit for sizes, so portrait and
    /// landscape canvases get proportionally similar strokes and shadows.
    u: f64,
    rot: f64,
    scale: f64,
    shift: (f64, f64),
}

impl Geo {
    fn new(bg: &GeneratedBackground, w: f64, h: f64, rot: f64) -> Self {
        Geo {
            w,
            h,
            u: w.min(h),
            rot,
            scale: bg.scale.max(0.05),
            shift: (bg.offset_x.clamp(-1.0, 1.0) * w * 0.5, bg.offset_y.clamp(-1.0, 1.0) * h * 0.5),
        }
    }

    fn p(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let (cx, cy) = (self.w / 2.0, self.h / 2.0);
        let (dx, dy) = (x - cx, y - cy);
        let (s, c) = self.rot.sin_cos();
        let (rx, ry) = (dx * c - dy * s, dx * s + dy * c);
        (cx + rx * self.scale + self.shift.0, cy + ry * self.scale + self.shift.1)
    }

    /// A direction vector (e.g. a shadow offset) through the rotation only.
    fn v(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let (s, c) = self.rot.sin_cos();
        (x * c - y * s, x * s + y * c)
    }

    fn diag(&self) -> f64 {
        self.w.hypot(self.h)
    }
}

fn mix(a: Rgba, b: Rgba, t: f64) -> Rgba {
    Rgba::new(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, 1.0)
}

fn lighten(c: Rgba, t: f64) -> Rgba {
    mix(c, Rgba::new(1.0, 1.0, 1.0, 1.0), t)
}

fn darken(c: Rgba, t: f64) -> Rgba {
    mix(c, Rgba::new(0.0, 0.0, 0.0, 1.0), t)
}

fn linear(p0: (f64, f64), p1: (f64, f64), stops: &[(f64, Rgba)]) -> LinearGradient {
    let g = LinearGradient::new(p0.0, p0.1, p1.0, p1.1);
    for &(o, c) in stops {
        g.add_color_stop_rgba(o.clamp(0.0, 1.0), c.r, c.g, c.b, c.a);
    }
    g
}

/// Appends a smooth curve through `points` to the current path
/// (Catmull-Rom with the end points repeated, as cubic Béziers), starting
/// with `move_to` when `start` is set and `line_to` otherwise.
pub(crate) fn smooth(ctx: &Context, points: &[(f64, f64)], start: bool) {
    let Some(&first) = points.first() else { return };
    if start {
        ctx.move_to(first.0, first.1);
    } else {
        ctx.line_to(first.0, first.1);
    }
    for i in 0..points.len().saturating_sub(1) {
        let p0 = if i == 0 { points[0] } else { points[i - 1] };
        let (p1, p2) = (points[i], points[i + 1]);
        let p3 = if i + 2 < points.len() { points[i + 2] } else { p2 };
        ctx.curve_to(
            p1.0 + (p2.0 - p0.0) / 6.0,
            p1.1 + (p2.1 - p0.1) / 6.0,
            p2.0 - (p3.0 - p1.0) / 6.0,
            p2.1 - (p3.1 - p1.1) / 6.0,
            p2.0,
            p2.1,
        );
    }
}

/// Paints the soft shadow of the shape `build` traces, shifted by
/// `offset`, onto `ctx`. The shape is rasterized on a small scratch
/// surface (capped at [`SHADOW_MAX_DIM`]), blurred there, and stretched
/// back over its footprint. The footprint is clipped to the canvas plus
/// the blur's reach, so shapes that deliberately extend far beyond the
/// canvas (planes, huge rings) don't waste resolution on invisible area;
/// the clip edge lies outside the visible canvas, so no seam shows.
fn cast_shadow(
    ctx: &Context,
    geo: &Geo,
    build: &dyn Fn(&Context),
    offset: (f64, f64),
    blur: f64,
    alpha: f64,
) -> Result<(), RenderError> {
    if alpha <= 0.0 {
        return Ok(());
    }
    ctx.save()?;
    ctx.new_path();
    build(ctx);
    let (x1, y1, x2, y2) = ctx.fill_extents()?;
    ctx.new_path();
    ctx.restore()?;

    let reach = blur * 3.0 + offset.0.abs().max(offset.1.abs()) + 2.0;
    let (x1, y1) = (x1.max(-reach), y1.max(-reach));
    let (x2, y2) = (x2.min(geo.w + reach), y2.min(geo.h + reach));
    if x2 <= x1 || y2 <= y1 {
        return Ok(());
    }
    let (bw, bh) = (x2 - x1, y2 - y1);
    let scale = (SHADOW_MAX_DIM / (bw.max(bh) + blur * 6.0)).min(1.0);
    let render_blur = blur * scale;
    let pad = (render_blur * 3.0 + 4.0).ceil();
    let sw = (bw * scale + pad * 2.0).ceil() as i32;
    let sh = (bh * scale + pad * 2.0).ceil() as i32;
    if sw <= 0 || sh <= 0 {
        return Ok(());
    }

    let mut surface = ImageSurface::create(Format::ARgb32, sw, sh)?;
    {
        let sctx = Context::new(&surface)?;
        sctx.translate(pad, pad);
        sctx.scale(scale, scale);
        sctx.translate(-x1, -y1);
        sctx.new_path();
        build(&sctx);
        sctx.set_source_rgba(0.0, 0.0, 0.0, 1.0);
        sctx.fill()?;
    }
    if render_blur > 0.0 {
        let stride = surface.stride();
        let mut data = surface.data()?;
        crate::blur::box_blur(&mut data, sw, sh, stride, render_blur);
    }

    ctx.save()?;
    ctx.translate(x1 + offset.0, y1 + offset.1);
    ctx.scale(1.0 / scale, 1.0 / scale);
    ctx.set_source_surface(&surface, -pad, -pad)?;
    ctx.source().set_filter(cairo::Filter::Good);
    ctx.paint_with_alpha(alpha)?;
    ctx.restore()?;
    Ok(())
}

/// Fills the whole canvas with a two-color linear gradient at `angle`.
fn base_gradient(ctx: &Context, geo: &Geo, a: Rgba, b: Rgba, angle: f64) -> Result<(), RenderError> {
    let (cx, cy, r) = (geo.w / 2.0, geo.h / 2.0, geo.diag() / 2.0);
    let (s, c) = angle.sin_cos();
    ctx.set_source(linear((cx - c * r, cy - s * r), (cx + c * r, cy + s * r), &[(0.0, a), (1.0, b)]))?;
    ctx.paint()?;
    Ok(())
}

/// The shared shadow look: strength from `contrast`, blur from `softness`.
fn shadow_params(bg: &GeneratedBackground, geo: &Geo) -> (f64, f64) {
    let blur = geo.u * (0.012 + 0.05 * bg.softness.clamp(0.0, 1.0));
    let alpha = 0.18 + 0.32 * bg.contrast.clamp(0.0, 1.0);
    (blur, alpha)
}

fn count(density: f64, min: usize, extra: usize) -> usize {
    min + (density.clamp(0.0, 1.0) * extra as f64).round() as usize
}

/// Renders `bg` (any style but `Waves`) onto a `width`×`height` canvas.
pub(crate) fn render(ctx: &Context, bg: &GeneratedBackground, width: f64, height: f64, regions: &[ScreenshotRegion]) -> Result<(), RenderError> {
    let mut rng = Rng::new(bg.seed);
    let rot = (bg.flow.clamp(0.0, 1.0) - 0.5) * 0.9;
    let geo = Geo::new(bg, width, height, rot);
    let rng = &mut rng.fork(3);
    match bg.style {
        GeneratorStyle::Layers => layers(ctx, bg, &geo, rng),
        GeneratorStyle::Arcs => arcs(ctx, bg, &geo, rng, regions),
        GeneratorStyle::Ribbons => ribbons(ctx, bg, &geo, rng),
        GeneratorStyle::Planes => planes(ctx, bg, &geo, rng),
        GeneratorStyle::Lines => lines(ctx, bg, &geo, rng),
        GeneratorStyle::Mist | GeneratorStyle::Waves => mist(ctx, bg, &geo, rng),
    }
}

/// Stacked paper waves: the lightest layer at the back near the top edge,
/// each following layer darker and lower, every one casting a soft shadow
/// onto the layer behind it.
fn layers(ctx: &Context, bg: &GeneratedBackground, geo: &Geo, rng: &mut Rng) -> Result<(), RenderError> {
    let k = count(bg.density, 4, 4);
    let (ramp, _) = ramp_and_accent(&bg.palette, k + 1);
    let (w, h) = (geo.w, geo.h);
    base_gradient(ctx, geo, lighten(ramp[k], 0.2), ramp[k], PI / 2.0)?;

    let variation = bg.variation.clamp(0.0, 1.0);
    let humps = (0.8 + variation * 1.4) * rng.range(0.85, 1.15);
    let half = geo.diag() * 0.75;
    let (blur, alpha) = shadow_params(bg, geo);
    let shadow_offset = geo.v((0.0, -geo.u * 0.012));

    for j in 0..k {
        let t = j as f64 / (k - 1).max(1) as f64;
        let base_y = h / 2.0 + (-0.56 + 0.96 * t) * h;
        let phase = rng.range(0.0, PI * 2.0);
        let amp = h * (0.035 + 0.075 * variation) * rng.range(0.8, 1.2);
        let points: Vec<(f64, f64)> = (0..=14)
            .map(|i| {
                let x = w / 2.0 - half + 2.0 * half * i as f64 / 14.0;
                geo.p((x, base_y + ((x - w / 2.0) / (2.0 * half) * humps * PI * 2.0 + phase).sin() * amp))
            })
            .collect();
        let bottom_right = geo.p((w / 2.0 + half, h / 2.0 + half * 1.5));
        let bottom_left = geo.p((w / 2.0 - half, h / 2.0 + half * 1.5));
        let build = |c: &Context| {
            smooth(c, &points, true);
            c.line_to(bottom_right.0, bottom_right.1);
            c.line_to(bottom_left.0, bottom_left.1);
            c.close_path();
        };
        cast_shadow(ctx, geo, &build, shadow_offset, blur, alpha)?;
        ctx.new_path();
        build(ctx);
        let color = ramp[k - 1 - j];
        let g = linear(geo.p((w / 2.0, base_y - amp)), geo.p((w / 2.0, base_y + h * 0.5)), &[(0.0, lighten(color, 0.1)), (1.0, color)]);
        ctx.set_source(&g)?;
        ctx.fill()?;
    }
    Ok(())
}

/// Concentric rings around a canvas corner (the least covered one when
/// adapting to the screenshots), optionally a smaller second set in the
/// accent color. `corner_bias` pulls the center from outside the canvas
/// (`0.0`, flatter arcs) right into the corner (`1.0`).
fn arcs(ctx: &Context, bg: &GeneratedBackground, geo: &Geo, rng: &mut Rng, regions: &[ScreenshotRegion]) -> Result<(), RenderError> {
    let rings = count(bg.density, 5, 4);
    let (ramp, accent) = ramp_and_accent(&bg.palette, rings);
    base_gradient(ctx, geo, ramp[0], darken(ramp[0], 0.25), 0.8)?;

    let (w, h) = (geo.w, geo.h);
    let main = crate::generator::choose_corner(rng, w, h, regions, bg.adapt_to_screenshots);
    let mut centers = vec![(main, false)];
    if rng.chance(0.6) {
        let other = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)][rng.index(4)];
        if other != main {
            centers.push((other, true));
        }
    }
    let (blur, alpha) = shadow_params(bg, geo);
    let pull = (1.0 - bg.corner_bias.clamp(0.0, 1.0)) * geo.u * 0.5;
    let spread = 0.7 + 0.25 * bg.variation.clamp(0.0, 1.0);

    for ((cx, cy), secondary) in centers {
        let outward = {
            let (dx, dy) = (cx - w / 2.0, cy - h / 2.0);
            let len = dx.hypot(dy).max(1.0);
            (dx / len, dy / len)
        };
        let center = geo.p((cx + outward.0 * pull, cy + outward.1 * pull));
        let r_max = w.max(h) * geo.scale * if secondary { rng.range(0.32, 0.48) } else { rng.range(0.78, 1.0) } + pull * geo.scale;
        let count = if secondary { rings.min(4) } else { rings };
        for k in 0..count {
            let t = k as f64 / (count - 1).max(1) as f64;
            let r = r_max * (1.0 - t * spread);
            let mut color = ramp[((1.0 - t) * (rings - 1) as f64).round() as usize];
            if secondary {
                color = mix(color, accent, 0.55);
            }
            let build = |c: &Context| {
                c.new_path();
                c.arc(center.0, center.1, r, 0.0, PI * 2.0);
            };
            cast_shadow(ctx, geo, &build, (0.0, geo.u * 0.004), blur, alpha)?;
            build(ctx);
            let g = RadialGradient::new(center.0, center.1, r * 0.55, center.0, center.1, r);
            let (inner, outer) = (darken(color, 0.06), lighten(color, 0.08));
            g.add_color_stop_rgb(0.0, inner.r, inner.g, inner.b);
            g.add_color_stop_rgb(1.0, outer.r, outer.g, outer.b);
            ctx.set_source(&g)?;
            ctx.fill()?;
        }
    }
    Ok(())
}

/// Wide silk ribbons crossing the canvas, each tapering toward its ends,
/// with alternating light/dark stops along its length for the folds.
fn ribbons(ctx: &Context, bg: &GeneratedBackground, geo: &Geo, rng: &mut Rng) -> Result<(), RenderError> {
    let n = 6;
    let (ramp, accent) = ramp_and_accent(&bg.palette, n);
    let (w, h, u) = (geo.w, geo.h, geo.u);
    base_gradient(ctx, geo, lighten(ramp[n - 1], 0.1), ramp[n - 2], rng.range(0.0, PI))?;

    let ribbons = count(bg.density, 2, 2);
    let bend = 0.5 + bg.variation.clamp(0.0, 1.0);
    let direction = if rng.chance(0.5) { 1.0 } else { -1.0 };
    let (blur, alpha) = shadow_params(bg, geo);
    let shadow_offset = geo.v((u * 0.01, u * 0.025));

    for k in 0..ribbons {
        let color = if k == ribbons - 1 && rng.chance(0.5) { accent } else { ramp[(k * 2 + rng.index(2)) % (n - 1)] };
        let (y0, y1) = (h * rng.range(-0.05, 1.05), h * rng.range(-0.05, 1.05));
        let thickness = u * rng.range(0.2, 0.4);
        let mid = rng.range(-0.25, 0.25) * h * bend * direction;
        let spine: Vec<(f64, f64)> = (0..=8)
            .map(|i| {
                let t = i as f64 / 8.0;
                (-0.15 * w + t * 1.3 * w, y0 + (y1 - y0) * t + (t * PI).sin() * mid)
            })
            .collect();
        let half_width = |i: usize| thickness * (0.55 + 0.45 * (i as f64 / 8.0 * PI).sin()) / 2.0;
        let top: Vec<(f64, f64)> = spine.iter().enumerate().map(|(i, &(x, y))| geo.p((x, y - half_width(i)))).collect();
        let bottom: Vec<(f64, f64)> = spine.iter().enumerate().rev().map(|(i, &(x, y))| geo.p((x, y + half_width(i)))).collect();
        let build = |c: &Context| {
            smooth(c, &top, true);
            smooth(c, &bottom, false);
            c.close_path();
        };
        cast_shadow(ctx, geo, &build, shadow_offset, blur, alpha)?;
        ctx.new_path();
        build(ctx);

        let folds = 3 + rng.index(3);
        let stops: Vec<(f64, Rgba)> = (0..=folds * 2)
            .map(|f| {
                let t = f as f64 / (folds * 2) as f64 + rng.range(-0.03, 0.03);
                (t, if f % 2 == 0 { darken(color, 0.22) } else { lighten(color, 0.18) })
            })
            .collect();
        ctx.set_source(linear(geo.p((-0.15 * w, h / 2.0)), geo.p((1.15 * w, h / 2.0)), &stops))?;
        ctx.fill()?;
    }
    Ok(())
}

/// Overlapping half-planes like folded paper: straight diagonal edges, a
/// soft shadow along each fold and a thin highlight on the edge itself.
fn planes(ctx: &Context, bg: &GeneratedBackground, geo: &Geo, rng: &mut Rng) -> Result<(), RenderError> {
    let k = count(bg.density, 4, 3);
    let (ramp, _) = ramp_and_accent(&bg.palette, k + 1);
    let (w, h, u) = (geo.w, geo.h, geo.u);
    let diag = geo.diag();
    base_gradient(ctx, geo, ramp[k], lighten(ramp[k - 1], 0.2), 1.2)?;

    let angle = rng.range(0.3, 1.2) * if rng.chance(0.5) { 1.0 } else { -1.0 };
    let tilt_range = 0.05 + 0.2 * bg.variation.clamp(0.0, 1.0);
    let (blur, alpha) = shadow_params(bg, geo);
    let blur = blur * 1.3;

    for j in 0..k {
        let t = j as f64 / k as f64;
        let offset = (0.5 - t) * diag * 0.7 + rng.range(-0.05, 0.05) * diag;
        let a = angle + rng.range(-tilt_range, tilt_range);
        let (dx, dy) = (a.cos(), a.sin());
        let (px, py) = (w / 2.0 - dy * offset, h / 2.0 + dx * offset);
        let corners = [
            geo.p((px - dx * diag, py - dy * diag)),
            geo.p((px + dx * diag, py + dy * diag)),
            geo.p((px + dx * diag + dy * diag * 2.0, py + dy * diag - dx * diag * 2.0)),
            geo.p((px - dx * diag + dy * diag * 2.0, py - dy * diag - dx * diag * 2.0)),
        ];
        let build = |c: &Context| {
            c.move_to(corners[0].0, corners[0].1);
            for p in &corners[1..] {
                c.line_to(p.0, p.1);
            }
            c.close_path();
        };
        cast_shadow(ctx, geo, &build, (0.0, 0.0), blur, alpha)?;
        ctx.new_path();
        build(ctx);
        let color = ramp[k - 1 - j];
        let g = linear(geo.p((px, py)), geo.p((px + dy * diag * 0.6, py - dx * diag * 0.6)), &[(0.0, darken(color, 0.05)), (1.0, lighten(color, 0.18))]);
        ctx.set_source(&g)?;
        ctx.fill()?;

        ctx.move_to(corners[0].0, corners[0].1);
        ctx.line_to(corners[1].0, corners[1].1);
        ctx.set_source_rgba(1.0, 1.0, 1.0, 0.18);
        ctx.set_line_width(u * 0.003 * geo.scale);
        ctx.stroke()?;
    }
    Ok(())
}

/// A bundle of fine wave lines that pinches and widens across the canvas,
/// over a gradient with a soft glow in the accent color.
fn lines(ctx: &Context, bg: &GeneratedBackground, geo: &Geo, rng: &mut Rng) -> Result<(), RenderError> {
    let n = 6;
    let (ramp, accent) = ramp_and_accent(&bg.palette, n);
    let (w, h, u) = (geo.w, geo.h, geo.u);
    base_gradient(ctx, geo, ramp[0], ramp[n / 2], rng.range(0.0, PI * 2.0))?;

    let glow_at = (w * rng.range(0.6, 0.9), h * rng.range(0.1, 0.4));
    let glow = RadialGradient::new(glow_at.0, glow_at.1, 0.0, glow_at.0, glow_at.1, w.max(h) * 0.7);
    glow.add_color_stop_rgba(0.0, accent.r, accent.g, accent.b, 0.45);
    glow.add_color_stop_rgba(1.0, accent.r, accent.g, accent.b, 0.0);
    ctx.set_source(&glow)?;
    ctx.paint()?;

    let lines = count(bg.density, 24, 36);
    let phase = rng.range(0.0, PI * 2.0);
    let frequency = rng.range(0.8, 1.6);
    let amp = h * (0.08 + 0.2 * bg.variation.clamp(0.0, 1.0));
    let light = lighten(ramp[n - 1], 0.2);
    ctx.set_line_width(u * 0.004 * geo.scale);
    for k in 0..lines {
        let t = k as f64 / lines as f64;
        let y0 = (-0.55 + t * 1.1) * h;
        let points: Vec<(f64, f64)> = (0..=18)
            .map(|i| {
                let x = -w + 2.0 * w * i as f64 / 18.0;
                let pinch = 0.35 + 0.65 * ((x / w * 0.8 + phase).cos()).abs();
                geo.p((w / 2.0 + x, h / 2.0 + y0 * pinch + (x / w * frequency * PI + phase + t * 0.8).sin() * amp))
            })
            .collect();
        ctx.new_path();
        smooth(ctx, &points, true);
        let c = mix(light, accent, t);
        ctx.set_source_rgba(c.r, c.g, c.b, 0.3 + 0.45 * (t * PI).sin());
        ctx.stroke()?;
    }
    Ok(())
}

/// Large soft color fields like a mesh gradient — the calmest style.
/// `softness` widens the fields, `variation` scatters them further.
fn mist(ctx: &Context, bg: &GeneratedBackground, geo: &Geo, rng: &mut Rng) -> Result<(), RenderError> {
    let n = 6;
    let (ramp, accent) = ramp_and_accent(&bg.palette, n);
    let (w, h) = (geo.w, geo.h);
    let base = ramp[n / 2];
    ctx.set_source_rgb(base.r, base.g, base.b);
    ctx.paint()?;

    let blobs = count(bg.density, 4, 4);
    let reach = 0.1 + 0.25 * bg.variation.clamp(0.0, 1.0);
    let size = 0.45 + 0.35 * bg.softness.clamp(0.0, 1.0);
    for k in 0..blobs {
        let color = if k == blobs - 1 { accent } else { ramp[rng.index(n)] };
        let (x, y) = geo.p((w * rng.range(-reach, 1.0 + reach), h * rng.range(-reach, 1.0 + reach)));
        let r = w.max(h) * size * rng.range(0.8, 1.25) * geo.scale;
        // A Gaussian-like falloff sampled in many stops: with only a few
        // linear segments, the kinks between them show up as rings.
        let g = RadialGradient::new(x, y, 0.0, x, y, r);
        for i in 0..=12 {
            let t = i as f64 / 12.0;
            let alpha = 0.95 * (-2.0 * t * t).exp() * (1.0 - t * t);
            g.add_color_stop_rgba(t, color.r, color.g, color.b, alpha);
        }
        ctx.set_source(&g)?;
        ctx.paint()?;
    }
    Ok(())
}

/// Overlays deterministic film grain on the `width`×`height` canvas:
/// a 128-pixel noise tile, repeated, blended with the overlay operator so
/// it brightens and darkens evenly without shifting colors. One noise
/// cell is about one pixel of a 1600-pixel-wide export and scales with
/// the canvas, so the preview and the export show the same texture.
pub(crate) fn apply_grain(ctx: &Context, width: f64, height: f64, amount: f64, seed: u64) -> Result<(), RenderError> {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 || width <= 0.0 || height <= 0.0 {
        return Ok(());
    }
    const TILE: i32 = 128;
    let mut tile = ImageSurface::create(Format::Rgb24, TILE, TILE)?;
    {
        let stride = tile.stride() as usize;
        let mut data = tile.data()?;
        let mut rng = Rng::new(seed ^ 0x6772_6169_6E00_0000);
        for y in 0..TILE as usize {
            for x in 0..TILE as usize {
                let v = ((rng.next_f64() + rng.next_f64()) * 0.5 * 255.0) as u8;
                let i = y * stride + x * 4;
                data[i] = v;
                data[i + 1] = v;
                data[i + 2] = v;
                data[i + 3] = 255;
            }
        }
    }
    let pattern = cairo::SurfacePattern::create(&tile);
    pattern.set_extend(cairo::Extend::Repeat);
    pattern.set_filter(cairo::Filter::Nearest);
    let cell = (width.max(height) / 1600.0).max(0.5);
    let mut matrix = cairo::Matrix::identity();
    matrix.scale(1.0 / cell, 1.0 / cell);
    pattern.set_matrix(matrix);

    ctx.save()?;
    ctx.rectangle(0.0, 0.0, width, height);
    ctx.clip();
    ctx.set_operator(cairo::Operator::Overlay);
    ctx.set_source(&pattern)?;
    ctx.paint_with_alpha(amount * 0.16)?;
    ctx.restore()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ColorStrategy, Mood};

    fn background(style: GeneratorStyle, seed: u64) -> GeneratedBackground {
        GeneratedBackground {
            style,
            palette: crate::palette::mood_palette(seed, Mood::Vivid, None, 1.0),
            color_strategy: ColorStrategy::Random,
            ..GeneratedBackground::new(seed)
        }
    }

    fn render_checksum(bg: &GeneratedBackground, w: i32, h: i32) -> u64 {
        let mut surface = ImageSurface::create(Format::ARgb32, w, h).unwrap();
        {
            let ctx = Context::new(&surface).unwrap();
            crate::generator::render(&ctx, bg, w as f64, h as f64, &[]).unwrap();
        }
        let data = surface.data().unwrap();
        data.iter().fold(1469598103934665603u64, |h, &b| (h ^ b as u64).wrapping_mul(1099511628211))
    }

    #[test]
    fn every_style_renders_deterministically() {
        for style in GeneratorStyle::MODERN {
            let bg = background(style, 4711);
            assert_eq!(render_checksum(&bg, 320, 180), render_checksum(&bg, 320, 180), "{style:?}");
        }
    }

    #[test]
    fn styles_and_seeds_produce_different_scenes() {
        let mut seen = std::collections::HashSet::new();
        for style in GeneratorStyle::MODERN {
            for seed in [1, 2] {
                assert!(seen.insert(render_checksum(&background(style, seed), 160, 90)), "{style:?} seed {seed} duplicated another scene");
            }
        }
    }

    #[test]
    fn every_style_paints_the_whole_canvas_opaquely() {
        for style in GeneratorStyle::MODERN {
            let bg = background(style, 9);
            let mut surface = ImageSurface::create(Format::ARgb32, 200, 120).unwrap();
            {
                let ctx = Context::new(&surface).unwrap();
                crate::generator::render(&ctx, &bg, 200.0, 120.0, &[]).unwrap();
            }
            let stride = surface.stride() as usize;
            let data = surface.data().unwrap();
            for (x, y) in [(0, 0), (199, 0), (0, 119), (199, 119), (100, 60)] {
                assert_eq!(data[y * stride + x * 4 + 3], 255, "{style:?} left ({x},{y}) transparent");
            }
        }
    }

    #[test]
    fn rendering_a_large_export_stays_fast() {
        for style in GeneratorStyle::MODERN {
            let bg = GeneratedBackground { density: 1.0, softness: 1.0, ..background(style, 3) };
            let start = std::time::Instant::now();
            let surface = ImageSurface::create(Format::ARgb32, 3840, 2160).unwrap();
            let ctx = Context::new(&surface).unwrap();
            crate::generator::render(&ctx, &bg, 3840.0, 2160.0, &[]).unwrap();
            // Generous for debug builds on slow machines; a release build
            // is roughly ten times faster.
            assert!(start.elapsed().as_secs_f64() < 8.0, "{style:?} took {:?}", start.elapsed());
        }
    }

    #[test]
    fn grain_changes_pixels_and_zero_grain_does_not() {
        let base = GeneratedBackground { grain: 0.0, ..background(GeneratorStyle::Mist, 5) };
        let grained = GeneratedBackground { grain: 0.8, ..base.clone() };
        let plain = render_checksum(&base, 160, 90);
        assert_ne!(plain, render_checksum(&grained, 160, 90));
        assert_eq!(plain, render_checksum(&base, 160, 90));
    }

    #[test]
    fn offset_and_scale_move_the_scene() {
        let bg = background(GeneratorStyle::Arcs, 12);
        let plain = render_checksum(&bg, 160, 90);
        assert_ne!(plain, render_checksum(&GeneratedBackground { offset_x: 0.4, ..bg.clone() }, 160, 90));
        assert_ne!(plain, render_checksum(&GeneratedBackground { scale: 1.4, ..bg.clone() }, 160, 90));
    }

    #[test]
    fn an_empty_palette_falls_back_without_panicking() {
        for style in GeneratorStyle::MODERN {
            let bg = GeneratedBackground { palette: Vec::new(), ..background(style, 1) };
            render_checksum(&bg, 64, 64);
        }
    }

    #[test]
    fn a_portrait_canvas_renders_every_style() {
        for style in GeneratorStyle::MODERN {
            render_checksum(&background(style, 21), 90, 200);
        }
    }
}
