//! Lens correction: undoing what the camera did to the picture.
//!
//! Four corrections that all belong together, because they are all about the
//! geometry of one photograph rather than about its colours:
//!
//! * **Distortion** — a lens bows straight lines. Wide angles bulge them
//!   outward (barrel), long ones pinch them inward (pincushion).
//! * **Perspective** — pointing a camera up at a building makes its sides
//!   lean together. Correcting that is a keystone: stretching one edge until
//!   the verticals are vertical again.
//! * **Rotation** — the horizon was not level.
//! * **Vignette** — the corners are darker than the middle, or, when someone
//!   wants the look rather than the correction, deliberately more so.
//!
//! # One pass, not four
//!
//! The three geometric corrections are composed into a single backward map and
//! sampled **once**. Applying them in sequence would be simpler to write and
//! visibly worse: every resampling pass costs a little sharpness, so a picture
//! straightened, then unbent, then de-keystoned comes out softer than one that
//! had all three done at the same moment. The vignette is not geometry at all
//! — it is a multiply — so it rides along in the same pass for free.
//!
//! # Which way is out
//!
//! Everything works in a square, isotropic space normalised so that the
//! **corner of the image is at radius 1**, whatever its aspect. That makes
//! rotation a real rotation rather than a shear, and makes the distortion
//! parameter mean the same thing on a portrait as on a landscape.

use crate::filters::plane::{fill_rows, Plane};
use crate::progress::Progress;

/// The corrections, as a set of numbers that a script or a window can carry.
///
/// All default to nothing, so a fresh one is the identity and
/// [`Lens::is_identity`] says so — worth asking before spending a pass over a
/// large image to change nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lens {
    /// Bend straight lines back. Positive pushes the picture outward, which is
    /// what corrects the barrel of a wide-angle lens; negative pulls it in and
    /// corrects pincushion. Roughly −1 to 1, though the useful range is small.
    pub distortion: f32,
    /// Keystone. `horizontal` leans the picture left and right, `vertical`
    /// leans it top and bottom — the correction for a camera that was not
    /// square on to what it was pointed at.
    pub perspective_h: f32,
    pub perspective_v: f32,
    /// Degrees, anticlockwise. For a horizon that was not level.
    pub rotation: f32,
    /// Darken the corners below zero, brighten them above. ±1.
    pub vignette: f32,
    /// Where the vignette begins, as a fraction of the way to the corner.
    /// Everything inside it is untouched.
    pub vignette_midpoint: f32,
    /// Scale about the centre, applied last. A way to push the empty corners
    /// out of frame by hand instead of cropping them away.
    pub scale: f32,
}

impl Default for Lens {
    fn default() -> Self {
        Lens {
            distortion: 0.0,
            perspective_h: 0.0,
            perspective_v: 0.0,
            rotation: 0.0,
            vignette: 0.0,
            vignette_midpoint: 0.5,
            scale: 1.0,
        }
    }
}

impl Lens {
    /// True when applying this would return the picture unchanged.
    pub fn is_identity(&self) -> bool {
        self.distortion == 0.0
            && self.perspective_h == 0.0
            && self.perspective_v == 0.0
            && self.rotation == 0.0
            && self.vignette == 0.0
            && (self.scale - 1.0).abs() < f32::EPSILON
    }

    /// True when the geometry can leave parts of the frame empty, and so when
    /// there is anything for a crop to do.
    pub fn moves_pixels(&self) -> bool {
        self.distortion != 0.0
            || self.perspective_h != 0.0
            || self.perspective_v != 0.0
            || self.rotation != 0.0
            || (self.scale - 1.0).abs() >= f32::EPSILON
    }
}

/// The mapping from a destination pixel back to where it came from.
///
/// Built once per image rather than per pixel: the trigonometry and the
/// normalising factors do not change across the frame.
struct Map {
    cx: f32,
    cy: f32,
    /// Half the diagonal: the radius at which a corner sits.
    unit: f32,
    sin: f32,
    cos: f32,
    lens: Lens,
}

