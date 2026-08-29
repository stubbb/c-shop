//! Filling pixels: the Paint Bucket, gradients, and the Edit > Fill command.
//!
//! All three end up doing the same thing — composite a colour over a region
//! with some coverage — so they share [`fill_region`]. What differs is only
//! where the coverage comes from: a flood fill, a gradient ramp, or the
//! selection itself.

use crate::blend::{composite, BlendMode};
use crate::color::{Rgba, Rgba8};
use crate::geom::{IRect, Vec2};
use crate::mask::MaskBuffer;
use crate::pixels::PixelBuffer;

/// What the Paint Bucket matches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BucketOptions {
    /// Maximum per-channel difference that still counts as a match.
    pub tolerance: u8,
    /// Restrict to the region connected to the click.
    pub contiguous: bool,
    pub antialias: bool,
    /// Sample the composited image rather than the active layer.
    pub sample_all_layers: bool,
    pub opacity: f32,
    pub mode: BlendMode,
}

impl Default for BucketOptions {
    fn default() -> Self {
        Self {
            tolerance: 32,
            contiguous: true,
            antialias: true,
            sample_all_layers: false,
            opacity: 1.0,
            mode: BlendMode::Normal,
        }
    }
}

/// Coverage for a bucket fill: which pixels the click reaches.
///
/// This is the Magic Wand's matching rule, which is exactly right — the two
/// tools differ only in what they do with the region they find.
pub fn bucket_coverage(
    source: &PixelBuffer,
    seed_x: i32,
    seed_y: i32,
    options: BucketOptions,
) -> MaskBuffer {
    let wand = crate::wand::magic_wand(
        source,
        seed_x,
        seed_y,
        crate::wand::WandOptions {
            tolerance: options.tolerance,
            contiguous: options.contiguous,
            antialias: options.antialias,
        },
    );
    wand.to_mask()
}

/// Composite `color` onto `dst` wherever `coverage` allows.
///
/// `dst` and `coverage` share an origin. `preserve_transparency` keeps the
/// fill inside pixels that already have alpha, which is what the layer's
/// lock-transparency flag means.
pub fn fill_region(
    dst: &mut PixelBuffer,
    coverage: &MaskBuffer,
    color: Rgba8,
    mode: BlendMode,
    opacity: f32,
    preserve_transparency: bool,
) -> IRect {
    use rayon::prelude::*;

    let opacity = opacity.clamp(0.0, 1.0);
    let src = color.to_f32();
    let width = dst.width() as usize;

    // Rows in parallel, and a rectangle unioned once per row rather than once
    // per pixel — which on a large fill cost more than the compositing did.
    let rows: Vec<IRect> = dst
        .pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .map(|(y, row)| {
            let y = y as i32;
            let mut touched = IRect::EMPTY;
            for x in 0..width as i32 {
                let mut amount = coverage.get(x, y) as f32 / 255.0 * opacity;
                if amount <= 0.0 {
                    continue;
                }
                let existing = row[x as usize];
                if preserve_transparency {
                    if existing.a == 0 {
                        continue;
                    }
                    // Scale by what is already there so the fill cannot spread
                    // beyond the layer's own shape.
                    amount *= existing.a as f32 / 255.0;
                }
                let out = composite(mode, existing.to_f32(), src, amount);
                let out = if preserve_transparency {
                    // Compositing raises alpha; the lock says it must not.
                    Rgba { a: existing.a as f32 / 255.0, ..out }
                } else {
                    out
                };
                row[x as usize] = out.to_u8();
                touched = touched.union(&IRect::new(x, y, x + 1, y + 1));
            }
            touched
        })
        .collect();

    rows.into_iter().fold(IRect::EMPTY, |a, b| a.union(&b))
}

// ---------------------------------------------------------------------------
// Gradients
// ---------------------------------------------------------------------------

/// A colour stop along a gradient.
pub use crate::adjust::GradientStop;

