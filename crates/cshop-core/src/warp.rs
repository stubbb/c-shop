//! Bending the middle of a layer, not just its corners.
//!
//! Free Transform moves four corners, and everything between them follows a
//! single projective map. That covers scaling, rotating, skewing and putting a
//! photograph on a wall, and covers nothing that bends: an arm that should
//! reach further, a label that should follow a bottle, a line of type that
//! should curve.
//!
//! # One engine, two tools
//!
//! A warp is a set of control points and where they have been moved to.
//! Everything else follows. That is true whether the points are the corners of
//! a grid laid over the layer — a *warp*, dragged by its mesh — or a handful
//! of pins put wherever they are wanted — a *puppet warp*. The difference is
//! entirely in where the points come from, so there is one implementation
//! here and two ways of collecting the input.
//!
//! # Moving least squares
//!
//! For each pixel, weight every control point by how near it is, and find the
//! transform that best fits the moved points under those weights. Near a pin
//! its own weight dominates and the pixel goes where the pin went; far from
//! every pin the weights even out and the region moves as a whole; in between
//! the two blend smoothly, which is what makes the result look like a bend
//! rather than a set of dents.
//!
//! The fit can be *affine*, which is free to stretch and shear, or *rigid*,
//! which is only allowed to rotate and translate. Rigid is what makes an arm
//! look like an arm after it has been moved: an affine fit will happily
//! squash it to reach the new position, because squashing costs it nothing.
//!
//! Following Schaefer, McPhail and Warren (2006).

use crate::geom::{IRect, Vec2};
use crate::pixels::PixelBuffer;
use crate::resample::Resampling;

/// How far a warp may grow the layer, so a pin flung across the screen asks
/// for a large picture rather than an impossible one.
const MAX_SIDE: i32 = 20_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Warp {
    /// Where the control points were.
    pub from: Vec<Vec2>,
    /// Where they are now. Same length as `from`.
    pub to: Vec<Vec2>,
    /// How local a pin's influence is. Larger keeps the effect closer to each
    /// pin and leaves more of the layer alone.
    pub falloff: f32,
    /// Rigid keeps shapes; affine lets them stretch to fit.
    pub rigid: bool,
}

impl Default for Warp {
    fn default() -> Self {
        Self { from: Vec::new(), to: Vec::new(), falloff: 1.0, rigid: true }
    }
}

impl Warp {
    /// A grid of control points over `rect`, none of them moved yet.
    pub fn grid(rect: IRect, cols: u32, rows: u32) -> Warp {
        let (cols, rows) = (cols.max(2), rows.max(2));
        let mut from = Vec::with_capacity((cols * rows) as usize);
        for j in 0..rows {
            for i in 0..cols {
                from.push(Vec2::new(
                    rect.x0 as f32 + rect.width() as f32 * i as f32 / (cols - 1) as f32,
                    rect.y0 as f32 + rect.height() as f32 * j as f32 / (rows - 1) as f32,
                ));
            }
        }
        let to = from.clone();
        Warp { from, to, falloff: 1.0, rigid: false }
    }

    /// Add a pin where it is, moving nothing.
    pub fn pin(&mut self, at: Vec2) {
        self.from.push(at);
        self.to.push(at);
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.from.len() {
            self.from.remove(index);
            self.to.remove(index);
        }
    }

    /// Whether anything has actually been moved.
    pub fn any(&self) -> bool {
        self.from.len() == self.to.len()
            && self.from.iter().zip(&self.to).any(|(a, b)| a.distance(*b) > 0.01)
    }

