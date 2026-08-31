//! The working buffer every spatial filter operates on.
//!
//! # Why premultiplied
//!
//! Blurring straight-alpha colour mixes in the colour of transparent pixels.
//! On anything with a soft edge that shows up as a dark halo — the same bug
//! that bites resampling, and for the same reason. Every filter here works
//! premultiplied and converts back once, at the end.
//!
//! # Why float
//!
//! Sharpening and embossing both overshoot deliberately. Clamping at every
//! intermediate step would flatten the overshoot into a hard edge, so values
//! stay in `f32` until the final conversion.

use crate::color::Rgba8;
use crate::pixels::PixelBuffer;
use crate::progress::Progress;
use rayon::prelude::*;

/// Fill a plane a row at a time, in parallel, saying so as it goes.
///
/// Every spatial filter here has the same shape — a new plane written row by
/// row from the old one — so the counting and the stopping live once, here,
/// rather than being remembered at twenty call sites.
///
/// A cancelled run leaves the remaining rows as it found them and returns.
/// That is not a valid picture and is not meant to be: the only caller that
/// can cancel is the one that throws the answer away.
pub fn fill_rows(out: &mut Plane, p: &Progress, f: impl Fn(usize, &mut [f32]) + Sync + Send) {
    let width = out.width as usize;
    if width == 0 {
        return;
    }
    out.data.par_chunks_mut(width * 4).enumerate().for_each(|(y, row)| {
        if p.cancelled() {
            return;
        }
        f(y, row);
        p.advance(1);
    });
}

/// A premultiplied `f32` RGBA image.
#[derive(Clone)]
pub struct Plane {
    pub width: u32,
    pub height: u32,
    /// Row-major, four floats per pixel.
    pub data: Vec<f32>,
}

impl Plane {
    pub fn new(width: u32, height: u32) -> Plane {
        Plane { width, height, data: vec![0.0; width as usize * height as usize * 4] }
    }

    pub fn from_pixels(src: &PixelBuffer) -> Plane {
        let mut plane = Plane::new(src.width(), src.height());
        // Converting a 24 MP image is a quarter of a second on one core, which
        // on the cheap filters costs more than the filter itself.
        plane
            .data
            .par_chunks_mut(4)
            .zip(src.pixels().par_iter())
            .for_each(|(slot, px)| {
                let a = px.a as f32 / 255.0;
                slot[0] = px.r as f32 / 255.0 * a;
                slot[1] = px.g as f32 / 255.0 * a;
                slot[2] = px.b as f32 / 255.0 * a;
                slot[3] = a;
            });
        plane
    }

    pub fn to_pixels(&self) -> PixelBuffer {
        let mut out = PixelBuffer::new(self.width, self.height);
        out.pixels_mut()
            .par_iter_mut()
            .zip(self.data.par_chunks(4))
            .for_each(|(slot, src)| {
                let a = src[3].clamp(0.0, 1.0);
                *slot = if a <= 1e-6 {
                    Rgba8::TRANSPARENT
                } else {
                    let f = |v: f32| ((v / a).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    Rgba8::new(f(src[0]), f(src[1]), f(src[2]), (a * 255.0 + 0.5) as u8)
                };
            });
        out
    }

    #[inline]
    pub fn index(&self, x: i32, y: i32) -> usize {
        // Clamp to the edge: the border is extended rather than treated
        // outside as transparent, which would darken every edge of a blur.
        let x = x.clamp(0, self.width as i32 - 1) as usize;
        let y = y.clamp(0, self.height as i32 - 1) as usize;
        (y * self.width as usize + x) * 4
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> [f32; 4] {
        let i = self.index(x, y);
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }

    /// Sample with bilinear interpolation, for the geometric filters.
    pub fn sample(&self, x: f32, y: f32) -> [f32; 4] {
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
                let p = self.get(x0 + dx, y0 + dy);
                for c in 0..4 {
                    out[c] += p[c] * w;
                }
            }
        }
        out
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, v: [f32; 4]) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.data[i..i + 4].copy_from_slice(&v);
    }

    /// Straight-alpha luminance at a pixel, for filters that key off tone.
    #[inline]
    pub fn luma(&self, x: i32, y: i32) -> f32 {
        let p = self.get(x, y);
        if p[3] <= 1e-6 {
            return 0.0;
        }
        (0.30 * p[0] + 0.59 * p[1] + 0.11 * p[2]) / p[3]
    }

    /// Mean colour of the whole plane, for Average and Difference Clouds.
    pub fn mean(&self) -> [f32; 4] {
        let n = (self.width as usize * self.height as usize).max(1) as f32;
        let mut sum = [0.0f32; 4];
        for chunk in self.data.chunks_exact(4) {
            for c in 0..4 {
                sum[c] += chunk[c];
            }
        }
        [sum[0] / n, sum[1] / n, sum[2] / n, sum[3] / n]
    }
}

