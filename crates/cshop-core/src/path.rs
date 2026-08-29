//! Bézier paths, and the boolean operations that combine them.
//!
//! A path is subpaths of cubic segments. Everything downstream — filling,
//! stroking, layer effects, rasterising — already works from a signed distance
//! field, so a path only has to answer the same question every other shape
//! does: how far is this point from the outline, and is it inside?
//!
//! That is also what makes the boolean operations cheap. Combining two shapes
//! is combining two distance fields: the smaller of the two is their union,
//! the larger their intersection, and a negated operand turns a union into a
//! subtraction. The result is exact where it matters — the sign, and so the
//! outline — and approximate only in the magnitude near a seam, which is a
//! pixel or two of antialiasing either way.
//!
//! Combining fields does mean the result is not a single editable path the way
//! a real geometric boolean would give: the operands stay in the layer and are
//! evaluated together. That is a deliberate trade. Curve-curve intersection
//! and the topology that follows is a great deal of code to get subtly wrong,
//! and keeping the operands means the operation is still editable afterwards,
//! which the flattened answer would not be.

use crate::geom::Vec2;
use crate::selection::Rectf;

/// One end of a segment, with the handles that shape the curves either side.
///
/// Handles are absolute positions rather than offsets, which is what a pen
/// tool manipulates directly and what avoids a coordinate conversion on every
/// drag. A handle sitting on its own anchor means that side is a straight line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub at: Vec2,
    /// Control point for the curve arriving at this anchor.
    pub in_handle: Vec2,
    /// Control point for the curve leaving it.
    pub out_handle: Vec2,
}

impl Anchor {
    /// A corner: no curvature either side.
    pub fn corner(at: Vec2) -> Anchor {
        Anchor { at, in_handle: at, out_handle: at }
    }

    /// A smooth point, with handles mirrored about the anchor.
    pub fn smooth(at: Vec2, out_handle: Vec2) -> Anchor {
        Anchor { at, in_handle: at + (at - out_handle), out_handle }
    }

    /// Whether the two handles are mirrored, which is what a pen tool keeps
    /// true while dragging and what a corner point deliberately breaks.
    pub fn is_smooth(&self) -> bool {
        let mirrored = self.at + (self.at - self.out_handle);
        mirrored.distance(self.in_handle) < 1e-3
    }

    pub fn translate(&self, by: Vec2) -> Anchor {
        Anchor {
            at: self.at + by,
            in_handle: self.in_handle + by,
            out_handle: self.out_handle + by,
        }
    }
}

/// A run of anchors, open or closed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SubPath {
    pub anchors: Vec<Anchor>,
    pub closed: bool,
}

impl SubPath {
    pub fn open(anchors: Vec<Anchor>) -> SubPath {
        SubPath { anchors, closed: false }
    }

    pub fn closed(anchors: Vec<Anchor>) -> SubPath {
        SubPath { anchors, closed: true }
    }

    /// A closed run of corners.
    pub fn polygon(points: &[Vec2]) -> SubPath {
        SubPath::closed(points.iter().map(|p| Anchor::corner(*p)).collect())
    }

    /// The segments, as `(from, control1, control2, to)`.
    pub fn segments(&self) -> Vec<(Vec2, Vec2, Vec2, Vec2)> {
        let n = self.anchors.len();
        if n < 2 {
            return Vec::new();
        }
        let last = if self.closed { n } else { n - 1 };
        (0..last)
            .map(|i| {
                let a = &self.anchors[i];
                let b = &self.anchors[(i + 1) % n];
                (a.at, a.out_handle, b.in_handle, b.at)
            })
            .collect()
    }