    /// The nearest control point to `at`, within `reach`.
    pub fn nearest(&self, at: Vec2, reach: f32) -> Option<usize> {
        self.to
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.distance(at)))
            .filter(|(_, d)| *d <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Where a point in the original lands.
    pub fn map(&self, v: Vec2) -> Vec2 {
        mls(&self.from, &self.to, v, self.falloff, self.rigid)
    }

    /// Where a point in the result came from.
    ///
    /// The same computation with the two sets of points exchanged. Rendering
    /// needs this direction and not the other: an output pixel has to know
    /// where to *sample*, and inverting a warp numerically would be both
    /// slower and worse behaved than simply asking the question the other way
    /// round.
    pub fn unmap(&self, v: Vec2) -> Vec2 {
        mls(&self.to, &self.from, v, self.falloff, self.rigid)
    }

    /// Render `src`, whose top-left is at `offset` in the warp's coordinates.
    ///
    /// Returns the warped pixels and where their top-left now sits, or `None`
    /// when the warp collapses the layer to nothing.
    pub fn apply(
        &self,
        src: &PixelBuffer,
        offset: (i32, i32),
        filter: Resampling,
        clip: Option<IRect>,
    ) -> Option<(PixelBuffer, (i32, i32))> {
        if self.from.len() != self.to.len() || self.from.is_empty() {
            return None;
        }
        let source = IRect::at(offset.0, offset.1, src.width(), src.height());
        let mut bounds = self.mapped_bounds(source);
        if let Some(clip) = clip {
            bounds = bounds.intersect(&clip);
        }
        if bounds.is_empty() || bounds.width() as i32 > MAX_SIDE || bounds.height() as i32 > MAX_SIDE
        {
            return None;
        }

        use rayon::prelude::*;
        let (w, h) = (bounds.width(), bounds.height());
        let rows: Vec<Vec<crate::color::Rgba8>> = (0..h)
            .into_par_iter()
            .map(|y| {
                (0..w)
                    .map(|x| {
                        let at = Vec2::new(
                            (bounds.x0 + x as i32) as f32 + 0.5,
                            (bounds.y0 + y as i32) as f32 + 0.5,
                        );
                        let s = self.unmap(at);
                        crate::resample::sample_at(
                            src,
                            s.x - offset.0 as f32,
                            s.y - offset.1 as f32,
                            filter,
                        )
                    })
                    .collect()
            })
            .collect();
        let data: Vec<crate::color::Rgba8> = rows.into_iter().flatten().collect();
        PixelBuffer::from_pixels(w, h, data).map(|px| (px, (bounds.x0, bounds.y0)))
    }

    /// Where the layer ends up, found by walking its edge rather than its four
    /// corners: a warp bends, so the furthest point of a warped rectangle is
    /// very often in the middle of a side.
    fn mapped_bounds(&self, rect: IRect) -> IRect {
        let steps = 24;
        let mut min = Vec2::new(f32::MAX, f32::MAX);
        let mut max = Vec2::new(f32::MIN, f32::MIN);
        let mut see = |p: Vec2| {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        };
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let x = rect.x0 as f32 + rect.width() as f32 * t;
            let y = rect.y0 as f32 + rect.height() as f32 * t;
            see(self.map(Vec2::new(x, rect.y0 as f32)));
            see(self.map(Vec2::new(x, rect.y1 as f32)));
            see(self.map(Vec2::new(rect.x0 as f32, y)));
            see(self.map(Vec2::new(rect.x1 as f32, y)));
        }
        if !min.x.is_finite() || !max.x.is_finite() {
            return IRect::EMPTY;
        }
        // A margin, because the edge walk samples the boundary and a bend
        // between two samples can bulge a little past both.
        IRect::new(
            min.x.floor() as i32 - 2,
            min.y.floor() as i32 - 2,
            max.x.ceil() as i32 + 2,
            max.y.ceil() as i32 + 2,
        )
    }
}

