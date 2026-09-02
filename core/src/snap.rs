//! Alignment ("smart") guides for `LayoutMode::Free` dragging — pure
//! geometry, no GTK dependency, so the snapping math is unit-testable
//! independently of the canvas widget that calls it.

/// An axis-aligned box in document space — deliberately a plain struct
/// (not [`crate::layout::Placement`]) since callers here don't need an
/// element id, just geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A single alignment line to draw, in document space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Guide {
    Vertical(f64),
    Horizontal(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SnapResult {
    pub x: f64,
    pub y: f64,
    pub guides: Vec<Guide>,
}

/// Snaps `moving` (already at its drag-proposed `x`/`y`) to the nearest
/// aligned edge or center among `others` and the canvas bounds/center,
/// independently on each axis, whenever that alignment is within
/// `threshold` document pixels. At most one guide is returned per axis —
/// the closest candidate wins, ties broken by whichever is checked first.
pub fn snap_position(moving: Rect, others: &[Rect], canvas_width: f64, canvas_height: f64, threshold: f64) -> SnapResult {
    let mut x_targets: Vec<f64> = vec![0.0, canvas_width, canvas_width / 2.0];
    let mut y_targets: Vec<f64> = vec![0.0, canvas_height, canvas_height / 2.0];
    for other in others {
        x_targets.push(other.x);
        x_targets.push(other.x + other.width);
        x_targets.push(other.x + other.width / 2.0);
        y_targets.push(other.y);
        y_targets.push(other.y + other.height);
        y_targets.push(other.y + other.height / 2.0);
    }

    let x_features = [moving.x, moving.x + moving.width, moving.x + moving.width / 2.0];
    let y_features = [moving.y, moving.y + moving.height, moving.y + moving.height / 2.0];

    let mut guides = Vec::new();
    let x = match best_snap(&x_features, &x_targets, threshold) {
        Some((feature, target)) => {
            guides.push(Guide::Vertical(target));
            moving.x + (target - feature)
        }
        None => moving.x,
    };
    let y = match best_snap(&y_features, &y_targets, threshold) {
        Some((feature, target)) => {
            guides.push(Guide::Horizontal(target));
            moving.y + (target - feature)
        }
        None => moving.y,
    };

    SnapResult { x, y, guides }
}

/// The closest (feature, target) pair within `threshold`, if any.
fn best_snap(features: &[f64], targets: &[f64], threshold: f64) -> Option<(f64, f64)> {
    let mut best: Option<(f64, f64, f64)> = None; // (feature, target, distance)
    for &feature in features {
        for &target in targets {
            let distance = (feature - target).abs();
            if distance <= threshold && best.is_none_or(|(_, _, best_distance)| distance < best_distance) {
                best = Some((feature, target, distance));
            }
        }
    }
    best.map(|(feature, target, _)| (feature, target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_left_edge_to_another_elements_left_edge() {
        let moving = Rect { x: 103.0, y: 500.0, width: 100.0, height: 100.0 };
        let others = [Rect { x: 100.0, y: 0.0, width: 50.0, height: 50.0 }];
        let result = snap_position(moving, &others, 2000.0, 2000.0, 10.0);
        assert_eq!(result.x, 100.0);
        assert!(result.guides.contains(&Guide::Vertical(100.0)));
    }

    #[test]
    fn snaps_center_to_canvas_center() {
        let moving = Rect { x: 448.0, y: 500.0, width: 100.0, height: 100.0 };
        // canvas width 1000 -> center 500; moving's center is at 498, within threshold.
        let result = snap_position(moving, &[], 1000.0, 1000.0, 10.0);
        assert_eq!(result.x, 450.0);
        assert!(result.guides.contains(&Guide::Vertical(500.0)));
    }

    #[test]
    fn does_not_snap_beyond_threshold() {
        let moving = Rect { x: 200.0, y: 200.0, width: 100.0, height: 100.0 };
        let others = [Rect { x: 0.0, y: 0.0, width: 50.0, height: 50.0 }];
        let result = snap_position(moving, &others, 2000.0, 2000.0, 5.0);
        assert_eq!(result.x, 200.0);
        assert_eq!(result.y, 200.0);
        assert!(result.guides.is_empty());
    }

    #[test]
    fn snaps_each_axis_independently() {
        let moving = Rect { x: 199.0, y: 301.0, width: 100.0, height: 100.0 };
        let others = [Rect { x: 200.0, y: 0.0, width: 10.0, height: 50.0 }];
        let result = snap_position(moving, &others, 5000.0, 5000.0, 10.0);
        // x snaps to the other element's left edge (200); y has no close target
        // among the built-in canvas guides (0, 2500, 5000) or the other
        // element's top/bottom/center (0, 50, 25), so it stays put.
        assert_eq!(result.x, 200.0);
        assert_eq!(result.y, 301.0);
    }

    #[test]
    fn picks_the_closest_candidate_when_several_are_in_range() {
        // Zero-width rects so each one contributes a single, unambiguous
        // x-target rather than three (left/right/center) that could
        // themselves be closer than the intended tie.
        let moving = Rect { x: 104.0, y: 0.0, width: 100.0, height: 100.0 };
        let others = [Rect { x: 100.0, y: 0.0, width: 0.0, height: 1.0 }, Rect { x: 108.0, y: 0.0, width: 0.0, height: 1.0 }];
        let result = snap_position(moving, &others, 5000.0, 5000.0, 10.0);
        // The moving box's left edge (104) is 4 away from both 100 and 108 --
        // exactly tied; either is an acceptable, equally-valid snap, so just
        // confirm it snapped to one of the two rather than asserting a
        // specific tie-break winner.
        assert!(result.x == 100.0 || result.x == 108.0);
    }
}
