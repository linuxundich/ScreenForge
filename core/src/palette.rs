//! Suggests a background gradient that *complements* a set of imported
//! screenshots rather than copying their colors back at the user — the
//! automatic-colors feature behind the background editor's "Generate from
//! screenshots" action.
//!
//! Works in [Oklab](https://bottosson.github.io/posts/oklab/), a
//! perceptually-uniform color space, rather than negating sRGB channels:
//! RGB inversion has no consistent relationship to hue or lightness (e.g.
//! inverting a mid-gray does almost nothing), while a complementary hue
//! plus an inverted lightness in Oklab reliably produces the kind of
//! contrast the spec's worked examples describe (a dark/cool screenshot
//! suggesting a light/warm background, and vice versa).

use std::f64::consts::PI;

use crate::model::{ColorStrategy, GradientKind, GradientSpec, Rgba};

/// One decoded image's premultiplied ARGB32 pixel bytes (native-endian
/// 0xAARRGGBB, i.e. byte order B, G, R, A on a little-endian target — the
/// same layout `cairo::Format::ARgb32` and the app's `import::DecodedImage`
/// use), plus its pixel size. `core` stays GTK-independent (see the crate
/// doc comment), so this is a deliberate, minimal mirror of
/// `DecodedImage`'s shape rather than a shared type — the app layer just
/// borrows its own `DecodedImage` fields into one of these.
pub struct PixelSample<'a> {
    pub bytes: &'a [u8],
    pub width: i32,
    pub height: i32,
}

/// Every Nth pixel, in each direction, is sampled when computing an
/// average color — an average doesn't need every pixel to be
/// representative, and a multi-megapixel screenshot would otherwise make
/// this noticeably slow for no visible benefit.
const SAMPLE_STRIDE: usize = 7;

/// The alpha-weighted average color across every given image, unpremultiplied.
/// Falls back to a neutral mid-gray if `images` is empty or fully
/// transparent, so callers never need to special-case "no screenshots yet".
pub fn average_color(images: &[PixelSample]) -> Rgba {
    let (mut sum_r, mut sum_g, mut sum_b, mut weight) = (0.0, 0.0, 0.0, 0.0);

    for image in images {
        if image.width <= 0 || image.height <= 0 {
            continue;
        }
        let stride = image.width as usize * 4;
        for y in (0..image.height as usize).step_by(SAMPLE_STRIDE) {
            let row_start = y * stride;
            if row_start + stride > image.bytes.len() {
                break;
            }
            let row = &image.bytes[row_start..row_start + stride];
            for x in (0..image.width as usize).step_by(SAMPLE_STRIDE) {
                let i = x * 4;
                let a = row[i + 3] as f64;
                if a <= 0.0 {
                    continue;
                }
                let a_frac = a / 255.0;
                // Premultiplied storage means the stored channel already
                // has `a_frac` folded in — dividing it back out is what
                // makes a mostly-transparent image's *visible* colors
                // count instead of skewing everything toward black.
                sum_b += (row[i] as f64 / a) * a_frac;
                sum_g += (row[i + 1] as f64 / a) * a_frac;
                sum_r += (row[i + 2] as f64 / a) * a_frac;
                weight += a_frac;
            }
        }
    }

    if weight <= 0.0 {
        return Rgba::new(0.5, 0.5, 0.5, 1.0);
    }
    Rgba::new((sum_r / weight).clamp(0.0, 1.0), (sum_g / weight).clamp(0.0, 1.0), (sum_b / weight).clamp(0.0, 1.0), 1.0)
}

