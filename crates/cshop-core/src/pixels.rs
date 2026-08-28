//! CPU-side pixel storage.
//!
//! Layer pixels live here as 8-bit straight-alpha sRGB. The GPU owns the copy
//! that gets composited; this is the authoritative version that undo snapshots,
//! file I/O and thumbnails read from.

use crate::color::Rgba8;
use crate::geom::IRect;
use bytemuck::cast_slice;

/// A tightly packed RGBA8 image. Rows are contiguous, stride is always
/// `width * 4` bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct PixelBuffer {
    width: u32,
    height: u32,
    data: Vec<Rgba8>,
}

impl std::fmt::Debug for PixelBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PixelBuffer")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl PixelBuffer {
    /// Fully transparent buffer.
    pub fn new(width: u32, height: u32) -> Self {
        Self::filled(width, height, Rgba8::TRANSPARENT)
    }

    pub fn filled(width: u32, height: u32, color: Rgba8) -> Self {
        let n = width as usize * height as usize;
        Self { width, height, data: vec![color; n] }
    }

    /// Wrap existing pixels. Returns `None` if `data` is not exactly
    /// `width * height` long.
    pub fn from_pixels(width: u32, height: u32, data: Vec<Rgba8>) -> Option<Self> {
        (data.len() == width as usize * height as usize).then_some(Self { width, height, data })
    }

    /// Wrap interleaved RGBA bytes.
    pub fn from_rgba_bytes(width: u32, height: u32, bytes: &[u8]) -> Option<Self> {
        let n = width as usize * height as usize;
        if bytes.len() != n * 4 {
            return None;
        }
        let data = bytes.chunks_exact(4).map(|c| Rgba8::new(c[0], c[1], c[2], c[3])).collect();
        Some(Self { width, height, data })
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
    pub fn pixels(&self) -> &[Rgba8] {
        &self.data
    }

    #[inline]
    pub fn pixels_mut(&mut self) -> &mut [Rgba8] {
        &mut self.data
    }

    /// Interleaved RGBA bytes, ready for a texture upload or an encoder.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        cast_slice(&self.data)
    }

    #[inline]
    pub fn row(&self, y: u32) -> &[Rgba8] {
        let start = y as usize * self.width as usize;
        &self.data[start..start + self.width as usize]
    }

    #[inline]
    pub fn row_mut(&mut self, y: u32) -> &mut [Rgba8] {
        let start = y as usize * self.width as usize;
        let w = self.width as usize;
        &mut self.data[start..start + w]
    }

    /// Out-of-bounds reads return transparent, which keeps sampling loops
    /// branch-light at the edges.
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> Rgba8 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return Rgba8::TRANSPARENT;
        }
        self.data[y as usize * self.width as usize + x as usize]
    }

    /// Out-of-bounds writes are dropped.
    #[inline]
    pub fn set(&mut self, x: i32, y: i32, c: Rgba8) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let w = self.width as usize;
        self.data[y as usize * w + x as usize] = c;
    }

    pub fn fill(&mut self, color: Rgba8) {
        self.data.fill(color);
    }

    pub fn fill_rect(&mut self, rect: IRect, color: Rgba8) {
        let r = rect.intersect(&self.bounds());
        for y in r.y0..r.y1 {
            let row = self.row_mut(y as u32);
            row[r.x0 as usize..r.x1 as usize].fill(color);
        }
    }

    /// Copy out a region, clipped to the buffer. Pixels outside are transparent
    /// so the result is always exactly `rect`-sized — undo snapshots depend on
    /// that to restore without re-clipping.
    pub fn copy_rect(&self, rect: IRect) -> PixelBuffer {
        let mut out = PixelBuffer::new(rect.width(), rect.height());
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
    pub fn paste(&mut self, src: &PixelBuffer, x: i32, y: i32) {
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
                if px.a != 0 {
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
    pub fn downscale(&self, dst_w: u32, dst_h: u32) -> PixelBuffer {
        let (dst_w, dst_h) = (dst_w.max(1), dst_h.max(1));
        let mut out = PixelBuffer::new(dst_w, dst_h);
        if self.width == 0 || self.height == 0 {
            return out;
        }
        for dy in 0..dst_h {
            let sy0 = (dy as u64 * self.height as u64 / dst_h as u64) as u32;
            let sy1 = (((dy + 1) as u64 * self.height as u64 / dst_h as u64) as u32).max(sy0 + 1);
            for dx in 0..dst_w {
                let sx0 = (dx as u64 * self.width as u64 / dst_w as u64) as u32;
                let sx1 = (((dx + 1) as u64 * self.width as u64 / dst_w as u64) as u32).max(sx0 + 1);

                // Weight colour by alpha so transparent pixels do not drag the
                // hue toward black.
                let (mut r, mut g, mut b, mut a) = (0f32, 0f32, 0f32, 0f32);
                let mut n = 0f32;
                for sy in sy0..sy1.min(self.height) {
                    for px in &self.row(sy)[sx0 as usize..(sx1.min(self.width)) as usize] {
                        let af = px.a as f32;
                        r += px.r as f32 * af;
                        g += px.g as f32 * af;
                        b += px.b as f32 * af;
                        a += af;
                        n += 1.0;
                    }
                }
                if n == 0.0 {
                    continue;
                }
                let c = if a > 0.0 {
                    Rgba8::new(
                        (r / a).round() as u8,
                        (g / a).round() as u8,
                        (b / a).round() as u8,
                        (a / n).round() as u8,
                    )
                } else {
                    Rgba8::TRANSPARENT
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