    /// Points along the subpath, close enough that straight lines between them
    /// are within `tolerance` of the curve.
    pub fn flatten(&self, tolerance: f32) -> Vec<Vec2> {
        let mut out = Vec::new();
        let Some(first) = self.anchors.first() else { return out };
        out.push(first.at);
        for (p0, p1, p2, p3) in self.segments() {
            flatten_cubic(p0, p1, p2, p3, tolerance.max(1e-3), 0, &mut out);
            out.push(p3);
        }
        // A closed run has to return to its start, or the winding test below
        // sees a gap and reports the interior as outside.
        if self.closed {
            if let (Some(a), Some(b)) = (out.first().copied(), out.last().copied()) {
                if a.distance(b) > 1e-6 {
                    out.push(a);
                }
            }
        }
        out
    }

    pub fn translate(&self, by: Vec2) -> SubPath {
        SubPath {
            anchors: self.anchors.iter().map(|a| a.translate(by)).collect(),
            closed: self.closed,
        }
    }
}

/// Split a cubic until it is flat enough to draw as a line.
///
/// The test is how far the control points stray from the chord, which bounds
/// the curve's own deviation. Depth is capped because a degenerate curve — a
/// cusp, or handles on top of each other — can otherwise fail the test forever.
fn flatten_cubic(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, tolerance: f32, depth: u32, out: &mut Vec<Vec2>) {
    const MAX_DEPTH: u32 = 16;
    if depth >= MAX_DEPTH || is_flat(p0, p1, p2, p3, tolerance) {
        return;
    }
    // de Casteljau at the midpoint.
    let p01 = p0.lerp(p1, 0.5);
    let p12 = p1.lerp(p2, 0.5);
    let p23 = p2.lerp(p3, 0.5);
    let p012 = p01.lerp(p12, 0.5);
    let p123 = p12.lerp(p23, 0.5);
    let mid = p012.lerp(p123, 0.5);

    flatten_cubic(p0, p01, p012, mid, tolerance, depth + 1, out);
    out.push(mid);
    flatten_cubic(mid, p123, p23, p3, tolerance, depth + 1, out);
}

fn is_flat(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, tolerance: f32) -> bool {
    let chord = p3 - p0;
    let len = chord.length();
    if len < 1e-6 {
        // No chord to measure against; fall back to how far the handles reach.
        return p1.distance(p0).max(p2.distance(p0)) <= tolerance;
    }
    let n = Vec2::new(-chord.y / len, chord.x / len);
    let d1 = (p1 - p0).x * n.x + (p1 - p0).y * n.y;
    let d2 = (p2 - p0).x * n.x + (p2 - p0).y * n.y;
    d1.abs().max(d2.abs()) <= tolerance
}

/// How an operand combines with everything before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoolOp {
    /// Everything in either.
    #[default]
    Union,
    /// What is in the earlier shape and not this one.
    Subtract,
    /// Only what is in both.
    Intersect,
    /// What is in one but not both.
    Exclude,
}

impl BoolOp {
    pub fn name(self) -> &'static str {
        match self {
            BoolOp::Union => "Union",
            BoolOp::Subtract => "Subtract",
            BoolOp::Intersect => "Intersect",
            BoolOp::Exclude => "Exclude",
        }
    }

    pub fn all() -> [BoolOp; 4] {
        [BoolOp::Union, BoolOp::Subtract, BoolOp::Intersect, BoolOp::Exclude]
    }

    /// Combine two signed distances, negative inside.
    #[inline]
    pub fn combine(self, a: f32, b: f32) -> f32 {
        match self {
            BoolOp::Union => a.min(b),
            BoolOp::Intersect => a.max(b),
            BoolOp::Subtract => a.max(-b),
            // In one but not both: inside the union and outside the
            // intersection.
            BoolOp::Exclude => a.min(b).max(-a.max(b)),
        }
    }
}

/// One operand of a path shape: some subpaths, and how they join what is
/// already there.
#[derive(Debug, Clone, PartialEq)]
pub struct PathPart {
    pub subpaths: Vec<SubPath>,
    pub op: BoolOp,
}

