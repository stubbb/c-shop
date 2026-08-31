//! Sharpening, noise, stylising, and the morphological and convolution
//! filters conventionally grouped under *Other*.

use super::plane::{convolve_2d, fill_rows, gaussian_blur, Plane, Rng};
use crate::progress::Progress;

/// Sharpen by adding back the difference from a slight blur.
pub fn sharpen(src: &Plane, amount: f32, p: &Progress) -> Plane {
    unsharp_mask(src, amount, 1.0, 0.0, p)
}

/// Unsharp Mask: the general form of sharpening.
///
/// `threshold` protects flat areas — without it, sharpening a photograph
/// amplifies its sensor noise as enthusiastically as its detail.
pub fn unsharp_mask(src: &Plane, amount: f32, radius: f32, threshold: f32, p: &Progress) -> Plane {
    if amount.abs() < 1e-4 {
        return src.clone();
    }
    let blurred = gaussian_blur(src, (radius / 3.0).max(0.1), p);
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;

    fill_rows(&mut out, p, |y, row| {
        let y = y as i32;
        for x in 0..width as i32 {
            let sharp = src.get(x, y);
            let soft = blurred.get(x, y);
            let i = x as usize * 4;

            // Compare in straight alpha so the threshold means what it says.
            let a = sharp[3].max(1e-6);
            let difference = (0..3)
                .map(|c| ((sharp[c] - soft[c]) / a).abs())
                .fold(0.0f32, f32::max);

            if difference < threshold {
                row[i..i + 4].copy_from_slice(&sharp);
                continue;
            }
            for c in 0..3 {
                row[i + c] = sharp[c] + (sharp[c] - soft[c]) * amount;
            }
            // Alpha is left alone: sharpening should not eat into an edge.
            row[i + 3] = sharp[3];
        }
    });
    out
}

/// High Pass: keep only what a blur would remove, centred on mid-grey.
pub fn high_pass(src: &Plane, radius: f32, p: &Progress) -> Plane {
    let blurred = gaussian_blur(src, (radius / 3.0).max(0.1), p);
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;

    fill_rows(&mut out, p, |y, row| {
        let y = y as i32;
        for x in 0..width as i32 {
            let sharp = src.get(x, y);
            let soft = blurred.get(x, y);
            let a = sharp[3];
            let i = x as usize * 4;
            for c in 0..3 {
                // Premultiplied, so mid-grey is 0.5 * alpha.
                row[i + c] = (sharp[c] - soft[c] + 0.5 * a).clamp(0.0, a.max(0.0));
            }
            row[i + 3] = a;
        }
    });
    out
}

/// Add grain.
pub fn add_noise(src: &Plane, amount: f32, monochromatic: bool, gaussian: bool, seed: u64, p: &Progress) -> Plane {
    let mut out = src.clone();
    let width = src.width as usize;

    fill_rows(&mut out, p, |y, row| {
        let y = y as i32;
        for x in 0..width as i32 {
            let i = x as usize * 4;
            let a = row[i + 3];
            if a <= 1e-6 {
                continue;
            }
            // Seeded from the position, so the grain is stable across
            // previews and identical however the work is parallelised.
            let mut rng = Rng::at(seed, x, y);
            let noise = |rng: &mut Rng| {
                if gaussian {
                    rng.next_gaussian() * amount
                } else {
                    (rng.next_f32() - 0.5) * 2.0 * amount
                }
            };
            let shared = noise(&mut rng);
            for c in 0..3 {
                let n = if monochromatic { shared } else { noise(&mut rng) };
                // Work in straight alpha, then re-premultiply.
                let straight = (row[i + c] / a + n).clamp(0.0, 1.0);
                row[i + c] = straight * a;
            }
        }
    });
    out
}

/// Median filter: replaces each pixel with the median of its neighbourhood.
///
/// Removes speckle without the softening a blur would cause, because a median
/// picks an actual neighbouring value rather than averaging across an edge.
pub fn median(src: &Plane, radius: u32, p: &Progress) -> Plane {
    let r = radius as i32;
    if r == 0 {
        return src.clone();
    }
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;

    fill_rows(&mut out, p, |y, row| {
        let y = y as i32;
        let mut window: Vec<f32> = Vec::with_capacity(((r * 2 + 1) * (r * 2 + 1)) as usize);
        for x in 0..width as i32 {
            let i = x as usize * 4;
            // Each channel is taken independently, which is what the usual
            // does; a vector median would be slower and no better here.
            for c in 0..4 {
                window.clear();
                for dy in -r..=r {
                    for dx in -r..=r {
                        window.push(src.get(x + dx, y + dy)[c]);
                    }
                }
                window.sort_by(f32::total_cmp);
                row[i + c] = window[window.len() / 2];
            }
        }
    });
    out
}

/// Dust & Scratches: a median, but applied only where the neighbourhood varies
/// by more than `threshold`, so detail below that survives untouched.
pub fn dust_and_scratches(src: &Plane, radius: u32, threshold: f32, p: &Progress) -> Plane {
    let smoothed = median(src, radius.max(1), p);
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;

    fill_rows(&mut out, p, |y, row| {
        let y = y as i32;
        for x in 0..width as i32 {
            let original = src.get(x, y);
            let median = smoothed.get(x, y);
            let i = x as usize * 4;
            let difference =
                (0..3).map(|c| (original[c] - median[c]).abs()).fold(0.0f32, f32::max);
            let chosen = if difference > threshold { median } else { original };
            row[i..i + 4].copy_from_slice(&chosen);
        }
    });
    out
}

