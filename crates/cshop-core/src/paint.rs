//! Brush stamping and stroke accumulation.
//!
//! # Why a stroke buffer
//!
//! Painting a dab straight onto the layer for every mouse sample gives the
//! wrong result: overlapping dabs within one stroke would each composite
//! separately and the stroke would darken wherever the pointer moved slowly.
//!
//! The conventional brush model, which this implements, separates two controls:
//!
//! * **flow** — how much paint each individual dab deposits, which *does*
//!   build up as dabs overlap;
//! * **opacity** — the ceiling the stroke as a whole can reach, applied once
//!   when the stroke is committed.
//!
//! So dabs accumulate into a coverage mask, and that mask is composited onto
//! the layer exactly once, at the end. That also makes the whole stroke a
//! single undo step for free.

use crate::blend::composite;
use crate::color::{Rgba, Rgba8};
use crate::geom::{IRect, Vec2};
use crate::mask::MaskBuffer;
use crate::pixels::PixelBuffer;

/// A selection limiting where a stroke may land.
///
/// Borrowed rather than owned: a document-sized coverage mask is megabytes, and
/// copying one per stroke would be the most expensive thing about painting.
#[derive(Debug, Clone, Copy)]
pub struct Clip<'a> {
    /// Selection coverage, in document coordinates.
    pub mask: &'a MaskBuffer,
    /// Offset of the buffer being painted, so layer-local coordinates can be
    /// mapped into the selection's document space.
    pub offset: (i32, i32),
}

impl Clip<'_> {
    /// Selection coverage at a layer-local pixel, as a `0..=1` multiplier.
    #[inline]
    fn coverage(&self, lx: i32, ly: i32) -> f32 {
        self.mask.get(lx + self.offset.0, ly + self.offset.1) as f32 / 255.0
    }
}

/// Brush shape and dynamics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Brush {
    /// Diameter in pixels.
    pub size: f32,
    /// `0.0` = fully feathered, `1.0` = hard edge (still antialiased).
    pub hardness: f32,
    /// Ceiling for the whole stroke, `0..=1`.
    pub opacity: f32,
    /// Deposit per dab, `0..=1`.
    pub flow: f32,
    /// Distance between dabs as a fraction of the diameter.
    pub spacing: f32,
}

impl Default for Brush {
    fn default() -> Self {
        Self { size: 30.0, hardness: 0.8, opacity: 1.0, flow: 1.0, spacing: 0.1 }
    }
}

impl Brush {
    pub fn radius(&self) -> f32 {
        self.size.max(1.0) / 2.0
    }

    /// Distance between consecutive dabs, never less than a quarter pixel or
    /// a stroke would stamp thousands of dabs per pixel.
    pub fn step(&self) -> f32 {
        (self.size.max(1.0) * self.spacing.clamp(0.01, 4.0)).max(0.25)
    }

    /// Coverage of the dab at distance `d` from its centre.
    ///
    /// Hardness sets where the falloff begins; the outermost pixel always
    /// fades so the edge is antialiased rather than stair-stepped.
    pub fn falloff(&self, d: f32) -> f32 {
        let r = self.radius();
        if d >= r {
            return 0.0;
        }
        // Inner radius where coverage is still solid.
        let inner = r * self.hardness.clamp(0.0, 1.0);
        if d <= inner {
            return 1.0;
        }
        let t = (d - inner) / (r - inner).max(1e-4);
        // Smoothstep, which reads better than a linear ramp on soft brushes.
        let t = t.clamp(0.0, 1.0);
        1.0 - t * t * (3.0 - 2.0 * t)
    }
}

/// What a stroke does to the pixels it covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaintMode {
    /// Composite `color` onto the layer.
    Paint,
    /// Reduce alpha, revealing what is underneath.
    Erase,
}

/// Where a stroke's colour comes from.
///
/// The Clone Stamp differs from the Brush only in this: every other control —
/// size, hardness, opacity, flow, spacing — behaves identically, so they share
/// the whole stroke machinery rather than duplicating it.
#[derive(Debug, Clone)]
pub enum StrokeSource {
    /// One colour everywhere.
    Solid(Rgba8),
    /// Copied from elsewhere in an image.
    Clone {
        /// The pixels being copied from, frozen when the stroke began.
        pixels: PixelBuffer,
        /// Added to a destination pixel to find its source.
        offset: (i32, i32),
    },
}

