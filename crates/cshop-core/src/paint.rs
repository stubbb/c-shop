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
//!
//! # Still to do: painting at sixteen bits
//!
//! Every type here works in [`Rgba8`]. A raster layer can hold sixteen bits a
//! channel and files round-trip them, but the brush cannot reach one — the
//! window says so and offers `Image ▸ Mode` rather than narrowing the layer
//! behind the user's back, which is the honest half of the situation and not
//! the whole of it.
//!
//! What it would take: the stroke's coverage mask is already independent of
//! depth, so the change is in the compositing at the end — [`Stroke::commit`]
//! and the [`composite`] call under it — plus the sampled sources
//! ([`StrokeSource`]) reading and writing at the layer's own depth rather than
//! at eight bits. It is not deep, but it is every path in this file, and half
//! of it done is worse than none: a brush that paints at sixteen bits into a
//! stroke buffer that clamps at eight would look right and quantise anyway.

use crate::blend::composite;
use crate::color::{Rgba, Rgba8};
use crate::geom::{IRect, Vec2};
use crate::mask::MaskBuffer;
use crate::pixels::PixelBuffer;
use crate::selection::Selection;
use crate::snapshot::Snapshot;

/// A selection limiting where a stroke may land.
///
/// Borrowed rather than owned: a document-sized coverage mask is megabytes, and
/// copying one per stroke would be the most expensive thing about painting.
#[derive(Debug, Clone, Copy)]
pub struct Clip<'a> {
    /// The selection to clip against. Held whole rather than as its buffer,
    /// because a selection stores coverage only where it has any and knows
    /// where that sits; a bare buffer would not.
    pub selection: &'a Selection,
    /// Offset of the buffer being painted, so layer-local coordinates can be
    /// mapped into the selection's document space.
    pub offset: (i32, i32),
}

impl Clip<'_> {
    /// Selection coverage at a layer-local pixel, as a `0..=1` multiplier.
    #[inline]
    fn coverage(&self, lx: i32, ly: i32) -> f32 {
        self.selection.coverage(lx + self.offset.0, ly + self.offset.1) as f32 / 255.0
    }
}

/// What a pen's pressure drives.
///
/// Pressure is one number and there are three things it could plausibly
/// change, so which ones is a choice rather than a default. Size alone is the
/// pencil; flow alone is the airbrush; both together is most brushes.
/// Nothing by default, until someone says so: a mouse reports full pressure
/// always, and a brush that quietly ignores its size setting because a tablet
/// happens to be plugged in is worse than one that needs a checkbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pressure {
    pub size: bool,
    pub flow: bool,
    pub opacity: bool,
}

impl Pressure {
    pub fn any(self) -> bool {
        self.size || self.flow || self.opacity
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
    /// Which of the above a pen's pressure drives.
    pub pressure: Pressure,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            size: 30.0,
            hardness: 0.8,
            opacity: 1.0,
            flow: 1.0,
            spacing: 0.1,
            pressure: Pressure::default(),
        }
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
///
/// The first two lay something down or take it away. The third does neither:
/// it reshapes what is already there, which is why it carries its settings
/// rather than reading the stroke's colour. See [`crate::retouch`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaintMode {
    /// Composite `color` onto the layer.
    Paint,
    /// Reduce alpha, revealing what is underneath.
    Erase,
    /// Lighten, darken or saturate what the brush passes over.
    Retouch(crate::retouch::Retouch),
}

/// A brush whose colour is worked out from the picture underneath it.
///
/// Blur and sharpen already exist as filters over a whole layer. What was
/// missing is applying them *through a brush*, and this is the difference: the
/// filter is evaluated per painted pixel against a copy of the layer frozen
/// when the stroke began, so the result is the ordinary stroke machinery —
/// coverage, flow, opacity, clipping — blending toward a filtered version of
/// what was already there.
///
/// Reading a frozen copy rather than the live layer is what keeps a stroke
/// from eating itself: a blur that read its own output would smear along the
/// direction the pointer happened to travel. One stroke blurs once, and a
/// second stroke over the same place blurs it again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BrushFilter {
    /// Average of a disc of radius `radius`, weighted toward the centre.
    Blur { radius: f32 },
    /// Unsharp mask: the pixel plus `amount` of its difference from a blur of
    /// itself. `amount` above about 2 starts to show halos.
    Sharpen { radius: f32, amount: f32 },
}