/// Convolve with a 1D kernel along one axis.
///
/// Separating a 2D kernel into two 1D passes turns an O(r²) filter into O(r),
/// which is what makes a large-radius Gaussian usable at all.
pub fn convolve_1d(src: &Plane, kernel: &[f32], horizontal: bool, p: &Progress) -> Plane {
    let mut out = Plane::new(src.width, src.height);
    let half = (kernel.len() / 2) as i32;
    let width = src.width as usize;

    // Rows are independent, so this parallelises directly.
    fill_rows(&mut out, p, |y, row| {
        let y = y as i32;
        for x in 0..width as i32 {
            let mut acc = [0.0f32; 4];
            for (k, &weight) in kernel.iter().enumerate() {
                let offset = k as i32 - half;
                let p = if horizontal {
                    src.get(x + offset, y)
                } else {
                    src.get(x, y + offset)
                };
                for c in 0..4 {
                    acc[c] += p[c] * weight;
                }
            }
            let i = x as usize * 4;
            row[i..i + 4].copy_from_slice(&acc);
        }
    });
    out
}

/// A normalised Gaussian kernel for the given standard deviation.
pub fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let sigma = sigma.max(0.01);
    // Three sigma captures over 99% of the curve; going wider costs time and
    // changes nothing visible.
    let radius = (sigma * 3.0).ceil().max(1.0) as i32;
    let mut kernel = Vec::with_capacity((radius * 2 + 1) as usize);
    let denom = 2.0 * sigma * sigma;
    for i in -radius..=radius {
        let x = i as f32;
        kernel.push((-x * x / denom).exp());
    }
    let sum: f32 = kernel.iter().sum();
    for k in &mut kernel {
        *k /= sum;
    }
    kernel
}

/// Separable Gaussian blur.
pub fn gaussian_blur(src: &Plane, sigma: f32, p: &Progress) -> Plane {
    if sigma <= 0.01 {
        return src.clone();
    }
    let kernel = gaussian_kernel(sigma);
    let horizontal = convolve_1d(src, &kernel, true, p);
    convolve_1d(&horizontal, &kernel, false, p)
}

/// Convolve with an arbitrary square kernel.
///
/// Used by the fixed 3×3 effects and by the Custom filter; anything larger and
/// separable should use [`convolve_1d`] twice instead.
pub fn convolve_2d(src: &Plane, kernel: &[f32], size: usize, divisor: f32, bias: f32, p: &Progress) -> Plane {
    debug_assert_eq!(kernel.len(), size * size);
    let mut out = Plane::new(src.width, src.height);
    let half = (size / 2) as i32;
    let width = src.width as usize;
    let divisor = if divisor.abs() < 1e-6 { 1.0 } else { divisor };

    fill_rows(&mut out, p, |y, row| {
        let y = y as i32;
        for x in 0..width as i32 {
            let mut acc = [0.0f32; 4];
            for ky in 0..size {
                for kx in 0..size {
                    let weight = kernel[ky * size + kx];
                    if weight == 0.0 {
                        continue;
                    }
                    let p = src.get(x + kx as i32 - half, y + ky as i32 - half);
                    for c in 0..4 {
                        acc[c] += p[c] * weight;
                    }
                }
            }
            let i = x as usize * 4;
            // Bias applies to colour but not alpha: an edge-detect kernel that
            // shifted alpha would make the image translucent.
            row[i] = acc[0] / divisor + bias;
            row[i + 1] = acc[1] / divisor + bias;
            row[i + 2] = acc[2] / divisor + bias;
            row[i + 3] = acc[3] / divisor;
        }
    });
    out
}

