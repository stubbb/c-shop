//! 2D projective transforms.
//!
//! A single 3×3 matrix covers everything Free Transform needs — translate,
//! scale, rotate, skew, distort and perspective — because a projective
//! transform is exactly the family that maps a quadrilateral to a
//! quadrilateral. Distort and perspective then need no special case: they are
//! just the corner handles moved somewhere an affine transform could not reach.

use crate::geom::{IRect, Vec2};

/// A 3×3 projective transform, row-major, applied to homogeneous points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub m: [[f32; 3]; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Transform =
        Transform { m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] };

    pub fn translate(dx: f32, dy: f32) -> Transform {
        Transform { m: [[1.0, 0.0, dx], [0.0, 1.0, dy], [0.0, 0.0, 1.0]] }
    }

    pub fn scale(sx: f32, sy: f32) -> Transform {
        Transform { m: [[sx, 0.0, 0.0], [0.0, sy, 0.0], [0.0, 0.0, 1.0]] }
    }

    /// Rotation by `radians`, counter-clockwise in a y-down coordinate system.
    pub fn rotate(radians: f32) -> Transform {
        let (s, c) = radians.sin_cos();
        Transform { m: [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]] }
    }

    pub fn skew(kx: f32, ky: f32) -> Transform {
        Transform { m: [[1.0, kx, 0.0], [ky, 1.0, 0.0], [0.0, 0.0, 1.0]] }
    }

    /// Rotate, scale or skew about `pivot` rather than the origin.
    pub fn about(pivot: Vec2, inner: Transform) -> Transform {
        // Move the pivot to the origin, transform, move it back. `then` reads
        // left to right, so the negative translation comes first.
        Transform::translate(-pivot.x, -pivot.y)
            .then(inner)
            .then(Transform::translate(pivot.x, pivot.y))
    }

    /// `self` followed by `next`.
    pub fn then(self, next: Transform) -> Transform {
        let mut m = [[0.0f32; 3]; 3];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, slot) in row.iter_mut().enumerate() {
                *slot = (0..3).map(|k| next.m[i][k] * self.m[k][j]).sum();
            }
        }
        Transform { m }
    }

    /// Map a point.
    pub fn apply(&self, p: Vec2) -> Vec2 {
        let m = &self.m;
        let w = m[2][0] * p.x + m[2][1] * p.y + m[2][2];
        // A near-zero w means the point maps to infinity; clamping keeps the
        // result finite rather than producing NaN that spreads through the
        // whole transform.
        let w = if w.abs() < 1e-9 { 1e-9_f32.copysign(w) } else { w };
        Vec2::new(
            (m[0][0] * p.x + m[0][1] * p.y + m[0][2]) / w,
            (m[1][0] * p.x + m[1][1] * p.y + m[1][2]) / w,
        )
    }

    pub fn determinant(&self) -> f32 {
        let m = &self.m;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// Matrix inverse, or `None` when the transform collapses the plane.
    pub fn invert(&self) -> Option<Transform> {
        let det = self.determinant();
        if det.abs() < 1e-12 {
            return None;
        }
        let m = &self.m;
        let inv = |a: usize, b: usize, c: usize, d: usize| {
            m[a][b] * m[c][d] - m[a][d] * m[c][b]
        };
        let mut out = [[0.0f32; 3]; 3];
        out[0][0] = inv(1, 1, 2, 2);
        out[0][1] = -inv(0, 1, 2, 2);
        out[0][2] = inv(0, 1, 1, 2);
        out[1][0] = -inv(1, 0, 2, 2);
        out[1][1] = inv(0, 0, 2, 2);
        out[1][2] = -inv(0, 0, 1, 2);
        out[2][0] = inv(1, 0, 2, 1);
        out[2][1] = -inv(0, 0, 2, 1);
        out[2][2] = inv(0, 0, 1, 1);
        for row in &mut out {
            for v in row {
                *v /= det;
            }
        }
        Some(Transform { m: out })
    }

    /// The transform mapping the unit square's corners to `dst`, in the order
    /// top-left, top-right, bottom-right, bottom-left.
    ///
    /// This is the standard projective construction: solve for the two
    /// perspective terms from the diagonal mismatch, then read the affine part
    /// straight off the corners.
    pub fn from_unit_quad(dst: [Vec2; 4]) -> Option<Transform> {
        let (p0, p1, p2, p3) = (dst[0], dst[1], dst[2], dst[3]);
        let dx1 = p1.x - p2.x;
        let dx2 = p3.x - p2.x;
        let dy1 = p1.y - p2.y;
        let dy2 = p3.y - p2.y;
        let sx = p0.x - p1.x + p2.x - p3.x;
        let sy = p0.y - p1.y + p2.y - p3.y;

        let den = dx1 * dy2 - dx2 * dy1;
        if den.abs() < 1e-12 {
            return None;
        }

        // A parallelogram has no perspective component; the general case
        // solves the 2x2 system for it.
        let (g, h) = if sx.abs() < 1e-9 && sy.abs() < 1e-9 {
            (0.0, 0.0)
        } else {
            ((sx * dy2 - dx2 * sy) / den, (dx1 * sy - sx * dy1) / den)
        };

        Some(Transform {
            m: [
                [p1.x - p0.x + g * p1.x, p3.x - p0.x + h * p3.x, p0.x],
                [p1.y - p0.y + g * p1.y, p3.y - p0.y + h * p3.y, p0.y],
                [g, h, 1.0],
            ],
        })
    }

    /// The transform mapping the corners of `src` to `dst`.
    ///
    /// Free Transform's whole model: the source rectangle's four corners are
    /// dragged wherever the user likes, and the matrix follows.
    pub fn from_quad(src: IRect, dst: [Vec2; 4]) -> Option<Transform> {
        let (w, h) = (src.width() as f32, src.height() as f32);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        // Source rect -> unit square, then unit square -> destination quad.
        let normalise = Transform::translate(-(src.x0 as f32), -(src.y0 as f32))
            .then(Transform::scale(1.0 / w, 1.0 / h));
        Some(normalise.then(Transform::from_unit_quad(dst)?))
    }

    /// Axis-aligned bounds of `rect` after transforming.
    pub fn transformed_bounds(&self, rect: IRect) -> IRect {
        let corners = [
            Vec2::new(rect.x0 as f32, rect.y0 as f32),
            Vec2::new(rect.x1 as f32, rect.y0 as f32),
            Vec2::new(rect.x1 as f32, rect.y1 as f32),
            Vec2::new(rect.x0 as f32, rect.y1 as f32),
        ];
        let mapped: Vec<Vec2> = corners.iter().map(|p| self.apply(*p)).collect();
        let min_x = mapped.iter().map(|p| p.x).fold(f32::MAX, f32::min);
        let max_x = mapped.iter().map(|p| p.x).fold(f32::MIN, f32::max);
        let min_y = mapped.iter().map(|p| p.y).fold(f32::MAX, f32::min);
        let max_y = mapped.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        if !min_x.is_finite() || !max_x.is_finite() {
            return IRect::EMPTY;
        }
        // Snap away floating-point noise before rounding outward. `cos` of a
        // right angle is not exactly zero in f32, so a 90-degree rotation would
        // otherwise land a corner at -3e-7, floor to -1, and grow the layer by
        // a pixel on every turn.
        const EPS: f32 = 1e-3;
        IRect::new(
            (min_x + EPS).floor() as i32,
            (min_y + EPS).floor() as i32,
            (max_x - EPS).ceil() as i32,
            (max_y - EPS).ceil() as i32,
        )
    }

    pub fn is_identity(&self) -> bool {
        let i = Transform::IDENTITY.m;
        (0..3).all(|r| (0..3).all(|c| (self.m[r][c] - i[r][c]).abs() < 1e-6))
    }
}

