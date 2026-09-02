//! Cache for rendered shadow bitmaps, keyed by everything that changes a
//! shadow's *pixels* at a given output resolution. Deliberately excludes
//! element position and shadow offset — `render::draw_shadow` renders the
//! shape unshifted and only translates it at paint time (translation
//! commutes with blur), so moving an element, or changing which direction
//! its shadow is cast, never needs to touch this cache. That's what makes
//! "move a screenshot with a cached shadow" cheap: generate once, cache,
//! move, reuse.
//!
//! Owned by whatever renders repeatedly against the same document (the
//! preview `Canvas` widget) and passed into `render::compose` by
//! reference; a one-shot caller (export) can just build a fresh, empty
//! `ShadowCache` and let every shadow miss once — there's nothing to gain
//! from caching across a single render.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::model::{CornerRadius, ShadowParams};
use crate::render::RenderError;

/// Bounds how large a single shadow bitmap is ever allowed to get,
/// regardless of the source element's size or the render scale — without
/// this, a large imported screenshot (or a large export target) combined
/// with a large blur radius could ask Cairo to allocate a gigabytes-sized
/// surface, which is slow at best and can abort the process at worst
/// (enabling shadows must never crash the app, including on "large
/// screenshots"). Blur is a smooth, low-frequency effect, so capping the
/// bitmap's own resolution and letting Cairo's paint transform upscale it
/// by a pixel or two is visually lossless in practice.
pub const MAX_SHADOW_SURFACE_DIM: i32 = 4096;

/// Every input that changes a shadow bitmap's pixels, quantized to whole
/// device pixels (colors/opacity to thousandths) so it can be hashed and
/// compared exactly rather than via approximate float equality. Notably
/// absent: `offset_x`/`offset_y` (paint-time translation only, see module
/// doc) and any element id — two elements with identical shape and shadow
/// settings share one cached bitmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ShadowCacheKey {
    width_px: i32,
    height_px: i32,
    corner_radius_px: (i32, i32, i32, i32),
    blur_px: i32,
    opacity_milli: i32,
    color_milli: (i32, i32, i32, i32),
}

impl ShadowCacheKey {
    /// `width`/`height`/`corner_radius` are in document pixels; `scale` is
    /// the *effective* render scale (already capped for
    /// `MAX_SHADOW_SURFACE_DIM`, see `render::shadow_render_scale`) that
    /// converts them to the device-pixel resolution the bitmap is actually
    /// rendered at — rendering at output resolution rather than always at
    /// full document resolution is what keeps a zoomed-out preview's
    /// shadows cheap, and it falls naturally out of this cache key since a
    /// different scale is a different key.
    fn new(width: f64, height: f64, corner_radius: &CornerRadius, shadow: &ShadowParams, scale: f64) -> Self {
        let px = |v: f64| (v * scale).round() as i32;
        let milli = |v: f64| (v * 1000.0).round() as i32;
        ShadowCacheKey {
            width_px: px(width),
            height_px: px(height),
            corner_radius_px: (
                px(corner_radius.top_left),
                px(corner_radius.top_right),
                px(corner_radius.bottom_right),
                px(corner_radius.bottom_left),
            ),
            blur_px: px(shadow.blur),
            opacity_milli: milli(shadow.opacity),
            color_milli: (milli(shadow.color.r), milli(shadow.color.g), milli(shadow.color.b), milli(shadow.color.a)),
        }
    }
}

struct CacheEntry {
    surface: Rc<cairo::ImageSurface>,
    last_used: u64,
}

/// Hard cap on distinct bitmaps kept at once. Enforced by evicting the
/// least-recently-used entries every frame once the cache is over this —
/// not by an "untouched for N frames" staleness threshold, which a
/// pathological case (e.g. a smooth resize drag, minting a fresh key on
/// nearly every frame) could keep every entry just barely under, letting
/// the cache grow without bound even though nothing is really being
/// reused. An element that's deleted, or resized away from a cached size,
/// simply stops being touched and becomes the next thing evicted once the
/// cache fills up with newer entries — no separate "this no longer exists"
/// signal needed.
const MAX_ENTRIES: usize = 64;

/// A shadow bitmap cache, plus a monotonic "frame" counter used for LRU
/// eviction. `begin_frame` should be called once per `render::compose`
/// pass.
pub struct ShadowCache {
    entries: RefCell<HashMap<ShadowCacheKey, CacheEntry>>,
    frame: Cell<u64>,
}

impl ShadowCache {
    pub fn new() -> Self {
        Self { entries: RefCell::new(HashMap::new()), frame: Cell::new(0) }
    }

    pub fn begin_frame(&self) {
        self.frame.set(self.frame.get() + 1);
        self.evict_to_cap();
    }

    fn evict_to_cap(&self) {
        let mut entries = self.entries.borrow_mut();
        if entries.len() <= MAX_ENTRIES {
            return;
        }
        let mut by_recency: Vec<(ShadowCacheKey, u64)> = entries.iter().map(|(key, entry)| (*key, entry.last_used)).collect();
        by_recency.sort_unstable_by_key(|(_, last_used)| *last_used);
        for (key, _) in by_recency.into_iter().take(entries.len() - MAX_ENTRIES) {
            entries.remove(&key);
        }
    }