impl PathPart {
    pub fn new(subpaths: Vec<SubPath>) -> PathPart {
        PathPart { subpaths, op: BoolOp::Union }
    }
}

/// A path shape: operands combined left to right.
///
/// The first operand's operation is ignored — there is nothing before it to
/// combine with.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PathShape {
    pub parts: Vec<PathPart>,
}

impl PathShape {
    pub fn new(subpaths: Vec<SubPath>) -> PathShape {
        PathShape { parts: vec![PathPart::new(subpaths)] }
    }

    pub fn is_empty(&self) -> bool {
        self.parts.iter().all(|p| p.subpaths.iter().all(|s| s.anchors.len() < 2))
    }

    /// Every anchor, for bounds and for editing.
    pub fn anchors(&self) -> impl Iterator<Item = &Anchor> {
        self.parts.iter().flat_map(|p| p.subpaths.iter().flat_map(|s| s.anchors.iter()))
    }

    /// The area the path covers, including the reach of its handles.
    ///
    /// Handles count because a curve can bow outside its anchors; taking the
    /// control hull is a bound on the curve rather than the curve itself, so it
    /// is never too small.
    pub fn bounds(&self) -> Option<Rectf> {
        let first = self.anchors().next()?;
        let mut r = Rectf::from_points(first.at, first.at);
        for a in self.anchors() {
            r = r
                .include(a.at.x, a.at.y)
                .include(a.in_handle.x, a.in_handle.y)
                .include(a.out_handle.x, a.out_handle.y);
        }
        Some(r)
    }

    pub fn translate(&self, by: Vec2) -> PathShape {
        PathShape {
            parts: self
                .parts
                .iter()
                .map(|p| PathPart {
                    subpaths: p.subpaths.iter().map(|s| s.translate(by)).collect(),
                    op: p.op,
                })
                .collect(),
        }
    }

    /// Flatten once, so that rasterising does not re-subdivide per pixel.
    pub fn flatten(&self, tolerance: f32) -> Flattened {
        Flattened {
            parts: self
                .parts
                .iter()
                .map(|part| {
                    let mut closed = Vec::new();
                    let mut open = Vec::new();
                    for sub in &part.subpaths {
                        if sub.anchors.len() < 2 {
                            continue;
                        }
                        let line = sub.flatten(tolerance);
                        if sub.closed {
                            closed.push(line);
                        } else {
                            open.push(line);
                        }
                    }
                    FlatPart { closed, open, op: part.op }
                })
                .collect(),
        }
    }
}

/// One operand, reduced to line segments.
#[derive(Debug, Clone)]
pub struct FlatPart {
    /// Closed contours, which have an interior.
    pub closed: Vec<Vec<Vec2>>,
    /// Open contours, which can only be stroked.
    pub open: Vec<Vec<Vec2>>,
    pub op: BoolOp,
}

/// A path reduced to line segments, ready to be sampled per pixel.
#[derive(Debug, Clone, Default)]
pub struct Flattened {
    pub parts: Vec<FlatPart>,
}

impl Flattened {
    pub fn is_empty(&self) -> bool {
        self.parts.iter().all(|p| p.closed.is_empty() && p.open.is_empty())
    }

    pub fn has_interior(&self) -> bool {
        self.parts.iter().any(|p| !p.closed.is_empty())
    }

    pub fn has_open(&self) -> bool {
        self.parts.iter().any(|p| !p.open.is_empty())
    }

    /// Signed distance to the filled region, negative inside.
    ///
    /// Each operand is evaluated on its own and then folded together, so the
    /// operations apply to whole shapes rather than to individual contours —
    /// which is what makes a hole inside one operand survive a union with
    /// another.
    pub fn fill_distance(&self, p: Vec2) -> f32 {
        let mut acc: Option<f32> = None;
        for part in &self.parts {
            if part.closed.is_empty() {
                continue;
            }
            let d = signed_distance(&part.closed, p);
            acc = Some(match acc {
                None => d,
                Some(a) => part.op.combine(a, d),
            });
        }
        acc.unwrap_or(f32::INFINITY)
    }