/// Moving least squares: where `v` goes, given that `from` went to `to`.
fn mls(from: &[Vec2], to: &[Vec2], v: Vec2, falloff: f32, rigid: bool) -> Vec2 {
    let n = from.len();
    if n == 0 || n != to.len() {
        return v;
    }
    let alpha = falloff.clamp(0.1, 8.0);

    let mut weights = Vec::with_capacity(n);
    let mut total = 0.0f32;
    for (i, p) in from.iter().enumerate() {
        let d2 = (p.x - v.x).powi(2) + (p.y - v.y).powi(2);
        // Sitting exactly on a control point: the answer is where that point
        // went, and the weight would be infinite.
        if d2 < 1e-8 {
            return to[i];
        }
        let w = 1.0 / d2.powf(alpha);
        weights.push(w);
        total += w;
    }
    if !total.is_finite() || total <= 0.0 {
        return v;
    }

    let centroid = |pts: &[Vec2]| {
        let mut c = Vec2::ZERO;
        for (p, w) in pts.iter().zip(&weights) {
            c.x += p.x * w;
            c.y += p.y * w;
        }
        Vec2::new(c.x / total, c.y / total)
    };
    let (pstar, qstar) = (centroid(from), centroid(to));
    let r = Vec2::new(v.x - pstar.x, v.y - pstar.y);

    if rigid {
        // The similarity fit, then its rotation taken on its own: the
        // magnitude is thrown away and replaced by how far `v` was from the
        // centroid, which is what stops the fit from stretching to reach.
        let mut acc = Vec2::ZERO;
        for i in 0..n {
            let p = Vec2::new(from[i].x - pstar.x, from[i].y - pstar.y);
            let q = Vec2::new(to[i].x - qstar.x, to[i].y - qstar.y);
            // The paper's A_i is [p̂ ; -p̂⊥] times [r ; -r⊥] transposed, which
            // comes out as the rotation-shaped [[d, -e], [e, d]] — not
            // [[d, e], [e, -d]], which is a reflection and turns the identity
            // warp into a flip about the horizontal.
            let d = p.x * r.x + p.y * r.y;
            let e = p.y * r.x - p.x * r.y;
            acc.x += weights[i] * (q.x * d + q.y * e);
            acc.y += weights[i] * (q.y * d - q.x * e);
        }
        let len = (acc.x * acc.x + acc.y * acc.y).sqrt();
        if len < 1e-9 {
            return qstar;
        }
        let scale = (r.x * r.x + r.y * r.y).sqrt() / len;
        return Vec2::new(qstar.x + acc.x * scale, qstar.y + acc.y * scale);
    }

    // Affine: solve the weighted 2x2 normal equations.
    let (mut a, mut b, mut c, mut d) = (0.0f32, 0.0, 0.0, 0.0);
    for i in 0..n {
        let p = Vec2::new(from[i].x - pstar.x, from[i].y - pstar.y);
        a += weights[i] * p.x * p.x;
        b += weights[i] * p.x * p.y;
        c += weights[i] * p.x * p.y;
        d += weights[i] * p.y * p.y;
    }
    let det = a * d - b * c;
    if det.abs() < 1e-9 {
        // The control points are in a line, so there is no unique fit and a
        // translation is the honest answer.
        return Vec2::new(v.x + qstar.x - pstar.x, v.y + qstar.y - pstar.y);
    }
    // r * M^-1, where M is the weighted covariance above.
    let inv = Vec2::new((r.x * d - r.y * c) / det, (-r.x * b + r.y * a) / det);
    let mut out = qstar;
    for i in 0..n {
        let p = Vec2::new(from[i].x - pstar.x, from[i].y - pstar.y);
        let q = Vec2::new(to[i].x - qstar.x, to[i].y - qstar.y);
        let k = weights[i] * (inv.x * p.x + inv.y * p.y);
        out.x += q.x * k;
        out.y += q.y * k;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;

    fn checker(w: u32, h: u32) -> PixelBuffer {
        let mut px = PixelBuffer::new(w, h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let on = (x / 8 + y / 8) % 2 == 0;
                px.set(x, y, if on { Rgba8::WHITE } else { Rgba8::opaque(40, 40, 40) });
            }
        }
        px
    }

    #[test]
    fn a_warp_that_moves_nothing_moves_nothing() {
        let w = Warp::grid(IRect::new(0, 0, 64, 64), 3, 3);
        assert!(!w.any());
        for at in [Vec2::new(0.0, 0.0), Vec2::new(32.0, 17.0), Vec2::new(63.0, 63.0)] {
            let there = w.map(at);
            assert!(there.distance(at) < 0.01, "{at:?} moved to {there:?}");
        }
    }

    #[test]
    fn a_pin_takes_its_own_point_exactly_where_it_is_put() {
        let mut w = Warp::default();
        w.pin(Vec2::new(10.0, 10.0));
        w.pin(Vec2::new(50.0, 50.0));
        w.to[1] = Vec2::new(60.0, 40.0);

        assert!(w.map(Vec2::new(50.0, 50.0)).distance(Vec2::new(60.0, 40.0)) < 0.01);
        assert!(w.map(Vec2::new(10.0, 10.0)).distance(Vec2::new(10.0, 10.0)) < 0.01);
    }

    /// The whole point of the falloff: a pin's influence should fade with
    /// distance rather than dragging the entire layer with it.
    #[test]
    fn influence_falls_off_with_distance() {
        let mut w = Warp::default();
        for at in [(0.0, 0.0), (100.0, 0.0), (0.0, 100.0), (100.0, 100.0)] {
            w.pin(Vec2::new(at.0, at.1));
        }
        w.pin(Vec2::new(50.0, 50.0));
        w.to[4] = Vec2::new(50.0, 30.0); // the middle pin, pulled up

        let near = w.map(Vec2::new(50.0, 55.0)).distance(Vec2::new(50.0, 55.0));
        let far = w.map(Vec2::new(50.0, 95.0)).distance(Vec2::new(50.0, 95.0));
        assert!(near > far * 2.0, "near moved {near:.2}, far moved {far:.2}");
    }

    /// A corner that is pinned in place must stay in place, or a warp would
    /// drift the whole layer whenever anything was moved.
    #[test]
    fn pinned_corners_hold_the_rest_still() {
        let mut w = Warp::default();
        for at in [(0.0, 0.0), (100.0, 0.0), (0.0, 100.0), (100.0, 100.0)] {
            w.pin(Vec2::new(at.0, at.1));
        }
        w.pin(Vec2::new(50.0, 50.0));
        w.to[4] = Vec2::new(70.0, 50.0);
        for i in 0..4 {
            let held = w.map(w.from[i]);
            assert!(held.distance(w.from[i]) < 0.01, "corner {i} drifted to {held:?}");
        }
    }

    /// Rigid is what keeps a shape a shape. Stretch one pin a long way and an
    /// affine fit will squash the region to reach it; a rigid one will not.
    #[test]
    fn rigid_keeps_its_proportions_and_affine_does_not() {
        let pins = |rigid: bool| {
            let mut w = Warp { rigid, ..Default::default() };
            // Three, not two: two points are collinear by definition, the
            // affine fit through them is singular, and the fallback is a
            // translation — which would keep its proportions and prove
            // nothing.
            w.pin(Vec2::new(20.0, 50.0));
            w.pin(Vec2::new(50.0, 10.0));
            w.pin(Vec2::new(80.0, 50.0));
            w.to[2] = Vec2::new(140.0, 50.0); // pulled far to the right
            w
        };
        // How tall a short vertical segment near the moving pin comes out.
        let height = |w: &Warp| {
            let a = w.map(Vec2::new(80.0, 40.0));
            let b = w.map(Vec2::new(80.0, 60.0));
            a.distance(b)
        };
        let (r, a) = (height(&pins(true)), height(&pins(false)));
        assert!((r - 20.0).abs() < 1.0, "rigid should keep it 20 tall, got {r:.1}");
        assert!(a > r + 2.0, "affine stretches it to reach: {a:.1} against {r:.1}");
    }

    #[test]
    fn unmapping_undoes_mapping() {
        let mut w = Warp::default();
        for at in [(0.0, 0.0), (100.0, 0.0), (0.0, 100.0), (100.0, 100.0), (50.0, 50.0)] {
            w.pin(Vec2::new(at.0, at.1));
        }
        w.to[4] = Vec2::new(62.0, 44.0);
        for at in [Vec2::new(30.0, 30.0), Vec2::new(70.0, 20.0), Vec2::new(50.0, 80.0)] {
            let round = w.unmap(w.map(at));
            assert!(round.distance(at) < 3.0, "{at:?} came back as {round:?}");
        }
    }

    #[test]
    fn rendering_grows_the_layer_to_fit_what_it_did() {
        let px = checker(64, 64);
        let mut w = Warp::default();
        for at in [(0.0, 0.0), (64.0, 0.0), (0.0, 64.0), (64.0, 64.0)] {
            w.pin(Vec2::new(at.0, at.1));
        }
        w.pin(Vec2::new(32.0, 32.0));
        w.to[4] = Vec2::new(32.0, 100.0); // dragged well below the layer

        let (out, at) = w.apply(&px, (0, 0), Resampling::Bilinear, None).expect("a picture");
        assert!(out.height() > 64, "the layer grew to hold the bulge: {}", out.height());
        assert!(at.1 <= 0, "and its top-left moved with it: {at:?}");
    }

    #[test]
    fn a_warp_with_no_pins_renders_nothing_rather_than_panicking() {
        let px = checker(16, 16);
        assert!(Warp::default().apply(&px, (0, 0), Resampling::Bilinear, None).is_none());
    }
}
