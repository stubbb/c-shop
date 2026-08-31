//! The healing brush: texture from one place, colour and brightness from
//! another.
//!
//! # What it does that the clone stamp cannot
//!
//! Cloning copies pixels. That is right for repeating a detail and wrong for
//! repairing one, because the copied pixels bring their own brightness with
//! them and leave a disc of slightly-wrong tone wherever the source and the
//! destination did not happen to match. On skin, on a wall, on a sky — on
//! anything with a gradient across it — the repair ends up more conspicuous
//! than the blemish was.
//!
//! Healing keeps the source's texture and throws away its tone, replacing it
//! with a correction that makes the repair meet the picture exactly at its own
//! edge:
//!
//! ```text
//! healed = source + correction,   correction fitted where the repair meets the picture
//! ```
//!
//! # Why the correction is fitted at the boundary
//!
//! The obvious formulation — take the smooth part of the destination and the
//! detailed part of the source — does not work, and it is worth saying why,
//! because it looks right. The smooth part of the destination is a blur of it,
//! and a blur taken *over the blemish* is dragged toward the blemish's own
//! colour. The repair then faithfully reproduces a fraction of the mark it was
//! asked to remove. Measured on a dark spot laid on a gradient, healing that
//! way lands within a percent of what plain cloning does: no better at all.
//!
//! So the correction is measured only on a ring just outside the dab, where
//! the destination is still the picture and not the damage, and carried across
//! the inside as the plane that best fits that ring. A plane, rather than a
//! constant, because the case cloning fails at *is* a gradient: a constant
//! offset would match the average and still tilt the wrong way.
//!
//! # Why it computes as it goes
//!
//! Everything here is per dab, over that dab's own rectangle. Blurring a whole
//! layer to begin a stroke costs a quarter of a second at twelve megapixels
//! and over half at twenty-four — a stall on every mouse-down, for a tool used
//! in short strokes on small areas. A dab costs about half a millisecond on
//! the same layer, because it does work proportional to the dab.

use crate::color::{Rgba, Rgba8};
use crate::geom::IRect;
use crate::pixels::PixelBuffer;

/// How far outside a dab the correction is measured, as a fraction of the
/// dab's own size. Far enough out to be past the damage, close enough in that
/// the picture there is still the picture here.
const RING: f32 = 0.15;

/// A healing brush's source, and the healed pixels worked out so far.
#[derive(Debug, Clone)]
pub struct Heal {
    /// Where the texture comes from, in layer space — `None` when that is the
    /// layer itself, which is the usual case and saves holding a second copy
    /// of a photograph.
    source: Option<PixelBuffer>,
    /// Added to a destination pixel to find its source.
    offset: (i32, i32),
    /// The layer as it was when the stroke began. Frozen, so a stroke reads
    /// the picture rather than its own output, and the ring it fits against
    /// stays the picture even after the middle has been repaired.
    dest: PixelBuffer,
    /// Healed pixels, filled in where the brush has been.
    healed: PixelBuffer,
}

impl Heal {
    /// Heal from a source at a fixed offset — the ordinary healing brush,
    /// with the source set the way the clone stamp's is.
    pub fn new(dest: PixelBuffer, source: PixelBuffer, offset: (i32, i32)) -> Heal {
        let healed = PixelBuffer::new(dest.width(), dest.height());
        Heal { source: Some(source), offset, dest, healed }
    }

    /// The same, taking the texture from the layer being repaired — which is
    /// what a healing brush usually does.
    pub fn within(dest: PixelBuffer, offset: (i32, i32)) -> Heal {
        let healed = PixelBuffer::new(dest.width(), dest.height());
        Heal { source: None, offset, dest, healed }
    }

    #[inline]
    fn source(&self) -> &PixelBuffer {
        self.source.as_ref().unwrap_or(&self.dest)
    }