/// Morphological dilate (`maximum`) or erode (`minimum`).
pub fn morphology(src: &Plane, radius: u32, maximum: bool, p: &Progress) -> Plane {
    let r = radius as i32;
    if r == 0 {
        return src.clone();
    }
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;

    fill_rows(&mut out, p, |y, row| {
        let y = y as i32;
        for x in 0..width as i32 {
            let i = x as usize * 4;
            let mut best = if maximum { [f32::MIN; 4] } else { [f32::MAX; 4] };
            for dy in -r..=r {
                for dx in -r..=r {
                    // A round structuring element; a square one leaves
                    // obvious corners on every dilated shape.
                    if dx * dx + dy * dy > r * r {
                        continue;
                    }
                    let p = src.get(x + dx, y + dy);
                    for c in 0..4 {
                        best[c] = if maximum { best[c].max(p[c]) } else { best[c].min(p[c]) };
                    }
                }
            }
            row[i..i + 4].copy_from_slice(&best);
        }
    });
    out
}

/// Shift the image, optionally wrapping.
pub fn offset(src: &Plane, dx: i32, dy: i32, wrap: bool, _p: &Progress) -> Plane {
    let mut out = Plane::new(src.width, src.height);
    let (w, h) = (src.width as i32, src.height as i32);
    for y in 0..h {
        for x in 0..w {
            let (mut sx, mut sy) = (x - dx, y - dy);
            if wrap {
                sx = sx.rem_euclid(w);
                sy = sy.rem_euclid(h);
            } else if sx < 0 || sy < 0 || sx >= w || sy >= h {
                // Not wrapping means shifting in transparency, not the edge
                // pixel that clamping would give.
                continue;
            }
            out.set(x, y, src.get(sx, sy));
        }
    }
    out
}

/// Find Edges: the Sobel gradient magnitude, inverted so edges read dark on
/// white, which is how it is conventionally presented.
pub fn find_edges(src: &Plane, p: &Progress) -> Plane {
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;

    fill_rows(&mut out, p, |y, row| {
        let y = y as i32;
        for x in 0..width as i32 {
            let i = x as usize * 4;
            let a = src.get(x, y)[3];
            for c in 0..3 {
                let at = |dx: i32, dy: i32| src.get(x + dx, y + dy)[c];
                let gx = at(-1, -1) + 2.0 * at(-1, 0) + at(-1, 1)
                    - at(1, -1)
                    - 2.0 * at(1, 0)
                    - at(1, 1);
                let gy = at(-1, -1) + 2.0 * at(0, -1) + at(1, -1)
                    - at(-1, 1)
                    - 2.0 * at(0, 1)
                    - at(1, 1);
                let magnitude = (gx * gx + gy * gy).sqrt();
                row[i + c] = ((a - magnitude).clamp(0.0, a.max(0.0))).min(a.max(0.0));
            }
            row[i + 3] = a;
        }
    });
    out
}

/// Emboss: a directional gradient over flat grey.
pub fn emboss(src: &Plane, angle_degrees: f32, height: f32, amount: f32, p: &Progress) -> Plane {
    let (sin, cos) = angle_degrees.to_radians().sin_cos();
    let dx = cos * height;
    let dy = -sin * height;
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;

    fill_rows(&mut out, p, |y, row| {
        let py = y as f32 + 0.5;
        for x in 0..width {
            let px = x as f32 + 0.5;
            let i = x * 4;
            let a = src.get(x as i32, y as i32)[3];
            let ahead = src.sample(px + dx, py + dy);
            let behind = src.sample(px - dx, py - dy);
            for c in 0..3 {
                // Grey plus the directional difference.
                row[i + c] =
                    (0.5 * a + (ahead[c] - behind[c]) * amount).clamp(0.0, a.max(0.0));
            }
            row[i + 3] = a;
        }
    });
    out
}

/// Invert every channel above mid-grey, leaving the rest — the photographic
/// solarisation effect.
pub fn solarize(src: &Plane, _p: &Progress) -> Plane {
    let mut out = src.clone();
    for chunk in out.data.chunks_exact_mut(4) {
        let a = chunk[3];
        if a <= 1e-6 {
            continue;
        }
        for channel in chunk.iter_mut().take(3) {
            let straight = *channel / a;
            let solarised = if straight > 0.5 { 1.0 - straight } else { straight };
            *channel = solarised * a;
        }
    }
    out
}

/// Diffuse: shuffle each pixel with a random neighbour.
pub fn diffuse(src: &Plane, amount: u32, seed: u64, p: &Progress) -> Plane {
    let r = amount.max(1) as i32;
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;

    fill_rows(&mut out, p, |y, row| {
        let y = y as i32;
        for x in 0..width as i32 {
            let mut rng = Rng::at(seed, x, y);
            let dx = (rng.next_f32() * (2 * r + 1) as f32) as i32 - r;
            let dy = (rng.next_f32() * (2 * r + 1) as f32) as i32 - r;
            let i = x as usize * 4;
            row[i..i + 4].copy_from_slice(&src.get(x + dx, y + dy));
        }
    });
    out
}

/// Apply an arbitrary 5x5 kernel.
pub fn custom(src: &Plane, kernel: &[f32; 25], divisor: f32, offset: f32, p: &Progress) -> Plane {
    convolve_2d(src, kernel, 5, divisor, offset, p)
}
