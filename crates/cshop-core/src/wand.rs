//! Colour-based selection: the Magic Wand, and the Grow and Similar commands
//! that reuse its matching rule.

use crate::color::Rgba8;
use crate::geom::IRect;
use crate::mask::MaskBuffer;
use crate::pixels::PixelBuffer;
use crate::selection::Selection;

/// Settings shared by the wand and by Grow / Similar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WandOptions {
    /// Maximum per-channel difference that still counts as a match, `0..=255`.
    pub tolerance: u8,
    /// Restrict the result to the region connected to the click point.
    pub contiguous: bool,
    /// Soften the staircase along the boundary.
    pub antialias: bool,
}

impl Default for WandOptions {
    fn default() -> Self {
        Self { tolerance: 32, contiguous: true, antialias: true }
    }
}

/// How close two colours are, as the largest difference across the channels.
///
/// Channels are compared independently rather than by a Euclidean
/// distance, so a tolerance of 32 means "within 32 levels on every channel".
/// Alpha participates, which is what stops the wand from spilling across the
/// edge of a transparent region.
#[inline]
fn difference(a: Rgba8, b: Rgba8) -> u8 {
    let d = |x: u8, y: u8| x.abs_diff(y);
    d(a.r, b.r).max(d(a.g, b.g)).max(d(a.b, b.b)).max(d(a.a, b.a))
}

/// Select pixels matching the colour at `(seed_x, seed_y)`.
///
/// `source` is the image being sampled — the active layer, or the composite
/// when *Sample All Layers* is on.
pub fn magic_wand(
    source: &PixelBuffer,
    seed_x: i32,
    seed_y: i32,
    options: WandOptions,
) -> Selection {
    let (w, h) = (source.width(), source.height());
    let mut mask = MaskBuffer::hide_all(w, h);
    if !source.bounds().contains(seed_x, seed_y) {
        return Selection::from_mask(mask);
    }
    let target = source.get(seed_x, seed_y);

    if options.contiguous {
        flood_fill(source, &mut mask, seed_x, seed_y, target, options.tolerance);
    } else {
        for y in 0..h {
            for x in 0..w {
                if difference(source.get(x as i32, y as i32), target) <= options.tolerance {
                    mask.set(x as i32, y as i32, 255);
                }
            }
        }
    }

    finish(mask, options.antialias)
}

/// Extend `selection` into neighbouring pixels that match the colours it
/// already covers — Select > Grow.
pub fn grow(source: &PixelBuffer, selection: &Selection, options: WandOptions) -> Selection {
    let (w, h) = (source.width(), source.height());
    let mut mask = selection.to_mask();

    // Seed the queue with every pixel already selected, then flood outward.
    let mut queue: Vec<(i32, i32)> = Vec::new();
    let bounds = selection.bounds();
    for y in bounds.y0..bounds.y1 {
        for x in bounds.x0..bounds.x1 {
            if mask.get(x, y) >= 128 {
                mask.set(x, y, 255);
                queue.push((x, y));
            }
        }
    }

    while let Some((x, y)) = queue.pop() {
        let colour = source.get(x, y);
        for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            if mask.get(nx, ny) >= 128 {
                continue;
            }
            if difference(source.get(nx, ny), colour) <= options.tolerance {
                mask.set(nx, ny, 255);
                queue.push((nx, ny));
            }
        }
    }

    finish(mask, options.antialias)
}

/// Select every pixel in the image resembling one already selected —
/// Select > Similar.
pub fn similar(source: &PixelBuffer, selection: &Selection, options: WandOptions) -> Selection {
    let (w, h) = (source.width(), source.height());

    // Collect the distinct colours under the selection. Quantising to the
    // tolerance keeps the set small on photographic content, where a selection
    // can otherwise hold tens of thousands of unique colours and turn the
    // comparison below into an hours-long scan.
    let step = (options.tolerance as u32).max(1);
    let mut palette: Vec<Rgba8> = Vec::new();
    let mut seen: ahash::AHashSet<(u32, u32, u32, u32)> = ahash::AHashSet::new();
    let bounds = selection.bounds();
    for y in bounds.y0..bounds.y1 {
        for x in bounds.x0..bounds.x1 {
            if selection.coverage(x, y) < 128 {
                continue;
            }
            let c = source.get(x, y);
            let key = (
                c.r as u32 / step,
                c.g as u32 / step,
                c.b as u32 / step,
                c.a as u32 / step,
            );
            if seen.insert(key) {
                palette.push(c);
            }
        }
    }

    let mut mask = MaskBuffer::hide_all(w, h);
    if palette.is_empty() {
        return Selection::from_mask(mask);
    }
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let c = source.get(x, y);
            if palette.iter().any(|&p| difference(c, p) <= options.tolerance) {
                mask.set(x, y, 255);
            }
        }
    }

    finish(mask, options.antialias)
}

