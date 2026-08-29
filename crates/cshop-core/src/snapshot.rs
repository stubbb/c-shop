//! The original pixels under a stroke, captured only where it paints.
//!
//! A stroke needs to know what was underneath it, for two reasons: each dab is
//! re-rendered from the original so that overlapping dabs blend against it
//! rather than compounding, and the undo entry needs the "before" of whatever
//! changed. Both only ever look inside the area actually painted.
//!
//! Copying the whole layer up front satisfies both and is what this replaces.
//! It also made pressing the mouse button cost a memcpy of the entire
//! document — 400 MB, about 150 ms, on a 10000x10000 canvas — for a stroke
//! that might cover a hundred pixels. Here the original is taken a tile at a
//! time, the first time a dab touches one, so the cost belongs to the stroke
//! rather than to the canvas.

use ahash::AHashMap;

use crate::color::Rgba8;
use crate::geom::IRect;
use crate::mask::MaskBuffer;
use crate::pixels::PixelBuffer;

/// Side of a capture tile.
///
/// Small enough that a dab captures little that it does not need, large enough
/// that a long stroke is not a hash lookup per pixel. At four bytes a pixel
/// this is 64 KB of pixels or 16 KB of mask.
const TILE: i32 = 128;

/// Something a snapshot can be taken of.
pub trait Grid {
    type Item: Copy + PartialEq;
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    fn at(&self, x: i32, y: i32) -> Self::Item;
}

impl Grid for PixelBuffer {
    type Item = Rgba8;
    fn width(&self) -> u32 {
        PixelBuffer::width(self)
    }
    fn height(&self) -> u32 {
        PixelBuffer::height(self)
    }
    fn at(&self, x: i32, y: i32) -> Rgba8 {
        self.get(x, y)
    }
}

impl Grid for MaskBuffer {
    type Item = u8;
    fn width(&self) -> u32 {
        MaskBuffer::width(self)
    }
    fn height(&self) -> u32 {
        MaskBuffer::height(self)
    }
    fn at(&self, x: i32, y: i32) -> u8 {
        self.get(x, y)
    }
}

/// What was there before, for the parts a stroke has reached.
pub struct Snapshot<T> {
    tiles: AHashMap<(i32, i32), Vec<T>>,
    width: u32,
    height: u32,
    outside: T,
}

impl<T: Copy + PartialEq> Snapshot<T> {
    pub fn new(width: u32, height: u32, outside: T) -> Snapshot<T> {
        Snapshot { tiles: AHashMap::new(), width, height, outside }
    }

    /// Take the original of every tile `rect` touches that is not already held.
    ///
    /// Must be called with the source still untouched for that region — which
    /// is to say, before the dab is drawn. Tiles already captured are left
    /// alone, so a second dab over the same place keeps the true original.
    pub fn capture<G: Grid<Item = T>>(&mut self, source: &G, rect: IRect) {
        let rect = rect.intersect(&IRect::new(0, 0, self.width as i32, self.height as i32));
        if rect.is_empty() {
            return;
        }
        for ty in (rect.y0.div_euclid(TILE))..=((rect.y1 - 1).div_euclid(TILE)) {
            for tx in (rect.x0.div_euclid(TILE))..=((rect.x1 - 1).div_euclid(TILE)) {
                self.tiles.entry((tx, ty)).or_insert_with(|| {
                    let (ox, oy) = (tx * TILE, ty * TILE);
                    let mut cell = Vec::with_capacity((TILE * TILE) as usize);
                    for y in 0..TILE {
                        for x in 0..TILE {
                            cell.push(source.at(ox + x, oy + y));
                        }
                    }
                    cell
                });
            }
        }
    }

