//! Refining a selection's edge against the picture it was made from.
//!
//! # The problem
//!
//! A selection knows where it thinks the boundary is; the picture knows where
//! the boundary actually is. Segmentation gets the *shape* right and the edge
//! approximate — it will cut a person out correctly and hand back a boundary
//! that steps across their hair a few pixels away from it. Growing, shrinking
//! or feathering that edge cannot help, because none of those look at the
//! photograph. They move the edge; they do not find it.
//!
//! # The guided filter
//!
//! The idea is to fit the mask to the picture locally. Over a small window,
//! assume the answer is a linear function of the image's brightness:
//!
//! ```text
//! coverage ≈ a * brightness + b
//! ```
//!
//! Take the `a` and `b` that best fit the mask over that window in the
//! least-squares sense, average them across every window a pixel belongs to,
//! and evaluate. Where the picture is flat the fit collapses to `a = 0` and
//! the mask is simply smoothed; where the picture has an edge, `a` is large
//! and the coverage follows the edge exactly — which is what pulls the
//! boundary onto the hair rather than near it.
//!
//! `epsilon` decides what counts as flat. Larger means more smoothing and less
//! following; it is the one number that trades a clean edge against a faithful
//! one, and it is what the *radius* and *contrast* controls move between them.
//!
//! Everything here works on one channel of brightness rather than on colour.
//! A colour guide fits three coefficients and needs a 3×3 inverse per window;
//! it is better where foreground and background differ in hue but not in
//! brightness, and several times the work everywhere else.

use crate::mask::MaskBuffer;
use crate::pixels::PixelBuffer;

/// What to do to an edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefineEdge {
    /// How far to look when fitting the mask to the picture, in pixels. Larger
    /// finds a wandering edge from further away and blurs a fine one.
    pub radius: f32,
    /// Smooth the mask before fitting, which takes the staircase off a
    /// selection that came from a rectangle or a wand.
    pub smooth: f32,
    /// Soften the result afterwards.
    pub feather: f32,
    /// Push coverage toward nothing or everything, `0..=1`. Undoes the
    /// haziness that a large radius introduces.
    pub contrast: f32,
    /// Move the whole edge in (negative) or out (positive), `-1..=1`.
    pub shift: f32,
}

impl Default for RefineEdge {
    fn default() -> Self {
        Self { radius: 3.0, smooth: 0.0, feather: 0.0, contrast: 0.0, shift: 0.0 }
    }
}

impl RefineEdge {
    /// Whether this would change anything.
    pub fn any(&self) -> bool {
        self.radius > 0.5
            || self.smooth > 0.0
            || self.feather > 0.0
            || self.contrast.abs() > 0.0
            || self.shift.abs() > 0.0
    }

    /// Refine `mask` against `guide`, which must be the same size.
    pub fn apply(&self, mask: &MaskBuffer, guide: &PixelBuffer) -> MaskBuffer {
        let (w, h) = (mask.width(), mask.height());
        if w != guide.width() || h != guide.height() || w == 0 || h == 0 {
            return mask.clone();
        }
        let n = (w * h) as usize;

        let mut p: Vec<f32> = Vec::with_capacity(n);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                p.push(mask.get(x, y) as f32 / 255.0);
            }
        }
        if self.smooth > 0.0 {
            p = box_blur(&p, w, h, self.smooth);
        }

        if self.radius >= 1.0 {
            let mut i: Vec<f32> = Vec::with_capacity(n);
            for y in 0..h {
                for c in guide.row(y) {
                    i.push(c.to_f32().luma());
                }
            }
            p = guided(&i, &p, w, h, self.radius, self.epsilon());
        }

        // Shifting moves the whole edge by moving the level that counts as the
        // boundary: coverage is a soft step, so raising the level someone
        // means by "the edge" pulls it inward and lowering it pushes it out.
        // Cheaper and smoother than eroding, and it cannot pick up a staircase
        // the way a structuring element does.
        let shift = self.shift.clamp(-1.0, 1.0) * 0.45;
        let contrast = self.contrast.clamp(0.0, 1.0);
        for v in p.iter_mut() {
            let mut x = (*v + shift).clamp(0.0, 1.0);
            if contrast > 0.0 {
                // Toward a step, without ever becoming one: a hard threshold
                // would throw away the partial coverage that all of this is
                // for.
                let s = x * x * (3.0 - 2.0 * x);
                x = x + (s - x) * contrast;
                let k = 1.0 + contrast * 3.0;
                x = ((x - 0.5) * k + 0.5).clamp(0.0, 1.0);
            }
            *v = x;
        }

        if self.feather > 0.0 {
            p = box_blur(&p, w, h, self.feather);
        }

        let mut out = MaskBuffer::hide_all(w, h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let v = p[(y as u32 * w + x as u32) as usize];
                out.set(x, y, (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
        }
        out
    }

    /// What counts as flat, for the fit.
    ///
    /// Tied to contrast rather than exposed on its own: a large epsilon and a
    /// low contrast are the same complaint — "this edge came out hazy" — and
    /// one control that fixes it is better than two that interact.
    fn epsilon(&self) -> f32 {
        let base = 1e-4;
        base * (1.0 + (1.0 - self.contrast.clamp(0.0, 1.0)) * 40.0)
    }
}

