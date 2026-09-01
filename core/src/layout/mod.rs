//! Pure layout computation — no GTK, no image decoding. Takes elements with
//! their natural (decoded) size and produces placements; the renderer and
//! the canvas widget consume [`Placement`]s identically.

use uuid::Uuid;

use crate::model::{LayoutMode, ScreenshotElement};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub element_id: Uuid,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Total canvas size a set of placements needs, given the same margin used
/// to compute them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasExtent {
    pub width: f64,
    pub height: f64,
}

pub fn compute_layout(
    mode: LayoutMode,
    elements: &[ScreenshotElement],
    spacing_px: f64,
    margin_px: f64,
) -> Vec<Placement> {
    match mode {
        LayoutMode::Horizontal => compute_horizontal_layout(elements, spacing_px, margin_px),
        LayoutMode::Vertical | LayoutMode::Grid | LayoutMode::Free => {
            todo!("layout mode {mode:?} is not implemented yet")
        }
    }
}

/// Scales every element to a common height (the smallest natural height in
/// the set), lays them out left-to-right with `spacing_px` gaps starting at
/// `margin_px`, and centers them vertically within `margin_px` top/bottom —
/// since every element shares the same height after scaling, that reduces
/// to placing every element's top edge at `margin_px`.
pub fn compute_horizontal_layout(
    elements: &[ScreenshotElement],
    spacing_px: f64,
    margin_px: f64,
) -> Vec<Placement> {
    if elements.is_empty() {
        return Vec::new();
    }

    let target_height = elements
        .iter()
        .map(|el| el.natural_height)
        .fold(f64::INFINITY, f64::min);

    let mut x = margin_px;
    let mut placements = Vec::with_capacity(elements.len());
    for el in elements {
        let scale = if el.natural_height > 0.0 { target_height / el.natural_height } else { 1.0 };
        let width = el.natural_width * scale;
        placements.push(Placement { element_id: el.id, x, y: margin_px, width, height: target_height });
        x += width + spacing_px;
    }
    placements
}

/// Canvas extent implied by a set of placements plus the margin used to
/// produce them (placements alone don't carry the trailing margin).
pub fn extent_for(placements: &[Placement], margin_px: f64) -> CanvasExtent {
    let width = placements.iter().map(|p| p.x + p.width).fold(0.0_f64, f64::max) + margin_px;
    let height = placements.iter().map(|p| p.y + p.height).fold(0.0_f64, f64::max) + margin_px;
    CanvasExtent { width, height }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ImageSource;
    use std::path::PathBuf;

    fn fixture(natural_width: f64, natural_height: f64) -> ScreenshotElement {
        ScreenshotElement::new(ImageSource::Path(PathBuf::from("test.png")), natural_width, natural_height)
    }

    #[test]
    fn empty_input_produces_no_placements() {
        assert_eq!(compute_horizontal_layout(&[], 24.0, 48.0), Vec::new());
    }

    #[test]
    fn single_element_is_offset_by_margin_only() {
        let elements = [fixture(400.0, 800.0)];
        let placements = compute_horizontal_layout(&elements, 24.0, 48.0);
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].x, 48.0);
        assert_eq!(placements[0].y, 48.0);
        assert_eq!(placements[0].width, 400.0);
        assert_eq!(placements[0].height, 800.0);
    }

    #[test]
    fn equal_height_elements_are_spaced_evenly_with_no_rescale() {
        let elements = [fixture(400.0, 800.0), fixture(400.0, 800.0), fixture(400.0, 800.0)];
        let placements = compute_horizontal_layout(&elements, 20.0, 0.0);
        assert_eq!(placements[0].x, 0.0);
        assert_eq!(placements[1].x, 420.0);
        assert_eq!(placements[2].x, 840.0);
        for p in &placements {
            assert_eq!(p.height, 800.0);
            assert_eq!(p.width, 400.0);
            assert_eq!(p.y, 0.0);
        }
    }

    #[test]
    fn differing_heights_are_scaled_to_the_smallest() {
        // 400x800 (aspect 0.5) and 300x900 (aspect 1/3) -> common height 800.
        let elements = [fixture(400.0, 800.0), fixture(300.0, 900.0)];
        let placements = compute_horizontal_layout(&elements, 0.0, 0.0);
        assert_eq!(placements[0].height, 800.0);
        assert_eq!(placements[0].width, 400.0);
        assert_eq!(placements[1].height, 800.0);
        // 300 * (800/900) = 266.666...
        assert!((placements[1].width - 266.666_666_67).abs() < 1e-6);
        assert_eq!(placements[1].x, placements[0].width);
    }

    #[test]
    fn zero_spacing_and_margin_are_respected() {
        let elements = [fixture(100.0, 100.0), fixture(100.0, 100.0)];
        let placements = compute_horizontal_layout(&elements, 0.0, 0.0);
        assert_eq!(placements[0].x, 0.0);
        assert_eq!(placements[1].x, 100.0);
    }

    #[test]
    fn extent_accounts_for_trailing_margin() {
        let elements = [fixture(400.0, 800.0), fixture(400.0, 800.0)];
        let placements = compute_horizontal_layout(&elements, 20.0, 48.0);
        let extent = extent_for(&placements, 48.0);
        // margin + 400 + 20 + 400 + margin
        assert_eq!(extent.width, 48.0 + 400.0 + 20.0 + 400.0 + 48.0);
        assert_eq!(extent.height, 48.0 + 800.0 + 48.0);
    }

    #[test]
    #[should_panic]
    fn vertical_mode_is_not_yet_implemented() {
        let elements = [fixture(400.0, 800.0)];
        compute_layout(LayoutMode::Vertical, &elements, 0.0, 0.0);
    }
}