    /// What was at this point before the stroke.
    ///
    /// A point in no captured tile was never painted, so nothing ever asks for
    /// it; answering with the outside value keeps that from being a panic.
    pub fn at(&self, x: i32, y: i32) -> T {
        let (tx, ty) = (x.div_euclid(TILE), y.div_euclid(TILE));
        match self.tiles.get(&(tx, ty)) {
            Some(cell) => cell[(y.rem_euclid(TILE) * TILE + x.rem_euclid(TILE)) as usize],
            None => self.outside,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// How many tiles the stroke has actually reached.
    pub fn captured_tiles(&self) -> usize {
        self.tiles.len()
    }
}

impl Snapshot<Rgba8> {
    /// Put the original back over `rect`, for a stroke that was abandoned.
    pub fn restore(&self, dst: &mut PixelBuffer, rect: IRect) {
        let rect = rect.intersect(&dst.bounds());
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                dst.set(x, y, self.at(x, y));
            }
        }
    }

    /// The original of a rectangle, as a buffer — what the undo entry stores.
    pub fn copy_rect(&self, rect: IRect) -> PixelBuffer {
        let mut out = PixelBuffer::new(rect.width(), rect.height());
        for y in 0..rect.height() as i32 {
            for x in 0..rect.width() as i32 {
                out.set(x, y, self.at(rect.x0 + x, rect.y0 + y));
            }
        }
        out
    }
}

impl Snapshot<u8> {
    pub fn restore(&self, dst: &mut MaskBuffer, rect: IRect) {
        let rect = rect.intersect(&dst.bounds());
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                dst.set(x, y, self.at(x, y));
            }
        }
    }

    pub fn copy_rect(&self, rect: IRect) -> MaskBuffer {
        let mut out = MaskBuffer::new(rect.width(), rect.height(), 0);
        for y in 0..rect.height() as i32 {
            for x in 0..rect.width() as i32 {
                out.set(x, y, self.at(rect.x0 + x, rect.y0 + y));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> PixelBuffer {
        let mut px = PixelBuffer::new(400, 400);
        for y in 0..400i32 {
            for x in 0..400i32 {
                px.set(x, y, Rgba8::opaque((x % 256) as u8, (y % 256) as u8, 7));
            }
        }
        px
    }

    #[test]
    fn it_keeps_what_was_there_before_the_first_touch() {
        let mut px = source();
        let mut snap = Snapshot::new(400, 400, Rgba8::TRANSPARENT);

        snap.capture(&px, IRect::new(10, 10, 40, 40));
        // Paint over it, then capture the same place again: the second capture
        // must not overwrite the first, or undo restores the paint.
        for y in 10..40 {
            for x in 10..40 {
                px.set(x, y, Rgba8::opaque(1, 2, 3));
            }
        }
        snap.capture(&px, IRect::new(10, 10, 40, 40));

        assert_eq!(snap.at(20, 20), Rgba8::opaque(20, 20, 7), "the original, not the paint");
    }

    #[test]
    fn it_matches_a_whole_copy_over_the_area_it_covers() {
        let px = source();
        let mut snap = Snapshot::new(400, 400, Rgba8::TRANSPARENT);
        let rect = IRect::new(30, 70, 260, 190);
        snap.capture(&px, rect);
        assert_eq!(snap.copy_rect(rect).pixels(), px.copy_rect(rect).pixels());
    }

    /// The point of the whole thing: cost follows the stroke, not the canvas.
    #[test]
    fn it_captures_only_what_the_stroke_reached() {
        let px = PixelBuffer::new(10000, 10000);
        let mut snap = Snapshot::new(10000, 10000, Rgba8::TRANSPARENT);
        snap.capture(&px, IRect::new(5000, 5000, 5040, 5030));
        assert_eq!(snap.captured_tiles(), 1, "one 128-pixel tile, not 6100 of them");
    }

    #[test]
    fn tiles_line_up_across_a_boundary() {
        let px = source();
        let mut snap = Snapshot::new(400, 400, Rgba8::TRANSPARENT);
        // Straddling the tile grid at 128 is where an index slip would show.
        snap.capture(&px, IRect::new(120, 120, 140, 140));
        for (x, y) in [(120, 120), (127, 127), (128, 128), (135, 139), (139, 120)] {
            assert_eq!(snap.at(x, y), px.get(x, y), "at {x},{y}");
        }
    }
}