/// The guided filter proper, on one channel.
fn guided(i: &[f32], p: &[f32], w: u32, h: u32, radius: f32, eps: f32) -> Vec<f32> {
    let mean_i = box_blur(i, w, h, radius);
    let mean_p = box_blur(p, w, h, radius);
    let ip: Vec<f32> = i.iter().zip(p).map(|(a, b)| a * b).collect();
    let ii: Vec<f32> = i.iter().map(|a| a * a).collect();
    let mean_ip = box_blur(&ip, w, h, radius);
    let mean_ii = box_blur(&ii, w, h, radius);

    let mut a = Vec::with_capacity(i.len());
    let mut b = Vec::with_capacity(i.len());
    for k in 0..i.len() {
        let cov = mean_ip[k] - mean_i[k] * mean_p[k];
        let var = mean_ii[k] - mean_i[k] * mean_i[k];
        // Where the window is flat, `var` is nothing and `a` goes to nothing
        // with it, which is exactly right: there is no edge to follow, so the
        // answer is the local average of the mask.
        let ak = cov / (var + eps);
        a.push(ak);
        b.push(mean_p[k] - ak * mean_i[k]);
    }
    let mean_a = box_blur(&a, w, h, radius);
    let mean_b = box_blur(&b, w, h, radius);
    (0..i.len()).map(|k| mean_a[k] * i[k] + mean_b[k]).collect()
}

