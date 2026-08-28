//! Filters that generate an image rather than transform one.

use super::plane::{Plane, Rng};
use crate::color::Rgba8;
use rayon::prelude::*;

/// Value noise at a point, interpolated smoothly between lattice values.
fn value_noise(x: f32, y: f32, seed: u64) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let tx = x - x0;
    let ty = y - y0;
    // Smoothstep, so the lattice does not show as a visible grid.
    let sx = tx * tx * (3.0 - 2.0 * tx);
    let sy = ty * ty * (3.0 - 2.0 * ty);

    let at = |ix: f32, iy: f32| Rng::at(seed, ix as i32, iy as i32).next_f32();
    let a = at(x0, y0);
    let b = at(x0 + 1.0, y0);
    let c = at(x0, y0 + 1.0);
    let d = at(x0 + 1.0, y0 + 1.0);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    top + (bottom - top) * sy
}

/// Fractal noise: several octaves of value noise, each finer and fainter.
fn fbm(x: f32, y: f32, seed: u64, octaves: u32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut normaliser = 0.0;
    for octave in 0..octaves {
        total += value_noise(x * frequency, y * frequency, seed ^ (octave as u64 * 0x9E37)) * amplitude;
        normaliser += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    total / normaliser.max(1e-6)
}

/// Clouds: fractal noise between the foreground and background colours.
///
/// `difference` reproduces Difference Clouds, which blends the generated
/// clouds into the existing image with a difference blend instead of replacing
/// it.
pub fn clouds(
    src: &Plane,
    scale: f32,
    seed: u64,
    foreground: Rgba8,
    background: Rgba8,
    difference: bool,
) -> Plane {
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;
    // Larger scale means larger features, so it divides the coordinates.
    let step = 1.0 / scale.max(1.0);
    let fg = foreground.to_f32();
    let bg = background.to_f32();

    out.data.par_chunks_mut(width * 4).enumerate().for_each(|(y, row)| {
        let py = y as f32 * step;
        for x in 0..width {
            let t = fbm(x as f32 * step, py, seed, 6).clamp(0.0, 1.0);
            let colour = [
                bg.r + (fg.r - bg.r) * t,
                bg.g + (fg.g - bg.g) * t,
                bg.b + (fg.b - bg.b) * t,
            ];
            let i = x * 4;
            if difference {
                // Difference against what is already there, keeping its alpha.
                let existing = src.get(x as i32, y as i32);
                let a = existing[3];
                for c in 0..3 {
                    let straight = if a > 1e-6 { existing[c] / a } else { 0.0 };
                    row[i + c] = (straight - colour[c]).abs() * a;
                }
                row[i + 3] = a;
            } else {
                // Clouds fill the layer, so they are opaque.
                row[i..i + 3].copy_from_slice(&colour);
                row[i + 3] = 1.0;
            }
        }
    });
    out
}

/// Fibers: vertical streaks between the two colours, as if woven.
pub fn fibers(src: &Plane, strength: f32, length: f32, seed: u64, foreground: Rgba8, background: Rgba8) -> Plane {
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;
    let fg = foreground.to_f32();
    let bg = background.to_f32();
    // Longer fibres mean slower variation down the image.
    let vertical_step = 1.0 / length.max(1.0);
    let variance = strength.clamp(0.0, 1.0);

    out.data.par_chunks_mut(width * 4).enumerate().for_each(|(y, row)| {
        let py = y as f32 * vertical_step;
        for x in 0..width {
            // High horizontal frequency, low vertical: that is what makes a
            // streak rather than a blob.
            let t = fbm(x as f32 * 0.6, py, seed, 4);
            let shaded = ((t - 0.5) * (1.0 + variance * 3.0) + 0.5).clamp(0.0, 1.0);
            let i = x * 4;
            for (c, (f, b)) in [(fg.r, bg.r), (fg.g, bg.g), (fg.b, bg.b)].into_iter().enumerate() {
                row[i + c] = b + (f - b) * shaded;
            }
            row[i + 3] = 1.0;
        }
    });
    out
}
