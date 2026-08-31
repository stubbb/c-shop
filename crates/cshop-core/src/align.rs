//! Finding the same thing in two photographs, and working out how one moved.
//!
//! This is what a panorama and a stack both need and neither can do without.
//! Two frames of the same scene differ by a transform; find enough points that
//! appear in both and the transform falls out. Everything else — stitching,
//! averaging away noise, combining focus — is what you do once you have it.
//!
//! # The four steps
//!
//! **Corners.** Somewhere the picture changes in two directions at once, so
//! that its position can be pinned down. An edge is not enough: a point on a
//! straight edge could be anywhere along it. Harris's measure asks exactly
//! that question of the local gradients.
//!
//! **Descriptions.** A corner on its own says nothing about which corner it
//! is. Each one gets a 256-bit description built by comparing pairs of pixels
//! around it — brighter or darker, one bit each. Crude, and it works: two
//! views of the same corner agree on most of the comparisons, two different
//! corners do not. The pattern is rotated to face the local centre of
//! brightness first, so a frame taken at a slight tilt still matches.
//!
//! **Matches.** For each description in one frame, the nearest and the next
//! nearest in the other, by how many bits differ. Keep it only if the nearest
//! is clearly nearer — a corner that matches two places about equally well has
//! told you nothing, and repeated texture is full of those.
//!
//! **The transform.** Most matches are right and some are nonsense, so fitting
//! all of them would be dragged off by the nonsense. RANSAC instead: take four
//! at random, work out the transform they imply, count how many other matches
//! agree, and keep the one with the most agreement. A wrong match agrees with
//! nothing, so it never wins.

use crate::color::Rgba8;
use crate::geom::Vec2;
use crate::pixels::PixelBuffer;
use crate::transform::Transform;

/// Bits in a descriptor. 256 is the usual compromise: enough to tell corners
/// apart, small enough to compare a million pairs quickly.
const BITS: usize = 256;

/// How far the sampling pattern reaches around a corner.
const PATCH: i32 = 15;

/// A corner and what it looks like.
#[derive(Debug, Clone)]
pub struct Feature {
    pub at: Vec2,
    /// How corner-like, for keeping the best of them.
    pub strength: f32,
    /// Which way the local brightness leans, so the pattern can be turned to
    /// match.
    pub angle: f32,
    bits: [u64; BITS / 64],
}

impl Feature {
    /// How many bits differ. Lower is a better match.
    pub fn distance(&self, other: &Feature) -> u32 {
        self.bits.iter().zip(&other.bits).map(|(a, b)| (a ^ b).count_ones()).sum()
    }
}

/// How the frames are allowed to differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Motion {
    /// The camera turned: a full projective map, which is what a panorama
    /// needs and what a stack must not use, since it can distort as well as
    /// move.
    Homography,
    /// The camera moved, tilted a little and perhaps came slightly closer.
    /// Four numbers rather than eight, so it is far harder to fit nonsense.
    #[default]
    Similarity,
    /// The camera only shifted. Two numbers, and the safest thing to fit when
    /// frames come off a tripod.
    Translation,
}

/// What to align, and how hard to look.
#[derive(Debug, Clone, Copy)]
pub struct Align {
    pub motion: Motion,
    /// The most corners to keep per frame.
    pub features: usize,
    /// How far a match may sit from where the transform puts it and still
    /// count as agreeing, in pixels.
    pub tolerance: f32,
    /// How many random samples RANSAC takes.
    pub attempts: usize,
}

impl Default for Align {
    fn default() -> Self {
        Self { motion: Motion::Similarity, features: 800, tolerance: 3.0, attempts: 2_000 }
    }
}

/// Why an alignment could not be found. Worth distinguishing: "no corners"
/// means the picture is featureless and "no agreement" means the two frames
/// are of different things, and the answer is different in each case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlignError {
    /// One of the frames has nothing to match on.
    NoFeatures,
    /// Corners were found in both, but too few describe the same places.
    NoMatches,
    /// Matches were found, but no transform explains enough of them.
    NoAgreement,
}

impl std::fmt::Display for AlignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            AlignError::NoFeatures => {
                "one of these frames has nothing distinctive enough to match on"
            }
            AlignError::NoMatches => {
                "these two frames have corners, but not the same ones — are they of the \
                 same scene?"
            }
            AlignError::NoAgreement => {
                "the matches do not agree on any one movement; try allowing more \
                 movement, or a looser tolerance"
            }
        };
        f.write_str(s)
    }
}

