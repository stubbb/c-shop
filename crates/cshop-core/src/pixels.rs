//! CPU-side pixel storage.
//!
//! Layer pixels live here as 8-bit straight-alpha sRGB. The GPU owns the copy
//! that gets composited; this is the authoritative version that undo snapshots,
//! file I/O and thumbnails read from.

use crate::color::{Rgba, Rgba8, Rgba16};
use crate::geom::IRect;
use bytemuck::cast_slice;

/// What one pixel can be made of.
///
/// Two implementations: [`Rgba8`], which is what a document holds unless it is
/// asked to hold more, and [`Rgba16`], which is what it holds when precision
/// through a chain of edits matters more than memory. Everything a buffer does
/// that does not depend on the depth — copying, pasting, bounds, downscaling —
/// is written once here and works for both.
pub trait Sample:
    Copy + bytemuck::Pod + PartialEq + Send + Sync + std::fmt::Debug + 'static
{
    /// Nothing there. Out-of-bounds reads return it, and new buffers are it.
    const CLEAR: Self;
    /// Fully opaque white, the other end most callers need by name.
    const WHITE: Self;

    fn to_f32(self) -> Rgba;
    fn from_f32(c: Rgba) -> Self;
    fn to_rgba8(self) -> Rgba8;
    fn from_rgba8(c: Rgba8) -> Self;

    /// Coverage, 0 to 1. Enough for the questions a buffer asks about alpha
    /// without unpacking the whole colour.
    fn alpha(self) -> f32;
}

impl Sample for Rgba8 {
    const CLEAR: Rgba8 = Rgba8::TRANSPARENT;
    const WHITE: Rgba8 = Rgba8::WHITE;

    #[inline]
    fn to_f32(self) -> Rgba {
        Rgba8::to_f32(self)
    }
    #[inline]
    fn from_f32(c: Rgba) -> Self {
        c.to_u8()
    }
    #[inline]
    fn to_rgba8(self) -> Rgba8 {
        self
    }
    #[inline]
    fn from_rgba8(c: Rgba8) -> Self {
        c
    }
    #[inline]
    fn alpha(self) -> f32 {
        self.a as f32 / 255.0
    }
}

impl Sample for Rgba16 {
    const CLEAR: Rgba16 = Rgba16::TRANSPARENT;
    const WHITE: Rgba16 = Rgba16::WHITE;

    #[inline]
    fn to_f32(self) -> Rgba {
        Rgba16::to_f32(self)
    }
    #[inline]
    fn from_f32(c: Rgba) -> Self {
        Rgba16::from_f32(c)
    }
    #[inline]
    fn to_rgba8(self) -> Rgba8 {
        Rgba16::to_rgba8(self)
    }
    #[inline]
    fn from_rgba8(c: Rgba8) -> Self {
        Rgba16::from_rgba8(c)
    }
    #[inline]
    fn alpha(self) -> f32 {
        self.a as f32 / 65535.0
    }
}

/// Sixteen bits a channel: sixty-four to a pixel.
pub type DeepBuffer = PixelBuffer<Rgba16>;

/// A tightly packed RGBA image. Rows are contiguous, stride is always
/// `width * 4` samples.
///
/// The sample type defaults to [`Rgba8`], so `PixelBuffer` unqualified means
/// what it has always meant and the depth is something only the code that
/// cares has to name.
#[derive(Clone, PartialEq, Eq)]
pub struct PixelBuffer<S: Sample = Rgba8> {
    width: u32,
    height: u32,
    data: Vec<S>,
}

impl<S: Sample> std::fmt::Debug for PixelBuffer<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PixelBuffer")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// How many samples a box-filter cell contributes along each axis.
///
/// Sixty-four samples per output pixel is far more than a thumbnail needs, and
/// it makes the work proportional to the thumbnail rather than to the image:
/// a 48-pixel square costs about 150 thousand samples whether the source is a
/// megapixel or a hundred. Averaging *every* source pixel instead is what made
/// large documents unusable to paint on.
///
/// Striding can alias against a regular pattern finer than the step, which at
/// this size is a thumbnail that shimmers slightly as a layer is edited. That
/// is a fair trade for the panel staying responsive.
pub const SAMPLES_PER_CELL: u32 = 8;