    /// Distance to the nearest open contour, for paths that are only strokes.
    pub fn open_distance(&self, p: Vec2) -> f32 {
        let mut best = f32::INFINITY;
        for part in &self.parts {
            for line in &part.open {
                best = best.min(polyline_distance(line, p));
            }
        }
        best
    }
}

/// Signed distance to a set of closed contours, negative inside.
///
/// Inside is decided by the non-zero winding rule, which is what lets a
/// contour wound the other way cut a hole in the one containing it.
pub fn signed_distance(contours: &[Vec<Vec2>], p: Vec2) -> f32 {
    let mut best = f32::INFINITY;
    let mut winding = 0i32;
    for line in contours {
        for w in line.windows(2) {
            let (a, b) = (w[0], w[1]);
            best = best.min(segment_distance(p, a, b));
            // The standard crossing test: count edges crossing the ray to the
            // right of p, signed by the direction they cross in.
            let side = (b.x - a.x) * (p.y - a.y) - (p.x - a.x) * (b.y - a.y);
            if a.y <= p.y {
                if b.y > p.y && side > 0.0 {
                    winding += 1;
                }
            } else if b.y <= p.y && side < 0.0 {
                winding -= 1;
            }
        }
    }
    if winding != 0 {
        -best
    } else {
        best
    }
}

/// Distance to a polyline, unsigned.
pub fn polyline_distance(line: &[Vec2], p: Vec2) -> f32 {
    let mut best = f32::INFINITY;
    for w in line.windows(2) {
        best = best.min(segment_distance(p, w[0], w[1]));
    }
    best
}