impl Map {
    fn new(width: u32, height: u32, lens: Lens) -> Map {
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let (cx, cy) = (w / 2.0, h / 2.0);
        let unit = (cx * cx + cy * cy).sqrt().max(1.0);
        let theta = lens.rotation.to_radians();
        Map { cx, cy, unit, sin: theta.sin(), cos: theta.cos(), lens }
    }

    /// Destination pixel centre to source pixel centre.
    ///
    /// The order is the order the camera did things, run backwards from the
    /// picture we want to the picture we have: undo the levelling, then the
    /// keystone, and only then bend the light the way the lens bent it.
    #[inline]
    fn source_of(&self, dx: f32, dy: f32) -> (f32, f32) {
        // Into the square, isotropic space where a corner is at radius 1.
        let mut x = (dx - self.cx) / self.unit;
        let mut y = (dy - self.cy) / self.unit;

        // Scale, about the centre.
        if self.lens.scale != 1.0 {
            let s = 1.0 / self.lens.scale.max(0.01);
            x *= s;
            y *= s;
        }

        // Rotation. A destination that has been levelled came from a source
        // that was tilted, so this turns the other way.
        if self.lens.rotation != 0.0 {
            let (rx, ry) = (x * self.cos - y * self.sin, x * self.sin + y * self.cos);
            x = rx;
            y = ry;
        }

        // Keystone. The plain projective form: one divide, and the centre
        // stays put because w is 1 there.
        let w = 1.0 + self.lens.perspective_h * x + self.lens.perspective_v * y;
        if w.abs() > 1e-4 {
            x /= w;
            y /= w;
        }

        // Distortion, last, because it is what the lens did first. A positive
        // amount makes the source radius grow faster than the destination's,
        // so the edges of the frame reach further out into the picture and
        // what was bulging is pushed back flat.
        if self.lens.distortion != 0.0 {
            let r2 = x * x + y * y;
            let k = 1.0 + self.lens.distortion * r2;
            x *= k;
            y *= k;
        }

        (x * self.unit + self.cx, y * self.unit + self.cy)
    }

    /// The vignette multiplier at a destination pixel.
    ///
    /// Measured in the destination rather than the source, because a vignette
    /// is a property of the picture being made, not of where its pixels came
    /// from — and because measuring it in the source would make it move about
    /// as the geometry changed.
    #[inline]
    fn vignette_at(&self, dx: f32, dy: f32) -> f32 {
        if self.lens.vignette == 0.0 {
            return 1.0;
        }
        let x = (dx - self.cx) / self.unit;
        let y = (dy - self.cy) / self.unit;
        let r = (x * x + y * y).sqrt();
        let mid = self.lens.vignette_midpoint.clamp(0.0, 0.99);
        if r <= mid {
            return 1.0;
        }
        // Smoothstep from the midpoint out to the corner, so the vignette has
        // no visible ring where it begins.
        let t = ((r - mid) / (1.0 - mid)).clamp(0.0, 1.0);
        let f = t * t * (3.0 - 2.0 * t);
        (1.0 + self.lens.vignette * f).max(0.0)
    }
}

/// Apply the corrections, in one pass.
///
/// Anything the map sends outside the source comes back empty, which is what
/// leaves the corners transparent after a rotation — and what
/// [`largest_opaque_rect`] is for.
pub fn apply(src: &Plane, lens: Lens, progress: &Progress) -> Plane {
    let map = Map::new(src.width, src.height, lens);
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;

    progress.begin("Lens Correction", src.height as u64);
    fill_rows(&mut out, progress, |y, row| {
        let dy = y as f32 + 0.5;
        for x in 0..width {
            let dx = x as f32 + 0.5;
            let (sx, sy) = map.source_of(dx, dy);
            let mut p = sample_or_empty(src, sx, sy);
            let v = map.vignette_at(dx, dy);
            if v != 1.0 {
                // Premultiplied, so scaling the colour and leaving alpha is
                // exactly "darker, just as opaque". A vignette dims a picture;
                // it does not make holes in it.
                for c in p.iter_mut().take(3) {
                    *c *= v;
                }
            }
            let i = x * 4;
            row[i..i + 4].copy_from_slice(&p);
        }
    });
    out
}