/// The stride that keeps a cell of `span` pixels under [`SAMPLES_PER_CELL`]
/// samples. Never zero, so it is always safe for `step_by`.
pub fn sample_step(span: u32) -> u32 {
    (span / SAMPLES_PER_CELL).max(1)
}

impl PixelBuffer<Rgba8> {
    /// Wrap interleaved RGBA bytes.
    pub fn from_rgba_bytes(width: u32, height: u32, bytes: &[u8]) -> Option<Self> {
        let n = width as usize * height as usize;
        if bytes.len() != n * 4 {
            return None;
        }
        let data = bytes.chunks_exact(4).map(|c| Rgba8::new(c[0], c[1], c[2], c[3])).collect();
        Some(Self { width, height, data })
    }

    /// The same picture at sixteen bits a channel. Exact: nothing is lost and
    /// nothing is invented, every value simply counted more finely.
    pub fn to_deep(&self) -> DeepBuffer {
        DeepBuffer {
            width: self.width,
            height: self.height,
            data: self.data.iter().map(|&c| Rgba16::from_rgba8(c)).collect(),
        }
    }
}

impl PixelBuffer<Rgba16> {
    /// Back to eight bits, rounding to nearest.
    ///
    /// This is where a deep document's precision is spent, so it belongs at
    /// the edges — writing an eight-bit file, uploading to the screen — and
    /// not in the middle of a chain of edits.
    pub fn to_eight(&self) -> PixelBuffer<Rgba8> {
        PixelBuffer {
            width: self.width,
            height: self.height,
            data: self.data.iter().map(|&c| c.to_rgba8()).collect(),
        }
    }
}

impl<S: Sample> PixelBuffer<S> {
    /// Fully transparent buffer.
    pub fn new(width: u32, height: u32) -> Self {
        Self::filled(width, height, S::CLEAR)
    }

    pub fn filled(width: u32, height: u32, color: S) -> Self {
        let n = width as usize * height as usize;
        Self { width, height, data: vec![color; n] }
    }

    /// Wrap existing pixels. Returns `None` if `data` is not exactly
    /// `width * height` long.
    pub fn from_pixels(width: u32, height: u32, data: Vec<S>) -> Option<Self> {
        (data.len() == width as usize * height as usize).then_some(Self { width, height, data })
    }

    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    #[inline]
    pub fn bounds(&self) -> IRect {
        IRect::from_size(self.width, self.height)
    }

    #[inline]
    pub fn pixels(&self) -> &[S] {
        &self.data
    }

    #[inline]
    pub fn pixels_mut(&mut self) -> &mut [S] {
        &mut self.data
    }

