//! Cache for the rendered generated-background bitmap, keyed by everything
//! that changes its pixels: the [`GeneratedBackground`]'s own fields, the
//! canvas size it's rendered at, and the screenshot regions it's drawn
//! around (for the corner-avoidance effect — see
//! `crate::generator::draw_wave_layers`). Deliberately *not* keyed on
//! anything about labels/callouts, since dragging one never changes any of
//! the above — that's what makes "drag a callout while a generated
//! background is active" cheap: the expensive wave-layer rasterization
//! (now at 2x supersampling, see `render::render_generated_background_2x`)
//! is skipped entirely on a cache hit, leaving only Pango layout and Cairo
//! compositing per frame.
//!
//! Unlike [`crate::shadow_cache::ShadowCache`], there is never more than one
//! generated background per document, so a single `Option` slot is enough —
//! no LRU/eviction needed.

use std::cell::RefCell;
use std::rc::Rc;

use crate::generator::ScreenshotRegion;
use crate::model::GeneratedBackground;
use crate::render::RenderError;

/// Every input that changes the generated background's pixels, quantized
/// (colors/numeric knobs to thousandths, sizes to whole device pixels) so
/// it can be hashed and compared exactly rather than via approximate float
/// equality. `color_strategy` itself is deliberately absent — it only
/// influences how `palette` was resolved upstream, never `generator::render`
/// itself, so `palette` alone already captures its effect on the pixels.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BackgroundCacheKey {
    seed: u64,
    palette_milli: Vec<(i32, i32, i32, i32)>,
    adapt_to_screenshots: bool,
    inverse_contrast_milli: i32,
    density_milli: i32,
    flow_milli: i32,
    variation_milli: i32,
    contrast_milli: i32,
    softness_milli: i32,
    corner_bias_milli: i32,
    offset_x_milli: i32,
    offset_y_milli: i32,
    scale_milli: i32,
    canvas_w_px: i32,
    canvas_h_px: i32,
    regions_px: Vec<(i32, i32, i32, i32)>,
}

impl BackgroundCacheKey {
    fn new(bg: &GeneratedBackground, canvas_w: f64, canvas_h: f64, regions: &[ScreenshotRegion]) -> Self {
        let milli = |v: f64| (v * 1000.0).round() as i32;
        let px = |v: f64| v.round() as i32;
        BackgroundCacheKey {
            seed: bg.seed,
            palette_milli: bg.palette.iter().map(|c| (milli(c.r), milli(c.g), milli(c.b), milli(c.a))).collect(),
            adapt_to_screenshots: bg.adapt_to_screenshots,
            inverse_contrast_milli: milli(bg.inverse_contrast),
            density_milli: milli(bg.density),
            flow_milli: milli(bg.flow),
            variation_milli: milli(bg.variation),
            contrast_milli: milli(bg.contrast),
            softness_milli: milli(bg.softness),
            corner_bias_milli: milli(bg.corner_bias),
            offset_x_milli: milli(bg.offset_x),
            offset_y_milli: milli(bg.offset_y),
            scale_milli: milli(bg.scale),
            canvas_w_px: px(canvas_w),
            canvas_h_px: px(canvas_h),
            regions_px: regions.iter().map(|r| (px(r.x), px(r.y), px(r.width), px(r.height))).collect(),
        }
    }
}

/// A cache holding at most one rendered background bitmap at a time.
pub struct BackgroundCache {
    entry: RefCell<Option<(BackgroundCacheKey, Rc<cairo::ImageSurface>)>>,
}

impl BackgroundCache {
    pub fn new() -> Self {
        Self { entry: RefCell::new(None) }
    }

    /// Returns the cached bitmap for this background at this canvas size
    /// and screenshot layout, rendering (and caching, replacing whatever
    /// was cached before) it first on a miss. `render` is only invoked
    /// when nothing matching is already cached.
    pub fn get_or_render(
        &self,
        bg: &GeneratedBackground,
        canvas_w: f64,
        canvas_h: f64,
        regions: &[ScreenshotRegion],
        render: impl FnOnce() -> Result<cairo::ImageSurface, RenderError>,
    ) -> Result<Rc<cairo::ImageSurface>, RenderError> {
        let key = BackgroundCacheKey::new(bg, canvas_w, canvas_h, regions);

        if let Some((cached_key, surface)) = self.entry.borrow().as_ref() {
            if *cached_key == key {
                return Ok(surface.clone());
            }
        }

        let surface = Rc::new(render()?);
        *self.entry.borrow_mut() = Some((key, surface.clone()));
        Ok(surface)
    }

