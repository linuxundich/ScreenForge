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
        LayoutMode::Vertical => compute_vertical_layout(elements, spacing_px, margin_px),
        LayoutMode::Grid => compute_grid_layout(elements, spacing_px, margin_px),
        LayoutMode::Free => todo!("free positioning (manual per-element placement) is not implemented yet"),
    }
}

/// Scales every element to a common width (the smallest natural width in
/// the set), stacks them top-to-bottom with `spacing_px` gaps starting at
/// `margin_px` — the vertical mirror of [`compute_horizontal_layout`].
pub fn compute_vertical_layout(elements: &[ScreenshotElement], spacing_px: f64, margin_px: f64) -> Vec<Placement> {
    if elements.is_empty() {
        return Vec::new();
    }

    let target_width = elements.iter().map(|el| el.natural_width).fold(f64::INFINITY, f64::min);

    let mut y = margin_px;
    let mut placements = Vec::with_capacity(elements.len());
    for el in elements {
        let scale = if el.natural_width > 0.0 { target_width / el.natural_width } else { 1.0 };
        let height = el.natural_height * scale;
        placements.push(Placement { element_id: el.id, x: margin_px, y, width: target_width, height });
        y += height + spacing_px;
    }
    placements
}

/// Arranges elements into a roughly square grid (`ceil(sqrt(n))` columns),
/// scaling every element to one common height — the same scaling rule as
/// [`compute_horizontal_layout`], just wrapped into rows — so columns don't
/// necessarily align edge-to-edge when aspect ratios differ, but every row
/// has uniform height.
pub fn compute_grid_layout(elements: &[ScreenshotElement], spacing_px: f64, margin_px: f64) -> Vec<Placement> {
    if elements.is_empty() {
        return Vec::new();
    }

    let columns = (elements.len() as f64).sqrt().ceil() as usize;
    let target_height = elements.iter().map(|el| el.natural_height).fold(f64::INFINITY, f64::min);

    let mut x = margin_px;
    let mut y = margin_px;
    let mut placements = Vec::with_capacity(elements.len());
    for (i, el) in elements.iter().enumerate() {
        if i > 0 && i % columns == 0 {
            x = margin_px;
            y += target_height + spacing_px;
        }
        let scale = if el.natural_height > 0.0 { target_height / el.natural_height } else { 1.0 };
        let width = el.natural_width * scale;
        placements.push(Placement { element_id: el.id, x, y, width, height: target_height });
        x += width + spacing_px;
    }
    placements
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
    fn free_mode_is_not_yet_implemented() {
        let elements = [fixture(400.0, 800.0)];
        compute_layout(LayoutMode::Free, &elements, 0.0, 0.0);
    }

    #[test]
    fn vertical_layout_stacks_top_to_bottom_scaled_to_common_width() {
        // 400x800 (aspect 2.0) and 200x300 (aspect 1.5) -> common width 200.
        let elements = [fixture(400.0, 800.0), fixture(200.0, 300.0)];
        let placements = compute_vertical_layout(&elements, 10.0, 5.0);

        assert_eq!(placements[0].x, 5.0);
        assert_eq!(placements[0].y, 5.0);
        assert_eq!(placements[0].width, 200.0);
        // 800 * (200/400) = 400
        assert_eq!(placements[0].height, 400.0);

        assert_eq!(placements[1].x, 5.0);
        // previous y (5) + previous height (400) + spacing (10)
        assert_eq!(placements[1].y, 415.0);
        assert_eq!(placements[1].width, 200.0);
        assert_eq!(placements[1].height, 300.0);
    }

    #[test]
    fn vertical_layout_of_empty_input_is_empty() {
        assert_eq!(compute_vertical_layout(&[], 10.0, 5.0), Vec::new());
    }

    #[test]
    fn grid_layout_wraps_after_ceil_sqrt_n_columns() {
        // 4 elements -> ceil(sqrt(4)) = 2 columns, 2 rows.
        let elements =
            [fixture(100.0, 100.0), fixture(100.0, 100.0), fixture(100.0, 100.0), fixture(100.0, 100.0)];
        let placements = compute_grid_layout(&elements, 10.0, 0.0);

        assert_eq!(placements[0].x, 0.0);
        assert_eq!(placements[0].y, 0.0);
        assert_eq!(placements[1].x, 110.0);
        assert_eq!(placements[1].y, 0.0);
        // wraps to a new row after 2 elements
        assert_eq!(placements[2].x, 0.0);
        assert_eq!(placements[2].y, 110.0);
        assert_eq!(placements[3].x, 110.0);
        assert_eq!(placements[3].y, 110.0);
    }

    #[test]
    fn grid_layout_scales_each_row_to_a_common_height() {
        let elements = [fixture(100.0, 100.0), fixture(50.0, 50.0), fixture(100.0, 100.0)];
        // ceil(sqrt(3)) = 2 columns.
        let placements = compute_grid_layout(&elements, 0.0, 0.0);
        for p in &placements {
            assert_eq!(p.height, 50.0);
        }
    }

    #[test]
    fn grid_layout_of_empty_input_is_empty() {
        assert_eq!(compute_grid_layout(&[], 10.0, 5.0), Vec::new());
    }
}
