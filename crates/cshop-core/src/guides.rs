//! Guides, and the arithmetic of snapping to things.
//!
//! A guide is a line across the whole document that nothing prints: somewhere
//! to line an edge up against. Snapping is what makes them worth having —
//! without it a guide is a thing you aim at by eye, which is what the editor
//! already made you do.
//!
//! The tolerance is given in *screen* pixels and divided by the zoom before it
//! is used, so a guide is equally easy to catch whatever the picture is
//! magnified to. Snapping in document units instead would make it impossible
//! to place anything precisely when zoomed in, and impossible to miss when
//! zoomed out.

use crate::geom::{IRect, Vec2};

/// A line across the document, at a whole or fractional pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guide {
    /// True for a line running top to bottom, at `at` across.
    pub vertical: bool,
    /// Where it sits, in document pixels.
    pub at: f32,
}

impl Guide {
    pub fn vertical(at: f32) -> Guide {
        Guide { vertical: true, at }
    }

    pub fn horizontal(at: f32) -> Guide {
        Guide { vertical: false, at }
    }
}

/// The lines something may snap to, in document coordinates.
///
/// Gathered by the caller, because what is worth snapping to depends on what
/// is being moved: a layer should not snap to its own edges.
#[derive(Debug, Clone, Default)]
pub struct SnapLines {
    /// Lines running top to bottom, at these positions across.
    pub vertical: Vec<f32>,
    /// Lines running left to right, at these positions down.
    pub horizontal: Vec<f32>,
}

impl SnapLines {
    /// The document's own edges and middle, which are what most things are
    /// lined up against.
    pub fn for_document(width: u32, height: u32) -> SnapLines {
        let (w, h) = (width as f32, height as f32);
        SnapLines {
            vertical: vec![0.0, w / 2.0, w],
            horizontal: vec![0.0, h / 2.0, h],
        }
    }

    pub fn add_guides(&mut self, guides: &[Guide]) {
        for g in guides {
            if g.vertical {
                self.vertical.push(g.at);
            } else {
                self.horizontal.push(g.at);
            }
        }
    }

    /// A grid every `spacing` pixels.
    ///
    /// The lines are generated rather than listed, because a fine grid on a
    /// large document is tens of thousands of them and only the handful near
    /// the point being snapped can possibly win.
    pub fn add_grid(&mut self, spacing: f32, near: Vec2, reach: f32) {
        if spacing <= 0.0 {
            return;
        }
        for (axis, lines) in [(near.x, &mut self.vertical), (near.y, &mut self.horizontal)] {
            let first = ((axis - reach) / spacing).floor() * spacing;
            let mut at = first;
            while at <= axis + reach {
                lines.push(at);
                at += spacing;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.vertical.is_empty() && self.horizontal.is_empty()
    }
}

/// The nearest line within `tolerance`, or the value unchanged.
fn nearest(value: f32, lines: &[f32], tolerance: f32) -> f32 {
    let mut best = value;
    let mut best_gap = tolerance;
    for &line in lines {
        let gap = (line - value).abs();
        if gap < best_gap {
            best_gap = gap;
            best = line;
        }
    }
    best
}

/// Snap a point to whichever lines it is near, each axis on its own.
///
/// The two axes are independent: a corner can catch a vertical guide without
/// its height changing, which is what makes a guide useful for aligning one
/// edge rather than a position.
pub fn snap_point(point: Vec2, lines: &SnapLines, tolerance: f32) -> Vec2 {
    Vec2::new(
        nearest(point.x, &lines.vertical, tolerance),
        nearest(point.y, &lines.horizontal, tolerance),
    )
}

/// Snap a rectangle by whichever of its edges is nearest a line.
///
/// The whole rectangle moves; it is not stretched. Every edge is offered to
/// every line and the smallest movement wins, so dragging a layer catches by
/// whichever side happens to come close first — which is what someone
/// aligning a picture to a guide is actually doing.
pub fn snap_offset(rect: IRect, lines: &SnapLines, tolerance: f32) -> Vec2 {
    let mut shift = Vec2::new(0.0, 0.0);

    let edges_x = [rect.x0 as f32, (rect.x0 + rect.x1) as f32 / 2.0, rect.x1 as f32];
    let mut best = tolerance;
    for edge in edges_x {
        for &line in &lines.vertical {
            let gap = line - edge;
            if gap.abs() < best {
                best = gap.abs();
                shift.x = gap;
            }
        }
    }

    let edges_y = [rect.y0 as f32, (rect.y0 + rect.y1) as f32 / 2.0, rect.y1 as f32];
    let mut best = tolerance;
    for edge in edges_y {
        for &line in &lines.horizontal {
            let gap = line - edge;
            if gap.abs() < best {
                best = gap.abs();
                shift.y = gap;
            }
        }
    }

    shift
}

/// A tick spacing that gives a readable ruler at this magnification.
///
/// Steps through 1, 2, 5 and their powers of ten, which is what every ruler
/// and every graph axis has always done: the eye reads halves and fifths
/// without having to be told the number.
pub fn ruler_step(zoom: f32) -> f32 {
    // Aim for a labelled tick roughly every eighty screen pixels.
    let want = 80.0 / zoom.max(1e-6);
    let magnitude = 10f32.powf(want.max(1.0).log10().floor());
    for factor in [1.0, 2.0, 5.0, 10.0] {
        if magnitude * factor >= want {
            return magnitude * factor;
        }
    }
    magnitude * 10.0
}
