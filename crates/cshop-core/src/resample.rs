//! Resampling: applying a transform to pixels, and resizing an image.
//!
//! # Premultiplied filtering
//!
//! Every filter here works on premultiplied alpha and un-premultiplies at the
//! end. Interpolating straight-alpha colour mixes in the colour of transparent
//! pixels, which shows up as a dark halo around anything with a soft edge —
//! the single most common resampling bug.
//!
//! # Two paths, deliberately
//!
//! * [`transform`] maps each destination pixel back through the inverse matrix
//!   and samples once. That is right for rotation and perspective, and fast
//!   enough to run interactively.
//! * [`resize`] uses a separable filter whose support widens with the
//!   reduction factor. Point-sampling a large downscale aliases badly however
//!   good the filter kernel is, because most source pixels are never read.

use crate::color::Rgba8;
use crate::geom::IRect;
use crate::pixels::PixelBuffer;
use crate::transform::Transform;

/// Reconstruction filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Resampling {
    /// Hard pixels; the right choice for pixel art and for anything that must
    /// not gain new colours.
    Nearest,
    /// Fast and soft.
    Bilinear,
    /// Catmull–Rom: sharper than bilinear, with a little overshoot that reads
    /// as crispness.
    #[default]
    Bicubic,
    /// Lanczos-3, for high-quality reduction.
    Lanczos3,
}

impl Resampling {
    pub fn name(self) -> &'static str {
        match self {
            Resampling::Nearest => "Nearest Neighbour",
            Resampling::Bilinear => "Bilinear",
            Resampling::Bicubic => "Bicubic",
            Resampling::Lanczos3 => "Lanczos",
        }
    }

    pub const ALL: [Resampling; 4] =
        [Resampling::Nearest, Resampling::Bilinear, Resampling::Bicubic, Resampling::Lanczos3];

    /// Half-width of the filter's support, in source pixels.
    fn radius(self) -> f32 {
        match self {
            Resampling::Nearest => 0.5,
            Resampling::Bilinear => 1.0,
            Resampling::Bicubic => 2.0,
            Resampling::Lanczos3 => 3.0,
        }
    }

    /// Resampling weight at distance `x` from the sample centre.
    fn weight(self, x: f32) -> f32 {
        let x = x.abs();
        match self {
            Resampling::Nearest => {
                if x <= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            Resampling::Bilinear => (1.0 - x).max(0.0),
            Resampling::Bicubic => {
                // Catmull-Rom (a = -0.5).
                const A: f32 = -0.5;
                if x < 1.0 {
                    (A + 2.0) * x * x * x - (A + 3.0) * x * x + 1.0
                } else if x < 2.0 {
                    A * x * x * x - 5.0 * A * x * x + 8.0 * A * x - 4.0 * A
                } else {
                    0.0
                }
            }
            Resampling::Lanczos3 => {
                if x < 1e-6 {
                    1.0
                } else if x < 3.0 {
                    let px = std::f32::consts::PI * x;
                    3.0 * (px).sin() * (px / 3.0).sin() / (px * px)
                } else {
                    0.0
                }
            }
        }
    }
}

/// A pixel in premultiplied form, for filtering.
#[derive(Clone, Copy, Default)]
struct Premul {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

impl Premul {
    #[inline]
    fn of(c: Rgba8) -> Premul {
        let a = c.a as f32 / 255.0;
        Premul {
            r: c.r as f32 / 255.0 * a,
            g: c.g as f32 / 255.0 * a,
            b: c.b as f32 / 255.0 * a,
            a,
        }
    }

