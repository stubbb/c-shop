//! 8-bit greyscale coverage buffers.
//!
//! Used for layer masks, selections and quick-mask editing. Kept separate from
//! [`crate::pixels::PixelBuffer`] because storing coverage as RGBA would waste
//! four times the memory on documents that are heavily masked.

use crate::geom::IRect;

/// A tightly packed 8-bit coverage buffer. `0` = fully hidden, `255` = fully
/// shown.
#[derive(Clone, PartialEq, Eq)]
pub struct MaskBuffer {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl std::fmt::Debug for MaskBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MaskBuffer")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl MaskBuffer {
    pub fn new(width: u32, height: u32, value: u8) -> Self {
        Self { width, height, data: vec![value; width as usize * height as usize] }
    }

    /// A mask that reveals everything — the default for a newly added layer
    /// mask.
    pub fn reveal_all(width: u32, height: u32) -> Self {
        Self::new(width, height, 255)
    }

    pub fn hide_all(width: u32, height: u32) -> Self {
        Self::new(width, height, 0)
    }

    pub fn from_bytes(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
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

    /// One row of coverage. Panics if `y` is outside the buffer, which is a
    /// caller error rather than something to paper over.
    #[inline]
    pub fn row(&self, y: u32) -> &[u8] {
        let start = y as usize * self.width as usize;
        &self.data[start..start + self.width as usize]
    }

    #[inline]
    pub fn row_mut(&mut self, y: u32) -> &mut [u8] {
        let start = y as usize * self.width as usize;
        let w = self.width as usize;
        &mut self.data[start..start + w]
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    #[inline]
    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Outside the buffer a mask reads as `0`, so a mask smaller than its layer
    /// hides the uncovered remainder, which is the conventional rule.
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return 0;
        }
        self.data[y as usize * self.width as usize + x as usize]
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, v: u8) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let w = self.width as usize;
        self.data[y as usize * w + x as usize] = v;
    }

    pub fn fill(&mut self, v: u8) {
        self.data.fill(v);
    }

    pub fn fill_rect(&mut self, rect: IRect, v: u8) {
        let r = rect.intersect(&self.bounds());
        for y in r.y0..r.y1 {
            let start = y as usize * self.width as usize;
            self.data[start + r.x0 as usize..start + r.x1 as usize].fill(v);
        }
    }

    pub fn invert(&mut self) {
        for v in &mut self.data {
            *v = 255 - *v;
        }
    }

    /// Bounding box of non-zero coverage, or [`IRect::EMPTY`] if the mask is
    /// blank. Lets tools skip work outside an active selection.
    /// The smallest rectangle containing every non-zero pixel.
    ///
    /// Row-parallel, and each row stops at its own first and last non-zero
    /// byte rather than examining every one. On a selection mask the size of a
    /// large canvas this is the difference between a scan that is felt and one
    /// that is not: the mask is mostly zero, and the interesting part of any
    /// row is at its two ends.
    pub fn coverage_bounds(&self) -> IRect {
        use rayon::prelude::*;
        let width = self.width as usize;
        if width == 0 || self.height == 0 {
            return IRect::EMPTY;
        }
        self.data
            .par_chunks(width)
            .enumerate()
            .filter_map(|(y, row)| {
                let first = row.iter().position(|&v| v != 0)?;
                // Having found one, the last is a search from the other end.
                let last = row.iter().rposition(|&v| v != 0).unwrap_or(first);
                Some(IRect::new(first as i32, y as i32, last as i32 + 1, y as i32 + 1))
            })
            .reduce(|| IRect::EMPTY, |a, b| a.union(&b))
    }

    /// Write `src` back over `rect`, the inverse of [`MaskBuffer::copy_rect`].
    pub fn paste(&mut self, src: &MaskBuffer, at_x: i32, at_y: i32) {
        for y in 0..src.height as i32 {
            for x in 0..src.width as i32 {
                self.set(at_x + x, at_y + y, src.get(x, y));
            }
        }
    }

    /// The coverage bounds of a sub-rectangle, in the buffer's own coordinates.
    ///
    /// Used after an edit that could only have touched `rect`: everything
    /// outside it is unchanged, so rescanning the whole buffer would be
    /// asking a question whose answer is already known.
    pub fn coverage_bounds_within(&self, rect: IRect) -> IRect {
        let rect = rect.intersect(&self.bounds());
        let mut found = IRect::EMPTY;
        for y in rect.y0..rect.y1 {
            let row = &self.data[y as usize * self.width as usize..][..self.width as usize];
            let slice = &row[rect.x0 as usize..rect.x1 as usize];
            if let Some(first) = slice.iter().position(|&v| v != 0) {
                let last = slice.iter().rposition(|&v| v != 0).unwrap_or(first);
                found = found.union(&IRect::new(
                    rect.x0 + first as i32,
                    y,
                    rect.x0 + last as i32 + 1,
                    y + 1,
                ));
            }
        }
        found
    }

    pub fn copy_rect(&self, rect: IRect) -> MaskBuffer {
        let mut out = MaskBuffer::new(rect.width(), rect.height(), 0);
        let clipped = rect.intersect(&self.bounds());
        if clipped.is_empty() {
            return out;
        }
        for y in clipped.y0..clipped.y1 {
            let s = y as usize * self.width as usize;
            let src = &self.data[s + clipped.x0 as usize..s + clipped.x1 as usize];
            let d = (y - rect.y0) as usize * out.width as usize + (clipped.x0 - rect.x0) as usize;
            out.data[d..d + src.len()].copy_from_slice(src);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_bounds_reads_as_hidden() {
        let m = MaskBuffer::reveal_all(4, 4);
        assert_eq!(m.get(0, 0), 255);
        assert_eq!(m.get(-1, 0), 0);
        assert_eq!(m.get(4, 0), 0);
    }

    #[test]
    fn invert_is_an_involution() {
        let mut m = MaskBuffer::new(4, 4, 30);
        m.set(1, 1, 200);
        let original = m.clone();
        m.invert();
        m.invert();
        assert!(m == original);
    }

    #[test]
    fn coverage_bounds_tracks_painted_area() {
        let mut m = MaskBuffer::hide_all(10, 10);
        assert!(m.coverage_bounds().is_empty());
        m.fill_rect(IRect::at(2, 3, 4, 2), 128);
        assert_eq!(m.coverage_bounds(), IRect::new(2, 3, 6, 5));
    }

    #[test]
    fn copy_rect_pads_with_zero() {
        let m = MaskBuffer::reveal_all(4, 4);
        let c = m.copy_rect(IRect::at(-1, -1, 3, 3));
        assert_eq!(c.get(0, 0), 0);
        assert_eq!(c.get(1, 1), 255);
    }
}
