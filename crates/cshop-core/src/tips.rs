//! The shapes a brush stamps.
//!
//! A brush whose dab is a disc draws a line. A brush whose dab is a *shape*,
//! scattered off the line and turned a different way each time, draws foliage,
//! spray, fur, a night sky. The difference is not in the stroke machinery —
//! [`crate::paint::Stroke`] stamps whatever it is given — but in having
//! something worth stamping, which is what this module is.
//!
//! Each shape is drawn once into a square coverage mask and then reused for
//! every dab of every stroke, so the cost of drawing one properly is paid once
//! at startup rather than thousands of times a second. They are drawn by
//! supersampling a containment test rather than by rasterising outlines: the
//! shapes are simple enough that asking "is this point inside" sixteen times a
//! pixel is both shorter and smoother than the alternative.

use crate::mask::MaskBuffer;
use crate::paint::Tip;

/// How many coverage levels the edge of a shape gets: 4×4 subsamples a pixel.
const SUB: u32 = 4;

/// The side of the square each shape is drawn into.
///
/// Large enough that a stamp scaled up to a few hundred pixels still has an
/// edge rather than a staircase, small enough that holding all of them costs
/// less than a single small photograph.
pub const TIP_RESOLUTION: u32 = 128;

/// A shape the brush can stamp, drawn from a formula rather than a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TipShape {
    /// A plain filled circle: spray, confetti, a starfield.
    Dot,
    /// A ring with a highlight, which reads as a soap bubble.
    Bubble,
    /// Five points. Sparkle, glitter, a child's night sky.
    Star,
    /// A capsule lying along the x axis, so that turning it with the stroke
    /// gives hatching, fur and grass.
    Line,
}

impl TipShape {
    /// Every shape, in the order they are offered.
    pub const ALL: [TipShape; 4] = [TipShape::Dot, TipShape::Bubble, TipShape::Star, TipShape::Line];

    pub fn name(self) -> &'static str {
        match self {
            TipShape::Dot => "Dot",
            TipShape::Bubble => "Bubble",
            TipShape::Star => "Star",
            TipShape::Line => "Line",
        }
    }

    /// Whether turning this shape changes what it looks like.
    ///
    /// A dot is a dot at every angle, so offering an angle control for one is
    /// offering a control that does nothing.
    pub fn has_direction(self) -> bool {
        !matches!(self, TipShape::Dot | TipShape::Bubble)
    }

    /// Is the point inside the shape? `u` and `v` run `-1..=1` across the box.
    fn contains(self, u: f32, v: f32) -> bool {
        match self {
            TipShape::Dot => u * u + v * v <= 0.92 * 0.92,
            TipShape::Bubble => {
                let r2 = u * u + v * v;
                let wall = (0.74 * 0.74..=0.94 * 0.94).contains(&r2);
                // A highlight up and to the left, where a light usually is.
                let (hu, hv) = (u + 0.40, v + 0.40);
                let highlight = hu * hu + hv * hv <= 0.15 * 0.15;
                wall || highlight
            }
            TipShape::Star => star_contains(u, v),
            TipShape::Line => {
                // A capsule: the distance to a horizontal segment.
                let x = u.abs().max(0.80) - 0.80;
                let dx = if u.abs() > 0.80 { x } else { 0.0 };
                dx * dx + v * v <= 0.15 * 0.15
            }
        }
    }

    /// Draw the shape into a square coverage mask.
    pub fn render(self, size: u32) -> MaskBuffer {
        let size = size.clamp(8, 1024);
        let mut m = MaskBuffer::hide_all(size, size);
        let half = size as f32 / 2.0;
        let step = 1.0 / SUB as f32;
        for y in 0..size {
            for x in 0..size {
                let mut hits = 0u32;
                for sy in 0..SUB {
                    for sx in 0..SUB {
                        // The centre of this subsample, in -1..=1.
                        let px = x as f32 + (sx as f32 + 0.5) * step;
                        let py = y as f32 + (sy as f32 + 0.5) * step;
                        let u = (px - half) / half;
                        let v = (py - half) / half;
                        if self.contains(u, v) {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    let v = hits * 255 / (SUB * SUB);
                    m.set(x as i32, y as i32, v as u8);
                }
            }
        }
        m
    }

    /// The shape as a brush tip, ready to stamp.
    pub fn tip(self) -> Tip {
        // Every shape covers pixels by construction, so `Tip::new` cannot fail
        // here; if one ever did, an empty stamp would silently paint nothing,
        // which is harder to notice than a panic in a test.
        Tip::new(self.render(TIP_RESOLUTION)).expect("a built-in shape always covers something")
    }
}

/// A five-pointed star, point upwards, by the crossing-number rule.
fn star_contains(u: f32, v: f32) -> bool {
    const OUTER: f32 = 0.97;
    const INNER: f32 = 0.42;
    // Ten vertices, alternating out and in, starting at the top. Screen y runs
    // downwards, so the first point is at negative v.
    let mut poly = [(0.0f32, 0.0f32); 10];
    for (i, p) in poly.iter_mut().enumerate() {
        let r = if i % 2 == 0 { OUTER } else { INNER };
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        *p = (r * a.cos(), r * a.sin());
    }
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > v) != (yj > v) && u < (xj - xi) * (v - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}