    #[inline]
    fn to_rgba8(self) -> Rgba8 {
        let a = self.a.clamp(0.0, 1.0);
        if a <= 1e-6 {
            return Rgba8::TRANSPARENT;
        }
        let f = |v: f32| ((v / a).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        Rgba8::new(f(self.r), f(self.g), f(self.b), (a * 255.0 + 0.5) as u8)
    }
}

/// Sample `src` at a fractional position with the given filter.
///
/// Positions outside the buffer contribute transparency, so an edge fades out
/// rather than smearing its last row.
fn sample(src: &PixelBuffer, x: f32, y: f32, filter: Resampling) -> Rgba8 {
    if filter == Resampling::Nearest {
        return src.get(x.floor() as i32, y.floor() as i32);
    }

    // Sample positions sit at pixel centres.
    let (cx, cy) = (x - 0.5, y - 0.5);
    let r = filter.radius();
    let x0 = (cx - r + 1.0).floor() as i32;
    let x1 = (cx + r).floor() as i32;
    let y0 = (cy - r + 1.0).floor() as i32;
    let y1 = (cy + r).floor() as i32;

    let mut acc = Premul::default();
    let mut total = 0.0f32;
    for sy in y0..=y1 {
        let wy = filter.weight(cy - sy as f32);
        if wy == 0.0 {
            continue;
        }
        for sx in x0..=x1 {
            let w = wy * filter.weight(cx - sx as f32);
            if w == 0.0 {
                continue;
            }
            let p = Premul::of(src.get(sx, sy));
            acc.r += p.r * w;
            acc.g += p.g * w;
            acc.b += p.b * w;
            acc.a += p.a * w;
            total += w;
        }
    }
    if total.abs() < 1e-6 {
        return Rgba8::TRANSPARENT;
    }
    // Normalising by the weights actually used keeps edges from darkening
    // where part of the kernel fell outside the image.
    Premul {
        r: acc.r / total,
        g: acc.g / total,
        b: acc.b / total,
        a: (acc.a / total).clamp(0.0, 1.0),
    }
    .to_rgba8()
}

/// Apply `matrix` to `src`, whose top-left sits at `offset` in document space.
///
/// Returns the transformed pixels and their new document-space offset, or
/// `None` if the transform collapses the layer to nothing.
pub fn transform(
    src: &PixelBuffer,
    offset: (i32, i32),
    matrix: Transform,
    filter: Resampling,
    clip: Option<IRect>,
) -> Option<(PixelBuffer, (i32, i32))> {
    let src_rect = IRect::at(offset.0, offset.1, src.width(), src.height());
    let mut dst_rect = matrix.transformed_bounds(src_rect);
    if let Some(clip) = clip {
        // Nothing outside the canvas is worth rendering, and a wild handle drag
        // could otherwise ask for a buffer of billions of pixels.
        dst_rect = dst_rect.intersect(&clip);
    }
    if dst_rect.is_empty() {
        return None;
    }

    let inverse = matrix.invert()?;
    let mut out = PixelBuffer::new(dst_rect.width(), dst_rect.height());

    for y in 0..dst_rect.height() as i32 {
        for x in 0..dst_rect.width() as i32 {
            // Sample at the destination pixel's centre, mapped back to source.
            let doc = crate::geom::Vec2::new(
                (dst_rect.x0 + x) as f32 + 0.5,
                (dst_rect.y0 + y) as f32 + 0.5,
            );
            let s = inverse.apply(doc);
            let local_x = s.x - offset.0 as f32;
            let local_y = s.y - offset.1 as f32;
            // A generous margin so filters that reach outside still contribute.
            if local_x < -4.0
                || local_y < -4.0
                || local_x > src.width() as f32 + 4.0
                || local_y > src.height() as f32 + 4.0
            {
                continue;
            }
            out.set(x, y, sample(src, local_x, local_y, filter));
        }
    }

    Some((out, (dst_rect.x0, dst_rect.y0)))
}

/// Resize to exactly `width` x `height`.
///
/// Separable and area-aware: when reducing, the filter's support grows with
/// the reduction factor so every source pixel contributes. That is the
/// difference between a clean downscale and a shimmering, aliased one.
pub fn resize(src: &PixelBuffer, width: u32, height: u32, filter: Resampling) -> PixelBuffer {
    let (width, height) = (width.max(1), height.max(1));
    if src.width() == width && src.height() == height {
        return src.clone();
    }
    if src.width() == 0 || src.height() == 0 {
        return PixelBuffer::new(width, height);
    }

    // Horizontal pass into an intermediate, then vertical: two 1D passes
    // instead of one 2D kernel.
    let horizontal = resize_axis(src, width, filter, true);
    resize_axis(&horizontal, height, filter, false)
}

/// Resample along one axis. `horizontal` selects which.
fn resize_axis(src: &PixelBuffer, target: u32, filter: Resampling, horizontal: bool) -> PixelBuffer {
    let (src_len, other) = if horizontal {
        (src.width(), src.height())
    } else {
        (src.height(), src.width())
    };
    let (w, h) = if horizontal { (target, other) } else { (other, target) };
    let mut out = PixelBuffer::new(w, h);
    if src_len == 0 {
        return out;
    }

    let scale = target as f32 / src_len as f32;
    // Reducing widens the kernel; enlarging leaves it at its natural width.
    let support = if scale < 1.0 { filter.radius() / scale } else { filter.radius() };
    let inv = if scale < 1.0 { scale } else { 1.0 };

    // Weights depend only on the output index, so build them once per column
    // and reuse across every row.
    for o in 0..target {
        let centre = (o as f32 + 0.5) / scale;
        let from = ((centre - support).floor() as i32).max(0);
        let to = ((centre + support).ceil() as i32).min(src_len as i32 - 1);

        let mut weights: Vec<(i32, f32)> = Vec::with_capacity((to - from + 1).max(1) as usize);
        let mut total = 0.0f32;
        for s in from..=to {
            let w = filter.weight((s as f32 + 0.5 - centre) * inv);
            if w != 0.0 {
                weights.push((s, w));
                total += w;
            }
        }
        if weights.is_empty() || total.abs() < 1e-9 {
            // Degenerate kernel: fall back to the nearest source pixel rather
            // than leaving a transparent column.
            let s = (centre.floor() as i32).clamp(0, src_len as i32 - 1);
            weights.push((s, 1.0));
            total = 1.0;
        }

        for q in 0..other {
            let mut acc = Premul::default();
            for &(s, weight) in &weights {
                let c = if horizontal { src.get(s, q as i32) } else { src.get(q as i32, s) };
                let p = Premul::of(c);
                acc.r += p.r * weight;
                acc.g += p.g * weight;
                acc.b += p.b * weight;
                acc.a += p.a * weight;
            }
            let px = Premul {
                r: acc.r / total,
                g: acc.g / total,
                b: acc.b / total,
                a: (acc.a / total).clamp(0.0, 1.0),
            }
            .to_rgba8();
            if horizontal {
                out.set(o as i32, q as i32, px);
            } else {
                out.set(q as i32, o as i32, px);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Vec2;

    fn checker(w: u32, h: u32) -> PixelBuffer {
        let mut px = PixelBuffer::new(w, h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let on = (x + y) % 2 == 0;
                px.set(x, y, if on { Rgba8::WHITE } else { Rgba8::BLACK });
            }
        }
        px
    }

    #[test]
    fn the_identity_transform_reproduces_the_image() {
        let src = checker(16, 16);
        for filter in Resampling::ALL {
            let (out, offset) =
                transform(&src, (0, 0), Transform::IDENTITY, filter, None).unwrap();
            assert_eq!(offset, (0, 0));
            assert_eq!((out.width(), out.height()), (16, 16));
            // Bicubic and Lanczos ring slightly even at unit scale, so allow a
            // level or two rather than demanding an exact copy.
            for y in 2..14 {
                for x in 2..14 {
                    let a = src.get(x, y);
                    let b = out.get(x, y);
                    assert!(
                        (a.r as i32 - b.r as i32).abs() <= 2,
                        "{filter:?} changed ({x},{y}): {a:?} -> {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn translation_moves_the_pixels_and_the_offset() {
        let src = PixelBuffer::filled(8, 8, Rgba8::WHITE);
        let (out, offset) =
            transform(&src, (0, 0), Transform::translate(20.0, 5.0), Resampling::Nearest, None)
                .unwrap();
        assert_eq!(offset, (20, 5));
        assert_eq!(out.get(4, 4), Rgba8::WHITE);
    }

    #[test]
    fn a_quarter_turn_swaps_the_dimensions() {
        let src = PixelBuffer::filled(20, 8, Rgba8::WHITE);
        let t = Transform::rotate(std::f32::consts::FRAC_PI_2);
        let (out, _) = transform(&src, (0, 0), t, Resampling::Bilinear, None).unwrap();
        assert_eq!((out.width(), out.height()), (8, 20), "20x8 rotated is 8x20");
    }

    #[test]
    fn scaling_up_doubles_the_size() {
        let src = PixelBuffer::filled(10, 10, Rgba8::opaque(200, 100, 50));
        let (out, _) =
            transform(&src, (0, 0), Transform::scale(2.0, 2.0), Resampling::Bilinear, None).unwrap();
        assert_eq!((out.width(), out.height()), (20, 20));
        assert_eq!(out.get(10, 10), Rgba8::opaque(200, 100, 50));
    }

    #[test]
    fn a_collapsed_transform_yields_nothing() {
        let src = PixelBuffer::filled(8, 8, Rgba8::WHITE);
        assert!(transform(&src, (0, 0), Transform::scale(0.0, 1.0), Resampling::Bilinear, None).is_none());
    }

    #[test]
    fn a_transform_can_be_clipped_to_the_canvas() {
        // Without a clip, a huge scale would allocate a huge buffer.
        let src = PixelBuffer::filled(10, 10, Rgba8::WHITE);
        let canvas = IRect::new(0, 0, 100, 100);
        let (out, offset) =
            transform(&src, (0, 0), Transform::scale(500.0, 500.0), Resampling::Nearest, Some(canvas))
                .unwrap();
        assert_eq!(offset, (0, 0));
        assert!(out.width() <= 100 && out.height() <= 100, "clip should bound the result");
    }

    #[test]
    fn resampling_does_not_produce_dark_fringes() {
        // The classic bug: a soft-edged shape over transparency picking up the
        // colour of the transparent pixels when scaled.
        let mut src = PixelBuffer::new(32, 32);
        for y in 0..32i32 {
            for x in 0..32i32 {
                let d = (((x - 16) * (x - 16) + (y - 16) * (y - 16)) as f32).sqrt();
                let a = (1.0 - (d / 12.0).clamp(0.0, 1.0)) * 255.0;
                // Bright yellow with a feathered edge.
                src.set(x, y, Rgba8::new(255, 255, 0, a as u8));
            }
        }

        for filter in [Resampling::Bilinear, Resampling::Bicubic, Resampling::Lanczos3] {
            let out = resize(&src, 96, 96, filter);
            // Anywhere with meaningful alpha, the colour must still be yellow —
            // straight-alpha filtering would drag it toward black.
            for y in 0..96i32 {
                for x in 0..96i32 {
                    let c = out.get(x, y);
                    if c.a > 40 {
                        assert!(
                            c.r > 200 && c.g > 200,
                            "{filter:?} produced a fringe at ({x},{y}): {c:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn resize_hits_the_requested_dimensions() {
        let src = checker(37, 23);
        for filter in Resampling::ALL {
            for (w, h) in [(1u32, 1u32), (10, 90), (200, 5), (37, 23)] {
                let out = resize(&src, w, h, filter);
                assert_eq!((out.width(), out.height()), (w, h), "{filter:?} at {w}x{h}");
            }
        }
    }

    #[test]
    fn downscaling_a_checkerboard_averages_to_grey() {
        // Point sampling would give either all black or all white; a proper
        // area-aware filter gives mid-grey.
        let src = checker(64, 64);
        let out = resize(&src, 4, 4, Resampling::Bilinear);
        for y in 0..4i32 {
            for x in 0..4i32 {
                let c = out.get(x, y);
                assert!(
                    (c.r as i32 - 128).abs() < 40,
                    "expected grey at ({x},{y}), got {c:?} — the reduction is aliasing"
                );
            }
        }
    }

    #[test]
    fn resize_preserves_a_flat_colour() {
        let src = PixelBuffer::filled(50, 50, Rgba8::opaque(37, 211, 90));
        for filter in Resampling::ALL {
            let out = resize(&src, 17, 83, filter);
            let c = out.get(8, 40);
            assert!(
                (c.r as i32 - 37).abs() <= 1
                    && (c.g as i32 - 211).abs() <= 1
                    && (c.b as i32 - 90).abs() <= 1,
                "{filter:?} shifted a flat colour to {c:?}"
            );
        }
    }

    #[test]
    fn nearest_neighbour_invents_no_new_colours() {
        let src = checker(16, 16);
        let out = resize(&src, 48, 48, Resampling::Nearest);
        for y in 0..48i32 {
            for x in 0..48i32 {
                let c = out.get(x, y);
                assert!(
                    c == Rgba8::WHITE || c == Rgba8::BLACK,
                    "nearest produced {c:?} at ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn a_transform_round_trip_returns_close_to_the_original() {
        let src = PixelBuffer::filled(40, 40, Rgba8::opaque(120, 80, 200));
        let there = Transform::about(Vec2::new(20.0, 20.0), Transform::rotate(0.7));
        let (rotated, offset) = transform(&src, (0, 0), there, Resampling::Bilinear, None).unwrap();

        let back = there.invert().unwrap();
        let (result, _) = transform(&rotated, offset, back, Resampling::Bilinear, None).unwrap();

        // The interior should come back to the original colour.
        let c = result.get(result.width() as i32 / 2, result.height() as i32 / 2);
        assert!(
            (c.r as i32 - 120).abs() < 6 && (c.b as i32 - 200).abs() < 6,
            "round trip drifted to {c:?}"
        );
    }
}