/// A small, fast, deterministic random source.
///
/// Filters need reproducible noise: the same seed must give the same grain, or
/// dragging a slider in a preview would reshuffle every pixel on every frame.
#[derive(Debug, Clone, Copy)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        // Any non-zero state works; xorshift never escapes zero.
        Rng(seed | 1)
    }

    /// Seed from a position, so a filter can be evaluated pixel by pixel in
    /// any order — including in parallel — and still be reproducible.
    pub fn at(seed: u64, x: i32, y: i32) -> Rng {
        let mixed = seed
            ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
        let mut rng = Rng(mixed | 1);
        // One round of mixing, so neighbouring pixels do not correlate.
        rng.next_u64();
        rng
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in `0.0..1.0`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Roughly standard-normal, by summing uniforms.
    pub fn next_gaussian(&mut self) -> f32 {
        let sum: f32 = (0..4).map(|_| self.next_f32()).sum();
        (sum - 2.0) * 0.866
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;

    #[test]
    fn conversion_round_trips() {
        let mut src = PixelBuffer::filled(8, 8, Rgba8::opaque(200, 100, 50));
        src.set(0, 0, Rgba8::new(10, 20, 30, 128));
        src.set(1, 1, Rgba8::TRANSPARENT);

        let back = Plane::from_pixels(&src).to_pixels();
        for y in 0..8i32 {
            for x in 0..8i32 {
                let a = src.get(x, y);
                let b = back.get(x, y);
                assert!(
                    (a.r as i32 - b.r as i32).abs() <= 1
                        && (a.a as i32 - b.a as i32).abs() <= 1,
                    "({x},{y}): {a:?} became {b:?}"
                );
            }
        }
    }

    #[test]
    fn sampling_outside_clamps_to_the_edge() {
        let mut src = PixelBuffer::new(4, 4);
        src.set(0, 0, Rgba8::WHITE);
        let plane = Plane::from_pixels(&src);
        assert_eq!(plane.get(-5, -5), plane.get(0, 0), "outside should read the corner");
        assert_eq!(plane.get(99, 99), plane.get(3, 3));
    }

    #[test]
    fn a_gaussian_kernel_is_normalised_and_symmetric() {
        for sigma in [0.5f32, 1.0, 4.0, 20.0] {
            let k = gaussian_kernel(sigma);
            let sum: f32 = k.iter().sum();
            assert!((sum - 1.0).abs() < 1e-4, "sigma {sigma} summed to {sum}");
            for i in 0..k.len() / 2 {
                assert!((k[i] - k[k.len() - 1 - i]).abs() < 1e-6, "asymmetric at sigma {sigma}");
            }
            // The centre must be the largest weight.
            assert_eq!(
                k.iter().cloned().fold(f32::MIN, f32::max),
                k[k.len() / 2],
                "peak is off-centre"
            );
        }
    }

    #[test]
    fn blurring_a_flat_colour_changes_nothing() {
        // Edge clamping means even the border must survive untouched.
        let src = PixelBuffer::filled(32, 32, Rgba8::opaque(90, 140, 210));
        let out = gaussian_blur(&Plane::from_pixels(&src), 5.0, &Progress::ignored()).to_pixels();
        for (x, y) in [(0, 0), (31, 31), (0, 15), (16, 16)] {
            let c = out.get(x, y);
            assert!(
                (c.r as i32 - 90).abs() <= 1 && (c.b as i32 - 210).abs() <= 1,
                "blur shifted a flat colour at ({x},{y}) to {c:?}"
            );
        }
    }

    #[test]
    fn blurring_does_not_produce_dark_fringes() {
        // A soft-edged shape over transparency: straight-alpha blurring would
        // drag the colour toward black.
        let mut src = PixelBuffer::new(48, 48);
        for y in 0..48i32 {
            for x in 0..48i32 {
                let d = (((x - 24).pow(2) + (y - 24).pow(2)) as f32).sqrt();
                let a = (1.0 - (d / 14.0).clamp(0.0, 1.0)) * 255.0;
                src.set(x, y, Rgba8::new(255, 240, 0, a as u8));
            }
        }
        let out = gaussian_blur(&Plane::from_pixels(&src), 4.0, &Progress::ignored()).to_pixels();
        for y in 0..48i32 {
            for x in 0..48i32 {
                let c = out.get(x, y);
                if c.a > 40 {
                    assert!(c.r > 200 && c.g > 190, "fringe at ({x},{y}): {c:?}");
                }
            }
        }
    }

    #[test]
    fn blur_spreads_a_point_symmetrically() {
        let mut src = PixelBuffer::new(41, 41);
        src.set(20, 20, Rgba8::WHITE);
        let out = gaussian_blur(&Plane::from_pixels(&src), 3.0, &Progress::ignored());

        let centre = out.get(20, 20)[3];
        assert!(centre > 0.0 && centre < 1.0, "the point should have spread");
        // Symmetry in all four directions.
        for d in 1..8 {
            let a = out.get(20 + d, 20)[3];
            let b = out.get(20 - d, 20)[3];
            let c = out.get(20, 20 + d)[3];
            assert!((a - b).abs() < 1e-5 && (a - c).abs() < 1e-5, "asymmetric at distance {d}");
            assert!(a < centre, "should fall off with distance");
        }
    }

    #[test]
    fn a_separable_blur_conserves_total_energy() {
        let mut src = PixelBuffer::new(64, 64);
        src.fill_rect(crate::geom::IRect::new(20, 20, 44, 44), Rgba8::WHITE);
        let before: f32 = Plane::from_pixels(&src).data.iter().skip(3).step_by(4).sum();
        let after: f32 = gaussian_blur(&Plane::from_pixels(&src), 3.0, &Progress::ignored())
            .data
            .iter()
            .skip(3)
            .step_by(4)
            .sum();
        assert!(
            (before - after).abs() / before < 0.01,
            "energy changed from {before} to {after}"
        );
    }

    #[test]
    fn an_identity_kernel_leaves_the_image_alone() {
        let src = PixelBuffer::filled(16, 16, Rgba8::opaque(30, 60, 90));
        let mut kernel = [0.0f32; 9];
        kernel[4] = 1.0;
        let out = convolve_2d(&Plane::from_pixels(&src), &kernel, 3, 1.0, 0.0, &Progress::ignored()).to_pixels();
        assert_eq!(out.get(8, 8), Rgba8::opaque(30, 60, 90));
    }

    #[test]
    fn a_zero_divisor_is_treated_as_one() {
        // The Custom filter lets the user type a divisor; zero must not divide.
        let src = PixelBuffer::filled(8, 8, Rgba8::opaque(100, 100, 100));
        let mut kernel = [0.0f32; 9];
        kernel[4] = 1.0;
        let out = convolve_2d(&Plane::from_pixels(&src), &kernel, 3, 0.0, 0.0, &Progress::ignored()).to_pixels();
        assert_eq!(out.get(4, 4).r, 100);
    }

    #[test]
    fn the_random_source_is_deterministic_and_position_dependent() {
        let a: Vec<f32> = (0..8).map(|_| Rng::new(42).next_f32()).collect();
        assert!(a.iter().all(|v| (*v - a[0]).abs() < 1e-9), "same seed, same value");

        let mut rng = Rng::new(42);
        let sequence: Vec<f32> = (0..8).map(|_| rng.next_f32()).collect();
        let mut again = Rng::new(42);
        let repeat: Vec<f32> = (0..8).map(|_| again.next_f32()).collect();
        assert_eq!(sequence, repeat, "the sequence must repeat exactly");

        // Neighbouring pixels must not produce the same value.
        assert_ne!(Rng::at(7, 10, 10).next_f32(), Rng::at(7, 11, 10).next_f32());
        assert_ne!(Rng::at(7, 10, 10).next_f32(), Rng::at(7, 10, 11).next_f32());
        assert_eq!(Rng::at(7, 10, 10).next_f32(), Rng::at(7, 10, 10).next_f32());
    }

    #[test]
    fn random_values_stay_in_range() {
        let mut rng = Rng::new(1234);
        for _ in 0..10_000 {
            let v = rng.next_f32();
            assert!((0.0..1.0).contains(&v), "uniform out of range: {v}");
            assert!(rng.next_gaussian().abs() < 2.0, "gaussian ran away");
        }
    }

    #[test]
    fn bilinear_sampling_interpolates_between_pixels() {
        let mut src = PixelBuffer::new(2, 1);
        src.set(0, 0, Rgba8::BLACK);
        src.set(1, 0, Rgba8::WHITE);
        let plane = Plane::from_pixels(&src);
        let mid = plane.sample(1.0, 0.5);
        assert!((mid[0] - 0.5).abs() < 0.01, "expected mid-grey, got {}", mid[0]);
    }
}