/// The eight handles of a transform box, plus the body for moving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    /// Inside the box: drag to move.
    Body,
    /// Just outside a corner: drag to rotate.
    Rotate,
}

impl Handle {
    /// The four corners, in the order [`Transform::from_unit_quad`] expects.
    pub const CORNERS: [Handle; 4] =
        [Handle::TopLeft, Handle::TopRight, Handle::BottomRight, Handle::BottomLeft];

    pub const ALL: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// Position within the unit square.
    pub fn unit_position(self) -> Vec2 {
        match self {
            Handle::TopLeft => Vec2::new(0.0, 0.0),
            Handle::Top => Vec2::new(0.5, 0.0),
            Handle::TopRight => Vec2::new(1.0, 0.0),
            Handle::Right => Vec2::new(1.0, 0.5),
            Handle::BottomRight => Vec2::new(1.0, 1.0),
            Handle::Bottom => Vec2::new(0.5, 1.0),
            Handle::BottomLeft => Vec2::new(0.0, 1.0),
            Handle::Left => Vec2::new(0.0, 0.5),
            Handle::Body | Handle::Rotate => Vec2::new(0.5, 0.5),
        }
    }

    /// Index into the corner quad, for handles that move a single corner.
    pub fn corner_index(self) -> Option<usize> {
        Handle::CORNERS.iter().position(|h| *h == self)
    }