/// Scanline flood fill.
///
/// Filling whole runs at a time rather than pushing every pixel keeps the
/// stack shallow; a naive four-way recursion overflows on large flat areas.
fn flood_fill(
    source: &PixelBuffer,
    mask: &mut MaskBuffer,
    seed_x: i32,
    seed_y: i32,
    target: Rgba8,
    tolerance: u8,
) {
    let (w, h) = (source.width() as i32, source.height() as i32);
    let matches = |x: i32, y: i32| difference(source.get(x, y), target) <= tolerance;

    let mut stack = vec![(seed_x, seed_y)];
    while let Some((sx, sy)) = stack.pop() {
        if sy < 0 || sy >= h || mask.get(sx, sy) != 0 || !matches(sx, sy) {
            continue;
        }

        // Walk left and right to the ends of this run.
        let mut left = sx;
        while left > 0 && mask.get(left - 1, sy) == 0 && matches(left - 1, sy) {
            left -= 1;
        }
        let mut right = sx;
        while right < w - 1 && mask.get(right + 1, sy) == 0 && matches(right + 1, sy) {
            right += 1;
        }

        for x in left..=right {
            mask.set(x, sy, 255);
        }

        // Seed the rows above and below, once per contiguous run rather than
        // once per pixel.
        for (dy, row) in [(-1, sy - 1), (1, sy + 1)] {
            let _ = dy;
            if row < 0 || row >= h {
                continue;
            }
            let mut x = left;
            while x <= right {
                if mask.get(x, row) == 0 && matches(x, row) {
                    stack.push((x, row));
                    // Skip the rest of this run; the pop above will expand it.
                    while x <= right && matches(x, row) {
                        x += 1;
                    }
                } else {
                    x += 1;
                }
            }
        }
    }
}

/// Turn a binary mask into a selection, softening the boundary if asked.
fn finish(mask: MaskBuffer, antialias: bool) -> Selection {
    let mut selection = Selection::from_mask(mask);
    if antialias && !selection.is_empty() {
        // Just enough to take the staircase off a diagonal edge without
        // visibly moving it.
        selection.feather(1.0);
    }
    selection
}

