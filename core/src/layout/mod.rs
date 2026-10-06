//! Pure layout computation — no GTK, no image decoding. Takes elements with
//! their natural (decoded) size and produces placements; the renderer and
//! the canvas widget consume [`Placement`]s identically.

use uuid::Uuid;

use crate::model::{Document, LayoutMode, ScreenshotElement, Transform};

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
    margin_x: f64,
    margin_y: f64,
) -> Vec<Placement> {
    match mode {
        LayoutMode::Horizontal => compute_horizontal_layout(elements, spacing_px, margin_x, margin_y),
        LayoutMode::Vertical => compute_vertical_layout(elements, spacing_px, margin_x, margin_y),
        LayoutMode::Grid => compute_grid_layout(elements, spacing_px, margin_x, margin_y),
        LayoutMode::Free => compute_free_layout(elements),
    }
}

/// Places every element at its own stored position/size (`spacing_px` and
/// the margin don't apply — there's nothing automatic to space out).
/// Elements newly switched into Free mode get their transform populated
/// from their last computed placement first (see
/// `command::EnterFreeLayout`), so this only ever sees meaningful
/// coordinates, never the `Transform::default()` zeroes.
pub fn compute_free_layout(elements: &[ScreenshotElement]) -> Vec<Placement> {
    elements
        .iter()
        .map(|el| Placement {
            element_id: el.id,
            x: el.transform.x,
            y: el.transform.y,
            width: el.transform.width,
            height: el.transform.height,
        })
        .collect()
}

/// Scales every element to a common width (the smallest natural width in
/// the set), stacks them top-to-bottom with `spacing_px` gaps starting at
/// `margin_y`, each left-aligned at `margin_x` — the vertical mirror of
/// [`compute_horizontal_layout`].
pub fn compute_vertical_layout(elements: &[ScreenshotElement], spacing_px: f64, margin_x: f64, margin_y: f64) -> Vec<Placement> {
    if elements.is_empty() {
        return Vec::new();
    }

    let target_width = elements.iter().map(|el| el.natural_width).fold(f64::INFINITY, f64::min);

    let mut y = margin_y;
    let mut placements = Vec::with_capacity(elements.len());
    for el in elements {
        let scale = if el.natural_width > 0.0 { target_width / el.natural_width } else { 1.0 };
        let height = el.natural_height * scale;
        placements.push(Placement { element_id: el.id, x: margin_x, y, width: target_width, height });
        y += height + spacing_px;
    }
    placements
}