    /// The handle diagonally opposite, which is the anchor a scale drag keeps
    /// fixed.
    pub fn opposite(self) -> Handle {
        match self {
            Handle::TopLeft => Handle::BottomRight,
            Handle::Top => Handle::Bottom,
            Handle::TopRight => Handle::BottomLeft,
            Handle::Right => Handle::Left,
            Handle::BottomRight => Handle::TopLeft,
            Handle::Bottom => Handle::Top,
            Handle::BottomLeft => Handle::TopRight,
            Handle::Left => Handle::Right,
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec2, b: Vec2) -> bool {
        (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3
    }

    #[test]
    fn the_identity_leaves_points_where_they_are() {
        let t = Transform::IDENTITY;
        assert!(t.is_identity());
        assert!(close(t.apply(Vec2::new(3.0, -7.0)), Vec2::new(3.0, -7.0)));
    }

    #[test]
    fn translation_and_scaling_compose_in_order() {
        // `then` means "this, then that", so the scale must act on the already
        // translated point.
        let t = Transform::translate(10.0, 0.0).then(Transform::scale(2.0, 2.0));
        assert!(close(t.apply(Vec2::new(1.0, 1.0)), Vec2::new(22.0, 2.0)));

        let other = Transform::scale(2.0, 2.0).then(Transform::translate(10.0, 0.0));
        assert!(close(other.apply(Vec2::new(1.0, 1.0)), Vec2::new(12.0, 2.0)));
    }

    #[test]
    fn rotation_is_a_quarter_turn_where_expected() {
        let t = Transform::rotate(std::f32::consts::FRAC_PI_2);
        // y-down, so a positive quarter turn takes +x to +y.
        assert!(close(t.apply(Vec2::new(1.0, 0.0)), Vec2::new(0.0, 1.0)));
        assert!(close(t.apply(Vec2::new(0.0, 1.0)), Vec2::new(-1.0, 0.0)));
    }

    #[test]
    fn rotating_about_a_pivot_leaves_the_pivot_alone() {
        let pivot = Vec2::new(50.0, 20.0);
        let t = Transform::about(pivot, Transform::rotate(0.9));
        assert!(close(t.apply(pivot), pivot));
    }

    #[test]
    fn inverting_round_trips() {
        let t = Transform::about(Vec2::new(12.0, 8.0), Transform::rotate(0.6))
            .then(Transform::scale(1.7, 0.6))
            .then(Transform::translate(-4.0, 9.0));
        let back = t.invert().expect("invertible");
        for p in [Vec2::new(0.0, 0.0), Vec2::new(30.0, -12.0), Vec2::new(-5.0, 44.0)] {
            assert!(close(back.apply(t.apply(p)), p), "round trip failed for {p:?}");
        }
    }

    #[test]
    fn a_collapsed_transform_has_no_inverse() {
        assert!(Transform::scale(0.0, 1.0).invert().is_none());
        assert!(Transform::scale(1.0, 0.0).invert().is_none());
    }

    #[test]
    fn a_quad_transform_lands_the_corners_exactly() {
        let src = IRect::new(0, 0, 100, 50);
        let dst = [
            Vec2::new(10.0, 20.0),
            Vec2::new(210.0, 5.0),
            Vec2::new(190.0, 130.0),
            Vec2::new(30.0, 110.0),
        ];
        let t = Transform::from_quad(src, dst).expect("a proper quad");

        let corners = [
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 50.0),
            Vec2::new(0.0, 50.0),
        ];
        for (from, to) in corners.iter().zip(dst.iter()) {
            assert!(close(t.apply(*from), *to), "{from:?} should map to {to:?}");
        }
    }

    #[test]
    fn a_perspective_quad_keeps_straight_lines_straight() {
        // Strong perspective: the top edge much shorter than the bottom.
        let src = IRect::new(0, 0, 100, 100);
        let dst = [
            Vec2::new(40.0, 0.0),
            Vec2::new(60.0, 0.0),
            Vec2::new(100.0, 100.0),
            Vec2::new(0.0, 100.0),
        ];
        let t = Transform::from_quad(src, dst).unwrap();

        // Three collinear source points must stay collinear.
        let a = t.apply(Vec2::new(0.0, 50.0));
        let b = t.apply(Vec2::new(50.0, 50.0));
        let c = t.apply(Vec2::new(100.0, 50.0));
        let cross = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        assert!(cross.abs() < 1e-2, "the line bent: cross = {cross}");
    }

    #[test]
    fn an_offset_source_rect_maps_correctly() {
        let src = IRect::new(20, 30, 60, 70);
        let dst = [
            Vec2::new(0.0, 0.0),
            Vec2::new(80.0, 0.0),
            Vec2::new(80.0, 80.0),
            Vec2::new(0.0, 80.0),
        ];
        let t = Transform::from_quad(src, dst).unwrap();
        assert!(close(t.apply(Vec2::new(20.0, 30.0)), Vec2::new(0.0, 0.0)));
        assert!(close(t.apply(Vec2::new(60.0, 70.0)), Vec2::new(80.0, 80.0)));
        // The centre maps to the centre.
        assert!(close(t.apply(Vec2::new(40.0, 50.0)), Vec2::new(40.0, 40.0)));
    }

    #[test]
    fn a_degenerate_quad_is_rejected() {
        let src = IRect::new(0, 0, 10, 10);
        let flat = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(0.0, 0.0),
        ];
        assert!(Transform::from_quad(src, flat).is_none());
        assert!(Transform::from_quad(IRect::EMPTY, flat).is_none());
    }