impl BrushFilter {
    /// The filtered colour at one pixel of `pixels`.
    ///
    /// A tent weight rather than a flat disc, because a flat average of a
    /// small disc leaves the faint square-ish signature of its own footprint
    /// on anything with fine detail.
    fn at(self, pixels: &PixelBuffer, x: i32, y: i32) -> Rgba8 {
        let base = pixels.get(x, y);
        match self {
            BrushFilter::Blur { radius } => blurred(pixels, x, y, radius).unwrap_or(base),
            BrushFilter::Sharpen { radius, amount } => {
                let Some(soft) = blurred(pixels, x, y, radius) else { return base };
                let (b, s) = (base.to_f32(), soft.to_f32());
                Rgba {
                    r: (b.r + (b.r - s.r) * amount).clamp(0.0, 1.0),
                    g: (b.g + (b.g - s.g) * amount).clamp(0.0, 1.0),
                    b: (b.b + (b.b - s.b) * amount).clamp(0.0, 1.0),
                    // Sharpening the edge of a layer's own transparency would
                    // carve a hard rim into it, so alpha is left as it was.
                    a: b.a,
                }
                .to_u8()
            }
        }
    }
}

/// Tent-weighted average of a disc, in premultiplied colour so a transparent
/// neighbour contributes nothing rather than dragging its colour in.
fn blurred(pixels: &PixelBuffer, x: i32, y: i32, radius: f32) -> Option<Rgba8> {
    let r = radius.max(0.0);
    let n = r.ceil() as i32;
    if n <= 0 {
        return None;
    }
    let (mut acc, mut weight) = (Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }, 0.0f32);
    for dy in -n..=n {
        for dx in -n..=n {
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            if d > r {
                continue;
            }
            let w = 1.0 - d / r;
            let c = pixels.get(x + dx, y + dy).to_f32();
            acc.r += c.r * c.a * w;
            acc.g += c.g * c.a * w;
            acc.b += c.b * c.a * w;
            acc.a += c.a * w;
            weight += w;
        }
    }
    if weight <= 0.0 || acc.a <= 0.0 {
        return None;
    }
    Some(
        Rgba { r: acc.r / acc.a, g: acc.g / acc.a, b: acc.b / acc.a, a: acc.a / weight }.to_u8(),
    )
}

/// Where a stroke's colour comes from.
///
/// The Clone Stamp differs from the Brush only in this: every other control —
/// size, hardness, opacity, flow, spacing — behaves identically, so they share
/// the whole stroke machinery rather than duplicating it. The blur and sharpen
/// brushes join them on the same terms.
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
    /// Worked out from the picture underneath, frozen when the stroke began.
    Filtered {
        pixels: PixelBuffer,
        filter: BrushFilter,
    },
    /// Texture from a source, tone from where it lands — the healing brush.
    /// See [`crate::heal`].
    Heal(Box<crate::heal::Heal>),
}

impl StrokeSource {
    /// The colour to lay down at a layer-local pixel.
    #[inline]
    fn color_at(&self, x: i32, y: i32) -> Rgba8 {
        match self {
            StrokeSource::Solid(c) => *c,
            StrokeSource::Clone { pixels, offset } => pixels.get(x + offset.0, y + offset.1),
            StrokeSource::Filtered { pixels, filter } => filter.at(pixels, x, y),
            StrokeSource::Heal(heal) => heal.at(x, y),
        }
    }

    /// Tell the source which rectangle is about to be painted.
    ///
    /// Most sources need no warning: a colour is a colour, and a clone or a
    /// filter reads whatever pixel it is asked for. Healing does, because it
    /// fits a correction to the edge of each dab and so has to know where the
    /// edge is. Called once per dab, before the dab is stamped.
    #[inline]
    fn prepare(&mut self, rect: IRect) {
        if let StrokeSource::Heal(heal) = self {
            heal.prepare(rect);
        }
    }
}