/// Arranges elements into a roughly square grid (`ceil(sqrt(n))` columns),
/// scaling every element to one common height — the same scaling rule as
/// [`compute_horizontal_layout`], just wrapped into rows — so columns don't
/// necessarily align edge-to-edge when aspect ratios differ, but every row
/// has uniform height.
pub fn compute_grid_layout(elements: &[ScreenshotElement], spacing_px: f64, margin_x: f64, margin_y: f64) -> Vec<Placement> {
    if elements.is_empty() {
        return Vec::new();
    }

    let columns = (elements.len() as f64).sqrt().ceil() as usize;
    let target_height = elements.iter().map(|el| el.natural_height).fold(f64::INFINITY, f64::min);

    let mut x = margin_x;
    let mut y = margin_y;
    let mut placements = Vec::with_capacity(elements.len());
    for (i, el) in elements.iter().enumerate() {
        if i > 0 && i % columns == 0 {
            x = margin_x;
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
/// `margin_x`, and centers them vertically within `margin_y` top/bottom —
/// since every element shares the same height after scaling, that reduces
/// to placing every element's top edge at `margin_y`.
pub fn compute_horizontal_layout(
    elements: &[ScreenshotElement],
    spacing_px: f64,
    margin_x: f64,
    margin_y: f64,
) -> Vec<Placement> {
    if elements.is_empty() {
        return Vec::new();
    }

    let target_height = elements
        .iter()
        .map(|el| el.natural_height)
        .fold(f64::INFINITY, f64::min);

    let mut x = margin_x;
    let mut placements = Vec::with_capacity(elements.len());
    for el in elements {
        let scale = if el.natural_height > 0.0 { target_height / el.natural_height } else { 1.0 };
        let width = el.natural_width * scale;
        placements.push(Placement { element_id: el.id, x, y: margin_y, width, height: target_height });
        x += width + spacing_px;
    }
    placements
}

/// Canvas extent implied by a set of placements plus the margin used to
/// produce them (placements alone don't carry the trailing margin).
pub fn extent_for(placements: &[Placement], margin_x: f64, margin_y: f64) -> CanvasExtent {
    let width = placements.iter().map(|p| p.x + p.width).fold(0.0_f64, f64::max) + margin_x;
    let height = placements.iter().map(|p| p.y + p.height).fold(0.0_f64, f64::max) + margin_y;
    CanvasExtent { width, height }
}

/// Resizes `doc.canvas` to exactly fit the current layout's content —
/// every visible element plus spacing/margin, plus any label/callout box
/// that spills past an element's own bounds (e.g. via a negative
/// `Semantic.padding`) — so the canvas always follows the content instead
/// of the other way around (a fixed canvas size used to let tall/portrait
/// screenshots get cropped off, and later, negative-margin labels get
/// clipped). A no-op while there are no visible elements, so a brand-new
/// or emptied project keeps a sensible starting size instead of collapsing
/// to just the margin. Called after every undo-tracked mutation (see
/// `command::UndoStack`) and on project load — never something the UI
/// calls directly, since there's no user-facing control for it anymore.
///
/// When content spills to the left of or above the nominal `(0, 0)`
/// origin, the canvas grows in that direction too, and `doc.canvas.
/// content_offset_x/y` records how far everything must be shifted right/
/// down at render time (see `render::compose`) so the existing content
/// keeps its position instead of sliding along with the new, larger
/// canvas.
pub fn fit_canvas_to_content(doc: &mut Document) {
    let visible: Vec<ScreenshotElement> = doc.elements.iter().filter(|e| e.visible).cloned().collect();
    if visible.is_empty() {
        return;
    }
    let placements = compute_layout(doc.layout.mode, &visible, doc.layout.spacing_px, doc.layout.margin_x, doc.layout.margin_y);

    let mut min_x = 0.0_f64;
    let mut min_y = 0.0_f64;
    let mut max_x = 0.0_f64;
    let mut max_y = 0.0_f64;

    for (el, placement) in visible.iter().zip(placements.iter()) {
        min_x = min_x.min(placement.x);
        min_y = min_y.min(placement.y);
        max_x = max_x.max(placement.x + placement.width);
        max_y = max_y.max(placement.y + placement.height);

        if el.label.enabled && !el.label.content.is_empty() {
            let resolved_label = el.label.resolve(&doc.label_defaults);
            if let Ok((bx, by, bw, bh)) = crate::render::measure_text_box(&resolved_label, placement.width, placement.height) {
                min_x = min_x.min(placement.x + bx);
                min_y = min_y.min(placement.y + by);
                max_x = max_x.max(placement.x + bx + bw);
                max_y = max_y.max(placement.y + by + bh);
            }
        }

        for callout in &el.callouts {
            if callout.enabled && !callout.text.content.is_empty() {
                if let Ok((bx, by, bw, bh)) =
                    crate::render::measure_text_box(&callout.text, placement.width, placement.height)
                {
                    min_x = min_x.min(placement.x + bx);
                    min_y = min_y.min(placement.y + by);
                    max_x = max_x.max(placement.x + bx + bw);
                    max_y = max_y.max(placement.y + by + bh);
                }
            }
        }
    }

    let margin_x = doc.layout.margin_x;
    let margin_y = doc.layout.margin_y;
    // Content that never goes negative reproduces the old behaviour
    // exactly: canvas left/top edge stays at the nominal origin, and the
    // layout's own leading margin (already baked into `placement.x/y`)
    // is the only gap on that side. Content that does spill past the
    // origin gets an extra margin-sized gap beyond its own extent too, so
    // it doesn't touch the canvas edge.
    let final_min_x = if min_x < 0.0 { min_x - margin_x } else { 0.0 };
    let final_min_y = if min_y < 0.0 { min_y - margin_y } else { 0.0 };
    let final_max_x = max_x + margin_x;
    let final_max_y = max_y + margin_y;

    doc.canvas.content_offset_x = -final_min_x;
    doc.canvas.content_offset_y = -final_min_y;
    doc.canvas.export_width = (final_max_x - final_min_x).round().max(1.0) as u32;
    doc.canvas.export_height = (final_max_y - final_min_y).round().max(1.0) as u32;
    apply_aspect(doc);
}

/// With a fixed `doc.canvas.aspect`, grows the content-fitted canvas in
/// whichever direction is too short and shifts the content by half the
/// growth, so it stays centered. Never shrinks, so nothing gets cropped.
fn apply_aspect(doc: &mut Document) {
    let Some((aw, ah)) = doc.canvas.aspect else { return };
    if aw == 0 || ah == 0 {
        return;
    }
    let ratio = aw as f64 / ah as f64;
    let (w, h) = (doc.canvas.export_width as f64, doc.canvas.export_height as f64);
    if w / h < ratio {
        let new_w = (h * ratio).round();
        doc.canvas.content_offset_x += (new_w - w) / 2.0;
        doc.canvas.export_width = new_w as u32;
    } else {
        let new_h = (w / ratio).round();
        doc.canvas.content_offset_y += (new_h - h) / 2.0;
        doc.canvas.export_height = new_h as u32;
    }
}

/// For the free layout: the visible screenshots' transforms rearranged
/// into one row in their current left-to-right order, `spacing` apart,
/// with their vertical centers on one shared line (the mean of their
/// current centers). Returns only the transforms that change.
pub fn balanced_transforms(elements: &[ScreenshotElement], spacing: f64) -> Vec<(Uuid, Transform, Transform)> {
    let mut visible: Vec<&ScreenshotElement> = elements.iter().filter(|e| e.visible).collect();
    if visible.len() < 2 {
        return Vec::new();
    }
    visible.sort_by(|a, b| a.transform.x.total_cmp(&b.transform.x));
    let center_y = visible.iter().map(|e| e.transform.y + e.transform.height / 2.0).sum::<f64>() / visible.len() as f64;
    let mut cursor = visible[0].transform.x;
    let mut changes = Vec::new();
    for element in visible {
        let old = element.transform;
        let new = Transform { x: cursor, y: center_y - old.height / 2.0, ..old };
        cursor += old.width + spacing;
        if new != old {
            changes.push((element.id, old, new));
        }
    }
    changes
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
        assert_eq!(compute_horizontal_layout(&[], 24.0, 48.0, 48.0), Vec::new());
    }

    #[test]
    fn single_element_is_offset_by_margin_only() {
        let elements = [fixture(400.0, 800.0)];
        let placements = compute_horizontal_layout(&elements, 24.0, 48.0, 48.0);
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].x, 48.0);
        assert_eq!(placements[0].y, 48.0);
        assert_eq!(placements[0].width, 400.0);
        assert_eq!(placements[0].height, 800.0);
    }

    #[test]
    fn equal_height_elements_are_spaced_evenly_with_no_rescale() {
        let elements = [fixture(400.0, 800.0), fixture(400.0, 800.0), fixture(400.0, 800.0)];
        let placements = compute_horizontal_layout(&elements, 20.0, 0.0, 0.0);
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
        let placements = compute_horizontal_layout(&elements, 0.0, 0.0, 0.0);
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
        let placements = compute_horizontal_layout(&elements, 0.0, 0.0, 0.0);
        assert_eq!(placements[0].x, 0.0);
        assert_eq!(placements[1].x, 100.0);
    }

    #[test]
    fn extent_accounts_for_trailing_margin() {
        let elements = [fixture(400.0, 800.0), fixture(400.0, 800.0)];
        let placements = compute_horizontal_layout(&elements, 20.0, 48.0, 48.0);
        let extent = extent_for(&placements, 48.0, 48.0);
        // margin + 400 + 20 + 400 + margin
        assert_eq!(extent.width, 48.0 + 400.0 + 20.0 + 400.0 + 48.0);
        assert_eq!(extent.height, 48.0 + 800.0 + 48.0);
    }

    #[test]
    fn fit_canvas_to_content_matches_the_layouts_extent() {
        use crate::model::Document;

        let mut doc = Document::new();
        doc.layout.spacing_px = 20.0;
        doc.layout.margin_x = 48.0;
        doc.layout.margin_y = 48.0;
        // A tall portrait screenshot (spec scenario: 1080x2424) — the bug
        // this guards against is the canvas staying at a fixed 1920x1080
        // and cropping content like this off at the bottom.
        doc.elements = vec![fixture(1080.0, 2424.0)];

        crate::layout::fit_canvas_to_content(&mut doc);

        assert_eq!(doc.canvas.export_width, (1080.0 + 48.0 * 2.0) as u32);
        assert_eq!(doc.canvas.export_height, (2424.0 + 48.0 * 2.0) as u32);
    }

    #[test]
    fn a_fixed_aspect_grows_the_canvas_and_centers_the_content() {
        let mut doc = Document::new();
        doc.elements.push(fixture(100.0, 200.0));
        doc.canvas.aspect = Some((16, 9));
        fit_canvas_to_content(&mut doc);
        let ratio = doc.canvas.export_width as f64 / doc.canvas.export_height as f64;
        assert!((ratio - 16.0 / 9.0).abs() < 0.01, "ratio {ratio}");
        assert!(doc.canvas.content_offset_x > 0.0);
        assert_eq!(doc.canvas.content_offset_y, 0.0);
    }

    #[test]
    fn balanced_transforms_line_up_with_equal_gaps_and_a_shared_center() {
        let mut elements = vec![fixture(100.0, 200.0), fixture(100.0, 100.0), fixture(50.0, 300.0)];
        elements[0].transform = Transform { x: 0.0, y: 0.0, width: 100.0, height: 200.0, ..Transform::default() };
        elements[1].transform = Transform { x: 400.0, y: 50.0, width: 100.0, height: 100.0, ..Transform::default() };
        elements[2].transform = Transform { x: 150.0, y: 300.0, width: 50.0, height: 300.0, ..Transform::default() };
        let changes = balanced_transforms(&elements, 20.0);
        let mut all: Vec<Transform> = elements.iter().map(|e| changes.iter().find(|c| c.0 == e.id).map(|c| c.2).unwrap_or(e.transform)).collect();
        all.sort_by(|a, b| a.x.total_cmp(&b.x));
        assert_eq!(all[1].x, all[0].x + all[0].width + 20.0);
        assert_eq!(all[2].x, all[1].x + all[1].width + 20.0);
        let centers: Vec<f64> = all.iter().map(|t| t.y + t.height / 2.0).collect();
        assert!(centers.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-9));
    }

    #[test]
    fn fit_canvas_to_content_is_a_noop_for_an_empty_document() {
        use crate::model::{CanvasSettings, Document};

        let mut doc = Document::new();
        let before = doc.canvas;
        crate::layout::fit_canvas_to_content(&mut doc);
        assert_eq!(doc.canvas, before);
        assert_eq!(doc.canvas, CanvasSettings::default());
    }

    #[test]
    fn a_negative_label_padding_pushes_the_canvas_origin_and_grows_it() {
        use crate::model::{Document, HorizontalAnchor, TextPosition, VerticalAnchor};

        let mut doc = Document::new();
        doc.layout.margin_x = 10.0;
        doc.layout.margin_y = 10.0;
        let mut el = fixture(200.0, 100.0);
        el.label.enabled = true;
        el.label.content = "Hi".to_string();
        // Push every label 50px above its screenshot's own top edge, via
        // the project's shared label style (see `crate::model::LabelStyle`).
        doc.label_defaults.position =
            TextPosition::Semantic { horizontal: HorizontalAnchor::Center, vertical: VerticalAnchor::Top, padding: -50.0 };
        doc.elements = vec![el];

        crate::layout::fit_canvas_to_content(&mut doc);

        // The label box's top edge sits at placement.y (margin_y) plus its
        // own resolved box_y, which is negative here — so content reaches
        // above the nominal origin and the canvas must grow upward too.
        assert!(doc.canvas.content_offset_y > 0.0);
        assert_eq!(doc.canvas.content_offset_x, 0.0);
        assert!(doc.canvas.export_height > (100.0 + 20.0) as u32);
    }

    #[test]
    fn content_that_never_goes_negative_keeps_a_zero_offset() {
        use crate::model::Document;

        let mut doc = Document::new();
        doc.layout.margin_x = 48.0;
        doc.layout.margin_y = 48.0;
        doc.elements = vec![fixture(400.0, 800.0)];

        crate::layout::fit_canvas_to_content(&mut doc);

        assert_eq!(doc.canvas.content_offset_x, 0.0);
        assert_eq!(doc.canvas.content_offset_y, 0.0);
    }

    #[test]
    fn fit_canvas_to_content_ignores_hidden_elements() {
        use crate::model::Document;

        let mut doc = Document::new();
        doc.layout.margin_x = 10.0;
        doc.layout.margin_y = 10.0;
        let mut hidden = fixture(2000.0, 2000.0);
        hidden.visible = false;
        let visible = fixture(100.0, 100.0);
        doc.elements = vec![hidden, visible];

        crate::layout::fit_canvas_to_content(&mut doc);

        assert_eq!(doc.canvas.export_width, 120);
        assert_eq!(doc.canvas.export_height, 120);
    }

    #[test]
    fn free_layout_uses_each_elements_own_stored_position_and_size() {
        let mut a = fixture(400.0, 800.0);
        a.transform.x = 10.0;
        a.transform.y = 20.0;
        a.transform.width = 111.0;
        a.transform.height = 222.0;
        let mut b = fixture(400.0, 800.0);
        b.transform.x = 300.0;
        b.transform.y = 5.0;
        b.transform.width = 50.0;
        b.transform.height = 60.0;

        // Spacing/margin have no effect — Free mode ignores them entirely.
        let placements = compute_layout(LayoutMode::Free, &[a, b], 999.0, 999.0, 999.0);

        assert_eq!(placements[0].x, 10.0);
        assert_eq!(placements[0].y, 20.0);
        assert_eq!(placements[0].width, 111.0);
        assert_eq!(placements[0].height, 222.0);
        assert_eq!(placements[1].x, 300.0);
        assert_eq!(placements[1].y, 5.0);
        assert_eq!(placements[1].width, 50.0);
        assert_eq!(placements[1].height, 60.0);
    }

    #[test]
    fn free_layout_of_empty_input_is_empty() {
        assert_eq!(compute_free_layout(&[]), Vec::new());
    }

    #[test]
    fn vertical_layout_stacks_top_to_bottom_scaled_to_common_width() {
        // 400x800 (aspect 2.0) and 200x300 (aspect 1.5) -> common width 200.
        let elements = [fixture(400.0, 800.0), fixture(200.0, 300.0)];
        let placements = compute_vertical_layout(&elements, 10.0, 5.0, 5.0);

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
        assert_eq!(compute_vertical_layout(&[], 10.0, 5.0, 5.0), Vec::new());
    }

    #[test]
    fn grid_layout_wraps_after_ceil_sqrt_n_columns() {
        // 4 elements -> ceil(sqrt(4)) = 2 columns, 2 rows.
        let elements =
            [fixture(100.0, 100.0), fixture(100.0, 100.0), fixture(100.0, 100.0), fixture(100.0, 100.0)];
        let placements = compute_grid_layout(&elements, 10.0, 0.0, 0.0);

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
        let placements = compute_grid_layout(&elements, 0.0, 0.0, 0.0);
        for p in &placements {
            assert_eq!(p.height, 50.0);
        }
    }

    #[test]
    fn grid_layout_of_empty_input_is_empty() {
        assert_eq!(compute_grid_layout(&[], 10.0, 5.0, 5.0), Vec::new());
    }
}