    /// Heal from somewhere nearby, chosen by looking — the spot form, which
    /// takes no source because it finds one.
    ///
    /// The only thing a source contributes is texture, so the search asks
    /// which nearby patch's surroundings most resemble this one's. A donor
    /// that already nearly matches needs the least correction, and a small
    /// correction is one that cannot go visibly wrong. Candidates sit on rings
    /// around the spot, far enough out not to overlap it.
    pub fn spot(dest: PixelBuffer, at: (i32, i32), brush_radius: f32) -> Heal {
        let step = (brush_radius * 1.6).max(6.0);
        let mut best: Option<((i32, i32), f32)> = None;
        for ring in 1..=3 {
            let d = step * ring as f32;
            for k in 0..12 {
                let a = std::f32::consts::TAU * k as f32 / 12.0;
                let off = ((d * a.cos()).round() as i32, (d * a.sin()).round() as i32);
                if let Some(score) = donor_score(&dest, at, off, brush_radius) {
                    if best.is_none_or(|(_, b)| score < b) {
                        best = Some((off, score));
                    }
                }
            }
        }
        // Nothing scored: the picture is smaller than the search. One brush
        // width to the left is at least not the spot itself.
        let offset = best.map_or((-(step as i32), 0), |(o, _)| o);
        Heal::within(dest, offset)
    }

    /// Where this takes its texture from, relative to the pixel being healed.
    pub fn offset(&self) -> (i32, i32) {
        self.offset
    }

    /// Work out the healed colour across `rect`, before it is painted.
    ///
    /// Idempotent: a dab overlapping an earlier one computes the same answer
    /// again, because both pictures it reads are frozen.
    pub fn prepare(&mut self, rect: IRect) {
        let rect = rect.intersect(&self.dest.bounds());
        if rect.is_empty() {
            return;
        }
        let cx = (rect.x0 + rect.x1) as f32 / 2.0;
        let cy = (rect.y0 + rect.y1) as f32 / 2.0;
        let Some(fit) = self.fit_ring(rect, cx, cy) else {
            // No usable ring — the dab is against an edge, or everything round
            // it is transparent. Leave the picture alone rather than guess.
            return;
        };

        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let src = self.source().get(x + self.offset.0, y + self.offset.1);
                let here = self.dest.get(x, y).to_f32();
                let c = if src.a < 128 {
                    // Nothing to take texture from; the correction alone is
                    // the best guess at what belongs here.
                    fit.at(x as f32 - cx, y as f32 - cy, Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 })
                } else {
                    fit.at(x as f32 - cx, y as f32 - cy, src.to_f32())
                };
                // Repairing must not punch a hole in the layer or fill one in.
                self.healed.set(x, y, Rgba { a: here.a, ..c }.to_u8());
            }
        }
    }

    /// The healed colour at a layer pixel. Only meaningful after `prepare`.
    #[inline]
    pub fn at(&self, x: i32, y: i32) -> Rgba8 {
        self.healed.get(x, y)
    }

    /// Least-squares plane through the destination-minus-source difference,
    /// measured on a ring just outside `rect`.
    fn fit_ring(&self, rect: IRect, cx: f32, cy: f32) -> Option<Plane> {
        let (rx, ry) = (rect.width() as f32 / 2.0, rect.height() as f32 / 2.0);
        let (ox, oy) = (rx * (1.0 + RING), ry * (1.0 + RING));
        let bounds = self.dest.bounds();

        // Sums for the 3x3 normal equations of `z = a + b*u + c*v`, one set
        // per channel but sharing the geometry.
        let (mut n, mut su, mut sv, mut suu, mut svv, mut suv) = (0.0f32, 0.0, 0.0, 0.0, 0.0, 0.0);
        let mut sz = [0.0f32; 3];
        let mut szu = [0.0f32; 3];
        let mut szv = [0.0f32; 3];

        for k in 0..48 {
            let a = std::f32::consts::TAU * k as f32 / 48.0;
            let (x, y) = ((cx + ox * a.cos()).round() as i32, (cy + oy * a.sin()).round() as i32);
            if !bounds.contains(x, y) {
                continue;
            }
            let d = self.dest.get(x, y);
            let s = self.source().get(x + self.offset.0, y + self.offset.1);
            if d.a < 128 || s.a < 128 {
                continue;
            }
            let (d, s) = (d.to_f32(), s.to_f32());
            let (u, v) = (x as f32 - cx, y as f32 - cy);
            n += 1.0;
            su += u;
            sv += v;
            suu += u * u;
            svv += v * v;
            suv += u * v;
            for (i, z) in [d.r - s.r, d.g - s.g, d.b - s.b].into_iter().enumerate() {
                sz[i] += z;
                szu[i] += z * u;
                szv[i] += z * v;
            }
        }
        if n < 6.0 {
            return None;
        }

        let mut plane = Plane::default();
        for i in 0..3 {
            let m = [[n, su, sv], [su, suu, suv], [sv, suv, svv]];
            let rhs = [sz[i], szu[i], szv[i]];
            let (a, b, c) = solve3(m, rhs).unwrap_or((sz[i] / n, 0.0, 0.0));
            plane.a[i] = a;
            plane.b[i] = b;
            plane.c[i] = c;
        }
        Some(plane)
    }
}

