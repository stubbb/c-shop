//! The blur family.

use super::plane::{convolve_1d, gaussian_blur, Plane};
use rayon::prelude::*;

/// Gaussian blur, given a *radius* rather than a standard deviation.
///
/// That radius is roughly three sigma — the point where the curve has
/// effectively fallen to nothing — so a radius of 3 has to mean sigma 1, not
/// sigma 3, or every blur comes out three times too strong.
pub fn gaussian(src: &Plane, radius: f32) -> Plane {
    gaussian_blur(src, radius / 3.0)
}

/// Box blur: a flat kernel, run twice per axis so the result is smooth enough
/// to be useful rather than obviously square.
pub fn box_blur(src: &Plane, radius: f32) -> Plane {
    let r = radius.round().max(0.0) as usize;
    if r == 0 {
        return src.clone();
    }
    let width = r * 2 + 1;
    let kernel = vec![1.0 / width as f32; width];
    let h = convolve_1d(src, &kernel, true);
    convolve_1d(&h, &kernel, false)
}

/// Directional blur along a line.
pub fn motion(src: &Plane, angle_degrees: f32, distance: f32) -> Plane {
    let distance = distance.max(0.0);
    if distance < 1.0 {
        return src.clone();
    }
    let steps = (distance.round() as i32).max(1);
    let (sin, cos) = angle_degrees.to_radians().sin_cos();
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;

    out.data.par_chunks_mut(width * 4).enumerate().for_each(|(y, row)| {
        let y = y as f32;
        for x in 0..width {
            let mut acc = [0.0f32; 4];
            // Sample along the line, centred on the pixel.
            for s in 0..steps {
                let t = s as f32 / steps as f32 - 0.5;
                let p = src.sample(
                    x as f32 + 0.5 + cos * t * distance,
                    y + 0.5 - sin * t * distance,
                );
                for c in 0..4 {
                    acc[c] += p[c];
                }
            }
            let i = x * 4;
            for c in 0..4 {
                row[i + c] = acc[c] / steps as f32;
            }
        }
    });
    out
}

/// Which way Radial Blur smears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadialKind {
    /// Around the centre.
    Spin,
    /// Outward from the centre.
    Zoom,
}

/// Spin or zoom blur about a point given in `0..=1` of the image.
pub fn radial(src: &Plane, amount: f32, kind: RadialKind, centre: (f32, f32)) -> Plane {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.001 {
        return src.clone();
    }
    let cx = centre.0 * src.width as f32;
    let cy = centre.1 * src.height as f32;
    // Enough samples that the smear reads as continuous rather than stepped.
    let steps = 24;
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;

    out.data.par_chunks_mut(width * 4).enumerate().for_each(|(y, row)| {
        let py = y as f32 + 0.5;
        for x in 0..width {
            let px = x as f32 + 0.5;
            let dx = px - cx;
            let dy = py - cy;
            let radius = (dx * dx + dy * dy).sqrt();
            let angle = dy.atan2(dx);

            let mut acc = [0.0f32; 4];
            for s in 0..steps {
                let t = s as f32 / (steps - 1) as f32 - 0.5;
                let sample = match kind {
                    RadialKind::Spin => {
                        // A fixed angular sweep, so the smear lengthens with
                        // distance exactly as a real rotation would.
                        let a = angle + t * amount * 0.5;
                        (cx + radius * a.cos(), cy + radius * a.sin())
                    }
                    RadialKind::Zoom => {
                        let scale = 1.0 + t * amount;
                        (cx + dx * scale, cy + dy * scale)
                    }
                };
                let p = src.sample(sample.0, sample.1);
                for c in 0..4 {
                    acc[c] += p[c];
                }
            }
            let i = x * 4;
            for c in 0..4 {
                row[i + c] = acc[c] / steps as f32;
            }
        }
    });
    out
}

/// Edge-preserving blur: neighbours only contribute when their tone is within
/// `threshold` of the centre, so flat areas smooth out while edges stay put.
pub fn surface(src: &Plane, radius: f32, threshold: f32) -> Plane {
    let r = radius.round().max(0.0) as i32;
    if r == 0 {
        return src.clone();
    }
    let threshold = threshold.clamp(0.001, 1.0);
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;

    out.data.par_chunks_mut(width * 4).enumerate().for_each(|(y, row)| {
        let y = y as i32;
        for x in 0..width as i32 {
            let centre = src.luma(x, y);
            let mut acc = [0.0f32; 4];
            let mut total = 0.0f32;
            for dy in -r..=r {
                for dx in -r..=r {
                    let difference = (src.luma(x + dx, y + dy) - centre).abs();
                    if difference > threshold {
                        continue;
                    }
                    // Weight falls off with tonal distance, not just spatial.
                    let w = 1.0 - difference / threshold;
                    let p = src.get(x + dx, y + dy);
                    for c in 0..4 {
                        acc[c] += p[c] * w;
                    }
                    total += w;
                }
            }
            let i = x as usize * 4;
            if total > 1e-6 {
                for c in 0..4 {
                    row[i + c] = acc[c] / total;
                }
            } else {
                row[i..i + 4].copy_from_slice(&src.get(x, y));
            }
        }
    });
    out
}

/// Replace everything with the image's mean colour.
pub fn average(src: &Plane) -> Plane {
    let mean = src.mean();
    let mut out = Plane::new(src.width, src.height);
    for chunk in out.data.chunks_exact_mut(4) {
        chunk.copy_from_slice(&mean);
    }
    out
}