    /// Returns the cached bitmap for this shadow shape at `scale`,
    /// rendering (and caching) it first on a miss. `render` is only
    /// invoked when nothing matching is already cached.
    pub fn get_or_render(
        &self,
        width: f64,
        height: f64,
        corner_radius: &CornerRadius,
        shadow: &ShadowParams,
        scale: f64,
        render: impl FnOnce() -> Result<cairo::ImageSurface, RenderError>,
    ) -> Result<Rc<cairo::ImageSurface>, RenderError> {
        let key = ShadowCacheKey::new(width, height, corner_radius, shadow, scale);
        let now = self.frame.get();

        if let Some(entry) = self.entries.borrow_mut().get_mut(&key) {
            entry.last_used = now;
            return Ok(entry.surface.clone());
        }

        let surface = Rc::new(render()?);
        self.entries.borrow_mut().insert(key, CacheEntry { surface: surface.clone(), last_used: now });
        Ok(surface)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.borrow().len()
    }
}

impl Default for ShadowCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rgba;

    fn shadow() -> ShadowParams {
        ShadowParams { enabled: true, offset_x: 0.0, offset_y: 6.0, blur: 16.0, opacity: 0.25, color: Rgba::BLACK }
    }

    fn render_stub() -> Result<cairo::ImageSurface, RenderError> {
        Ok(cairo::ImageSurface::create(cairo::Format::ARgb32, 4, 4).unwrap())
    }

    #[test]
    fn identical_shape_hits_the_cache_without_rendering_again() {
        let cache = ShadowCache::new();
        cache.begin_frame();
        let radius = CornerRadius::none();
        let s = shadow();

        let calls = Cell::new(0);
        let render = || {
            calls.set(calls.get() + 1);
            render_stub()
        };
        let a = cache.get_or_render(100.0, 100.0, &radius, &s, 1.0, render).unwrap();
        let b = cache.get_or_render(100.0, 100.0, &radius, &s, 1.0, render).unwrap();

        assert_eq!(calls.get(), 1, "second lookup with identical inputs should reuse the cached bitmap");
        assert!(Rc::ptr_eq(&a, &b));
    }

    #[test]
    fn moving_the_element_does_not_change_the_cache_key() {
        // draw_shadow always renders the *unshifted* shape and applies
        // offset only at paint time, so position changes never touch this
        // cache — width/height/shape/blur/color are identical here, only
        // the (irrelevant to this key) notion of "where it's painted"
        // would differ in a real call site.
        let cache = ShadowCache::new();
        cache.begin_frame();
        assert_eq!(cache.len(), 0);
        cache.get_or_render(100.0, 100.0, &CornerRadius::none(), &shadow(), 1.0, render_stub).unwrap();
        assert_eq!(cache.len(), 1);
        cache.get_or_render(100.0, 100.0, &CornerRadius::none(), &shadow(), 1.0, render_stub).unwrap();
        assert_eq!(cache.len(), 1, "identical shape at a new position should still be a single cache entry");
    }

    #[test]
    fn different_blur_is_a_different_cache_entry() {
        let cache = ShadowCache::new();
        cache.begin_frame();
        let mut a = shadow();
        let mut b = shadow();
        a.blur = 8.0;
        b.blur = 20.0;
        cache.get_or_render(100.0, 100.0, &CornerRadius::none(), &a, 1.0, render_stub).unwrap();
        cache.get_or_render(100.0, 100.0, &CornerRadius::none(), &b, 1.0, render_stub).unwrap();
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn different_scale_is_a_different_cache_entry() {
        // A preview zoom change (or export at a different target size)
        // must not reuse a bitmap rendered for a different resolution.
        let cache = ShadowCache::new();
        cache.begin_frame();
        cache.get_or_render(100.0, 100.0, &CornerRadius::none(), &shadow(), 1.0, render_stub).unwrap();
        cache.get_or_render(100.0, 100.0, &CornerRadius::none(), &shadow(), 0.5, render_stub).unwrap();
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn growing_past_the_cap_evicts_the_least_recently_used_entries() {
        let cache = ShadowCache::new();

        // Fill to exactly the cap, one distinct key (by blur) per frame so
        // each has its own `last_used`.
        for i in 0..MAX_ENTRIES {
            cache.begin_frame();
            let mut s = shadow();
            s.blur = i as f64 + 1.0;
            cache.get_or_render(100.0, 100.0, &CornerRadius::none(), &s, 1.0, render_stub).unwrap();
        }
        assert_eq!(cache.len(), MAX_ENTRIES);

        // One more, distinct key: now over the cap, so the very next
        // `begin_frame` must evict something to get back to the cap --
        // and it must be blur=1.0 (key 0), the least recently touched.
        let mut overflow = shadow();
        overflow.blur = MAX_ENTRIES as f64 + 1.0;
        cache.get_or_render(100.0, 100.0, &CornerRadius::none(), &overflow, 1.0, render_stub).unwrap();
        assert_eq!(cache.len(), MAX_ENTRIES + 1, "insertion itself is never refused, only the next sweep trims it");

        cache.begin_frame();
        assert_eq!(cache.len(), MAX_ENTRIES, "a sweep once over the cap brings it back down to the cap");

        let mut oldest = shadow();
        oldest.blur = 1.0;
        let calls = Cell::new(0);
        cache
            .get_or_render(100.0, 100.0, &CornerRadius::none(), &oldest, 1.0, || {
                calls.set(calls.get() + 1);
                render_stub()
            })
            .unwrap();
        assert_eq!(calls.get(), 1, "the least-recently-used entry should have been the one evicted, so this re-renders");

        let mut newest = shadow();
        newest.blur = MAX_ENTRIES as f64 + 1.0;
        let calls = Cell::new(0);
        cache
            .get_or_render(100.0, 100.0, &CornerRadius::none(), &newest, 1.0, || {
                calls.set(calls.get() + 1);
                render_stub()
            })
            .unwrap();
        assert_eq!(calls.get(), 0, "the entry just inserted should still be cached, not evicted");
    }
}
