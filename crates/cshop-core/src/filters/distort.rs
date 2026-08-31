//! Geometric filters: the distortions and the pixelating effects.
//!
//! All of these are backward maps — for each destination pixel, work out where
//! it came from and sample there. Mapping forward would leave holes wherever
//! the transform stretches.

use super::plane::{fill_rows, Plane, Rng};
use crate::progress::Progress;

/// Run a backward map over the whole plane.
///
/// `map` receives a destination position in pixels and returns the source
/// position to sample.
fn warp(src: &Plane, p: &Progress, map: impl Fn(f32, f32) -> (f32, f32) + Sync) -> Plane {
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;
    fill_rows(&mut out, p, |y, row| {
        let py = y as f32 + 0.5;
        for x in 0..width {
            let (sx, sy) = map(x as f32 + 0.5, py);
            let sample = src.sample(sx, sy);
            let i = x * 4;
            row[i..i + 4].copy_from_slice(&sample);
        }
    });
    out
}

/// Geometry shared by the filters that work in a circle inscribed in the image.
struct Polar {
    cx: f32,
    cy: f32,
    radius: f32,
}

impl Polar {
    fn of(src: &Plane) -> Polar {
        let cx = src.width as f32 / 2.0;
        let cy = src.height as f32 / 2.0;
        Polar { cx, cy, radius: cx.min(cy).max(1.0) }
    }
}

/// Rotate by an amount that falls off to nothing at the edge of the circle.
pub fn twirl(src: &Plane, angle_degrees: f32, p: &Progress) -> Plane {
    let g = Polar::of(src);
    let max_angle = angle_degrees.to_radians();
    warp(src, p, move |x, y| {
        let dx = x - g.cx;
        let dy = y - g.cy;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance >= g.radius {
            return (x, y);
        }
        // Full twist at the centre, none at the rim.
        let t = 1.0 - distance / g.radius;
        let angle = max_angle * t * t;
        let (sin, cos) = angle.sin_cos();
        (g.cx + dx * cos - dy * sin, g.cy + dx * sin + dy * cos)
    })
}

/// Pull toward the centre (positive) or push outward (negative).
pub fn pinch(src: &Plane, amount: f32, p: &Progress) -> Plane {
    let g = Polar::of(src);
    let amount = amount.clamp(-1.0, 1.0);
    warp(src, p, move |x, y| {
        let dx = x - g.cx;
        let dy = y - g.cy;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance >= g.radius || distance < 1e-4 {
            return (x, y);
        }
        let t = distance / g.radius;
        // Raising the normalised radius to a power bunches samples toward the
        // centre or spreads them outward.
        let scaled = t.powf(1.0 + amount) / t;
        (g.cx + dx * scaled, g.cy + dy * scaled)
    })
}

/// Bulge the image as if wrapped over a sphere, or dent it inward.
pub fn spherize(src: &Plane, amount: f32, p: &Progress) -> Plane {
    let g = Polar::of(src);
    let amount = amount.clamp(-1.0, 1.0);
    warp(src, p, move |x, y| {
        let dx = x - g.cx;
        let dy = y - g.cy;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance >= g.radius || distance < 1e-4 {
            return (x, y);
        }
        let t = distance / g.radius;
        // The refraction of a sphere: sin of the arc, not the arc itself.
        let bulge = (t * std::f32::consts::FRAC_PI_2).sin();
        let scaled = 1.0 + amount * (bulge / t - 1.0);
        (g.cx + dx / scaled, g.cy + dy / scaled)
    })
}

/// Sinusoidal displacement along one axis.
pub fn wave(src: &Plane, amplitude: f32, wavelength: f32, vertical: bool, p: &Progress) -> Plane {
    let wavelength = wavelength.max(1.0);
    warp(src, p, move |x, y| {
        let phase = if vertical { x } else { y } / wavelength * std::f32::consts::TAU;
        let shift = phase.sin() * amplitude;
        if vertical {
            (x, y + shift)
        } else {
            (x + shift, y)
        }
    })
}

/// Convert between rectangular and polar coordinates.
pub fn polar_coordinates(src: &Plane, to_polar: bool, p: &Progress) -> Plane {
    let w = src.width as f32;
    let h = src.height as f32;
    let cx = w / 2.0;
    let cy = h / 2.0;
    let max_radius = cx.hypot(cy);

    warp(src, p, move |x, y| {
        if to_polar {
            // Destination x is the angle, y is the radius.
            let angle = (x / w) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            let radius = (y / h) * max_radius;
            (cx + radius * angle.cos(), cy + radius * angle.sin())
        } else {
            let dx = x - cx;
            let dy = y - cy;
            let radius = (dx * dx + dy * dy).sqrt();
            let mut angle = dy.atan2(dx) + std::f32::consts::FRAC_PI_2;
            if angle < 0.0 {
                angle += std::f32::consts::TAU;
            }
            (angle / std::f32::consts::TAU * w, radius / max_radius * h)
        }
    })
}