/// Separable box blur with a running sum, so the cost does not grow with the
/// radius. Edges are clamped, which is what keeps a mask that runs off the
/// canvas from being pulled toward nothing there.
fn box_blur(src: &[f32], w: u32, h: u32, radius: f32) -> Vec<f32> {
    let r = radius.round().max(0.0) as i32;
    if r == 0 {
        return src.to_vec();
    }
    let (wi, hi) = (w as i32, h as i32);
    let span = (2 * r + 1) as f32;
    let at = |v: &[f32], x: i32, y: i32| {
        v[(y.clamp(0, hi - 1) * wi + x.clamp(0, wi - 1)) as usize]
    };

    let mut mid = vec![0.0f32; src.len()];
    for y in 0..hi {
        let mut acc = 0.0;
        for x in -r..=r {
            acc += at(src, x, y);
        }
        for x in 0..wi {
            mid[(y * wi + x) as usize] = acc / span;
            acc -= at(src, x - r, y);
            acc += at(src, x + r + 1, y);
        }
    }

    let mut out = vec![0.0f32; src.len()];
    for x in 0..wi {
        let mut acc = 0.0;
        for y in -r..=r {
            acc += at(&mid, x, y);
        }
        for y in 0..hi {
            out[(y * wi + x) as usize] = acc / span;
            acc -= at(&mid, x, y - r);
            acc += at(&mid, x, y + r + 1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;

    /// A picture with a hard vertical edge at `at`, and a mask whose edge is
    /// somewhere else — the situation refining exists for.
    fn edge_and_mask(w: u32, h: u32, image_at: i32, mask_at: i32) -> (PixelBuffer, MaskBuffer) {
        let mut px = PixelBuffer::new(w, h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let v = if x < image_at { 230 } else { 25 };
                px.set(x, y, Rgba8::opaque(v, v, v));
            }
        }
        let mut m = MaskBuffer::hide_all(w, h);
        for y in 0..h as i32 {
            for x in 0..mask_at {
                m.set(x, y, 255);
            }
        }
        (px, m)
    }

    /// Where the mask crosses half coverage, along one row.
    fn crossing(m: &MaskBuffer, y: i32) -> f32 {
        for x in 0..m.width() as i32 - 1 {
            let (a, b) = (m.get(x, y) as f32, m.get(x + 1, y) as f32);
            if a >= 128.0 && b < 128.0 {
                return x as f32 + (a - 128.0) / (a - b).max(1e-4);
            }
        }
        -1.0
    }

    /// The claim: a selection whose edge is near the picture's edge is pulled
    /// onto it, from either side.
    #[test]
    fn refining_pulls_the_edge_onto_the_one_in_the_picture() {
        for mask_at in [30, 34] {
            let (px, m) = edge_and_mask(64, 24, 32, mask_at);
            let before = crossing(&m, 12);
            let after =
                crossing(&RefineEdge { radius: 4.0, ..Default::default() }.apply(&m, &px), 12);
            assert!(
                (after - 31.0).abs() < 1.5,
                "from {before} it should reach the picture's edge at 31, not {after}"
            );
        }
    }

    /// And the limit, which is what the radius control is for: the fit is
    /// local, so an edge further away than the window is not found. Someone
    /// whose selection is badly out has to say how far out it is.
    #[test]
    fn the_radius_has_to_reach_the_edge_to_find_it() {
        let (px, m) = edge_and_mask(64, 24, 32, 26);
        let at = |r: f32| crossing(&RefineEdge { radius: r, ..Default::default() }.apply(&m, &px), 12);
        assert!(at(4.0) < 27.0, "six pixels out is beyond a four-pixel window: {}", at(4.0));
        assert!(
            (at(16.0) - 31.0).abs() < 1.5,
            "and within a sixteen-pixel one: {}",
            at(16.0)
        );
    }

    /// The other half of the claim: where the picture has no edge, refining
    /// must not invent one or move what is there.
    #[test]
    fn a_flat_picture_leaves_the_edge_where_it_was() {
        let mut px = PixelBuffer::new(64, 24);
        for y in 0..24 {
            for x in 0..64 {
                px.set(x, y, Rgba8::opaque(128, 128, 128));
            }
        }
        let mut m = MaskBuffer::hide_all(64, 24);
        for y in 0..24 {
            for x in 0..26 {
                m.set(x, y, 255);
            }
        }
        let out = RefineEdge { radius: 6.0, ..Default::default() }.apply(&m, &px);
        let after = crossing(&out, 12);
        assert!((after - 25.0).abs() < 2.0, "nothing to follow, so nothing moves: {after}");
    }

    /// Partial coverage is the point. A hard in-or-out answer would be no
    /// better than the selection that went in.
    #[test]
    fn the_result_is_a_matte_and_not_a_verdict() {
        let (px, m) = edge_and_mask(64, 24, 32, 26);
        let out = RefineEdge { radius: 8.0, ..Default::default() }.apply(&m, &px);
        let partial =
            (0..64).filter(|&x| (8..248).contains(&(out.get(x, 12) as i32))).count();
        assert!(partial >= 2, "the edge should be soft, not a step: {partial} partial pixels");
    }

    #[test]
    fn contrast_tightens_a_hazy_edge() {
        let (px, m) = edge_and_mask(64, 24, 32, 26);
        let width = |c: f32| {
            let out = RefineEdge { radius: 10.0, contrast: c, ..Default::default() }.apply(&m, &px);
            (0..64).filter(|&x| (8..248).contains(&(out.get(x, 12) as i32))).count()
        };
        assert!(width(0.9) < width(0.0), "{} against {}", width(0.9), width(0.0));
    }

    #[test]
    fn shifting_moves_the_edge_in_and_out() {
        let (px, m) = edge_and_mask(64, 24, 32, 26);
        let at = |s: f32| {
            crossing(&RefineEdge { radius: 4.0, shift: s, ..Default::default() }.apply(&m, &px), 12)
        };
        assert!(at(-0.5) < at(0.0), "negative pulls the edge in");
        assert!(at(0.5) > at(0.0), "positive pushes it out");
    }

    #[test]
    fn feathering_softens_and_nothing_else_moves() {
        let (px, m) = edge_and_mask(64, 24, 32, 26);
        let plain = RefineEdge { radius: 4.0, ..Default::default() }.apply(&m, &px);
        let soft =
            RefineEdge { radius: 4.0, feather: 4.0, ..Default::default() }.apply(&m, &px);
        let width = |m: &MaskBuffer| {
            (0..64).filter(|&x| (8..248).contains(&(m.get(x, 12) as i32))).count()
        };
        assert!(width(&soft) > width(&plain));
        // The middle of each side is still solid and still empty.
        assert!(soft.get(2, 12) > 240 && soft.get(60, 12) < 15);
    }

    #[test]
    fn a_mask_of_a_different_size_is_left_alone_rather_than_read_past() {
        let m = MaskBuffer::hide_all(10, 10);
        let px = PixelBuffer::new(20, 20);
        let out = RefineEdge::default().apply(&m, &px);
        assert_eq!(out.width(), 10);
    }
}