/// Entries in a baked ramp.
///
/// A gradient is read at 8-bit precision and dithered besides, so a table this
/// long is finer than anything that survives to the screen; the interpolation
/// between entries is there for the dither to have something to move between.
const RAMP_SIZE: usize = 1024;

/// A gradient's colours, sampled once so they can be read per pixel.
#[derive(Debug, Clone)]
pub struct Ramp {
    table: [Rgba8; RAMP_SIZE],
}

impl Ramp {
    #[inline]
    pub fn at(&self, t: f32) -> Rgba8 {
        let i = (t.clamp(0.0, 1.0) * (RAMP_SIZE - 1) as f32) as usize;
        self.table[i.min(RAMP_SIZE - 1)]
    }
}

/// How a gradient's parameter is derived from position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GradientKind {
    /// Along the drag line.
    #[default]
    Linear,
    /// Outward from the start point.
    Radial,
    /// Sweeping around the start point.
    Angle,
    /// Mirrored either side of the start point.
    Reflected,
    /// Concentric diamonds around the start point.
    Diamond,
}

impl GradientKind {
    pub fn name(self) -> &'static str {
        match self {
            GradientKind::Linear => "Linear",
            GradientKind::Radial => "Radial",
            GradientKind::Angle => "Angle",
            GradientKind::Reflected => "Reflected",
            GradientKind::Diamond => "Diamond",
        }
    }

    pub const ALL: [GradientKind; 5] = [
        GradientKind::Linear,
        GradientKind::Radial,
        GradientKind::Angle,
        GradientKind::Reflected,
        GradientKind::Diamond,
    ];
}

/// A gradient and how to lay it down.
#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    pub stops: Vec<GradientStop>,
    pub kind: GradientKind,
    pub reverse: bool,
    pub opacity: f32,
    pub mode: BlendMode,
    /// Break up banding in long, shallow ramps with a little noise.
    pub dither: bool,
}

impl Default for Gradient {
    fn default() -> Self {
        Self {
            stops: vec![
                GradientStop { position: 0.0, color: Rgba8::BLACK },
                GradientStop { position: 1.0, color: Rgba8::WHITE },
            ],
            kind: GradientKind::Linear,
            reverse: false,
            opacity: 1.0,
            mode: BlendMode::Normal,
            dither: true,
        }
    }
}

impl Gradient {
    /// The two-stop ramp between a pair of colours.
    pub fn between(from: Rgba8, to: Rgba8) -> Gradient {
        Gradient {
            stops: vec![
                GradientStop { position: 0.0, color: from },
                GradientStop { position: 1.0, color: to },
            ],
            ..Default::default()
        }
    }

    /// A ramp from a colour to nothing, for fading something out.
    pub fn to_transparent(from: Rgba8) -> Gradient {
        Gradient::between(from, Rgba8::new(from.r, from.g, from.b, 0))
    }

    /// The gradient's parameter at a point, given the drag from `a` to `b`.
    pub fn parameter(&self, p: Vec2, a: Vec2, b: Vec2) -> f32 {
        let d = b - a;
        let length_sq = d.x * d.x + d.y * d.y;
        let t = match self.kind {
            GradientKind::Linear | GradientKind::Reflected => {
                if length_sq < 1e-6 {
                    0.0
                } else {
                    // Projection of the point onto the drag line.
                    let raw = ((p.x - a.x) * d.x + (p.y - a.y) * d.y) / length_sq;
                    if self.kind == GradientKind::Reflected {
                        raw.abs()
                    } else {
                        raw
                    }
                }
            }
            GradientKind::Radial => {
                let length = length_sq.sqrt();
                if length < 1e-6 {
                    0.0
                } else {
                    p.distance(a) / length
                }
            }
            GradientKind::Angle => {
                // A full turn maps to the whole ramp, starting along the drag.
                let base = d.y.atan2(d.x);
                let angle = (p.y - a.y).atan2(p.x - a.x) - base;
                let turns = angle / std::f32::consts::TAU;
                turns - turns.floor()
            }
            GradientKind::Diamond => {
                let length = length_sq.sqrt();
                if length < 1e-6 {
                    0.0
                } else {
                    // Chebyshev-style distance in the drag's frame.
                    let ux = d.x / length;
                    let uy = d.y / length;
                    let rx = (p.x - a.x) * ux + (p.y - a.y) * uy;
                    let ry = -(p.x - a.x) * uy + (p.y - a.y) * ux;
                    (rx.abs() + ry.abs()) / length
                }
            }
        };
        let t = t.clamp(0.0, 1.0);
        if self.reverse {
            1.0 - t
        } else {
            t
        }
    }