impl Align {
    /// Where `moving` has to go to sit on top of `reference`.
    pub fn between(
        &self,
        reference: &PixelBuffer,
        moving: &PixelBuffer,
    ) -> Result<Transform, AlignError> {
        let a = features(reference, self.features);
        let b = features(moving, self.features);
        if a.len() < 4 || b.len() < 4 {
            return Err(AlignError::NoFeatures);
        }
        let pairs = matches(&b, &a);
        if pairs.len() < 4 {
            return Err(AlignError::NoMatches);
        }
        self.fit(&pairs).ok_or(AlignError::NoAgreement)
    }

    /// RANSAC over the matches.
    fn fit(&self, pairs: &[(Vec2, Vec2)]) -> Option<Transform> {
        let need = match self.motion {
            Motion::Homography => 4,
            Motion::Similarity => 2,
            Motion::Translation => 1,
        };
        if pairs.len() < need {
            return None;
        }
        let mut rng = Rng::new(0x9E37_79B9_7F4A_7C15);
        let tol2 = self.tolerance * self.tolerance;
        let mut best: Option<(usize, Transform)> = None;

        for _ in 0..self.attempts {
            let mut pick = Vec::with_capacity(need);
            for _ in 0..need {
                pick.push(pairs[rng.below(pairs.len())]);
            }
            let Some(t) = self.model(&pick) else { continue };
            let agree = pairs
                .iter()
                .filter(|(from, to)| {
                    let p = t.apply(*from);
                    (p.x - to.x).powi(2) + (p.y - to.y).powi(2) <= tol2
                })
                .count();
            if best.as_ref().is_none_or(|(n, _)| agree > *n) {
                best = Some((agree, t));
            }
        }

        // A model that only its own sample agrees with has explained nothing.
        // The test is a share of the matches rather than a count: a real
        // movement has most of them agreeing, and a handful agreeing out of
        // hundreds is what coincidence looks like.
        let (count, rough) = best?;
        if count < 8 || (count as f32) < 0.25 * pairs.len() as f32 {
            return None;
        }
        // Refit on everything that agreed, which is what turns a transform
        // that four points happened to imply into one the whole frame does.
        let inliers: Vec<(Vec2, Vec2)> = pairs
            .iter()
            .copied()
            .filter(|(from, to)| {
                let p = rough.apply(*from);
                (p.x - to.x).powi(2) + (p.y - to.y).powi(2) <= tol2
            })
            .collect();
        self.model(&inliers).or(Some(rough))
    }

    /// The transform implied by a set of correspondences.
    fn model(&self, pairs: &[(Vec2, Vec2)]) -> Option<Transform> {
        match self.motion {
            Motion::Translation => {
                let n = pairs.len() as f32;
                if n < 1.0 {
                    return None;
                }
                let (mut dx, mut dy) = (0.0, 0.0);
                for (a, b) in pairs {
                    dx += b.x - a.x;
                    dy += b.y - a.y;
                }
                Some(Transform::translate(dx / n, dy / n))
            }
            Motion::Similarity => similarity(pairs),
            Motion::Homography => homography(pairs),
        }
    }
}

/// The least-squares similarity: rotation, uniform scale and translation.
fn similarity(pairs: &[(Vec2, Vec2)]) -> Option<Transform> {
    let n = pairs.len() as f32;
    if pairs.len() < 2 {
        return None;
    }
    let mean = |f: fn(&(Vec2, Vec2)) -> Vec2| {
        let mut c = Vec2::ZERO;
        for p in pairs {
            let v = f(p);
            c.x += v.x;
            c.y += v.y;
        }
        Vec2::new(c.x / n, c.y / n)
    };
    let (ca, cb) = (mean(|p| p.0), mean(|p| p.1));

    let (mut sxx, mut sxy, mut saa) = (0.0f32, 0.0f32, 0.0f32);
    for (a, b) in pairs {
        let (ax, ay) = (a.x - ca.x, a.y - ca.y);
        let (bx, by) = (b.x - cb.x, b.y - cb.y);
        sxx += ax * bx + ay * by;
        sxy += ax * by - ay * bx;
        saa += ax * ax + ay * ay;
    }
    if saa < 1e-9 {
        return None;
    }
    // The complex-number form: one rotation and one scale together.
    let (c, s) = (sxx / saa, sxy / saa);
    if !c.is_finite() || !s.is_finite() {
        return None;
    }
    // A least-squares fit through matches that agree on nothing collapses to
    // "send everything to the centroid" — a scale of zero, which is a
    // perfectly good minimum and not a movement any camera made. Two frames
    // of different scenes produce exactly this, so it has to be refused here
    // rather than returned as an answer.
    let scale2 = c * c + s * s;
    if !(0.04..=25.0).contains(&scale2) {
        return None;
    }
    let m = Transform {
        m: [
            [c, -s, cb.x - (c * ca.x - s * ca.y)],
            [s, c, cb.y - (s * ca.x + c * ca.y)],
            [0.0, 0.0, 1.0],
        ],
    };
    Some(m)
}