/// Bilinear sample that returns nothing outside the image.
///
/// [`Plane::sample`] clamps to the edge, which is right for a blur and wrong
/// here: clamping would smear the edge pixels outward into the empty corners
/// instead of leaving them empty, and there would be nothing to crop away.
#[inline]
fn sample_or_empty(src: &Plane, x: f32, y: f32) -> [f32; 4] {
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor() as i32, fy.floor() as i32);
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let mut out = [0.0f32; 4];
    for (dy, wy) in [(0, 1.0 - ty), (1, ty)] {
        for (dx, wx) in [(0, 1.0 - tx), (1, tx)] {
            let w = wx * wy;
            if w == 0.0 {
                continue;
            }
            let (px, py) = (x0 + dx, y0 + dy);
            if px < 0 || py < 0 || px >= src.width as i32 || py >= src.height as i32 {
                continue;
            }
            let p = src.get(px, py);
            for c in 0..4 {
                out[c] += p[c] * w;
            }
        }
    }
    out
}

// --- autocrop --------------------------------------------------------------

/// The largest axis-aligned rectangle containing no transparent pixel.
///
/// After a rotation or a keystone the picture is a quadrilateral with empty
/// corners, and after a distortion its edges are curved. Rather than reason
/// about which shape it is, this reads the alpha it actually got and finds the
/// biggest rectangle inside it — the classic largest-rectangle-under-a-
/// histogram sweep, one pass down the image, linear in its size.
///
/// It is exact for whatever shape the corrections happened to leave, including
/// shapes no formula would have predicted, such as the ones a strong barrel
/// correction makes along the middle of each edge.
///
/// Returns an empty rect when every pixel is transparent, which is the honest
/// answer to "crop this to nothing".
pub fn largest_opaque_rect(plane: &Plane) -> crate::geom::IRect {
    let (w, h) = (plane.width as usize, plane.height as usize);
    if w == 0 || h == 0 {
        return crate::geom::IRect::EMPTY;
    }
    // A pixel counts as solid only if it is fully opaque. A half-covered edge
    // pixel is exactly what a crop is meant to remove.
    let solid = |x: usize, y: usize| plane.data[(y * w + x) * 4 + 3] >= 0.999;

    // Heights of the run of solid pixels ending at each column of this row.
    let mut heights = vec![0u32; w];
    let mut best = crate::geom::IRect::EMPTY;
    let mut best_area = 0u64;

    // Reused across rows so the sweep allocates once.
    let mut stack: Vec<(usize, u32)> = Vec::with_capacity(w + 1);

    for y in 0..h {
        for (x, hgt) in heights.iter_mut().enumerate() {
            *hgt = if solid(x, y) { *hgt + 1 } else { 0 };
        }

        // Widest rectangle whose height is limited by each bar, found by
        // keeping the stack of bars that are still growing.
        stack.clear();
        // One extra step past the end, with a height of zero, to flush
        // whatever is still on the stack when the row runs out.
        let bars = heights.iter().copied().chain(std::iter::once(0)).enumerate();
        for (x, hgt) in bars {
            let mut start = x;
            while let Some(&(sx, sh)) = stack.last() {
                if sh <= hgt {
                    break;
                }
                stack.pop();
                let area = sh as u64 * (x - sx) as u64;
                if area > best_area {
                    best_area = area;
                    best = crate::geom::IRect::new(
                        sx as i32,
                        (y + 1 - sh as usize) as i32,
                        x as i32,
                        (y + 1) as i32,
                    );
                }
                start = sx;
            }
            if hgt > 0 {
                stack.push((start, hgt));
            }
        }
    }
    best
}