/// Suggests a two-stop linear gradient that complements `images`' average
/// color. `seed` only changes *which* complementary hues/angle the
/// suggestion lands on — every seed still produces a background that
/// contrasts the source images the same way — so a "Regenerate" action can
/// just increment it for a genuinely different, still-suitable palette,
/// with the same seed always reproducing the same result (useful for
/// tests, and means nothing extra needs to be stored to make regeneration
/// repeatable).
pub fn suggest_gradient(images: &[PixelSample], seed: u32) -> GradientSpec {
    let avg = average_color(images);
    let (lightness, a, b) = rgb_to_oklab(avg);
    let source_chroma = a.hypot(b);
    let source_hue = b.atan2(a);

    // ~180° opposite hue + inverted lightness: a dark, saturated blue
    // screenshot (low lightness, hue in blue's range) lands on a light,
    // warm suggestion (high lightness, hue rotated into orange/yellow's
    // range) -- and a light, warm/orange one lands on a dark, cool
    // suggestion. Exactly the two worked examples in the spec, and it
    // falls out of the hue/lightness math rather than needing separate
    // "if warm then..." branching.
    let complementary_hue = source_hue + PI;
    let base_lightness = (1.0 - lightness).clamp(0.20, 0.85);
    // Backgrounds read best with restrained chroma regardless of how
    // saturated the source was -- this keeps every suggestion in a
    // plausible "background", not "neon", range.
    let base_chroma = (source_chroma * 0.7).clamp(0.02, 0.09);

    // A deterministic, seed-derived spread: golden-angle-ish multipliers
    // keep consecutive seeds from landing near each other, so repeated
    // "Regenerate" clicks feel varied rather than cycling through a
    // handful of near-duplicates.
    let spread = (seed as f64 * 0.6180339887 * 2.0 * PI).sin() * 0.5;
    let angle_deg = (seed as f64 * 63.398).rem_euclid(360.0);

    let stop0 = rgba_from_oklch(base_lightness, base_chroma, complementary_hue - 0.4 + spread);
    let stop1_lightness = (base_lightness * 1.15).clamp(0.08, 0.95);
    let stop1 = rgba_from_oklch(stop1_lightness, base_chroma * 0.8, complementary_hue + 0.4 + spread);

    GradientSpec { kind: GradientKind::Linear { angle_deg }, stops: vec![(0.0, stop0), (1.0, stop1)] }
}

fn rgba_from_oklch(lightness: f64, chroma: f64, hue: f64) -> Rgba {
    oklab_to_rgb(lightness, chroma * hue.cos(), chroma * hue.sin())
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// sRGB -> Oklab. Coefficients are Björn Ottosson's reference matrices.
fn rgb_to_oklab(c: Rgba) -> (f64, f64, f64) {
    let (r, g, b) = (srgb_to_linear(c.r), srgb_to_linear(c.g), srgb_to_linear(c.b));

    let l = 0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b;
    let m = 0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b;
    let s = 0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b;
    let (l_, m_, s_) = (l.cbrt(), m.cbrt(), s.cbrt());

    (
        0.210_454_255_3 * l_ + 0.793_617_785_0 * m_ - 0.004_072_046_8 * s_,
        1.977_998_495_1 * l_ - 2.428_592_205_0 * m_ + 0.450_593_709_9 * s_,
        0.025_904_037_1 * l_ + 0.782_771_766_2 * m_ - 0.808_675_766_0 * s_,
    )
}

/// Oklab -> sRGB, the inverse of `rgb_to_oklab`. Not every Oklab
/// coordinate maps to a representable sRGB color, so the result is
/// clamped to `0.0..=1.0` per channel — visually this just desaturates a
/// suggestion slightly rather than producing an invalid color.
fn oklab_to_rgb(l: f64, a: f64, b: f64) -> Rgba {
    let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
    let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
    let s_ = l - 0.089_484_177_5 * a - 1.291_485_548_0 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);

    let r = 4.076_741_662_1 * l3 - 3.307_711_591_3 * m3 + 0.230_969_929_2 * s3;
    let g = -1.268_438_004_6 * l3 + 2.609_757_401_1 * m3 - 0.341_319_396_5 * s3;
    let bl = -0.004_196_086_3 * l3 - 0.703_418_614_7 * m3 + 1.707_614_701_0 * s3;

    Rgba::new(
        linear_to_srgb(r.clamp(0.0, 1.0)),
        linear_to_srgb(g.clamp(0.0, 1.0)),
        linear_to_srgb(bl.clamp(0.0, 1.0)),
        1.0,
    )
}