/// The direct linear transform: the projective map through four or more pairs.
fn homography(pairs: &[(Vec2, Vec2)]) -> Option<Transform> {
    if pairs.len() < 4 {
        return None;
    }
    // Eight unknowns, two equations per pair, solved in the least-squares
    // sense through the normal equations. Fine at this scale, and it avoids
    // needing a singular value decomposition for eight numbers.
    let mut ata = [[0.0f64; 8]; 8];
    let mut atb = [0.0f64; 8];
    for (a, b) in pairs {
        let (x, y) = (a.x as f64, a.y as f64);
        let (u, v) = (b.x as f64, b.y as f64);
        let rows = [
            ([x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y], u),
            ([0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y], v),
        ];
        for (row, rhs) in rows {
            for i in 0..8 {
                for j in 0..8 {
                    ata[i][j] += row[i] * row[j];
                }
                atb[i] += row[i] * rhs;
            }
        }
    }
    let h = solve8(ata, atb)?;
    let m = Transform {
        m: [
            [h[0] as f32, h[1] as f32, h[2] as f32],
            [h[3] as f32, h[4] as f32, h[5] as f32],
            [h[6] as f32, h[7] as f32, 1.0],
        ],
    };
    m.m.iter().flatten().all(|v| v.is_finite()).then_some(m)
}

/// Gaussian elimination with partial pivoting.
fn solve8(mut a: [[f64; 8]; 8], mut b: [f64; 8]) -> Option<[f64; 8]> {
    for i in 0..8 {
        let pivot = (i..8).max_by(|&p, &q| a[p][i].abs().total_cmp(&a[q][i].abs()))?;
        if a[pivot][i].abs() < 1e-12 {
            return None;
        }
        a.swap(i, pivot);
        b.swap(i, pivot);
        let (row, above) = (a[i], b[i]);
        for (k, lower) in a.iter_mut().enumerate().skip(i + 1) {
            let f = lower[i] / row[i];
            for j in i..8 {
                lower[j] -= f * row[j];
            }
            b[k] -= f * above;
        }
    }
    let mut x = [0.0f64; 8];
    for i in (0..8).rev() {
        let mut s = b[i];
        for j in i + 1..8 {
            s -= a[i][j] * x[j];
        }
        x[i] = s / a[i][i];
    }
    Some(x)
}