    /// Interleaved RGBA bytes, ready for a texture upload or an encoder.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        cast_slice(&self.data)
    }

    #[inline]
    pub fn row(&self, y: u32) -> &[S] {
        let start = y as usize * self.width as usize;
        &self.data[start..start + self.width as usize]
    }

    #[inline]
    pub fn row_mut(&mut self, y: u32) -> &mut [S] {
        let start = y as usize * self.width as usize;
        let w = self.width as usize;
        &mut self.data[start..start + w]
    }

    /// Out-of-bounds reads return transparent, which keeps sampling loops
    /// branch-light at the edges.
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> S {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return S::CLEAR;
        }
        self.data[y as usize * self.width as usize + x as usize]
    }

    /// Out-of-bounds writes are dropped.
    #[inline]
    pub fn set(&mut self, x: i32, y: i32, c: S) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let w = self.width as usize;
        self.data[y as usize * w + x as usize] = c;
    }

    pub fn fill(&mut self, color: S) {
        self.data.fill(color);
    }

    pub fn fill_rect(&mut self, rect: IRect, color: S) {
        let r = rect.intersect(&self.bounds());
        for y in r.y0..r.y1 {
            let row = self.row_mut(y as u32);
            row[r.x0 as usize..r.x1 as usize].fill(color);
        }
    }

    /// Copy out a region, clipped to the buffer. Pixels outside are transparent
    /// so the result is always exactly `rect`-sized — undo snapshots depend on
    /// that to restore without re-clipping.
    pub fn copy_rect(&self, rect: IRect) -> PixelBuffer<S> {
        let mut out = PixelBuffer::<S>::new(rect.width(), rect.height());
        let clipped = rect.intersect(&self.bounds());
        if clipped.is_empty() {
            return out;
        }
        for y in clipped.y0..clipped.y1 {
            let src = &self.row(y as u32)[clipped.x0 as usize..clipped.x1 as usize];
            let dy = (y - rect.y0) as u32;
            let dx = (clipped.x0 - rect.x0) as usize;
            out.row_mut(dy)[dx..dx + src.len()].copy_from_slice(src);
        }
        out
    }

    /// Paste `src` with its top-left at `(x, y)`, replacing pixels outright.
    pub fn paste(&mut self, src: &PixelBuffer<S>, x: i32, y: i32) {
        let dst_rect = IRect::at(x, y, src.width(), src.height()).intersect(&self.bounds());
        if dst_rect.is_empty() {
            return;
        }
        for dy in dst_rect.y0..dst_rect.y1 {
            let sy = (dy - y) as u32;
            let sx = (dst_rect.x0 - x) as usize;
            let n = dst_rect.width() as usize;
            let src_row = &src.row(sy)[sx..sx + n];
            let dx = dst_rect.x0 as usize;
            self.row_mut(dy as u32)[dx..dx + n].copy_from_slice(src_row);
        }
    }

    /// Tight bounding box of pixels with non-zero alpha, or
    /// [`IRect::EMPTY`] when the buffer is fully transparent.
    pub fn opaque_bounds(&self) -> IRect {
        let (mut x0, mut y0) = (i32::MAX, i32::MAX);
        let (mut x1, mut y1) = (i32::MIN, i32::MIN);
        for y in 0..self.height {
            for (x, px) in self.row(y).iter().enumerate() {
                if px.alpha() != 0.0 {
                    let x = x as i32;
                    x0 = x0.min(x);
                    x1 = x1.max(x + 1);
                    y0 = y0.min(y as i32);
                    y1 = y1.max(y as i32 + 1);
                }
            }
        }
        if x0 == i32::MAX {
            IRect::EMPTY
        } else {
            IRect::new(x0, y0, x1, y1)
        }
    }

    /// Box-filtered downscale, used for layer thumbnails. Averaging in sRGB is
    /// deliberate: thumbnails should match what the canvas shows.
    ///
    /// Cost is bounded by the *output* size rather than the input's, because a
    /// thumbnail is regenerated every time its layer changes — which, during a
    /// brush stroke, is every frame. Averaging every source pixel makes that
    /// cost proportional to the canvas, so a 10000x10000 document spent 150 ms
    /// per stroke step building a 48-pixel picture nobody was looking at that
    /// closely. See [`SAMPLES_PER_CELL`].
    pub fn downscale(&self, dst_w: u32, dst_h: u32) -> PixelBuffer<S> {
        let (dst_w, dst_h) = (dst_w.max(1), dst_h.max(1));
        let mut out = PixelBuffer::<S>::new(dst_w, dst_h);
        if self.width == 0 || self.height == 0 {
            return out;
        }
        for dy in 0..dst_h {
            let sy0 = (dy as u64 * self.height as u64 / dst_h as u64) as u32;
            let sy1 = (((dy + 1) as u64 * self.height as u64 / dst_h as u64) as u32).max(sy0 + 1);
            let step_y = sample_step(sy1.min(self.height).saturating_sub(sy0));
            for dx in 0..dst_w {
                let sx0 = (dx as u64 * self.width as u64 / dst_w as u64) as u32;
                let sx1 = (((dx + 1) as u64 * self.width as u64 / dst_w as u64) as u32).max(sx0 + 1);
                let step_x = sample_step(sx1.min(self.width).saturating_sub(sx0));

                // Weight colour by alpha so transparent pixels do not drag the
                // hue toward black.
                let (mut r, mut g, mut b, mut a) = (0f32, 0f32, 0f32, 0f32);
                let mut n = 0f32;
                for sy in (sy0..sy1.min(self.height)).step_by(step_y as usize) {
                    let row = self.row(sy);
                    for sx in (sx0..sx1.min(self.width)).step_by(step_x as usize) {
                        let px = row[sx as usize].to_f32();
                        let af = px.a;
                        r += px.r * af;
                        g += px.g * af;
                        b += px.b * af;
                        a += af;
                        n += 1.0;
                    }
                }
                if n == 0.0 {
                    continue;
                }
                let c = if a > 0.0 {
                    S::from_f32(Rgba::new(r / a, g / a, b / a, a / n))
                } else {
                    S::CLEAR
                };
                out.set(dx as i32, dy as i32, c);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_rect_clips_to_bounds() {
        let mut b = PixelBuffer::new(4, 4);
        b.fill_rect(IRect::at(-2, -2, 4, 4), Rgba8::WHITE);
        assert_eq!(b.get(0, 0), Rgba8::WHITE);
        assert_eq!(b.get(1, 1), Rgba8::WHITE);
        assert_eq!(b.get(2, 2), Rgba8::TRANSPARENT);
    }

    #[test]
    fn copy_rect_pads_outside_with_transparency() {
        let mut b = PixelBuffer::filled(4, 4, Rgba8::WHITE);
        b.set(0, 0, Rgba8::BLACK);
        let c = b.copy_rect(IRect::at(-1, -1, 3, 3));
        assert_eq!(c.width(), 3);
        assert_eq!(c.get(0, 0), Rgba8::TRANSPARENT);
        assert_eq!(c.get(1, 1), Rgba8::BLACK);
        assert_eq!(c.get(2, 2), Rgba8::WHITE);
    }

    #[test]
    fn copy_then_paste_round_trips() {
        let mut b = PixelBuffer::filled(8, 8, Rgba8::WHITE);
        b.fill_rect(IRect::at(2, 2, 3, 3), Rgba8::BLACK);
        let rect = IRect::at(1, 1, 5, 5);
        let snapshot = b.copy_rect(rect);
        b.fill(Rgba8::TRANSPARENT);
        b.paste(&snapshot, rect.x0, rect.y0);
        assert_eq!(b.get(3, 3), Rgba8::BLACK);
        assert_eq!(b.get(1, 1), Rgba8::WHITE);
    }

    #[test]
    fn paste_fully_outside_is_a_noop() {
        let mut b = PixelBuffer::filled(4, 4, Rgba8::WHITE);
        let before = b.clone();
        b.paste(&PixelBuffer::filled(2, 2, Rgba8::BLACK), 90, 90);
        b.paste(&PixelBuffer::filled(2, 2, Rgba8::BLACK), -9, -9);
        assert!(b == before);
    }

    #[test]
    fn opaque_bounds_finds_the_tight_box() {
        let mut b = PixelBuffer::new(16, 16);
        assert!(b.opaque_bounds().is_empty());
        b.set(3, 5, Rgba8::WHITE);
        b.set(9, 11, Rgba8::WHITE);
        assert_eq!(b.opaque_bounds(), IRect::new(3, 5, 10, 12));
    }

    #[test]
    fn downscale_preserves_a_flat_colour() {
        let b = PixelBuffer::filled(64, 64, Rgba8::opaque(200, 100, 50));
        let t = b.downscale(8, 8);
        assert_eq!(t.width(), 8);
        assert_eq!(t.get(4, 4), Rgba8::opaque(200, 100, 50));
    }

    #[test]
    fn downscale_ignores_colour_under_transparency() {
        // A red-but-transparent field must not tint the visible white.
        let mut b = PixelBuffer::filled(4, 4, Rgba8::new(255, 0, 0, 0));
        b.set(0, 0, Rgba8::WHITE);
        let t = b.downscale(1, 1);
        let p = t.get(0, 0);
        assert_eq!((p.r, p.g, p.b), (255, 255, 255));
    }

    #[test]
    fn byte_view_is_interleaved_rgba() {
        let b = PixelBuffer::filled(2, 1, Rgba8::new(1, 2, 3, 4));
        assert_eq!(b.as_bytes(), &[1, 2, 3, 4, 1, 2, 3, 4]);
        assert_eq!(PixelBuffer::from_rgba_bytes(2, 1, b.as_bytes()).unwrap(), b);
        assert!(PixelBuffer::from_rgba_bytes(3, 1, b.as_bytes()).is_none());
    }
}