    #[cfg(test)]
    pub(crate) fn is_cached(&self) -> bool {
        self.entry.borrow().is_some()
    }
}

impl Default for BackgroundCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rgba;
    use std::cell::Cell;

    fn bg() -> GeneratedBackground {
        GeneratedBackground::new(42)
    }

    fn render_stub() -> Result<cairo::ImageSurface, RenderError> {
        Ok(cairo::ImageSurface::create(cairo::Format::ARgb32, 4, 4).unwrap())
    }

    #[test]
    fn identical_inputs_hit_the_cache_without_rendering_again() {
        let cache = BackgroundCache::new();
        let calls = Cell::new(0);
        let render = || {
            calls.set(calls.get() + 1);
            render_stub()
        };
        let a = cache.get_or_render(&bg(), 800.0, 600.0, &[], render).unwrap();
        let b = cache.get_or_render(&bg(), 800.0, 600.0, &[], render).unwrap();

        assert_eq!(calls.get(), 1, "second lookup with identical inputs should reuse the cached bitmap");
        assert!(Rc::ptr_eq(&a, &b));
    }

    #[test]
    fn a_moved_callout_or_label_never_changes_the_key() {
        // Nothing about a callout/label lives in `GeneratedBackground` or in
        // the screenshot regions, so an unrelated drag must always hit —
        // this is the whole point of the cache (fixes sluggish callout
        // dragging over a generated background).
        let cache = BackgroundCache::new();
        let regions = [ScreenshotRegion { x: 10.0, y: 10.0, width: 200.0, height: 100.0 }];
        cache.get_or_render(&bg(), 800.0, 600.0, &regions, render_stub).unwrap();
        assert!(cache.is_cached());
        let calls = Cell::new(0);
        cache
            .get_or_render(&bg(), 800.0, 600.0, &regions, || {
                calls.set(calls.get() + 1);
                render_stub()
            })
            .unwrap();
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn a_different_seed_is_a_cache_miss() {
        let cache = BackgroundCache::new();
        cache.get_or_render(&bg(), 800.0, 600.0, &[], render_stub).unwrap();
        let mut other = bg();
        other.seed = 43;
        let calls = Cell::new(0);
        cache
            .get_or_render(&other, 800.0, 600.0, &[], || {
                calls.set(calls.get() + 1);
                render_stub()
            })
            .unwrap();
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_moved_screenshot_region_is_a_cache_miss() {
        // Correctness matters more than the hit rate here: the wave layers'
        // corner-avoidance depends on where the screenshots actually are,
        // so a real screenshot move must invalidate, unlike a label/callout
        // drag.
        let cache = BackgroundCache::new();
        let region_a = [ScreenshotRegion { x: 0.0, y: 0.0, width: 100.0, height: 100.0 }];
        let region_b = [ScreenshotRegion { x: 50.0, y: 0.0, width: 100.0, height: 100.0 }];
        cache.get_or_render(&bg(), 800.0, 600.0, &region_a, render_stub).unwrap();
        let calls = Cell::new(0);
        cache
            .get_or_render(&bg(), 800.0, 600.0, &region_b, || {
                calls.set(calls.get() + 1);
                render_stub()
            })
            .unwrap();
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_different_canvas_size_is_a_cache_miss() {
        let cache = BackgroundCache::new();
        cache.get_or_render(&bg(), 800.0, 600.0, &[], render_stub).unwrap();
        let calls = Cell::new(0);
        cache
            .get_or_render(&bg(), 900.0, 600.0, &[], || {
                calls.set(calls.get() + 1);
                render_stub()
            })
            .unwrap();
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_different_palette_is_a_cache_miss() {
        let cache = BackgroundCache::new();
        let mut other = bg();
        other.palette = vec![Rgba::new(1.0, 0.0, 0.0, 1.0)];
        cache.get_or_render(&bg(), 800.0, 600.0, &[], render_stub).unwrap();
        let calls = Cell::new(0);
        cache
            .get_or_render(&other, 800.0, 600.0, &[], || {
                calls.set(calls.get() + 1);
                render_stub()
            })
            .unwrap();
        assert_eq!(calls.get(), 1);
    }
}