/// Extracts a small, perceptually meaningful set of dominant colors from
/// `images` — not the most frequent raw pixel values, but a handful of
/// representative tones ("the resulting palette should contain a small
/// number of meaningful colors rather than hundreds of individual
/// colors"). Pixels are bucketed by a coarse Oklab grid — a lightweight
/// histogram/quantization pass, not iterative k-means — and the `count`
/// heaviest buckets' average colors become the palette, ordered by
/// weight (most representative first).
///
/// Sorting breaks ties by bucket key rather than relying on `HashMap`
/// iteration order: Rust's default hasher is randomized per process, so
/// without an explicit, order-independent tie-break, two runs over the
/// identical image could disagree on tied buckets' relative order —
/// silently breaking the "same seed always reproduces the same
/// background" guarantee everything downstream of this depends on.
pub fn extract_palette(images: &[PixelSample], count: usize) -> Vec<Rgba> {
    use std::collections::HashMap;

    /// A quantized Oklab bucket's accumulated `(sum_r, sum_g, sum_b, weight)`.
    type BucketTotals = (f64, f64, f64, f64);
    let mut buckets: HashMap<(i32, i32, i32), BucketTotals> = HashMap::new();

    for image in images {
        if image.width <= 0 || image.height <= 0 {
            continue;
        }
        let stride = image.width as usize * 4;
        for y in (0..image.height as usize).step_by(SAMPLE_STRIDE) {
            let row_start = y * stride;
            if row_start + stride > image.bytes.len() {
                break;
            }
            let row = &image.bytes[row_start..row_start + stride];
            for x in (0..image.width as usize).step_by(SAMPLE_STRIDE) {
                let i = x * 4;
                let a = row[i + 3] as f64;
                if a <= 0.0 {
                    continue;
                }
                let a_frac = a / 255.0;
                let color =
                    Rgba::new((row[i + 2] as f64 / a).clamp(0.0, 1.0), (row[i + 1] as f64 / a).clamp(0.0, 1.0), (row[i] as f64 / a).clamp(0.0, 1.0), 1.0);
                let (l, ca, cb) = rgb_to_oklab(color);
                let key = ((l * 12.0).round() as i32, (ca * 24.0).round() as i32, (cb * 24.0).round() as i32);
                let entry = buckets.entry(key).or_insert((0.0, 0.0, 0.0, 0.0));
                entry.0 += color.r * a_frac;
                entry.1 += color.g * a_frac;
                entry.2 += color.b * a_frac;
                entry.3 += a_frac;
            }
        }
    }

    let mut clusters: Vec<((i32, i32, i32), Rgba, f64)> =
        buckets.into_iter().map(|(key, (r, g, b, w))| (key, Rgba::new(r / w, g / w, b / w, 1.0), w)).collect();
    clusters.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
    clusters.into_iter().take(count.max(1)).map(|(_, c, _)| c).collect()
}

/// Builds a small palette for `strategy` from `images`. `ColorStrategy::Manual`
/// has nothing to resolve — the user's own chosen colors *are* the palette —
/// so this returns an empty `Vec` for it rather than panicking; callers must
/// special-case `Manual` themselves and keep the palette the user already
/// set instead of calling this. `Random` ignores `images` entirely and uses
/// `seed` instead. `inverse_contrast` (`0.0..=1.0`) only affects
/// `FromScreenshots`: `0.0` stays close to the screenshots' own dominant
/// hue/lightness, `1.0` pushes toward their Oklab complement (opposite hue,
/// inverted lightness) — see `suggest_gradient`'s doc comment for why Oklab
/// rather than RGB inversion. Falls back to a neutral gray-based palette if
/// `images` is empty (nothing has been imported yet).
pub fn resolve_palette(images: &[PixelSample], strategy: ColorStrategy, inverse_contrast: f64, seed: u64) -> Vec<Rgba> {
    match strategy {
        ColorStrategy::Manual => Vec::new(),
        ColorStrategy::Random => random_palette(seed),
        ColorStrategy::Grayscale => {
            let dominant = extract_palette(images, 3);
            let base = dominant.first().copied().unwrap_or_else(|| average_color(images));
            let (lightness, _, _) = rgb_to_oklab(base);
            let strength = inverse_contrast.clamp(0.0, 1.0);
            let shifted_lightness = lightness * (1.0 - strength) + (1.0 - lightness) * strength;
            lightness_variations(0.0, shifted_lightness, 0.0, 0.8, 4)
        }
        ColorStrategy::FromScreenshots => {
            let dominant = extract_palette(images, 3);
            let base = dominant.first().copied().unwrap_or_else(|| average_color(images));
            let (lightness, a, b) = rgb_to_oklab(base);
            let source_hue = b.atan2(a);
            let source_chroma = a.hypot(b);
            let strength = inverse_contrast.clamp(0.0, 1.0);
            let hue = source_hue + PI * strength;
            let shifted_lightness = lightness * (1.0 - strength) + (1.0 - lightness) * strength;
            lightness_variations(hue, shifted_lightness, (source_chroma * 0.8).clamp(0.03, 0.16), 0.15, 4)
        }
    }
}