/// Mosaic: average each cell and fill it with that colour.
pub fn mosaic(src: &Plane, size: u32, p: &Progress) -> Plane {
    let size = size.max(1) as i32;
    let mut out = Plane::new(src.width, src.height);
    let (w, h) = (src.width as i32, src.height as i32);

    let mut cy = 0;
    while cy < h {
        let mut cx = 0;
        while cx < w {
            let mut acc = [0.0f32; 4];
            let mut n = 0.0f32;
            for y in cy..(cy + size).min(h) {
                for x in cx..(cx + size).min(w) {
                    let p = src.get(x, y);
                    for c in 0..4 {
                        acc[c] += p[c];
                    }
                    n += 1.0;
                }
            }
            if n > 0.0 {
                let mean = [acc[0] / n, acc[1] / n, acc[2] / n, acc[3] / n];
                for y in cy..(cy + size).min(h) {
                    for x in cx..(cx + size).min(w) {
                        out.set(x, y, mean);
                    }
                }
            }
            cx += size;
        }
        cy += size;
        // Counted in rows rather than in bands, so the total is the image's
        // height like every other filter's.
        p.advance(size.min(h - cy + size).max(0) as u64);
        if p.cancelled() {
            break;
        }
    }
    out
}

/// Crystallize: a Voronoi mosaic with jittered cell centres, so the cells look
/// like crystals rather than a grid.
pub fn crystallize(src: &Plane, size: u32, seed: u64, p: &Progress) -> Plane {
    let size = size.max(2) as i32;
    let (w, h) = (src.width as i32, src.height as i32);

    // One jittered site per grid cell, so finding the nearest only needs to
    // look at the neighbouring cells rather than every site in the image.
    let cols = w.div_euclid(size) + 2;
    let rows = h.div_euclid(size) + 2;
    let site = |gx: i32, gy: i32| -> (f32, f32) {
        let mut rng = Rng::at(seed, gx, gy);
        (
            (gx as f32 + rng.next_f32()) * size as f32,
            (gy as f32 + rng.next_f32()) * size as f32,
        )
    };

    // Average the source over each site's cell.
    let mut sums = vec![[0.0f32; 5]; (cols * rows) as usize];
    for y in 0..h {
        p.advance(1);
        if p.cancelled() {
            break;
        }
        for x in 0..w {
            let (gx, gy) = nearest_site(x, y, size, &site);
            let idx = (gy.clamp(0, rows - 1) * cols + gx.clamp(0, cols - 1)) as usize;
            let p = src.get(x, y);
            for c in 0..4 {
                sums[idx][c] += p[c];
            }
            sums[idx][4] += 1.0;
        }
    }

    let mut out = Plane::new(src.width, src.height);
    let sums = &sums;
    fill_rows(&mut out, p, |y, row| {
        let y = y as i32;
        for x in 0..w {
            let (gx, gy) = nearest_site(x, y, size, &site);
            let idx = (gy.clamp(0, rows - 1) * cols + gx.clamp(0, cols - 1)) as usize;
            let n = sums[idx][4].max(1.0);
            let i = x as usize * 4;
            for c in 0..4 {
                row[i + c] = sums[idx][c] / n;
            }
        }
    });
    out
}

/// Grid coordinates of the site nearest to a pixel.
fn nearest_site(
    x: i32,
    y: i32,
    size: i32,
    site: &impl Fn(i32, i32) -> (f32, f32),
) -> (i32, i32) {
    let gx = x.div_euclid(size);
    let gy = y.div_euclid(size);
    let mut best = (gx, gy);
    let mut best_distance = f32::MAX;
    // The nearest jittered site is always in one of the nine surrounding
    // cells, because each site stays inside its own cell.
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (sx, sy) = site(gx + dx, gy + dy);
            let d = (sx - x as f32).powi(2) + (sy - y as f32).powi(2);
            if d < best_distance {
                best_distance = d;
                best = (gx + dx, gy + dy);
            }
        }
    }
    best
}

/// Fragment: four offset copies averaged together, as if slightly out of
/// register.
pub fn fragment(src: &Plane, distance: i32, p: &Progress) -> Plane {
    let d = distance.max(1);
    let mut out = Plane::new(src.width, src.height);
    let width = src.width as usize;
    fill_rows(&mut out, p, |y, row| {
        let y = y as i32;
        for x in 0..width as i32 {
            let mut acc = [0.0f32; 4];
            for (dx, dy) in [(-d, -d), (d, -d), (-d, d), (d, d)] {
                let p = src.get(x + dx, y + dy);
                for c in 0..4 {
                    acc[c] += p[c] * 0.25;
                }
            }
            let i = x as usize * 4;
            row[i..i + 4].copy_from_slice(&acc);
        }
    });
    out
}