/// The fitted correction: a constant and a slope in each direction, per
/// channel.
#[derive(Debug, Default, Clone, Copy)]
struct Plane {
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
}

impl Plane {
    #[inline]
    fn at(&self, u: f32, v: f32, base: Rgba) -> Rgba {
        let f = |i: usize, x: f32| (x + self.a[i] + self.b[i] * u + self.c[i] * v).clamp(0.0, 1.0);
        Rgba { r: f(0, base.r), g: f(1, base.g), b: f(2, base.b), a: base.a }
    }
}

/// Gaussian elimination on a symmetric 3x3. `None` when it is singular, which
/// happens when every sample sits on one line.
fn solve3(mut m: [[f32; 3]; 3], mut r: [f32; 3]) -> Option<(f32, f32, f32)> {
    for i in 0..3 {
        let pivot = (i..3).max_by(|&a, &b| m[a][i].abs().total_cmp(&m[b][i].abs()))?;
        if m[pivot][i].abs() < 1e-6 {
            return None;
        }
        m.swap(i, pivot);
        r.swap(i, pivot);
        let (row, above) = (m[i], r[i]);
        for (k, below) in m.iter_mut().enumerate().skip(i + 1) {
            let f = below[i] / row[i];
            for (dst, src) in below.iter_mut().zip(row).skip(i) {
                *dst -= f * src;
            }
            r[k] -= f * above;
        }
    }
    let mut x = [0.0f32; 3];
    for i in (0..3).rev() {
        let mut s = r[i];
        for j in i + 1..3 {
            s -= m[i][j] * x[j];
        }
        x[i] = s / m[i][i];
    }
    Some((x[0], x[1], x[2]))
}

/// How badly a donor at `offset` matches the neighbourhood around `at`.
///
/// Lower is better. `None` when the donor falls outside the picture, which is
/// what keeps the search from choosing the empty space past an edge.
fn donor_score(px: &PixelBuffer, at: (i32, i32), offset: (i32, i32), radius: f32) -> Option<f32> {
    let r = radius.round().max(1.0) as i32;
    let (dx, dy) = (at.0 + offset.0, at.1 + offset.1);
    let bounds = px.bounds();
    if !bounds.contains(dx - r, dy - r) || !bounds.contains(dx + r, dy + r) {
        return None;
    }
    // Compare a ring rather than a disc: the middle of the destination is the
    // blemish, and matching a blemish is the opposite of the point.
    let (mut total, mut n) = (0.0, 0.0);
    for k in 0..16 {
        let a = std::f32::consts::TAU * k as f32 / 16.0;
        let (ox, oy) = ((r as f32 * 1.2 * a.cos()) as i32, (r as f32 * 1.2 * a.sin()) as i32);
        let here = px.get(at.0 + ox, at.1 + oy).to_f32();
        let there = px.get(dx + ox, dy + oy).to_f32();
        if here.a < 0.5 || there.a < 0.5 {
            continue;
        }
        total += (here.r - there.r).abs() + (here.g - there.g).abs() + (here.b - there.b).abs();
        n += 1.0;
    }
    (n > 0.0).then(|| total / n)
}