/// Corners, described, strongest first.
pub fn features(px: &PixelBuffer, most: usize) -> Vec<Feature> {
    let (w, h) = (px.width() as i32, px.height() as i32);
    if w < PATCH * 2 + 3 || h < PATCH * 2 + 3 {
        return Vec::new();
    }
    let luma: Vec<f32> = px.pixels().iter().map(|c| c.to_f32().luma()).collect();
    let at = |x: i32, y: i32| luma[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize];

    // Harris, over a 3x3 window: a corner is where the gradient covariance has
    // two large eigenvalues, which the determinant-minus-trace form measures
    // without having to find them.
    let margin = PATCH + 2;
    let mut found: Vec<(f32, i32, i32)> = Vec::new();
    for y in margin..h - margin {
        for x in margin..w - margin {
            let (mut ixx, mut iyy, mut ixy) = (0.0f32, 0.0f32, 0.0f32);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let gx = at(x + dx + 1, y + dy) - at(x + dx - 1, y + dy);
                    let gy = at(x + dx, y + dy + 1) - at(x + dx, y + dy - 1);
                    ixx += gx * gx;
                    iyy += gy * gy;
                    ixy += gx * gy;
                }
            }
            let det = ixx * iyy - ixy * ixy;
            let trace = ixx + iyy;
            let score = det - 0.04 * trace * trace;
            if score > 1e-5 {
                found.push((score, x, y));
            }
        }
    }
    if found.is_empty() {
        return Vec::new();
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0));

    // Thin them out, so a single strong corner does not claim a hundred slots
    // for the same place — and so the ones that survive are spread across the
    // frame, which is what a transform needs to be pinned down.
    let spacing = 8;
    let (cw, ch) = ((w / spacing + 1) as usize, (h / spacing + 1) as usize);
    let mut taken = vec![false; cw * ch];
    let mut out = Vec::with_capacity(most);
    for (score, x, y) in found {
        let cell = (y / spacing) as usize * cw + (x / spacing) as usize;
        if taken[cell] {
            continue;
        }
        taken[cell] = true;
        let angle = orientation(&luma, w, h, x, y);
        out.push(Feature {
            at: Vec2::new(x as f32, y as f32),
            strength: score,
            angle,
            bits: describe(&luma, w, h, x, y, angle),
        });
        if out.len() >= most {
            break;
        }
    }
    out
}

/// Which way the brightness around a point leans, from its intensity centroid.
fn orientation(luma: &[f32], w: i32, h: i32, x: i32, y: i32) -> f32 {
    let at = |x: i32, y: i32| luma[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize];
    let (mut mx, mut my) = (0.0f32, 0.0f32);
    for dy in -PATCH..=PATCH {
        for dx in -PATCH..=PATCH {
            if dx * dx + dy * dy > PATCH * PATCH {
                continue;
            }
            let v = at(x + dx, y + dy);
            mx += dx as f32 * v;
            my += dy as f32 * v;
        }
    }
    my.atan2(mx)
}

/// The 256 brighter-or-darker comparisons.
fn describe(luma: &[f32], w: i32, h: i32, x: i32, y: i32, angle: f32) -> [u64; BITS / 64] {
    let at = |x: i32, y: i32| luma[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize];
    let (sin, cos) = angle.sin_cos();
    let mut bits = [0u64; BITS / 64];
    // The pattern is fixed and generated from a fixed seed, so two runs of the
    // program describe the same corner the same way. A random pattern per run
    // would make descriptors incomparable between them, and the point of a
    // descriptor is to be compared.
    let mut rng = Rng::new(0xB5AD_4ECE_DA1C_E2A9);
    for i in 0..BITS {
        let p = pattern_point(&mut rng);
        let q = pattern_point(&mut rng);
        let turn = |v: (f32, f32)| {
            (
                (v.0 * cos - v.1 * sin).round() as i32,
                (v.0 * sin + v.1 * cos).round() as i32,
            )
        };
        let (px1, py1) = turn(p);
        let (px2, py2) = turn(q);
        if at(x + px1, y + py1) < at(x + px2, y + py2) {
            bits[i / 64] |= 1 << (i % 64);
        }
    }
    bits
}

fn pattern_point(rng: &mut Rng) -> (f32, f32) {
    let r = PATCH as f32 - 1.0;
    (
        (rng.next_f32() * 2.0 - 1.0) * r,
        (rng.next_f32() * 2.0 - 1.0) * r,
    )
}

/// Pairs that describe the same place, as (in `from`, in `to`) positions.
///
/// Kept only when the best match is clearly better than the second best. A
/// corner that matches two places about equally has said nothing, and a brick
/// wall is nothing but such corners.
pub fn matches(from: &[Feature], to: &[Feature]) -> Vec<(Vec2, Vec2)> {
    let mut out = Vec::new();
    for f in from {
        let (mut best, mut second) = (u32::MAX, u32::MAX);
        let mut at = 0usize;
        for (i, g) in to.iter().enumerate() {
            let d = f.distance(g);
            if d < best {
                second = best;
                best = d;
                at = i;
            } else if d < second {
                second = d;
            }
        }
        // Lowe's ratio, at the usual threshold.
        if best < 96 && (best as f32) < 0.8 * second as f32 {
            out.push((f.at, to[at].at));
        }
    }
    out
}

/// A small deterministic generator, so the sampling pattern and the RANSAC
/// draws are the same on every run and a failure can be reproduced.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        // xorshift64*, which is plenty for choosing sample points.
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