/// Bounding box of a wand result, for the caller to report or zoom to.
pub fn result_bounds(selection: &Selection) -> IRect {
    selection.bounds()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::IRect;

    /// Left half red, right half blue, with a green square in the red side.
    fn scene() -> PixelBuffer {
        let mut px = PixelBuffer::filled(32, 32, Rgba8::opaque(200, 30, 30));
        px.fill_rect(IRect::new(16, 0, 32, 32), Rgba8::opaque(30, 30, 200));
        px.fill_rect(IRect::new(2, 2, 8, 8), Rgba8::opaque(30, 200, 30));
        px
    }

    fn opts(tolerance: u8, contiguous: bool) -> WandOptions {
        WandOptions { tolerance, contiguous, antialias: false }
    }

    #[test]
    fn the_wand_selects_the_region_under_the_click() {
        let s = magic_wand(&scene(), 20, 16, opts(10, true));
        assert_eq!(s.coverage(20, 16), 255, "the clicked region");
        assert_eq!(s.coverage(10, 16), 0, "the other colour");
        assert_eq!(s.bounds(), IRect::new(16, 0, 32, 32));
    }

    #[test]
    fn contiguous_mode_stops_at_the_boundary() {
        // Two separate red areas: only the connected one should be picked up.
        let mut px = PixelBuffer::filled(32, 8, Rgba8::opaque(0, 0, 0));
        px.fill_rect(IRect::new(0, 0, 8, 8), Rgba8::opaque(255, 0, 0));
        px.fill_rect(IRect::new(24, 0, 32, 8), Rgba8::opaque(255, 0, 0));

        let near = magic_wand(&px, 4, 4, opts(10, true));
        assert_eq!(near.coverage(4, 4), 255);
        assert_eq!(near.coverage(28, 4), 0, "the disconnected patch stays out");

        let global = magic_wand(&px, 4, 4, opts(10, false));
        assert_eq!(global.coverage(28, 4), 255, "non-contiguous reaches both");
    }

    #[test]
    fn tolerance_widens_what_matches() {
        let mut px = PixelBuffer::filled(16, 16, Rgba8::opaque(100, 100, 100));
        px.fill_rect(IRect::new(8, 0, 16, 16), Rgba8::opaque(120, 100, 100));

        let tight = magic_wand(&px, 2, 8, opts(5, true));
        assert_eq!(tight.coverage(12, 8), 0, "20 levels apart, tolerance 5");

        let loose = magic_wand(&px, 2, 8, opts(30, true));
        assert_eq!(loose.coverage(12, 8), 255, "tolerance 30 spans the difference");
    }

    #[test]
    fn the_wand_respects_alpha() {
        // Same RGB, different alpha: the transparent half must not be included.
        let mut px = PixelBuffer::filled(16, 16, Rgba8::opaque(80, 80, 80));
        px.fill_rect(IRect::new(8, 0, 16, 16), Rgba8::new(80, 80, 80, 0));

        let s = magic_wand(&px, 2, 8, opts(10, true));
        assert_eq!(s.coverage(2, 8), 255);
        assert_eq!(s.coverage(12, 8), 0, "alpha differs by 255");
    }

    #[test]
    fn clicking_outside_the_image_selects_nothing() {
        let s = magic_wand(&scene(), -5, 100, opts(50, true));
        assert!(s.is_empty());
    }

    #[test]
    fn a_full_tolerance_wand_takes_everything() {
        let s = magic_wand(&scene(), 0, 0, opts(255, true));
        assert_eq!(s.bounds(), IRect::new(0, 0, 32, 32));
    }

    #[test]
    fn flood_fill_handles_a_spiral_without_overflowing() {
        // A long thin corridor is the case that breaks naive recursion.
        let mut px = PixelBuffer::filled(200, 200, Rgba8::BLACK);
        let mut y = 0;
        let mut x = 0;
        let mut dir = 0;
        for _ in 0..8000 {
            px.set(x, y, Rgba8::WHITE);
            let (dx, dy) = [(1, 0), (0, 1), (-1, 0), (0, -1)][dir];
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= 200 || ny >= 200 {
                dir = (dir + 1) % 4;
                continue;
            }
            x = nx;
            y = ny;
        }
        let s = magic_wand(&px, 0, 0, opts(10, true));
        assert!(!s.is_empty(), "the corridor should be selected without a stack overflow");
    }

    #[test]
    fn flood_fill_reaches_every_pixel_of_a_plain_field() {
        let px = PixelBuffer::filled(120, 90, Rgba8::opaque(7, 7, 7));
        let s = magic_wand(&px, 60, 45, opts(0, true));
        assert_eq!(s.bounds(), IRect::new(0, 0, 120, 90));
        for (x, y) in [(0, 0), (119, 89), (0, 89), (119, 0), (60, 45)] {
            assert_eq!(s.coverage(x, y), 255, "missed ({x}, {y})");
        }
    }

    #[test]
    fn grow_extends_into_matching_neighbours() {
        let mut px = PixelBuffer::filled(32, 8, Rgba8::opaque(0, 0, 0));
        px.fill_rect(IRect::new(0, 0, 20, 8), Rgba8::opaque(255, 0, 0));

        // Start with a sliver of the red band.
        let seed = Selection::from_rect(
            32,
            8,
            crate::selection::Rectf { x0: 2.0, y0: 2.0, x1: 4.0, y1: 4.0 },
            false,
        );
        let grown = grow(&px, &seed, opts(10, true));
        assert_eq!(grown.coverage(18, 4), 255, "grew to the end of the red band");
        assert_eq!(grown.coverage(24, 4), 0, "stopped at the black");
    }

    #[test]
    fn similar_finds_disconnected_matches() {
        let mut px = PixelBuffer::filled(32, 8, Rgba8::opaque(0, 0, 0));
        px.fill_rect(IRect::new(0, 0, 4, 8), Rgba8::opaque(255, 0, 0));
        px.fill_rect(IRect::new(28, 0, 32, 8), Rgba8::opaque(255, 0, 0));

        let seed = Selection::from_rect(
            32,
            8,
            crate::selection::Rectf { x0: 1.0, y0: 1.0, x1: 3.0, y1: 3.0 },
            false,
        );
        let s = similar(&px, &seed, opts(10, true));
        assert_eq!(s.coverage(30, 4), 255, "the far patch shares the colour");
        assert_eq!(s.coverage(16, 4), 0, "the black background does not");
    }

    #[test]
    fn similar_on_an_empty_selection_does_nothing() {
        let s = similar(&scene(), &Selection::empty(32, 32), opts(10, true));
        assert!(s.is_empty());
    }

    #[test]
    fn antialiasing_softens_the_boundary() {
        let px = {
            let mut p = PixelBuffer::filled(32, 32, Rgba8::BLACK);
            p.fill_rect(IRect::new(0, 0, 16, 32), Rgba8::WHITE);
            p
        };
        let hard = magic_wand(&px, 4, 16, WandOptions { tolerance: 10, contiguous: true, antialias: false });
        let soft = magic_wand(&px, 4, 16, WandOptions { tolerance: 10, contiguous: true, antialias: true });

        assert_eq!(hard.coverage(15, 16), 255);
        assert_eq!(hard.coverage(16, 16), 0);
        // The soft edge trades a hard step for a ramp.
        let edge = soft.coverage(15, 16);
        assert!(edge > 0 && edge < 255, "expected a partial edge, got {edge}");
    }
}