/// Places dab centres along a polyline at the brush's spacing.
///
/// Split out from the stroke because the smudge tool needs the same spacing
/// and the same carry across segments, and having two copies of this is how
/// two tools end up feeling subtly different for no reason anyone can name.
#[derive(Debug, Clone)]
pub struct DabWalk {
    step: f32,
    last: Option<Vec2>,
    /// Distance left over from the previous segment, which keeps dab spacing
    /// even across the joins between pointer samples.
    carry: f32,
}

impl DabWalk {
    pub fn new(brush: &Brush) -> DabWalk {
        DabWalk { step: brush.step(), last: None, carry: 0.0 }
    }

    /// Extend to `p`, appending every dab centre that falls on the way.
    pub fn advance(&mut self, p: Vec2, out: &mut Vec<Vec2>) {
        match self.last {
            None => {
                out.push(p);
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
                let mut travelled = self.step - self.carry;
                while travelled <= dist {
                    out.push(prev + dir * travelled);
                    travelled += self.step;
                }
                self.carry = dist - (travelled - self.step);
                self.last = Some(p);
            }
        }
    }
}

/// A brush tip made from a picture rather than from a formula.
///
/// The built-in tip is a disc with a falloff: two numbers, and every stroke it
/// makes looks like every other. A tip taken from a selection is a shape —
/// a leaf, a spatter, a piece of texture — and stamping it along a stroke is
/// how a brush stops looking like a brush.
///
/// Held behind a pointer because a stroke may be started many times a second
/// and a tip is a picture; copying one per stroke would be the most expensive
/// thing about painting with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Tip {
    /// Coverage, normalised so its strongest pixel is full — otherwise a tip
    /// taken from something faint would paint faintly however hard you
    /// pressed, and the opacity control would appear broken.
    coverage: MaskBuffer,
}

impl Tip {
    /// Build a tip from coverage, normalising it.
    pub fn new(coverage: MaskBuffer) -> Option<Tip> {
        let strongest = coverage.as_bytes().iter().copied().max().unwrap_or(0);
        if coverage.width() == 0 || coverage.height() == 0 || strongest == 0 {
            return None;
        }
        let mut out = MaskBuffer::hide_all(coverage.width(), coverage.height());
        for y in 0..coverage.height() as i32 {
            for x in 0..coverage.width() as i32 {
                let v = coverage.get(x, y) as u32 * 255 / strongest as u32;
                out.set(x, y, v.min(255) as u8);
            }
        }
        Some(Tip { coverage: out })
    }

    /// A tip from a picture's own transparency — what a cut-out shape gives.
    pub fn from_alpha(px: &PixelBuffer) -> Option<Tip> {
        let mut m = MaskBuffer::hide_all(px.width(), px.height());
        for y in 0..px.height() as i32 {
            for x in 0..px.width() as i32 {
                m.set(x, y, px.get(x, y).a);
            }
        }
        Tip::new(m)
    }

    pub fn size(&self) -> (u32, u32) {
        (self.coverage.width(), self.coverage.height())
    }

