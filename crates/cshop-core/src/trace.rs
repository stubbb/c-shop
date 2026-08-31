//! Turning a selection back into a path.
//!
//! A selection and a path describe the same thing two ways: a region as
//! coverage per pixel, and a region as an outline. Going one way is already
//! easy — a path can be drawn as coverage, which is what a vector mask does.
//! This is the other way, and it is two steps.
//!
//! The first is already done. [`crate::selection::Selection::contours`] traces
//! the boundary for the marching ants, by collecting the *cracks* between
//! selected and unselected pixels: every such boundary is one unit-length
//! edge, and those join end to end into closed loops with no special cases,
//! wound so a hole comes out the other way round and cuts a hole when filled.
//! It also collapses runs that are already straight.
//!
//! The second is here, and it is the interesting one. What a trace gives is a
//! staircase: the outline of a circle has a few hundred corners, every one of
//! them a right angle, and it is described just as well by a few dozen points
//! in the right places. Finding which points those are is what makes the
//! result something a person can edit rather than a shape with a handle every
//! pixel.

use crate::geom::Vec2;
use crate::path::{Anchor, PathShape, SubPath};
use crate::selection::Selection;

/// Drop points that are already on the line between their neighbours.
///
/// Douglas–Peucker, which keeps the point furthest from the chord and recurses
/// on both halves — so what it keeps are the corners, at whatever scale they
/// happen to be, rather than every nth point.
pub fn simplify(line: &[Vec2], tolerance: f32) -> Vec<Vec2> {
    if line.len() < 3 || tolerance <= 0.0 {
        return line.to_vec();
    }
    let mut keep = vec![false; line.len()];
    keep[0] = true;
    let last = line.len() - 1;
    keep[last] = true;
    let mut stack = vec![(0usize, last)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (mut worst, mut at) = (0.0f32, a);
        for (i, p) in line.iter().enumerate().take(b).skip(a + 1) {
            let d = crate::path::polyline_distance(&[line[a], line[b]], *p);
            if d > worst {
                worst = d;
                at = i;
            }
        }
        if worst > tolerance {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    line.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// The same, for a loop.
///
/// Douglas–Peucker pins the two ends of what it is given, and a loop has no
/// ends — so handing it one as though it did keeps whichever point the trace
/// happened to start at, and never simplifies the segment that wraps round to
/// it. A rectangle comes out with six points instead of four.
///
/// So the loop is first rotated to start at its topmost-leftmost point, which
/// on a crack outline is always a real corner, and then closed by repeating
/// that point at the end. Now both pinned ends are the same corner, and every
/// segment including the wrap is simplified on the same terms.
pub fn simplify_closed(line: &[Vec2], tolerance: f32) -> Vec<Vec2> {
    if line.len() < 3 {
        return line.to_vec();
    }
    let start = line
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0);
    let mut closed: Vec<Vec2> = line[start..].iter().chain(&line[..start]).copied().collect();
    closed.push(closed[0]);
    let mut out = simplify(&closed, tolerance);
    out.pop();
    out
}

/// A selection's outline as a path, ready to edit with the Pen.
///
/// `tolerance` is in pixels: how far the outline may stray from the staircase
/// it was traced from. About a pixel is right for a shape someone drew;
/// tighter than that mostly preserves the aliasing.
pub fn path_from_selection(selection: &mut Selection, tolerance: f32) -> PathShape {
    let subpaths: Vec<SubPath> = selection
        .contours()
        .iter()
        .map(|line| simplify_closed(line, tolerance))
        .filter(|line| line.len() >= 3)
        .map(|line| SubPath {
            anchors: line.into_iter().map(Anchor::corner).collect(),
            closed: true,
        })
        .collect();
    PathShape::new(subpaths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask::MaskBuffer;

    fn selection_of(w: u32, h: u32, fill: impl Fn(i32, i32) -> bool) -> Selection {
        let mut m = MaskBuffer::hide_all(w, h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                if fill(x, y) {
                    m.set(x, y, 255);
                }
            }
        }
        Selection::from_mask(m)
    }

    #[test]
    fn a_rectangle_becomes_four_anchors() {
        let mut s = selection_of(32, 32, |x, y| (8..20).contains(&x) && (6..26).contains(&y));
        let path = path_from_selection(&mut s, 1.0);
        let sub = &path.parts[0].subpaths[0];
        assert!(sub.closed);
        assert_eq!(sub.anchors.len(), 4, "a rectangle needs four points, not a hundred");
        let xs: Vec<f32> = sub.anchors.iter().map(|a| a.at.x).collect();
        let ys: Vec<f32> = sub.anchors.iter().map(|a| a.at.y).collect();
        assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), 8.0);
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 20.0);
        assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), 6.0);
        assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 26.0);
    }

    /// Without closing the loop first, Douglas–Peucker pins whichever point
    /// the trace started at and never simplifies the wrap, so a rectangle
    /// comes out with six points. This is the test for that.
    #[test]
    fn a_loop_is_simplified_across_its_own_join() {
        // A staircase-free square whose trace starts mid-edge.
        let line: Vec<Vec2> = (0..40)
            .map(|i| {
                let (x, y) = match i / 10 {
                    0 => (i as f32, 0.0),
                    1 => (10.0, (i - 10) as f32),
                    2 => (10.0 - (i - 20) as f32, 10.0),
                    _ => (0.0, 10.0 - (i - 30) as f32),
                };
                Vec2::new(x, y)
            })
            .collect();
        // Rotated so it begins halfway along an edge, as a real trace might.
        let rotated: Vec<Vec2> = line[5..].iter().chain(&line[..5]).copied().collect();
        assert_eq!(simplify_closed(&rotated, 0.5).len(), 4);
        assert!(
            simplify(&rotated, 0.5).len() > 4,
            "and the open version keeps the arbitrary start, which is the bug"
        );
    }

    /// A hole has to come out wound the other way, or filling the path would
    /// fill the hole in as well.
    #[test]
    fn a_hole_stays_a_hole_through_the_round_trip() {
        let mut s = selection_of(40, 40, |x, y| {
            (5..35).contains(&x)
                && (5..35).contains(&y)
                && !((15..25).contains(&x) && (15..25).contains(&y))
        });
        let path = path_from_selection(&mut s, 1.0);
        assert_eq!(path.parts[0].subpaths.len(), 2, "the outside and the hole");

        let redrawn = crate::layer::mask_from_path(&path, 40, 40, false);
        assert!(redrawn.get(8, 8) > 200, "inside the shape");
        assert!(redrawn.get(20, 20) < 50, "and outside it again, in the hole");
    }

    /// The point of simplifying: a traced circle is hundreds of right angles
    /// and is described just as well by a few dozen points.
    #[test]
    fn simplifying_keeps_the_shape_and_drops_the_staircase() {
        let c = Vec2::new(32.0, 32.0);
        let mut s = selection_of(64, 64, |x, y| {
            c.distance(Vec2::new(x as f32 + 0.5, y as f32 + 0.5)) <= 24.0
        });
        let steps = s.contours()[0].len();
        assert!(steps > 100, "a traced circle is a lot of corners: {steps}");

        let path = path_from_selection(&mut s, 1.0);
        let points = path.parts[0].subpaths[0].anchors.len();
        assert!(points < steps / 3, "and far fewer once simplified: {points} of {steps}");

        // And it is still the circle: drawn back, it covers what it covered.
        let redrawn = crate::layer::mask_from_path(&path, 64, 64, false);
        let (mut agree, mut total) = (0, 0);
        for y in 0..64 {
            for x in 0..64 {
                total += 1;
                if (s.coverage(x, y) > 128) == (redrawn.get(x, y) > 128) {
                    agree += 1;
                }
            }
        }
        let ratio = agree as f32 / total as f32;
        assert!(ratio > 0.98, "the traced circle should still be the circle: {ratio:.3}");
    }

    #[test]
    fn two_separate_regions_become_two_subpaths() {
        let mut s = selection_of(40, 20, |x, y| {
            ((2..10).contains(&x) && (2..10).contains(&y))
                || ((25..35).contains(&x) && (4..12).contains(&y))
        });
        assert_eq!(path_from_selection(&mut s, 1.0).parts[0].subpaths.len(), 2);
    }

    #[test]
    fn an_empty_selection_becomes_an_empty_path() {
        let mut s = Selection::from_mask(MaskBuffer::hide_all(16, 16));
        assert!(path_from_selection(&mut s, 1.0).is_empty());
    }
}