/// The average of several frames, which is how noise is removed by stacking:
/// the picture is the same in every frame and the noise is not, so the noise
/// averages toward nothing and the picture does not.
///
/// Frames are expected to be aligned already and the same size. Pixels no
/// frame covers stay transparent.
pub fn mean(frames: &[PixelBuffer]) -> Option<PixelBuffer> {
    let first = frames.first()?;
    let (w, h) = (first.width(), first.height());
    if frames.iter().any(|f| f.width() != w || f.height() != h) {
        return None;
    }
    let mut out = PixelBuffer::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let (mut r, mut g, mut b, mut n) = (0.0f32, 0.0, 0.0, 0.0);
            for f in frames {
                let c = f.get(x, y);
                // Weighted by coverage, so a frame that does not reach this
                // pixel contributes nothing rather than darkening it.
                let a = c.a as f32 / 255.0;
                r += c.r as f32 * a;
                g += c.g as f32 * a;
                b += c.b as f32 * a;
                n += a;
            }
            if n <= 0.0 {
                continue;
            }
            out.set(
                x,
                y,
                Rgba8::new(
                    (r / n).round() as u8,
                    (g / n).round() as u8,
                    (b / n).round() as u8,
                    ((n / frames.len() as f32) * 255.0).round().clamp(0.0, 255.0) as u8,
                ),
            );
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resample::Resampling;

    /// A picture with plenty of distinct corners: scattered squares of varying
    /// size and brightness, which is what a real scene looks like to a corner
    /// detector and a flat gradient does not.
    fn scene(w: u32, h: u32) -> PixelBuffer {
        let mut px = PixelBuffer::filled(w, h, Rgba8::opaque(80, 90, 110));
        let mut rng = Rng::new(0x1234_5678_9ABC_DEF0);
        for _ in 0..120 {
            let x = (rng.next_f32() * (w as f32 - 30.0)) as i32 + 8;
            let y = (rng.next_f32() * (h as f32 - 30.0)) as i32 + 8;
            let s = 4 + (rng.next_f32() * 10.0) as i32;
            let v = (rng.next_f32() * 200.0) as u8 + 30;
            for dy in 0..s {
                for dx in 0..s {
                    px.set(x + dx, y + dy, Rgba8::opaque(v, v / 2 + 40, 255 - v));
                }
            }
        }
        px
    }

    fn moved(px: &PixelBuffer, by: Transform) -> PixelBuffer {
        let (w, h) = (px.width(), px.height());
        let mut out = PixelBuffer::new(w, h);
        let inverse = by.invert().unwrap();
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let s = inverse.apply(Vec2::new(x as f32 + 0.5, y as f32 + 0.5));
                out.set(x, y, crate::resample::sample_at(px, s.x, s.y, Resampling::Bilinear));
            }
        }
        out
    }

    /// How far a transform is from the one that was applied, measured where it
    /// matters: on the corners of the frame.
    fn error(found: Transform, truth: Transform, w: f32, h: f32) -> f32 {
        [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]
            .into_iter()
            .map(|(x, y)| {
                let p = Vec2::new(x, y);
                found.apply(p).distance(truth.apply(p))
            })
            .fold(0.0f32, f32::max)
    }

    #[test]
    fn a_shifted_frame_is_found_and_measured() {
        let a = scene(320, 240);
        let truth = Transform::translate(17.0, -11.0);
        let b = moved(&a, truth);

        // `b` was made by moving `a`, so putting `b` back on `a` is the
        // opposite move.
        let found = Align { motion: Motion::Translation, ..Default::default() }
            .between(&a, &b)
            .expect("a shift should be findable");
        let back = truth.invert().unwrap();
        let e = error(found, back, 320.0, 240.0);
        assert!(e < 2.0, "off by {e:.2} pixels");
    }

    #[test]
    fn a_turned_and_scaled_frame_is_found() {
        let a = scene(360, 300);
        let centre = Vec2::new(180.0, 150.0);
        let truth = Transform::about(
            centre,
            Transform::rotate(0.12).then(Transform::scale(1.06, 1.06)),
        )
        .then(Transform::translate(9.0, -6.0));
        let b = moved(&a, truth);

        let found = Align::default().between(&a, &b).expect("a similarity should be findable");
        let e = error(found, truth.invert().unwrap(), 360.0, 300.0);
        assert!(e < 3.0, "off by {e:.2} pixels");
    }

    #[test]
    fn a_camera_that_turned_needs_the_projective_fit() {
        let a = scene(360, 300);
        let quad = [
            Vec2::new(10.0, 4.0),
            Vec2::new(352.0, 22.0),
            Vec2::new(344.0, 292.0),
            Vec2::new(4.0, 274.0),
        ];
        let truth =
            Transform::from_quad(crate::geom::IRect::new(0, 0, 360, 300), quad).unwrap();
        let b = moved(&a, truth);

        let found = Align { motion: Motion::Homography, tolerance: 4.0, ..Default::default() }
            .between(&a, &b)
            .expect("a projective move should be findable");
        let e = error(found, truth.invert().unwrap(), 360.0, 300.0);
        assert!(e < 6.0, "off by {e:.2} pixels");
    }

    /// The failure that matters: two photographs of different things should be
    /// refused rather than aligned to nonsense.
    #[test]
    fn two_unrelated_frames_are_refused() {
        let a = scene(320, 240);
        let mut rng = Rng::new(0xDEAD_BEEF_CAFE_1234);
        let mut b = PixelBuffer::filled(320, 240, Rgba8::opaque(30, 30, 30));
        for _ in 0..120 {
            let x = (rng.next_f32() * 290.0) as i32 + 8;
            let y = (rng.next_f32() * 210.0) as i32 + 8;
            for dy in 0..7 {
                for dx in 0..7 {
                    b.set(x + dx, y + dy, Rgba8::opaque(220, 30, 200));
                }
            }
        }
        let outcome = Align::default().between(&a, &b);
        assert!(outcome.is_err(), "it should not have found a movement: {outcome:?}");
    }

    #[test]
    fn a_frame_with_nothing_in_it_says_so() {
        let flat = PixelBuffer::filled(200, 200, Rgba8::opaque(128, 128, 128));
        let scene = scene(200, 200);
        assert_eq!(Align::default().between(&scene, &flat), Err(AlignError::NoFeatures));
    }

    #[test]
    fn describing_the_same_corner_twice_gives_the_same_bits() {
        let px = scene(200, 200);
        let a = features(&px, 50);
        let b = features(&px, 50);
        assert!(!a.is_empty());
        assert_eq!(a.len(), b.len());
        for (f, g) in a.iter().zip(&b) {
            assert_eq!(f.distance(g), 0, "the same run must describe a corner the same way");
        }
    }

    /// Stacking: the picture is the same in every frame and the noise is not.
    #[test]
    fn averaging_frames_removes_noise_and_keeps_the_picture() {
        let clean = scene(160, 120);
        let mut rng = Rng::new(0xA1B2_C3D4_E5F6_0718);
        let noisy: Vec<PixelBuffer> = (0..8)
            .map(|_| {
                let mut f = clean.clone();
                for y in 0..120 {
                    for x in 0..160 {
                        let c = clean.get(x, y);
                        let n = |v: u8, r: f32| {
                            (v as f32 + (r - 0.5) * 90.0).clamp(0.0, 255.0) as u8
                        };
                        f.set(
                            x,
                            y,
                            Rgba8::new(
                                n(c.r, rng.next_f32()),
                                n(c.g, rng.next_f32()),
                                n(c.b, rng.next_f32()),
                                c.a,
                            ),
                        );
                    }
                }
                f
            })
            .collect();

        let error_of = |px: &PixelBuffer| {
            let mut total = 0.0f64;
            for y in 0..120 {
                for x in 0..160 {
                    let (a, b) = (px.get(x, y), clean.get(x, y));
                    total += (a.r as f64 - b.r as f64).powi(2);
                }
            }
            (total / (160.0 * 120.0)).sqrt()
        };
        let one = error_of(&noisy[0]);
        let stacked = mean(&noisy).expect("same size, so it should stack");
        let many = error_of(&stacked);
        assert!(
            many < one / 2.0,
            "eight frames should roughly cut the noise by the square root of eight: \
             {one:.1} became {many:.1}"
        );
    }

    #[test]
    fn frames_of_different_sizes_are_refused_rather_than_read_past() {
        let a = PixelBuffer::new(10, 10);
        let b = PixelBuffer::new(12, 10);
        assert!(mean(&[a, b]).is_none());
    }
}