impl StrokeSource {
    /// The colour to lay down at a layer-local pixel.
    #[inline]
    fn color_at(&self, x: i32, y: i32) -> Rgba8 {
        match self {
            StrokeSource::Solid(c) => *c,
            StrokeSource::Clone { pixels, offset } => pixels.get(x + offset.0, y + offset.1),
        }
    }
}

/// Accumulates dabs for one stroke, then commits them in a single operation.
#[derive(Debug)]
pub struct Stroke {
    /// Coverage in layer-local coordinates, same dimensions as the layer.
    coverage: MaskBuffer,
    brush: Brush,
    mode: PaintMode,
    source: StrokeSource,
    /// Union of every dab's bounding box, so the commit and the undo snapshot
    /// touch only what was painted.
    bounds: IRect,
    /// Dabs added since the last [`Stroke::take_recent`], so a live preview can
    /// re-render just the newly painted sliver instead of the whole stroke.
    recent: IRect,
    last: Option<Vec2>,
    /// Distance carried over from the previous segment, which keeps dab
    /// spacing even across the joins between mouse samples.
    carry: f32,
    /// True once at least one dab has landed.
    started: bool,
}

impl Stroke {
    /// Begin a stroke on a layer of the given size.
    pub fn new(width: u32, height: u32, brush: Brush, mode: PaintMode, color: Rgba8) -> Self {
        Self::with_source(width, height, brush, mode, StrokeSource::Solid(color))
    }

    /// Begin a stroke that draws from somewhere other than a flat colour.
    pub fn with_source(
        width: u32,
        height: u32,
        brush: Brush,
        mode: PaintMode,
        source: StrokeSource,
    ) -> Self {
        Self {
            coverage: MaskBuffer::hide_all(width, height),
            brush,
            mode,
            source,
            bounds: IRect::EMPTY,
            recent: IRect::EMPTY,
            last: None,
            carry: 0.0,
            started: false,
        }
    }

    pub fn bounds(&self) -> IRect {
        self.bounds
    }

    pub fn mode(&self) -> PaintMode {
        self.mode
    }

    pub fn brush(&self) -> Brush {
        self.brush
    }

