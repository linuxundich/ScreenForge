//! Preset vector-decoration patterns for `Background::Decoration` — small,
//! procedurally generated shape sets rather than a full per-shape editor,
//! matching the rest of the app's "presets over granular editors" pattern
//! (see e.g. `ShadowParams::none()`/`subtle()`/...).

use crate::model::{Rgba, VectorShape};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationPreset {
    Dots,
    DiagonalLines,
}

impl DecorationPreset {
    /// The shapes for this preset, sized to cover a `width`×`height`
    /// canvas, in `color`.
    pub fn shapes(self, width: f64, height: f64, color: Rgba) -> Vec<VectorShape> {
        match self {
            DecorationPreset::Dots => dot_grid(width, height, color),
            DecorationPreset::DiagonalLines => diagonal_lines(width, height, color),
        }
    }
}

const DOT_SPACING: f64 = 40.0;
const DOT_RADIUS: f64 = 3.0;

/// An evenly spaced grid of dots, inset by half a spacing unit on all
/// sides so a row/column never sits exactly on the canvas edge.
fn dot_grid(width: f64, height: f64, color: Rgba) -> Vec<VectorShape> {
    let mut shapes = Vec::new();
    let mut y = DOT_SPACING / 2.0;
    while y < height {
        let mut x = DOT_SPACING / 2.0;
        while x < width {
            shapes.push(VectorShape::Circle { cx: x, cy: y, radius: DOT_RADIUS, color });
            x += DOT_SPACING;
        }
        y += DOT_SPACING;
    }
    shapes
}

const LINE_SPACING: f64 = 60.0;
const LINE_WIDTH: f64 = 2.0;

/// 45° stripes spanning the whole canvas regardless of aspect ratio: each
/// line runs from `(offset, 0)` to `(offset + height, height)`, and
/// `offset` sweeps from `-height` (line's bottom end at the canvas's
/// bottom-left corner) to `width` (line's top end at the top-right
/// corner) — the full range a 45° line needs to have swept across the
/// rectangle at least once.
fn diagonal_lines(width: f64, height: f64, color: Rgba) -> Vec<VectorShape> {
    let mut shapes = Vec::new();
    let mut offset = -height;
    while offset < width {
        shapes.push(VectorShape::Line { x1: offset, y1: 0.0, x2: offset + height, y2: height, width: LINE_WIDTH, color });
        offset += LINE_SPACING;
    }
    shapes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_grid_covers_the_canvas_without_touching_its_edges() {
        let shapes = DecorationPreset::Dots.shapes(200.0, 100.0, Rgba::new(0.0, 0.0, 0.0, 1.0));
        assert!(!shapes.is_empty());
        for shape in &shapes {
            let VectorShape::Circle { cx, cy, radius, .. } = shape else { panic!("expected only circles") };
            assert!(*cx - radius >= 0.0 && *cx + radius <= 200.0);
            assert!(*cy - radius >= 0.0 && *cy + radius <= 100.0);
        }
    }

    #[test]
    fn dot_grid_uses_the_given_color() {
        let color = Rgba::new(0.2, 0.4, 0.6, 0.8);
        let shapes = DecorationPreset::Dots.shapes(100.0, 100.0, color);
        assert!(shapes.iter().all(|s| matches!(s, VectorShape::Circle { color: c, .. } if *c == color)));
    }

    #[test]
    fn diagonal_lines_cover_the_canvas_corners() {
        let shapes = DecorationPreset::DiagonalLines.shapes(300.0, 150.0, Rgba::new(0.0, 0.0, 0.0, 1.0));
        assert!(!shapes.is_empty());
        // The first line's bottom end should reach the bottom-left corner,
        // and the last line's top end should reach the top-right corner.
        let VectorShape::Line { x1, y2, .. } = shapes.first().unwrap() else { panic!("expected only lines") };
        assert_eq!((*x1, *y2), (-150.0, 150.0));
        let VectorShape::Line { x2, y1, .. } = shapes.last().unwrap() else { panic!("expected only lines") };
        assert!(*x2 >= 300.0);
        assert_eq!(*y1, 0.0);
    }

    #[test]
    fn empty_canvas_produces_no_shapes() {
        assert!(DecorationPreset::Dots.shapes(0.0, 0.0, Rgba::new(0.0, 0.0, 0.0, 1.0)).is_empty());
        assert!(DecorationPreset::DiagonalLines.shapes(0.0, 0.0, Rgba::new(0.0, 0.0, 0.0, 1.0)).is_empty());
    }
}