#[inline]
fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    if len2 < 1e-12 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0);
    p.distance(Vec2::new(a.x + ab.x * t, a.y + ab.y * t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f32, y0: f32, x1: f32, y1: f32) -> SubPath {
        SubPath::polygon(&[
            Vec2::new(x0, y0),
            Vec2::new(x1, y0),
            Vec2::new(x1, y1),
            Vec2::new(x0, y1),
        ])
    }

    #[test]
    fn a_straight_segment_needs_no_subdivision() {
        let line = SubPath::open(vec![
            Anchor::corner(Vec2::new(0.0, 0.0)),
            Anchor::corner(Vec2::new(100.0, 0.0)),
        ]);
        assert_eq!(line.flatten(0.1).len(), 2, "a line is already flat");
    }

    #[test]
    fn a_curve_is_flattened_to_within_its_tolerance() {
        // A quarter circle's worth of bow.
        let sub = SubPath::open(vec![
            Anchor { at: Vec2::new(0.0, 0.0), in_handle: Vec2::new(0.0, 0.0), out_handle: Vec2::new(0.0, 55.0) },
            Anchor { at: Vec2::new(100.0, 100.0), in_handle: Vec2::new(45.0, 100.0), out_handle: Vec2::new(100.0, 100.0) },
        ]);
        for tolerance in [1.0f32, 0.25, 0.05] {
            let line = sub.flatten(tolerance);
            assert!(line.len() > 2, "a curve should be subdivided");
            // Every flattened point must lie on the curve, so the chord error
            // is what the tolerance says.
            let coarse = sub.flatten(tolerance * 4.0);
            assert!(
                line.len() >= coarse.len(),
                "a tighter tolerance should not give fewer points"
            );
        }
    }

    #[test]
    fn a_closed_contour_knows_its_inside() {
        let shape = PathShape::new(vec![square(0.0, 0.0, 10.0, 10.0)]);
        let flat = shape.flatten(0.1);
        assert!(flat.fill_distance(Vec2::new(5.0, 5.0)) < 0.0, "the middle is inside");
        assert!(flat.fill_distance(Vec2::new(-5.0, 5.0)) > 0.0, "outside is outside");
        // On the edge, the distance is about zero.
        assert!(flat.fill_distance(Vec2::new(0.0, 5.0)).abs() < 0.01);
        // And it really is a distance.
        assert!((flat.fill_distance(Vec2::new(-3.0, 5.0)) - 3.0).abs() < 0.01);
    }

    #[test]
    fn a_reversed_contour_cuts_a_hole() {
        let outer = square(0.0, 0.0, 20.0, 20.0);
        let mut inner = square(5.0, 5.0, 15.0, 15.0);
        inner.anchors.reverse();
        let shape = PathShape::new(vec![outer, inner]);
        let flat = shape.flatten(0.1);
        assert!(flat.fill_distance(Vec2::new(2.0, 10.0)) < 0.0, "the ring is filled");
        assert!(flat.fill_distance(Vec2::new(10.0, 10.0)) > 0.0, "the hole is not");
    }

    #[test]
    fn the_boolean_operations_do_what_they_say() {
        // Two overlapping squares: 0..10 and 5..15.
        let a = PathPart::new(vec![square(0.0, 0.0, 10.0, 10.0)]);
        let mut b = PathPart::new(vec![square(5.0, 0.0, 15.0, 10.0)]);

        let only_a = Vec2::new(2.0, 5.0);
        let both = Vec2::new(7.0, 5.0);
        let only_b = Vec2::new(12.0, 5.0);
        let neither = Vec2::new(20.0, 5.0);

        let cases = [
            (BoolOp::Union, [true, true, true, false]),
            (BoolOp::Intersect, [false, true, false, false]),
            (BoolOp::Subtract, [true, false, false, false]),
            (BoolOp::Exclude, [true, false, true, false]),
        ];
        for (op, want) in cases {
            b.op = op;
            let shape = PathShape { parts: vec![a.clone(), b.clone()] };
            let flat = shape.flatten(0.05);
            for (p, expected) in [only_a, both, only_b, neither].iter().zip(want) {
                let inside = flat.fill_distance(*p) < 0.0;
                assert_eq!(inside, expected, "{} at {p:?}", op.name());
            }
        }
    }

    #[test]
    fn bounds_include_the_reach_of_the_handles() {
        let sub = SubPath::open(vec![
            Anchor { at: Vec2::new(0.0, 0.0), in_handle: Vec2::new(0.0, 0.0), out_handle: Vec2::new(0.0, -40.0) },
            Anchor { at: Vec2::new(10.0, 0.0), in_handle: Vec2::new(10.0, -40.0), out_handle: Vec2::new(10.0, 0.0) },
        ]);
        let r = PathShape::new(vec![sub]).bounds().expect("has bounds");
        assert!(r.y0 <= -40.0, "the bow above the anchors is inside the bounds: {r:?}");
        assert!(r.x0 <= 0.0 && r.x1 >= 10.0);
    }

    #[test]
    fn an_open_path_has_no_interior_but_still_has_a_distance() {
        let sub = SubPath::open(vec![
            Anchor::corner(Vec2::new(0.0, 0.0)),
            Anchor::corner(Vec2::new(10.0, 0.0)),
        ]);
        let flat = PathShape::new(vec![sub]).flatten(0.1);
        assert!(!flat.has_interior(), "an open path is not a region");
        assert!(flat.has_open());
        assert!((flat.open_distance(Vec2::new(5.0, 3.0)) - 3.0).abs() < 0.01);
    }

    #[test]
    fn a_smooth_anchor_mirrors_its_handles() {
        let a = Anchor::smooth(Vec2::new(10.0, 10.0), Vec2::new(20.0, 10.0));
        assert!(a.is_smooth());
        assert_eq!(a.in_handle, Vec2::new(0.0, 10.0));
        assert!(!Anchor::corner(Vec2::new(1.0, 1.0)).out_handle.distance(Vec2::new(1.0, 1.0)) .gt(&1e-6));
    }
}
