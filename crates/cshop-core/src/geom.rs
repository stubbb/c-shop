//! Pixel-space geometry. Everything here is integer device pixels; floating
//! point creeps in only at the interaction layer.

use std::ops::{Add, Mul, Sub};

/// Integer rectangle with an exclusive maximum corner.
///
/// An empty rect is any rect where `x1 <= x0 || y1 <= y0`; [`IRect::EMPTY`] is
/// the canonical one and is the identity for [`IRect::union`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct IRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl IRect {
    pub const EMPTY: IRect = IRect { x0: 0, y0: 0, x1: 0, y1: 0 };

    #[inline]
    pub const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    #[inline]
    pub const fn from_size(w: u32, h: u32) -> Self {
        Self { x0: 0, y0: 0, x1: w as i32, y1: h as i32 }
    }

    #[inline]
    pub const fn at(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x0: x, y0: y, x1: x + w as i32, y1: y + h as i32 }
    }

    #[inline]
    pub const fn width(&self) -> u32 {
        if self.x1 > self.x0 { (self.x1 - self.x0) as u32 } else { 0 }
    }

    #[inline]
    pub const fn height(&self) -> u32 {
        if self.y1 > self.y0 { (self.y1 - self.y0) as u32 } else { 0 }
    }

    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    #[inline]
    pub const fn area(&self) -> u64 {
        self.width() as u64 * self.height() as u64
    }

    #[inline]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }

    /// Smallest rect covering both. An empty operand is ignored, which makes
    /// this usable as a fold over dirty regions starting from [`IRect::EMPTY`].
    #[inline]
    pub fn union(&self, other: &IRect) -> IRect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        IRect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    #[inline]
    pub fn intersect(&self, other: &IRect) -> IRect {
        let r = IRect {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        if r.is_empty() { IRect::EMPTY } else { r }
    }

    #[inline]
    pub fn intersects(&self, other: &IRect) -> bool {
        !self.intersect(other).is_empty()
    }

    #[inline]
    pub fn translate(&self, dx: i32, dy: i32) -> IRect {
        IRect { x0: self.x0 + dx, y0: self.y0 + dy, x1: self.x1 + dx, y1: self.y1 + dy }
    }

    /// Grow on every side. Negative values shrink, clamping to empty.
    #[inline]
    pub fn inflate(&self, by: i32) -> IRect {
        let r = IRect {
            x0: self.x0 - by,
            y0: self.y0 - by,
            x1: self.x1 + by,
            y1: self.y1 + by,
        };
        if r.is_empty() { IRect::EMPTY } else { r }
    }

    /// Rect covering the two points, normalised so the corners are ordered.
    pub fn from_points(ax: i32, ay: i32, bx: i32, by: i32) -> IRect {
        IRect {
            x0: ax.min(bx),
            y0: ay.min(by),
            x1: ax.max(bx),
            y1: ay.max(by),
        }
    }
}

/// Floating-point 2D point, used for tool interaction and transforms.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    #[inline]
    pub fn distance(self, other: Vec2) -> f32 {
        (self - other).length()
    }

    #[inline]
    pub fn lerp(self, other: Vec2, t: f32) -> Vec2 {
        Vec2::new(self.x + (other.x - self.x) * t, self.y + (other.y - self.y) * t)
    }

    /// Angle from the positive x axis, in radians.
    #[inline]
    pub fn angle(self) -> f32 {
        self.y.atan2(self.x)
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f32> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn mul(self, s: f32) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_ignores_empty() {
        let a = IRect::at(10, 10, 5, 5);
        assert_eq!(IRect::EMPTY.union(&a), a);
        assert_eq!(a.union(&IRect::EMPTY), a);
    }

    #[test]
    fn intersect_disjoint_is_empty() {
        let a = IRect::at(0, 0, 4, 4);
        let b = IRect::at(10, 10, 4, 4);
        assert!(a.intersect(&b).is_empty());
        assert!(!a.intersects(&b));
    }

    #[test]
    fn size_accessors_saturate() {
        let inverted = IRect::new(10, 10, 0, 0);
        assert_eq!(inverted.width(), 0);
        assert_eq!(inverted.height(), 0);
        assert_eq!(inverted.area(), 0);
    }

    #[test]
    fn inflate_negative_can_empty() {
        assert!(IRect::at(0, 0, 4, 4).inflate(-3).is_empty());
        assert_eq!(IRect::at(0, 0, 4, 4).inflate(1), IRect::new(-1, -1, 5, 5));
    }
}