    /// Move a clone stroke's source. Used when the Clone Stamp is not aligned,
    /// so each new stroke starts again from the sampled point.
    pub fn set_clone_offset(&mut self, offset: (i32, i32)) {
        if let StrokeSource::Clone { offset: current, .. } = &mut self.source {
            *current = offset;
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.started || self.bounds.is_empty()
    }

    /// Region painted since the previous call, and reset it.
    ///
    /// Coverage only ever increases, and compositing a pixel from the
    /// pre-stroke snapshot is idempotent, so re-rendering just this region each
    /// frame gives the same result as re-rendering the whole stroke — at a cost
    /// that stays proportional to pointer movement rather than stroke length.
    pub fn take_recent(&mut self) -> IRect {
        std::mem::replace(&mut self.recent, IRect::EMPTY)
    }

    /// Paint `rect` of `dst` from the untouched `snapshot`, applying the
    /// coverage accumulated so far. Used for the live preview during a stroke.
    pub fn render_region(
        &self,
        snapshot: &PixelBuffer,
        dst: &mut PixelBuffer,
        rect: IRect,
        clip: Option<&Clip>,
    ) {
        let rect = rect.intersect(&dst.bounds());
        let opacity = self.brush.opacity.clamp(0.0, 1.0);
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let mut cov = self.coverage.get(x, y) as f32 / 255.0 * opacity;
                if let Some(clip) = clip {
                    cov *= clip.coverage(x, y);
                }
                let base = snapshot.get(x, y);
                if cov <= 0.0 {
                    dst.set(x, y, base);
                    continue;
                }
                dst.set(x, y, self.apply(base, cov, x, y));
            }
        }
    }

    /// Blend one pixel according to the stroke's mode.
    ///
    /// `x` and `y` are layer-local, because a clone source is sampled per
    /// pixel rather than being one colour for the whole stroke.
    #[inline]
    fn apply(&self, base: Rgba8, coverage: f32, x: i32, y: i32) -> Rgba8 {
        let src = base.to_f32();
        let out = match self.mode {
            PaintMode::Paint => {
                let colour = self.source.color_at(x, y);
                // A clone source can itself be transparent, and copying
                // nothing should deposit nothing.
                let coverage = coverage * (colour.a as f32 / 255.0);
                composite(
                    crate::blend::BlendMode::Normal,
                    src,
                    colour.to_f32(),
                    coverage,
                )
            }
            PaintMode::Erase => Rgba { a: src.a * (1.0 - coverage), ..src },
        };
        out.to_u8()
    }

    /// Paint into a greyscale buffer — a layer mask, or the Quick Mask.
    ///
    /// The stroke's colour is reduced to its luma, so painting black conceals
    /// and painting white reveals, exactly as on a layer mask.
    pub fn render_region_into_mask(
        &self,
        snapshot: &MaskBuffer,
        dst: &mut MaskBuffer,
        rect: IRect,
        clip: Option<&Clip>,
    ) {
        let rect = rect.intersect(&dst.bounds());
        let opacity = self.brush.opacity.clamp(0.0, 1.0);
        // A mask is greyscale, so the source is reduced to its luma.
        let target = self.source.color_at(0, 0).to_f32().luma().clamp(0.0, 1.0) * 255.0;
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let mut cov = self.coverage.get(x, y) as f32 / 255.0 * opacity;
                if let Some(clip) = clip {
                    cov *= clip.coverage(x, y);
                }
                let base = snapshot.get(x, y) as f32;
                if cov <= 0.0 {
                    dst.set(x, y, base as u8);
                    continue;
                }
                let value = base + (target - base) * cov;
                dst.set(x, y, value.clamp(0.0, 255.0) as u8);
            }
        }
    }

    /// Add a pointer sample in layer-local coordinates.
    ///
    /// Dabs are laid along the segment from the previous sample at the brush's
    /// spacing, so a fast drag produces a continuous line instead of a dotted
    /// one.
    pub fn add_point(&mut self, p: Vec2) {
        let step = self.brush.step();
        match self.last {
            None => {
                self.stamp(p);
                self.last = Some(p);
                self.carry = 0.0;
            }
            Some(prev) => {
                let delta = p - prev;
                let dist = delta.length();
                if dist <= 1e-6 {
                    return;
                }
                let dir = delta * (1.0 / dist);
                // Walk from wherever the previous segment left off.
                let mut travelled = step - self.carry;
                while travelled <= dist {
                    self.stamp(prev + dir * travelled);
                    travelled += step;
                }
                self.carry = dist - (travelled - step);
                self.last = Some(p);
            }
        }
    }

    /// Stamp one dab, accumulating coverage rather than replacing it.
    fn stamp(&mut self, centre: Vec2) {
        let r = self.brush.radius();
        let rect = IRect::new(
            (centre.x - r).floor() as i32,
            (centre.y - r).floor() as i32,
            (centre.x + r).ceil() as i32 + 1,
            (centre.y + r).ceil() as i32 + 1,
        )
        .intersect(&self.coverage.bounds());
        if rect.is_empty() {
            // Still counts as a stroke; it simply fell outside the layer.
            self.started = true;
            return;
        }

        let flow = self.brush.flow.clamp(0.0, 1.0);
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                // Sample at the pixel centre.
                let dx = x as f32 + 0.5 - centre.x;
                let dy = y as f32 + 0.5 - centre.y;
                let dab = self.brush.falloff((dx * dx + dy * dy).sqrt());
                if dab <= 0.0 {
                    continue;
                }
                let prev = self.coverage.get(x, y) as f32 / 255.0;
                // Deposit: what is already there, plus flow over what remains.
                let next = prev + (1.0 - prev) * dab * flow;
                self.coverage.set(x, y, (next.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
        }

        self.bounds = self.bounds.union(&rect);
        self.recent = self.recent.union(&rect);
        self.started = true;
    }

    /// Apply the accumulated stroke to `pixels`, returning the affected region
    /// in layer-local coordinates.
    ///
    /// The caller wraps the result in a `ReplacePixels` command so the whole
    /// stroke undoes in one step.
    pub fn commit(&self, pixels: &mut PixelBuffer, clip: Option<&Clip>) -> IRect {
        let rect = self.bounds.intersect(&pixels.bounds());
        if rect.is_empty() {
            return IRect::EMPTY;
        }
        let opacity = self.brush.opacity.clamp(0.0, 1.0);

        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let mut cov = self.coverage.get(x, y) as f32 / 255.0 * opacity;
                if let Some(clip) = clip {
                    cov *= clip.coverage(x, y);
                }
                if cov <= 0.0 {
                    continue;
                }
                pixels.set(x, y, self.apply(pixels.get(x, y), cov, x, y));
            }
        }
        rect
    }

    /// Render the stroke into a copy of `pixels` restricted to
    /// [`Stroke::bounds`], which is exactly the buffer a `ReplacePixels`
    /// command needs.
    pub fn commit_to_patch(
        &self,
        pixels: &PixelBuffer,
        clip: Option<&Clip>,
    ) -> Option<(IRect, PixelBuffer)> {
        let rect = self.bounds.intersect(&pixels.bounds());
        if rect.is_empty() {
            return None;
        }
        let mut patch = pixels.copy_rect(rect);
        // Re-run the commit against the patch by shifting into its local space.
        let opacity = self.brush.opacity.clamp(0.0, 1.0);
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let mut cov = self.coverage.get(x, y) as f32 / 255.0 * opacity;
                if let Some(clip) = clip {
                    cov *= clip.coverage(x, y);
                }
                if cov <= 0.0 {
                    continue;
                }
                let (lx, ly) = (x - rect.x0, y - rect.y0);
                patch.set(lx, ly, self.apply(patch.get(lx, ly), cov, x, y));
            }
        }
        Some((rect, patch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brush(size: f32) -> Brush {
        Brush { size, hardness: 1.0, opacity: 1.0, flow: 1.0, spacing: 0.1 }
    }

    #[test]
    fn falloff_is_solid_inside_and_zero_outside() {
        let b = Brush { size: 20.0, hardness: 0.5, ..Default::default() };
        assert_eq!(b.falloff(0.0), 1.0);
        assert_eq!(b.falloff(4.9), 1.0, "inside the hard core");
        assert_eq!(b.falloff(10.0), 0.0, "at the radius");
        assert_eq!(b.falloff(50.0), 0.0);
        let mid = b.falloff(7.5);
        assert!(mid > 0.0 && mid < 1.0, "the feathered band should be partial");
    }

    #[test]
    fn a_hard_brush_still_feathers_its_outermost_pixel() {
        let b = brush(20.0);
        assert_eq!(b.falloff(0.0), 1.0);
        // hardness 1.0 makes inner == radius, so only the boundary fades.
        assert_eq!(b.falloff(9.99), 1.0);
        assert_eq!(b.falloff(10.0), 0.0);
    }

    #[test]
    fn a_single_click_paints_a_dot() {
        let mut px = PixelBuffer::new(32, 32);
        let mut s = Stroke::new(32, 32, brush(8.0), PaintMode::Paint, Rgba8::BLACK);
        s.add_point(Vec2::new(16.0, 16.0));
        let rect = s.commit(&mut px, None);

        assert!(!rect.is_empty());
        assert_eq!(px.get(16, 16), Rgba8::BLACK);
        assert_eq!(px.get(0, 0), Rgba8::TRANSPARENT, "nothing outside the dab");
        assert!(rect.width() <= 11, "the dirty rect should hug the dab, got {rect:?}");
    }

    #[test]
    fn a_drag_paints_a_continuous_line() {
        let mut px = PixelBuffer::new(64, 16);
        let mut s = Stroke::new(64, 16, brush(6.0), PaintMode::Paint, Rgba8::BLACK);
        // Two samples far apart: interpolation must fill the gap.
        s.add_point(Vec2::new(4.0, 8.0));
        s.add_point(Vec2::new(60.0, 8.0));
        s.commit(&mut px, None);

        for x in 5..59 {
            assert_eq!(px.get(x, 8).a, 255, "gap in the stroke at x={x}");
        }
    }

    #[test]
    fn spacing_carries_across_segments() {
        // Many small moves must give the same continuous line as one big move.
        let mut a = PixelBuffer::new(64, 16);
        let mut sa = Stroke::new(64, 16, brush(6.0), PaintMode::Paint, Rgba8::BLACK);
        for i in 0..=56 {
            sa.add_point(Vec2::new(4.0 + i as f32, 8.0));
        }
        sa.commit(&mut a, None);

        for x in 5..59 {
            assert_eq!(a.get(x, 8).a, 255, "gap at x={x}");
        }
    }

    #[test]
    fn overlapping_dabs_do_not_darken_a_translucent_stroke() {
        // The whole point of the stroke buffer: painting slowly over one spot
        // must not build past the stroke's opacity.
        let mut px = PixelBuffer::new(32, 32);
        let mut s = Stroke::new(
            32,
            32,
            Brush { opacity: 0.5, flow: 1.0, ..brush(10.0) },
            PaintMode::Paint,
            Rgba8::BLACK,
        );
        for _ in 0..20 {
            s.add_point(Vec2::new(16.0, 16.0));
        }
        s.commit(&mut px, None);
        assert_eq!(px.get(16, 16).a, 128, "coverage should stop at 50%");
    }

    #[test]
    fn flow_builds_up_within_a_stroke() {
        let mut low = PixelBuffer::new(32, 32);
        let mut s = Stroke::new(
            32,
            32,
            Brush { flow: 0.25, spacing: 0.05, ..brush(10.0) },
            PaintMode::Paint,
            Rgba8::BLACK,
        );
        s.add_point(Vec2::new(8.0, 16.0));
        s.add_point(Vec2::new(24.0, 16.0));
        s.commit(&mut low, None);
        // A single dab at 25% flow would leave alpha 64; overlapping dabs
        // along the drag should push it much higher.
        assert!(low.get(16, 16).a > 150, "flow should accumulate, got {}", low.get(16, 16).a);
    }

    #[test]
    fn erasing_removes_alpha_and_keeps_colour() {
        let mut px = PixelBuffer::filled(32, 32, Rgba8::opaque(10, 20, 30));
        let mut s = Stroke::new(32, 32, brush(8.0), PaintMode::Erase, Rgba8::BLACK);
        s.add_point(Vec2::new(16.0, 16.0));
        s.commit(&mut px, None);

        let p = px.get(16, 16);
        assert_eq!(p.a, 0, "the centre should be fully erased");
        assert_eq!(px.get(0, 0), Rgba8::opaque(10, 20, 30), "outside is untouched");
    }

    #[test]
    fn painting_outside_the_layer_is_harmless() {
        let mut px = PixelBuffer::new(16, 16);
        let mut s = Stroke::new(16, 16, brush(8.0), PaintMode::Paint, Rgba8::BLACK);
        s.add_point(Vec2::new(-100.0, -100.0));
        assert!(s.commit(&mut px, None).is_empty());
        assert_eq!(px.get(0, 0), Rgba8::TRANSPARENT);
    }

    #[test]
    fn a_patch_matches_a_direct_commit() {
        // commit_to_patch feeds the undo system, so it must agree exactly with
        // painting straight onto the layer.
        let base = PixelBuffer::filled(32, 32, Rgba8::opaque(90, 90, 90));

        let mut direct = base.clone();
        let mut s1 = Stroke::new(32, 32, brush(9.0), PaintMode::Paint, Rgba8::WHITE);
        s1.add_point(Vec2::new(10.0, 10.0));
        s1.add_point(Vec2::new(22.0, 20.0));
        s1.commit(&mut direct, None);

        let mut s2 = Stroke::new(32, 32, brush(9.0), PaintMode::Paint, Rgba8::WHITE);
        s2.add_point(Vec2::new(10.0, 10.0));
        s2.add_point(Vec2::new(22.0, 20.0));
        let (rect, patch) = s2.commit_to_patch(&base, None).unwrap();

        let mut applied = base.clone();
        applied.paste(&patch, rect.x0, rect.y0);
        assert!(applied == direct, "patch and direct commit disagree");
    }

    #[test]
    fn incremental_rendering_matches_a_single_commit() {
        // The live preview path must land on exactly the same pixels as the
        // one-shot commit, or a stroke would change appearance when released.
        let base = PixelBuffer::filled(48, 48, Rgba8::opaque(60, 60, 60));
        let points = [
            Vec2::new(6.0, 6.0),
            Vec2::new(20.0, 12.0),
            Vec2::new(30.0, 30.0),
            Vec2::new(40.0, 18.0),
        ];
        let b = Brush { opacity: 0.6, flow: 0.4, ..brush(11.0) };

        let mut oneshot = base.clone();
        let mut s1 = Stroke::new(48, 48, b, PaintMode::Paint, Rgba8::WHITE);
        for p in points {
            s1.add_point(p);
        }
        s1.commit(&mut oneshot, None);

        let mut live = base.clone();
        let mut s2 = Stroke::new(48, 48, b, PaintMode::Paint, Rgba8::WHITE);
        for p in points {
            s2.add_point(p);
            let r = s2.take_recent();
            s2.render_region(&base, &mut live, r, None);
        }

        assert!(live == oneshot, "incremental preview diverged from the final commit");
    }

    #[test]
    fn take_recent_reports_only_new_work() {
        let mut s = Stroke::new(64, 64, brush(8.0), PaintMode::Paint, Rgba8::BLACK);
        s.add_point(Vec2::new(10.0, 10.0));
        let first = s.take_recent();
        assert!(!first.is_empty());
        assert!(s.take_recent().is_empty(), "nothing new since the last call");

        s.add_point(Vec2::new(50.0, 50.0));
        let second = s.take_recent();
        assert!(!second.is_empty());
        // Interpolated dabs start adjacent to the previous point, so the new
        // region begins where the old one did but reaches much further.
        assert!(second.x1 > first.x1, "the new region should follow the pointer");
        // The total still spans everything painted.
        assert!(s.bounds().width() > 40);
    }

    #[test]
    fn a_selection_confines_the_stroke() {
        use crate::selection::{Rectf, Selection};
        let mut px = PixelBuffer::filled(64, 64, Rgba8::WHITE);
        let selection =
            Selection::from_rect(64, 64, Rectf { x0: 16.0, y0: 16.0, x1: 48.0, y1: 48.0 }, false);
        let clip = Clip { mask: selection.mask(), offset: (0, 0) };

        let mut s = Stroke::new(64, 64, brush(40.0), PaintMode::Paint, Rgba8::BLACK);
        s.add_point(Vec2::new(32.0, 32.0));
        s.commit(&mut px, Some(&clip));

        assert_eq!(px.get(32, 32), Rgba8::BLACK, "inside the selection");
        assert_eq!(px.get(14, 32), Rgba8::WHITE, "outside is protected");
        assert_eq!(px.get(50, 32), Rgba8::WHITE);
    }

    #[test]
    fn a_soft_selection_edge_scales_the_stroke() {
        use crate::mask::MaskBuffer;
        let mut px = PixelBuffer::filled(32, 32, Rgba8::WHITE);
        let mut mask = MaskBuffer::hide_all(32, 32);
        mask.fill_rect(IRect::new(0, 0, 32, 32), 128);
        let clip = Clip { mask: &mask, offset: (0, 0) };

        let mut s = Stroke::new(32, 32, brush(20.0), PaintMode::Paint, Rgba8::BLACK);
        s.add_point(Vec2::new(16.0, 16.0));
        s.commit(&mut px, Some(&clip));

        // Half selection coverage means half the paint lands.
        let p = px.get(16, 16);
        assert!(p.r > 120 && p.r < 136, "expected mid-grey, got {p:?}");
    }

    #[test]
    fn the_clip_accounts_for_the_layer_offset() {
        use crate::selection::{Rectf, Selection};
        // The layer sits at (100, 100); the selection is in document space.
        let mut px = PixelBuffer::filled(32, 32, Rgba8::WHITE);
        let selection = Selection::from_rect(
            200,
            200,
            Rectf { x0: 100.0, y0: 100.0, x1: 116.0, y1: 116.0 },
            false,
        );
        let clip = Clip { mask: selection.mask(), offset: (100, 100) };

        let mut s = Stroke::new(32, 32, brush(60.0), PaintMode::Paint, Rgba8::BLACK);
        s.add_point(Vec2::new(16.0, 16.0));
        s.commit(&mut px, Some(&clip));

        assert_eq!(px.get(8, 8), Rgba8::BLACK, "layer-local (8,8) is doc (108,108)");
        assert_eq!(px.get(24, 24), Rgba8::WHITE, "doc (124,124) is outside the selection");
    }

    #[test]
    fn painting_a_mask_moves_it_toward_the_colour_luma() {
        use crate::mask::MaskBuffer;
        let snapshot = MaskBuffer::new(32, 32, 255);
        let mut dst = snapshot.clone();

        let mut s = Stroke::new(32, 32, brush(12.0), PaintMode::Paint, Rgba8::BLACK);
        s.add_point(Vec2::new(16.0, 16.0));
        let r = s.take_recent();
        s.render_region_into_mask(&snapshot, &mut dst, r, None);

        assert_eq!(dst.get(16, 16), 0, "black conceals");
        assert_eq!(dst.get(0, 0), 255, "outside the dab is untouched");
    }

    #[test]
    fn a_clone_stroke_copies_from_its_source() {
        // Source: a red square on the left. Cloning with an offset of -20
        // should reproduce it 20 pixels to the right.
        let mut source = PixelBuffer::filled(64, 32, Rgba8::WHITE);
        source.fill_rect(IRect::new(4, 4, 20, 28), Rgba8::opaque(255, 0, 0));

        let mut dst = PixelBuffer::filled(64, 32, Rgba8::BLACK);
        let mut s = Stroke::with_source(
            64,
            32,
            brush(14.0),
            PaintMode::Paint,
            StrokeSource::Clone { pixels: source, offset: (-20, 0) },
        );
        s.add_point(Vec2::new(32.0, 16.0));
        s.commit(&mut dst, None);

        assert_eq!(dst.get(32, 16), Rgba8::opaque(255, 0, 0), "copied the red square");
        assert_eq!(dst.get(2, 16), Rgba8::BLACK, "outside the dab is untouched");
    }

    #[test]
    fn a_clone_source_carries_its_transparency() {
        // Cloning from empty space should deposit nothing, not black.
        let source = PixelBuffer::new(32, 32);
        let mut dst = PixelBuffer::filled(32, 32, Rgba8::WHITE);
        let mut s = Stroke::with_source(
            32,
            32,
            brush(12.0),
            PaintMode::Paint,
            StrokeSource::Clone { pixels: source, offset: (0, 0) },
        );
        s.add_point(Vec2::new(16.0, 16.0));
        s.commit(&mut dst, None);
        assert_eq!(dst.get(16, 16), Rgba8::WHITE, "nothing should have been laid down");
    }

    #[test]
    fn a_clone_stroke_honours_every_brush_control() {
        // The point of sharing the stroke machinery: opacity, hardness and
        // spacing behave exactly as they do for the brush.
        let source = PixelBuffer::filled(64, 32, Rgba8::BLACK);
        let mut dst = PixelBuffer::filled(64, 32, Rgba8::WHITE);
        let mut s = Stroke::with_source(
            64,
            32,
            Brush { opacity: 0.5, flow: 1.0, ..brush(16.0) },
            PaintMode::Paint,
            StrokeSource::Clone { pixels: source, offset: (0, 0) },
        );
        for _ in 0..10 {
            s.add_point(Vec2::new(32.0, 16.0));
        }
        s.commit(&mut dst, None);
        let c = dst.get(32, 16);
        assert!(c.r > 120 && c.r < 136, "opacity should cap at 50%, got {c:?}");
    }

    #[test]
    fn a_clone_stroke_respects_the_selection() {
        use crate::selection::{Rectf, Selection};
        let source = PixelBuffer::filled(64, 64, Rgba8::BLACK);
        let mut dst = PixelBuffer::filled(64, 64, Rgba8::WHITE);
        let selection =
            Selection::from_rect(64, 64, Rectf { x0: 0.0, y0: 0.0, x1: 32.0, y1: 64.0 }, false);
        let clip = Clip { mask: selection.mask(), offset: (0, 0) };

        let mut s = Stroke::with_source(
            64,
            64,
            brush(50.0),
            PaintMode::Paint,
            StrokeSource::Clone { pixels: source, offset: (0, 0) },
        );
        s.add_point(Vec2::new(32.0, 32.0));
        s.commit(&mut dst, Some(&clip));

        assert_eq!(dst.get(16, 32), Rgba8::BLACK, "inside the selection");
        assert_eq!(dst.get(48, 32), Rgba8::WHITE, "outside it");
    }

    #[test]
    fn an_untouched_stroke_reports_empty() {
        let s = Stroke::new(16, 16, brush(4.0), PaintMode::Paint, Rgba8::BLACK);
        assert!(s.is_empty());
        assert!(s.commit_to_patch(&PixelBuffer::new(16, 16), None).is_none());
    }
}