/// `count` swatches at the same hue/chroma, spread across a `spread`-wide
/// lightness band centered on `lightness`.
fn lightness_variations(hue: f64, lightness: f64, chroma: f64, spread: f64, count: usize) -> Vec<Rgba> {
    (0..count)
        .map(|i| {
            let t = if count <= 1 { 0.0 } else { (i as f64 / (count - 1) as f64) - 0.5 };
            rgba_from_oklch((lightness + t * spread).clamp(0.05, 0.95), chroma, hue)
        })
        .collect()
}

/// `count` swatches at the same lightness/chroma, spread across hues
/// neighboring `base_hue` within `+/- spread_radians / 2`.
fn hue_variations(base_hue: f64, lightness: f64, chroma: f64, spread_radians: f64, count: usize) -> Vec<Rgba> {
    (0..count)
        .map(|i| {
            let t = if count <= 1 { 0.0 } else { (i as f64 / (count - 1) as f64) - 0.5 };
            rgba_from_oklch(lightness, chroma, base_hue + t * spread_radians)
        })
        .collect()
}

/// A small palette that ignores the screenshots entirely — deterministic
/// from `seed` alone, still constrained to a plausible hue/chroma/
/// lightness range so it reads as "an aesthetically valid palette",
/// per the spec, rather than arbitrary noise.
fn random_palette(seed: u64) -> Vec<Rgba> {
    let mut rng = crate::rng::Rng::new(seed ^ 0xA5A5_A5A5_A5A5_A5A5);
    let base_hue = rng.range(0.0, PI * 2.0);
    let chroma = rng.range(0.06, 0.14);
    let lightness = rng.range(0.3, 0.7);
    let spread = rng.range(0.6, 1.6);
    hue_variations(base_hue, lightness, chroma, spread, 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A solid-color `w`x`h` premultiplied-ARGB32 buffer.
    fn solid_pixels(w: i32, h: i32, r: u8, g: u8, b: u8, a: u8) -> Vec<u8> {
        let af = a as f64 / 255.0;
        let (pr, pg, pb) = ((r as f64 * af) as u8, (g as f64 * af) as u8, (b as f64 * af) as u8);
        [pb, pg, pr, a].repeat((w * h) as usize)
    }

    #[test]
    fn average_color_of_a_solid_image_is_that_color() {
        let pixels = solid_pixels(20, 20, 200, 100, 50, 255);
        let avg = average_color(&[PixelSample { bytes: &pixels, width: 20, height: 20 }]);
        assert!((avg.r - 200.0 / 255.0).abs() < 0.02);
        assert!((avg.g - 100.0 / 255.0).abs() < 0.02);
        assert!((avg.b - 50.0 / 255.0).abs() < 0.02);
    }

    #[test]
    fn average_color_of_no_images_is_a_neutral_fallback_not_a_panic() {
        let avg = average_color(&[]);
        assert_eq!(avg, Rgba::new(0.5, 0.5, 0.5, 1.0));
    }

    #[test]
    fn fully_transparent_pixels_are_excluded_rather_than_counted_as_black() {
        let pixels = solid_pixels(20, 20, 10, 10, 10, 0);
        let avg = average_color(&[PixelSample { bytes: &pixels, width: 20, height: 20 }]);
        // Every pixel has alpha 0, so nothing should contribute -- the
        // neutral fallback, not near-black.
        assert_eq!(avg, Rgba::new(0.5, 0.5, 0.5, 1.0));
    }

    #[test]
    fn a_dark_blue_source_suggests_a_lighter_warmer_background() {
        let pixels = solid_pixels(20, 20, 10, 20, 90, 255); // dark, cool blue
        let gradient = suggest_gradient(&[PixelSample { bytes: &pixels, width: 20, height: 20 }], 0);

        for (_, color) in &gradient.stops {
            let (l, a, b) = rgb_to_oklab(*color);
            assert!(l > 0.35, "expected a lighter background, got lightness {l}");
            // A warm hue (red/orange/yellow) has a positive Oklab `a` and
            // small/positive `b`; a cool blue source (roughly hue ~250°)
            // rotated 180° lands near hue ~70°, i.e. warm.
            let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
            assert!((0.0..=160.0).contains(&hue), "expected a warm-ish hue, got {hue}°");
        }
    }

    #[test]
    fn a_light_orange_source_suggests_a_darker_cooler_background() {
        let pixels = solid_pixels(20, 20, 230, 150, 60, 255); // light, warm orange
        let gradient = suggest_gradient(&[PixelSample { bytes: &pixels, width: 20, height: 20 }], 0);

        for (_, color) in &gradient.stops {
            let (l, _, _) = rgb_to_oklab(*color);
            assert!(l < 0.65, "expected a darker background, got lightness {l}");
        }
    }

    #[test]
    fn different_seeds_produce_different_but_still_complementary_palettes() {
        let pixels = solid_pixels(20, 20, 20, 40, 180, 255);
        let a = suggest_gradient(&[PixelSample { bytes: &pixels, width: 20, height: 20 }], 0);
        let b = suggest_gradient(&[PixelSample { bytes: &pixels, width: 20, height: 20 }], 1);
        assert_ne!(a.stops, b.stops, "a different seed should suggest a different palette");
    }

    #[test]
    fn the_same_seed_reproduces_the_same_palette() {
        let pixels = solid_pixels(20, 20, 20, 40, 180, 255);
        let a = suggest_gradient(&[PixelSample { bytes: &pixels, width: 20, height: 20 }], 7);
        let b = suggest_gradient(&[PixelSample { bytes: &pixels, width: 20, height: 20 }], 7);
        assert_eq!(a.stops, b.stops);
    }

    #[test]
    fn oklab_round_trips_srgb_within_tolerance() {
        for color in [
            Rgba::new(1.0, 0.0, 0.0, 1.0),
            Rgba::new(0.0, 1.0, 0.0, 1.0),
            Rgba::new(0.0, 0.0, 1.0, 1.0),
            Rgba::new(0.5, 0.5, 0.5, 1.0),
            Rgba::new(0.2, 0.7, 0.9, 1.0),
        ] {
            let (l, a, b) = rgb_to_oklab(color);
            let back = oklab_to_rgb(l, a, b);
            assert!((back.r - color.r).abs() < 0.01, "r: {back:?} vs {color:?}");
            assert!((back.g - color.g).abs() < 0.01, "g: {back:?} vs {color:?}");
            assert!((back.b - color.b).abs() < 0.01, "b: {back:?} vs {color:?}");
        }
    }

    #[test]
    fn suggested_gradient_never_produces_nan_or_out_of_range_channels() {
        // Pure black and pure white are the edge cases most likely to
        // send lightness/chroma math to a degenerate hue (atan2(0,0)).
        for (r, g, b) in [(0, 0, 0), (255, 255, 255)] {
            let pixels = solid_pixels(10, 10, r, g, b, 255);
            let gradient = suggest_gradient(&[PixelSample { bytes: &pixels, width: 10, height: 10 }], 3);
            for (_, color) in &gradient.stops {
                for channel in [color.r, color.g, color.b] {
                    assert!(channel.is_finite() && (0.0..=1.0).contains(&channel), "bad channel: {channel}");
                }
            }
        }
    }

    /// Two very different solid-color images should extract into (at
    /// least) two distinct dominant colors, not collapse into one.
    #[test]
    fn extract_palette_finds_distinct_dominant_colors_across_images() {
        let red = solid_pixels(20, 20, 220, 30, 30, 255);
        let blue = solid_pixels(20, 20, 30, 30, 220, 255);
        let palette = extract_palette(
            &[PixelSample { bytes: &red, width: 20, height: 20 }, PixelSample { bytes: &blue, width: 20, height: 20 }],
            4,
        );
        assert!(palette.len() >= 2, "expected at least two dominant clusters, got {}", palette.len());
        let has_reddish = palette.iter().any(|c| c.r > c.b);
        let has_bluish = palette.iter().any(|c| c.b > c.r);
        assert!(has_reddish && has_bluish, "expected both a reddish and a bluish cluster: {palette:?}");
    }

    #[test]
    fn extract_palette_never_returns_more_than_requested() {
        let pixels = solid_pixels(20, 20, 100, 150, 200, 255);
        let palette = extract_palette(&[PixelSample { bytes: &pixels, width: 20, height: 20 }], 2);
        assert!(palette.len() <= 2);
    }

    #[test]
    fn extract_palette_of_no_images_is_empty_not_a_panic() {
        assert!(extract_palette(&[], 4).is_empty());
    }

    #[test]
    fn resolve_palette_is_deterministic_for_every_strategy() {
        let pixels = solid_pixels(20, 20, 40, 90, 180, 255);
        for strategy in [ColorStrategy::FromScreenshots, ColorStrategy::Grayscale, ColorStrategy::Random] {
            let images = [PixelSample { bytes: &pixels, width: 20, height: 20 }];
            let a = resolve_palette(&images, strategy, 0.5, 99);
            let b = resolve_palette(&images, strategy, 0.5, 99);
            assert_eq!(a, b, "{strategy:?} should be deterministic for the same inputs");
            assert!(!a.is_empty(), "{strategy:?} produced an empty palette");
            for color in &a {
                for channel in [color.r, color.g, color.b, color.a] {
                    assert!(channel.is_finite() && (0.0..=1.0).contains(&channel), "{strategy:?} produced a bad channel: {channel}");
                }
            }
        }
    }

    #[test]
    fn manual_strategy_resolves_to_an_empty_palette() {
        // Manual has nothing to derive -- the user's own colors are the
        // palette, set directly by the caller, never through this function.
        let pixels = solid_pixels(20, 20, 40, 90, 180, 255);
        let images = [PixelSample { bytes: &pixels, width: 20, height: 20 }];
        assert!(resolve_palette(&images, ColorStrategy::Manual, 0.5, 1).is_empty());
    }

    #[test]
    fn random_strategy_ignores_the_images_and_only_depends_on_seed() {
        let pixels = solid_pixels(20, 20, 10, 200, 10, 255);
        let images = [PixelSample { bytes: &pixels, width: 20, height: 20 }];
        let with_images = resolve_palette(&images, ColorStrategy::Random, 0.5, 7);
        let without_images = resolve_palette(&[], ColorStrategy::Random, 0.5, 7);
        assert_eq!(with_images, without_images);
    }

    #[test]
    fn different_seeds_give_different_random_palettes() {
        let a = resolve_palette(&[], ColorStrategy::Random, 0.5, 1);
        let b = resolve_palette(&[], ColorStrategy::Random, 0.5, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn grayscale_strategy_produces_neutral_colors() {
        let pixels = solid_pixels(20, 20, 220, 30, 30, 255); // a strongly saturated red source
        let palette = resolve_palette(&[PixelSample { bytes: &pixels, width: 20, height: 20 }], ColorStrategy::Grayscale, 0.5, 1);
        for color in &palette {
            let max_channel = color.r.max(color.g).max(color.b);
            let min_channel = color.r.min(color.g).min(color.b);
            assert!(max_channel - min_channel < 0.02, "expected a neutral color, got {color:?}");
        }
    }

    #[test]
    fn from_screenshots_strategy_leans_toward_complement_as_inverse_contrast_rises() {
        let pixels = solid_pixels(20, 20, 10, 20, 90, 255); // dark, cool blue
        let low = resolve_palette(&[PixelSample { bytes: &pixels, width: 20, height: 20 }], ColorStrategy::FromScreenshots, 0.0, 1);
        let high = resolve_palette(&[PixelSample { bytes: &pixels, width: 20, height: 20 }], ColorStrategy::FromScreenshots, 1.0, 1);
        assert_ne!(low, high, "inverse_contrast should change the resolved palette");
    }

    #[test]
    fn resolve_palette_with_no_images_falls_back_gracefully() {
        for strategy in [ColorStrategy::FromScreenshots, ColorStrategy::Grayscale] {
            let palette = resolve_palette(&[], strategy, 0.5, 1);
            assert!(!palette.is_empty());
        }
    }
}
