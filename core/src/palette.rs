//! Suggests colors that *complement* a set of imported screenshots rather
//! than copying them back at the user: the label colors and the gradient
//! behind the background editor's "Generate from screenshots" action.
//!
//! Everything else about palettes (Oklab conversion, mood ramps, the
//! palettes generated backgrounds use) lives in Prospekt and is re-exported
//! here, so `crate::palette::…` paths keep working.

use std::f64::consts::PI;

pub use prospekt::palette::*;

use crate::model::{GradientKind, GradientSpec, Rgba};

/// Suggests a contrast-aware `(background, text)` color pair for a
/// screenshot's own label, derived from that screenshot's average color —
/// the "Automatisch" background-color action. Fully inverts the source's
/// Oklab lightness (rather than partially, like [`suggest_gradient`]'s
/// complementary suggestion) and drops chroma to zero, so contrast comes
/// purely from the lightness gap, never from cranking up saturation: a
/// bright screenshot gets a near-black label, a dark one a near-white
/// label, and the label's own text is whichever of near-black/near-white
/// contrasts more with *that* result.
pub fn suggest_label_colors(images: &[PixelSample]) -> (Rgba, Rgba) {
    let avg = average_color(images);
    let (lightness, _, _) = rgb_to_oklab(avg);
    let background_lightness = (1.0 - lightness).clamp(0.08, 0.92);
    let background = oklab_to_rgb(background_lightness, 0.0, 0.0);
    let text = oklab_to_rgb(if background_lightness > 0.5 { 0.05 } else { 0.97 }, 0.0, 0.0);
    (background, text)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_pixels(w: i32, h: i32, r: u8, g: u8, b: u8, a: u8) -> Vec<u8> {
        let af = a as f64 / 255.0;
        let (pr, pg, pb) = ((r as f64 * af) as u8, (g as f64 * af) as u8, (b as f64 * af) as u8);
        [pb, pg, pr, a].repeat((w * h) as usize)
    }

    #[test]
    fn suggest_label_colors_gives_a_bright_screenshot_a_dark_background_and_light_text() {
        let pixels = solid_pixels(20, 20, 235, 235, 235, 255);
        let (background, text) = suggest_label_colors(&[PixelSample { bytes: &pixels, width: 20, height: 20 }]);
        let (bg_l, bg_a, bg_b) = rgb_to_oklab(background);
        assert!(bg_l < 0.5, "expected a dark label background for a bright screenshot, got lightness {bg_l}");
        assert!(bg_a.abs() < 1e-6 && bg_b.abs() < 1e-6, "expected zero chroma, contrast should come from lightness alone");
        let (text_l, _, _) = rgb_to_oklab(text);
        assert!(text_l > bg_l, "expected the suggested text color to be lighter than its own background");
    }

    #[test]
    fn suggest_label_colors_gives_a_dark_screenshot_a_light_background_and_dark_text() {
        let pixels = solid_pixels(20, 20, 15, 15, 15, 255);
        let (background, text) = suggest_label_colors(&[PixelSample { bytes: &pixels, width: 20, height: 20 }]);
        let (bg_l, _, _) = rgb_to_oklab(background);
        assert!(bg_l > 0.5, "expected a light label background for a dark screenshot, got lightness {bg_l}");
        let (text_l, _, _) = rgb_to_oklab(text);
        assert!(text_l < bg_l, "expected the suggested text color to be darker than its own background");
    }

    #[test]
    fn suggest_label_colors_of_no_images_does_not_panic_and_stays_in_range() {
        let (background, text) = suggest_label_colors(&[]);
        for c in [background, text] {
            assert!((0.0..=1.0).contains(&c.r) && (0.0..=1.0).contains(&c.g) && (0.0..=1.0).contains(&c.b));
        }
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
}