    /// Colour at a point along the ramp.
    /// The ramp sampled into a table, for the paths that read it per pixel.
    ///
    /// [`Gradient::color_at`] sorts the stops on every call, which is fine for
    /// a swatch and ruinous for a fill: rendering a 10000x10000 gradient meant
    /// a hundred million heap allocations and sorts, and was over two seconds
    /// of the five it took. The same mistake the curves dialog once made.
    pub fn bake(&self) -> Ramp {
        let mut table = [Rgba8::TRANSPARENT; RAMP_SIZE];
        for (i, slot) in table.iter_mut().enumerate() {
            *slot = self.color_at(i as f32 / (RAMP_SIZE - 1) as f32);
        }
        Ramp { table }
    }

    pub fn color_at(&self, t: f32) -> Rgba8 {
        if self.stops.is_empty() {
            return Rgba8::TRANSPARENT;
        }
        let t = t.clamp(0.0, 1.0);
        let mut sorted: Vec<&GradientStop> = self.stops.iter().collect();
        sorted.sort_by(|a, b| a.position.total_cmp(&b.position));

        if t <= sorted[0].position {
            return sorted[0].color;
        }
        let last = sorted[sorted.len() - 1];
        if t >= last.position {
            return last.color;
        }
        for w in sorted.windows(2) {
            let (a, b) = (w[0], w[1]);
            if t >= a.position && t <= b.position {
                let span = (b.position - a.position).max(1e-6);
                let f = (t - a.position) / span;
                // Interpolate premultiplied, so a ramp into transparency does
                // not drag its colour toward black on the way.
                let lerp = |x: u8, y: u8, ax: f32, ay: f32| {
                    let px = x as f32 / 255.0 * ax;
                    let py = y as f32 / 255.0 * ay;
                    px + (py - px) * f
                };
                let (aa, ba) = (a.color.a as f32 / 255.0, b.color.a as f32 / 255.0);
                let out_a = aa + (ba - aa) * f;
                if out_a <= 1e-6 {
                    return Rgba8::TRANSPARENT;
                }
                let un = |v: f32| ((v / out_a).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                return Rgba8::new(
                    un(lerp(a.color.r, b.color.r, aa, ba)),
                    un(lerp(a.color.g, b.color.g, aa, ba)),
                    un(lerp(a.color.b, b.color.b, aa, ba)),
                    (out_a * 255.0 + 0.5) as u8,
                );
            }
        }
        last.color
    }

    /// Draw the gradient onto `dst`, whose top-left is at `origin` in the same
    /// space as `a` and `b`.
    ///
    /// `coverage` limits where it lands — the selection, usually.
    pub fn render(
        &self,
        dst: &mut PixelBuffer,
        origin: (i32, i32),
        a: Vec2,
        b: Vec2,
        coverage: Option<&MaskBuffer>,
        preserve_transparency: bool,
    ) -> IRect {
        use rayon::prelude::*;

        let opacity = self.opacity.clamp(0.0, 1.0);
        // Sampled once rather than per pixel; see `Gradient::bake`.
        let ramp = self.bake();
        let width = dst.width() as i32;

        // A row at a time, so the work spreads across every core and the
        // extent each row touched comes back with it. Unioning a rectangle per
        // *pixel*, as this used to, cost more than the compositing did.
        let width_usize = width as usize;
        let rows: Vec<IRect> = dst
            .pixels_mut()
            .par_chunks_mut(width_usize)
            .enumerate()
            .map(|(y, row)| {
                let y = y as i32;
                let mut touched = IRect::EMPTY;
                for x in 0..width {
                    let doc = Vec2::new((origin.0 + x) as f32 + 0.5, (origin.1 + y) as f32 + 0.5);
                    let mut t = self.parameter(doc, a, b);
                    if self.dither {
                        // A quarter-level of noise, which is below what the eye
                        // sees but enough to break up banding on a long ramp.
                        let n = crate::filters::plane::Rng::at(0x9E37, origin.0 + x, origin.1 + y)
                            .next_f32();
                        t = (t + (n - 0.5) / 255.0).clamp(0.0, 1.0);
                    }
                    let colour = ramp.at(t);

                    let mut amount = opacity;
                    if let Some(mask) = coverage {
                        amount *= mask.get(origin.0 + x, origin.1 + y) as f32 / 255.0;
                    }
                    if amount <= 0.0 {
                        continue;
                    }
                    let existing = row[x as usize];
                    if preserve_transparency {
                        if existing.a == 0 {
                            continue;
                        }
                        amount *= existing.a as f32 / 255.0;
                    }
                    let out = composite(self.mode, existing.to_f32(), colour.to_f32(), amount);
                    let out = if preserve_transparency {
                        Rgba { a: existing.a as f32 / 255.0, ..out }
                    } else {
                        out
                    };
                    row[x as usize] = out.to_u8();
                    touched = touched.union(&IRect::new(x, y, x + 1, y + 1));
                }
                touched
            })
            .collect();

        rows.into_iter().fold(IRect::EMPTY, |a, b| a.union(&b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(w: u32, h: u32) -> PixelBuffer {
        let mut px = PixelBuffer::filled(w, h, Rgba8::BLACK);
        px.fill_rect(IRect::new(w as i32 / 2, 0, w as i32, h as i32), Rgba8::WHITE);
        px
    }

    // --- bucket ------------------------------------------------------------

    #[test]
    fn the_bucket_covers_the_region_under_the_click() {
        let src = split(32, 16);
        let coverage = bucket_coverage(
            &src,
            4,
            8,
            BucketOptions { antialias: false, ..Default::default() },
        );
        assert_eq!(coverage.get(4, 8), 255, "the clicked half");
        assert_eq!(coverage.get(28, 8), 0, "the other half");
    }

    #[test]
    fn a_non_contiguous_bucket_reaches_every_match() {
        let mut src = PixelBuffer::filled(32, 8, Rgba8::BLACK);
        src.fill_rect(IRect::new(0, 0, 6, 8), Rgba8::WHITE);
        src.fill_rect(IRect::new(26, 0, 32, 8), Rgba8::WHITE);

        let near = bucket_coverage(
            &src,
            2,
            4,
            BucketOptions { contiguous: true, antialias: false, ..Default::default() },
        );
        assert_eq!(near.get(28, 4), 0);

        let far = bucket_coverage(
            &src,
            2,
            4,
            BucketOptions { contiguous: false, antialias: false, ..Default::default() },
        );
        assert_eq!(far.get(28, 4), 255);
    }

    #[test]
    fn filling_composites_the_colour() {
        let mut dst = PixelBuffer::filled(8, 8, Rgba8::BLACK);
        let coverage = MaskBuffer::reveal_all(8, 8);
        let touched =
            fill_region(&mut dst, &coverage, Rgba8::WHITE, BlendMode::Normal, 1.0, false);
        assert_eq!(dst.get(4, 4), Rgba8::WHITE);
        assert_eq!(touched, IRect::new(0, 0, 8, 8));
    }

    #[test]
    fn fill_opacity_blends_rather_than_replaces() {
        let mut dst = PixelBuffer::filled(4, 4, Rgba8::BLACK);
        fill_region(
            &mut dst,
            &MaskBuffer::reveal_all(4, 4),
            Rgba8::WHITE,
            BlendMode::Normal,
            0.5,
            false,
        );
        let c = dst.get(2, 2);
        assert!(c.r > 120 && c.r < 136, "expected mid-grey, got {c:?}");
    }

    #[test]
    fn preserving_transparency_keeps_the_fill_inside_the_shape() {
        let mut dst = PixelBuffer::new(8, 8);
        dst.fill_rect(IRect::new(0, 0, 4, 8), Rgba8::opaque(20, 20, 20));

        fill_region(
            &mut dst,
            &MaskBuffer::reveal_all(8, 8),
            Rgba8::WHITE,
            BlendMode::Normal,
            1.0,
            true,
        );
        assert_eq!(dst.get(2, 4), Rgba8::WHITE, "inside the shape is filled");
        assert_eq!(dst.get(6, 4).a, 0, "the empty half stays empty");
    }

    #[test]
    fn a_fill_respects_its_blend_mode() {
        let mut dst = PixelBuffer::filled(4, 4, Rgba8::opaque(128, 128, 128));
        fill_region(
            &mut dst,
            &MaskBuffer::reveal_all(4, 4),
            Rgba8::opaque(128, 128, 128),
            BlendMode::Multiply,
            1.0,
            false,
        );
        let c = dst.get(2, 2);
        assert!(c.r > 58 && c.r < 70, "multiply of two mid-greys, got {c:?}");
    }

    // --- gradients ---------------------------------------------------------

    #[test]
    fn a_linear_gradient_runs_along_the_drag() {
        let g = Gradient::default();
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(100.0, 0.0);
        assert!(g.parameter(Vec2::new(0.0, 0.0), a, b) < 0.01);
        assert!((g.parameter(Vec2::new(50.0, 0.0), a, b) - 0.5).abs() < 0.01);
        assert!(g.parameter(Vec2::new(100.0, 0.0), a, b) > 0.99);
        // Past either end it clamps rather than repeating.
        assert_eq!(g.parameter(Vec2::new(-50.0, 0.0), a, b), 0.0);
        assert_eq!(g.parameter(Vec2::new(200.0, 0.0), a, b), 1.0);
    }

    #[test]
    fn reversing_flips_the_ramp() {
        let mut g = Gradient::default();
        let (a, b) = (Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0));
        let before = g.parameter(Vec2::new(25.0, 0.0), a, b);
        g.reverse = true;
        let after = g.parameter(Vec2::new(25.0, 0.0), a, b);
        assert!((before + after - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_radial_gradient_depends_only_on_distance() {
        let g = Gradient { kind: GradientKind::Radial, ..Default::default() };
        let (a, b) = (Vec2::new(50.0, 50.0), Vec2::new(100.0, 50.0));
        let up = g.parameter(Vec2::new(50.0, 25.0), a, b);
        let right = g.parameter(Vec2::new(75.0, 50.0), a, b);
        assert!((up - right).abs() < 0.01, "same distance should give the same value");
        assert!(g.parameter(a, a, b) < 0.01, "the centre is the start of the ramp");
    }

    #[test]
    fn a_reflected_gradient_is_symmetric_about_the_start() {
        let g = Gradient { kind: GradientKind::Reflected, ..Default::default() };
        let (a, b) = (Vec2::new(50.0, 0.0), Vec2::new(100.0, 0.0));
        let left = g.parameter(Vec2::new(25.0, 0.0), a, b);
        let right = g.parameter(Vec2::new(75.0, 0.0), a, b);
        assert!((left - right).abs() < 0.01, "{left} vs {right}");
    }

    #[test]
    fn every_gradient_kind_stays_in_range() {
        let (a, b) = (Vec2::new(30.0, 40.0), Vec2::new(90.0, 10.0));
        for kind in GradientKind::ALL {
            let g = Gradient { kind, ..Default::default() };
            for p in [
                Vec2::new(0.0, 0.0),
                Vec2::new(200.0, 200.0),
                Vec2::new(-50.0, 30.0),
                a,
                b,
            ] {
                let t = g.parameter(p, a, b);
                assert!((0.0..=1.0).contains(&t), "{} gave {t} at {p:?}", kind.name());
            }
        }
    }

    #[test]
    fn a_zero_length_drag_does_not_divide_by_zero() {
        let a = Vec2::new(10.0, 10.0);
        for kind in GradientKind::ALL {
            let g = Gradient { kind, ..Default::default() };
            let t = g.parameter(Vec2::new(20.0, 20.0), a, a);
            assert!(t.is_finite(), "{} produced {t}", kind.name());
        }
    }

    #[test]
    fn the_ramp_interpolates_between_its_stops() {
        let g = Gradient::between(Rgba8::BLACK, Rgba8::WHITE);
        assert_eq!(g.color_at(0.0), Rgba8::BLACK);
        assert_eq!(g.color_at(1.0), Rgba8::WHITE);
        let mid = g.color_at(0.5);
        assert!(mid.r > 120 && mid.r < 136, "got {mid:?}");
    }

    #[test]
    fn fading_to_transparent_keeps_the_colour() {
        // Interpolating straight alpha would drag the hue toward black.
        let g = Gradient::to_transparent(Rgba8::opaque(255, 220, 0));
        for i in 0..=10 {
            let c = g.color_at(i as f32 / 10.0);
            if c.a > 8 {
                assert!(c.r > 240 && c.g > 200, "fringe at {i}: {c:?}");
            }
        }
        assert_eq!(g.color_at(1.0).a, 0);
    }

    #[test]
    fn rendering_lays_down_a_ramp() {
        let mut dst = PixelBuffer::filled(64, 8, Rgba8::TRANSPARENT);
        let g = Gradient { dither: false, ..Gradient::between(Rgba8::BLACK, Rgba8::WHITE) };
        g.render(&mut dst, (0, 0), Vec2::new(0.0, 0.0), Vec2::new(64.0, 0.0), None, false);

        assert!(dst.get(1, 4).r < 12);
        assert!(dst.get(62, 4).r > 243);
        // And it increases all the way across.
        let mut previous = -1i32;
        for x in 0..64i32 {
            let v = dst.get(x, 4).r as i32;
            assert!(v >= previous, "the ramp went backwards at x={x}");
            previous = v;
        }
    }

    #[test]
    fn a_gradient_respects_its_coverage_mask() {
        let mut dst = PixelBuffer::filled(32, 8, Rgba8::TRANSPARENT);
        let mut coverage = MaskBuffer::hide_all(32, 8);
        coverage.fill_rect(IRect::new(0, 0, 16, 8), 255);

        let g = Gradient { dither: false, ..Default::default() };
        g.render(
            &mut dst,
            (0, 0),
            Vec2::new(0.0, 0.0),
            Vec2::new(32.0, 0.0),
            Some(&coverage),
            false,
        );
        assert!(dst.get(8, 4).a > 0, "inside the mask");
        assert_eq!(dst.get(24, 4).a, 0, "outside it");
    }

    #[test]
    fn dithering_changes_nothing_visible() {
        // It should break up banding, not shift the ramp.
        let mut plain = PixelBuffer::filled(64, 4, Rgba8::TRANSPARENT);
        let mut noisy = plain.clone();
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(64.0, 0.0);
        Gradient { dither: false, ..Default::default() }
            .render(&mut plain, (0, 0), a, b, None, false);
        Gradient { dither: true, ..Default::default() }
            .render(&mut noisy, (0, 0), a, b, None, false);

        for x in 0..64i32 {
            let d = (plain.get(x, 2).r as i32 - noisy.get(x, 2).r as i32).abs();
            assert!(d <= 2, "dither shifted the ramp by {d} at x={x}");
        }
    }
}