    #[test]
    fn transformed_bounds_cover_the_rotated_rect() {
        let rect = IRect::new(0, 0, 100, 100);
        let t = Transform::about(Vec2::new(50.0, 50.0), Transform::rotate(std::f32::consts::FRAC_PI_4));
        let b = t.transformed_bounds(rect);
        // A square rotated 45 degrees has a diagonal of 141.4.
        assert!(b.width() >= 141 && b.width() <= 143, "got {b:?}");
        assert!(b.x0 < 0 && b.x1 > 100);
    }

    #[test]
    fn right_angle_rotations_do_not_grow_the_bounds() {
        // Four quarter turns must return exactly to the original size, not
        // creep outward by a pixel each time.
        let mut rect = IRect::new(0, 0, 37, 21);
        let original = (rect.width(), rect.height());
        for _ in 0..4 {
            let t = Transform::rotate(std::f32::consts::FRAC_PI_2);
            rect = t.transformed_bounds(rect);
        }
        assert_eq!((rect.width(), rect.height()), original, "size drifted to {rect:?}");
    }

    #[test]
    fn handles_know_their_opposites_and_positions() {
        assert_eq!(Handle::TopLeft.opposite(), Handle::BottomRight);
        assert_eq!(Handle::Left.opposite(), Handle::Right);
        assert_eq!(Handle::TopLeft.unit_position(), Vec2::new(0.0, 0.0));
        assert_eq!(Handle::BottomRight.unit_position(), Vec2::new(1.0, 1.0));
        assert_eq!(Handle::TopRight.corner_index(), Some(1));
        assert_eq!(Handle::Top.corner_index(), None);
    }
}