    /// Coverage at a point of the dab, given the dab's radius.
    ///
    /// The tip's longer side is fitted to the dab's diameter and its shape is
    /// kept. Stretching it to fill the square instead would mean a wide, thin
    /// tip stamped as a square — which is the one thing a shaped brush must
    /// not do, since the shape is the entire reason for having one.
    #[inline]
    fn at(&self, dx: f32, dy: f32, radius: f32) -> f32 {
        let (w, h) = (self.coverage.width() as f32, self.coverage.height() as f32);
        let longest = w.max(h);
        if longest <= 0.0 || radius <= 0.0 {
            return 0.0;
        }
        // Pixels of the tip per pixel of the dab.
        let per = longest / (radius * 2.0);
        let u = dx * per + w / 2.0;
        let v = dy * per + h / 2.0;
        if u < 0.0 || v < 0.0 || u >= w || v >= h {
            return 0.0;
        }
        self.coverage.get(u as i32, v as i32) as f32 / 255.0
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
    walk: DabWalk,
    /// The shape each dab stamps, when it is not the built-in disc.
    tip: Option<std::sync::Arc<Tip>>,
    /// Pressure at the last point, so a segment can interpolate along itself
    /// rather than stepping at every sample the pointer happens to send.
    last_pressure: f32,
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
            walk: DabWalk::new(&brush),
            tip: None,
            last_pressure: 1.0,
            started: false,
        }
    }

    /// Stamp a shape rather than a disc.
    pub fn with_tip(mut self, tip: Option<std::sync::Arc<Tip>>) -> Self {
        self.tip = tip;
        self
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
        snapshot: &Snapshot<Rgba8>,
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
                let base = snapshot.at(x, y);
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
            PaintMode::Retouch(r) => r.apply(src, coverage),
        };
        out.to_u8()
    }

    /// Paint into a greyscale buffer — a layer mask, or the Quick Mask.
    ///
    /// The stroke's colour is reduced to its luma, so painting black conceals
    /// and painting white reveals, exactly as on a layer mask.
    pub fn render_region_into_mask(
        &self,
        snapshot: &Snapshot<u8>,
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
                let base = snapshot.at(x, y) as f32;
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
        self.add_point_pressed(p, 1.0);
    }

    /// The same, with how hard the pen is pressed, `0..=1`.
    ///
    /// The pressure is interpolated along the segment rather than applied at
    /// its end: a pointer sends a handful of samples a second and a stroke
    /// lays down dabs far faster than that, so stepping the pressure at each
    /// sample makes a visibly banded line.
    pub fn add_point_pressed(&mut self, p: Vec2, pressure: f32) {
        let from = self.last_pressure;
        let to = pressure.clamp(0.0, 1.0);
        let mut centres = Vec::new();
        self.walk.advance(p, &mut centres);
        let n = centres.len().max(1) as f32;
        for (i, c) in centres.into_iter().enumerate() {
            let t = (i + 1) as f32 / n;
            self.stamp_pressed(c, from + (to - from) * t);
        }
        self.last_pressure = to;
    }

    /// Stamp one dab, accumulating coverage rather than replacing it.
    fn stamp_pressed(&mut self, centre: Vec2, pressure: f32) {
        // A brush that ignores pressure gets full pressure, whatever the pen
        // said — so a stroke made with the settings off is the stroke it would
        // have been with a mouse.
        let driven = self.brush.pressure;
        let k = if driven.any() { pressure.clamp(0.0, 1.0) } else { 1.0 };
        // Never quite nothing: a dab of zero radius is not a lighter mark, it
        // is a gap in the line.
        let scale = if driven.size { 0.05 + 0.95 * k } else { 1.0 };
        let r = self.brush.radius() * scale;
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

        self.source.prepare(rect);
        let flow = self.brush.flow.clamp(0.0, 1.0) * if driven.flow { k } else { 1.0 };
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                // Sample at the pixel centre.
                let dx = x as f32 + 0.5 - centre.x;
                let dy = y as f32 + 0.5 - centre.y;
                // A tip stamps its own shape; without one the dab is the
                // built-in disc and its falloff.
                let dab = match &self.tip {
                    Some(tip) => tip.at(dx, dy, r),
                    None => self.brush.falloff((dx * dx + dy * dy).sqrt()),
                };
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


/// The smudge tool: colour picked up under the brush and dragged along.
///
/// # Why this cannot be a stroke
///
/// Every other brush here accumulates coverage into a mask and composites it
/// onto the layer once, which makes overlapping dabs behave and gives one undo
/// step for free. Smudging cannot work that way: what it lays down at a pixel
/// depends on what the brush picked up several dabs ago, so the dabs have to
/// happen in order and each one has to see the result of the last. It writes
/// to the layer as it goes, and the caller keeps the original in a
/// [`crate::snapshot::Snapshot`] so it is still one undo step.
///
/// # What it carries
///
/// A square of colour the size of the brush, in brush-local coordinates, which
/// is deliberately *not* moved when the brush moves. That is the whole trick:
/// the colour held at local position `(i, j)` was picked up from an image
/// position one dab-step back, so putting it down at `(i, j)` now drags it
/// forward. Shifting the buffer with the brush would smear nothing anywhere.
#[derive(Debug, Clone)]
pub struct Smudge {
    brush: Brush,
    /// Premultiplied colour under the brush, `side * side`, brush-local.
    carried: Vec<Rgba>,
    side: i32,
    /// How much of the picture is taken up per dab, `0..=1`. Low values let go
    /// of what was picked up quickly, so the smear is short.
    strength: f32,
    walk: DabWalk,
    loaded: bool,
    bounds: IRect,
}

impl Smudge {
    /// `strength` is how far each dab drags: `0` does nothing, `1` carries the
    /// picked-up colour indefinitely.
    pub fn new(brush: Brush, strength: f32) -> Smudge {
        let side = (brush.radius().ceil() as i32) * 2 + 1;
        Smudge {
            brush,
            carried: vec![Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }; (side * side).max(1) as usize],
            side,
            strength: strength.clamp(0.0, 1.0),
            walk: DabWalk::new(&brush),
            loaded: false,
            bounds: IRect::EMPTY,
        }
    }

    /// Everything this stroke has touched so far.
    pub fn bounds(&self) -> IRect {
        self.bounds
    }

    pub fn is_empty(&self) -> bool {
        self.bounds.is_empty()
    }

    /// The rectangle the next `advance` to `p` will touch, so the caller can
    /// take its undo snapshot before anything is written.
    pub fn reach(&self, from: Vec2, to: Vec2) -> IRect {
        let r = self.brush.radius() + 1.0;
        IRect::new(
            (from.x.min(to.x) - r).floor() as i32,
            (from.y.min(to.y) - r).floor() as i32,
            (from.x.max(to.x) + r).ceil() as i32 + 1,
            (from.y.max(to.y) + r).ceil() as i32 + 1,
        )
    }

    /// Extend the stroke to `p`, smudging as it goes. Returns what changed.
    pub fn advance(&mut self, pixels: &mut PixelBuffer, p: Vec2, clip: Option<&Clip>) -> IRect {
        let mut centres = Vec::new();
        self.walk.advance(p, &mut centres);
        let mut touched = IRect::EMPTY;
        for c in centres {
            touched = touched.union(&self.dab(pixels, c, clip));
        }
        self.bounds = self.bounds.union(&touched);
        touched
    }

    fn dab(&mut self, pixels: &mut PixelBuffer, centre: Vec2, clip: Option<&Clip>) -> IRect {
        let r = self.brush.radius();
        let (x0, y0) = ((centre.x - r).floor() as i32, (centre.y - r).floor() as i32);
        let rect = IRect::new(x0, y0, x0 + self.side, y0 + self.side).intersect(&pixels.bounds());
        if rect.is_empty() {
            return IRect::EMPTY;
        }
        let flow = self.brush.flow.clamp(0.0, 1.0) * self.brush.opacity.clamp(0.0, 1.0);

        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let (lx, ly) = (x - x0, y - y0);
                let i = (ly * self.side + lx) as usize;
                let Some(held) = self.carried.get(i).copied() else { continue };

                let d = ((x as f32 + 0.5 - centre.x).powi(2) + (y as f32 + 0.5 - centre.y).powi(2))
                    .sqrt();
                let fall = self.brush.falloff(d);
                if fall <= 0.0 {
                    continue;
                }

                let here = pixels.get(x, y).to_f32();
                let under = Rgba { r: here.r * here.a, g: here.g * here.a, b: here.b * here.a, a: here.a };

                // Pick up first: the brush is always freshening its load, or a
                // long stroke would drag one colour across the whole layer.
                let pickup = (1.0 - self.strength) * fall;
                let held = if self.loaded { mix(held, under, pickup) } else { under };
                self.carried[i] = held;

                if !self.loaded {
                    continue; // Nothing to put down until something is held.
                }
                let mut w = fall * flow;
                if let Some(clip) = clip {
                    w *= clip.coverage(x, y);
                }
                if w <= 0.0 {
                    continue;
                }
                let out = mix(under, held, w);
                // Back to straight colour; a fully transparent result keeps
                // the hue it had rather than becoming an arbitrary black.
                let a = out.a.clamp(0.0, 1.0);
                let straight = if a > 1e-6 {
                    Rgba { r: out.r / a, g: out.g / a, b: out.b / a, a }
                } else {
                    Rgba { a, ..here }
                };
                pixels.set(x, y, straight.to_u8());
            }
        }
        self.loaded = true;
        rect
    }
}

#[inline]
fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brush(size: f32) -> Brush {
        Brush { size, hardness: 1.0, opacity: 1.0, flow: 1.0, spacing: 0.1, ..Default::default() }
    }

    fn edge(w: u32, h: u32) -> PixelBuffer {
        let mut px = PixelBuffer::new(w, h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let v = if x < w as i32 / 2 { 0 } else { 255 };
                px.set(x, y, Rgba8::opaque(v, v, v));
            }
        }
        px
    }

    #[test]
    fn smudging_drags_colour_the_way_the_brush_went() {
        let mut px = edge(64, 32);
        let dark_before = px.get(20, 16).r;
        assert_eq!(dark_before, 0, "it starts black on the left");

        // From the white side, across the edge, into the black. Flow well
        // under one, or the carried colour simply replaces each pixel and the
        // trail has no shape to measure.
        let soft = Brush { flow: 0.35, ..brush(16.0) };
        let mut s = Smudge::new(soft, 0.75);
        for x in (8..=44).rev() {
            s.advance(&mut px, Vec2::new(x as f32, 16.0), None);
        }

        let near = px.get(26, 16).r;
        let far = px.get(12, 16).r;
        assert!(near > 40, "white should have been dragged left; at 26 it is {near}");
        assert!(near < 255, "but mixed with what was there, not replacing it");
        // The trail fades: the brush lets go of its load as it refreshes.
        assert!(far < near, "the far end should be fainter: {far} against {near}");
    }

    #[test]
    fn smudging_leaves_everything_it_did_not_touch() {
        let mut px = edge(64, 32);
        let before = px.clone();
        let mut s = Smudge::new(brush(10.0), 0.8);
        for x in 28..40 {
            s.advance(&mut px, Vec2::new(x as f32, 16.0), None);
        }
        // Well clear of a 10px brush walked along y = 16.
        for y in [0, 1, 30, 31] {
            for x in [0, 63] {
                assert_eq!(px.get(x, y), before.get(x, y), "({x}, {y}) should be untouched");
            }
        }
        assert!(!s.is_empty() && s.bounds().y0 >= 10, "and it should know what it touched");
    }

    /// Strength is the whole control: at zero the brush refreshes its load
    /// completely at every dab, so it puts back what it just took and the
    /// picture does not move.
    #[test]
    fn no_strength_is_no_smudge() {
        let mut px = edge(64, 32);
        let before = px.clone();
        let mut s = Smudge::new(brush(12.0), 0.0);
        for x in (12..=44).rev() {
            s.advance(&mut px, Vec2::new(x as f32, 16.0), None);
        }
        let moved = (0..64)
            .map(|x| (px.get(x, 16).r as i32 - before.get(x, 16).r as i32).abs())
            .max()
            .unwrap();
        assert!(moved <= 2, "nothing should have moved; the worst was {moved}");
    }

    #[test]
    fn the_blur_brush_softens_and_the_sharpen_brush_does_not() {
        let px = edge(64, 32);
        let soft = BrushFilter::Blur { radius: 4.0 };
        // Right at the edge, a blur has to land between the two sides.
        let at = soft.at(&px, 32, 16).r;
        assert!(at > 40 && at < 215, "a blur across an edge lands between: {at}");
        // Away from it, there is nothing to average, so nothing changes.
        assert_eq!(soft.at(&px, 5, 16).r, 0);
        assert_eq!(soft.at(&px, 60, 16).r, 255);

        // Sharpening pushes the dark side of an edge darker. It is already at
        // zero here, so measure the bright side instead: it should not dim.
        let keen = BrushFilter::Sharpen { radius: 3.0, amount: 1.0 };
        assert!(keen.at(&px, 34, 16).r >= 255 - 1, "the light side of an edge should not dim");
        assert_eq!(keen.at(&px, 5, 16).r, 0, "and flat areas are left alone");
    }

    /// A tip taken from a picture stamps that picture's shape. The built-in
    /// disc makes every stroke look like every other, which is the whole
    /// reason to want another one.
    #[test]
    fn a_custom_tip_stamps_its_own_shape() {
        // A tip that is a horizontal bar: wide and thin.
        let mut bar = MaskBuffer::hide_all(32, 32);
        for y in 14..18 {
            for x in 0..32 {
                bar.set(x, y, 255);
            }
        }
        let tip = std::sync::Arc::new(Tip::new(bar).expect("a tip with something in it"));

        let mut px = PixelBuffer::new(64, 64);
        let mut s = Stroke::new(64, 64, brush(24.0), PaintMode::Paint, Rgba8::BLACK)
            .with_tip(Some(tip));
        s.add_point(Vec2::new(32.0, 32.0));
        s.commit(&mut px, None);

        let tall = (0..64).filter(|&y| px.get(32, y).a > 8).count();
        let wide = (0..64).filter(|&x| px.get(x, 32).a > 8).count();
        assert!(wide > tall * 3, "a bar-shaped tip makes a bar: {wide} across, {tall} down");
    }

    /// A tip taken from something faint has to paint at full strength, or the
    /// opacity control appears broken.
    #[test]
    fn a_faint_tip_is_normalised_to_full_strength() {
        let mut faint = MaskBuffer::hide_all(8, 8);
        for y in 2..6 {
            for x in 2..6 {
                faint.set(x, y, 40);
            }
        }
        let tip = Tip::new(faint).unwrap();
        let mut px = PixelBuffer::new(32, 32);
        let mut s = Stroke::new(32, 32, brush(16.0), PaintMode::Paint, Rgba8::BLACK)
            .with_tip(Some(std::sync::Arc::new(tip)));
        s.add_point(Vec2::new(16.0, 16.0));
        s.commit(&mut px, None);
        assert!(px.get(16, 16).a > 240, "it should paint fully: {:?}", px.get(16, 16));
    }

    #[test]
    fn a_tip_of_nothing_is_refused_rather_than_painting_nothing() {
        assert!(Tip::new(MaskBuffer::hide_all(8, 8)).is_none());
        assert!(Tip::new(MaskBuffer::hide_all(0, 0)).is_none());
    }

    /// The tip's longer side is fitted to the brush's size, so the size
    /// control means the same thing whatever the tip was made from.
    #[test]
    fn the_brush_size_still_governs_a_custom_tip() {
        let mut square = MaskBuffer::hide_all(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                square.set(x, y, 255);
            }
        }
        let tip = std::sync::Arc::new(Tip::new(square).unwrap());
        let width = |size: f32| {
            let mut px = PixelBuffer::new(96, 96);
            let mut s = Stroke::new(96, 96, brush(size), PaintMode::Paint, Rgba8::BLACK)
                .with_tip(Some(tip.clone()));
            s.add_point(Vec2::new(48.0, 48.0));
            s.commit(&mut px, None);
            (0..96).filter(|&x| px.get(x, 48).a > 8).count()
        };
        let (small, large) = (width(12.0), width(40.0));
        assert!(large > small * 2, "{small} against {large}");
    }

    /// Pressure is one number and which of the three it drives is a choice.
    /// A brush that ignores it must lay down exactly what a mouse would.
    #[test]
    fn a_brush_that_ignores_pressure_paints_as_though_there_were_none() {
        let mut light = PixelBuffer::new(64, 64);
        let mut heavy = PixelBuffer::new(64, 64);
        for (px, pressure) in [(&mut light, 0.2f32), (&mut heavy, 1.0)] {
            let mut s = Stroke::new(64, 64, brush(16.0), PaintMode::Paint, Rgba8::BLACK);
            s.add_point_pressed(Vec2::new(16.0, 32.0), pressure);
            s.add_point_pressed(Vec2::new(48.0, 32.0), pressure);
            s.commit(px, None);
        }
        assert_eq!(light.pixels(), heavy.pixels(), "with pressure off, it must not matter");
    }

    #[test]
    fn pressure_drives_the_size_when_it_is_asked_to() {
        let width = |pressure: f32| {
            let mut b = brush(20.0);
            b.pressure = Pressure { size: true, flow: false, opacity: false };
            let mut px = PixelBuffer::new(64, 64);
            let mut s = Stroke::new(64, 64, b, PaintMode::Paint, Rgba8::BLACK);
            s.add_point_pressed(Vec2::new(16.0, 32.0), pressure);
            s.add_point_pressed(Vec2::new(48.0, 32.0), pressure);
            s.commit(&mut px, None);
            (0..64).filter(|&y| px.get(32, y).a > 8).count()
        };
        let (light, heavy) = (width(0.25), width(1.0));
        assert!(heavy > light * 2, "a harder press should be wider: {light} against {heavy}");
        assert!(light > 0, "and a light one should still mark: {light}");
    }

    /// One dab, because flow is what a dab deposits and overlapping dabs
    /// accumulate: stroke far enough at any flow above nothing and the
    /// coverage saturates, which is what flow is *for* and would hide the
    /// difference being measured here.
    #[test]
    fn pressure_drives_the_flow_when_it_is_asked_to() {
        let darkness = |pressure: f32| {
            let mut b = brush(16.0);
            b.flow = 0.5;
            b.pressure = Pressure { size: false, flow: true, opacity: false };
            let mut px = PixelBuffer::new(64, 64);
            let mut s = Stroke::new(64, 64, b, PaintMode::Paint, Rgba8::BLACK);
            s.add_point_pressed(Vec2::new(32.0, 32.0), pressure);
            s.commit(&mut px, None);
            px.get(32, 32).a as i32
        };
        let (heavy, light) = (darkness(1.0), darkness(0.2));
        assert!(heavy > light + 20, "a harder press should deposit more: {light} then {heavy}");
        assert!(light > 0, "and a light one should still mark");
    }

    /// A pointer sends a handful of samples a second and a stroke lays down
    /// dabs far faster, so pressure has to interpolate along a segment or the
    /// line comes out banded.
    #[test]
    fn pressure_changes_along_a_segment_and_not_at_its_end() {
        let mut b = brush(20.0);
        b.pressure = Pressure { size: true, flow: false, opacity: false };
        let mut px = PixelBuffer::new(128, 64);
        let mut s = Stroke::new(128, 64, b, PaintMode::Paint, Rgba8::BLACK);
        s.add_point_pressed(Vec2::new(10.0, 32.0), 0.15);
        s.add_point_pressed(Vec2::new(118.0, 32.0), 1.0);
        s.commit(&mut px, None);

        let width_at = |x: i32| (0..64).filter(|&y| px.get(x, y).a > 8).count();
        let (a, b_, c) = (width_at(25), width_at(64), width_at(105));
        assert!(a < b_ && b_ < c, "the line should widen along its length: {a}, {b_}, {c}");
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
        let mut snapshot = Snapshot::new(48, 48, Rgba8::TRANSPARENT);
        let mut s2 = Stroke::new(48, 48, b, PaintMode::Paint, Rgba8::WHITE);
        for p in points {
            s2.add_point(p);
            let r = s2.take_recent();
            // Captured from the live buffer, as the editor does — which is
            // also a check that a tile taken once is not retaken after paint.
            snapshot.capture(&live, r);
            s2.render_region(&snapshot, &mut live, r, None);
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
        let clip = Clip { selection: &selection, offset: (0, 0) };

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
        let half = Selection::from_mask(mask);
        let clip = Clip { selection: &half, offset: (0, 0) };

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
        let clip = Clip { selection: &selection, offset: (100, 100) };

        let mut s = Stroke::new(32, 32, brush(60.0), PaintMode::Paint, Rgba8::BLACK);
        s.add_point(Vec2::new(16.0, 16.0));
        s.commit(&mut px, Some(&clip));

        assert_eq!(px.get(8, 8), Rgba8::BLACK, "layer-local (8,8) is doc (108,108)");
        assert_eq!(px.get(24, 24), Rgba8::WHITE, "doc (124,124) is outside the selection");
    }

    #[test]
    fn painting_a_mask_moves_it_toward_the_colour_luma() {
        use crate::mask::MaskBuffer;
        let mut dst = MaskBuffer::new(32, 32, 255);
        let mut snapshot = Snapshot::new(32, 32, 0);

        let mut s = Stroke::new(32, 32, brush(12.0), PaintMode::Paint, Rgba8::BLACK);
        s.add_point(Vec2::new(16.0, 16.0));
        let r = s.take_recent();
        snapshot.capture(&dst, r);
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
        let clip = Clip { selection: &selection, offset: (0, 0) };

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
