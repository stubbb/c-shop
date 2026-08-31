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
    use rayon::prelude::*;

    let (w, h) = (source.width(), source.height());
    let mut mask = MaskBuffer::hide_all(w, h);
    if !source.bounds().contains(seed_x, seed_y) {
        return Selection::from_mask(mask);
    }
    let target = source.get(seed_x, seed_y);

    let covered = if options.contiguous {
        flood_fill(source, &mut mask, seed_x, seed_y, target, options.tolerance)
    } else {
        // Every pixel is judged on its own, so this is one parallel pass.
        let width = w as usize;
        mask.as_bytes_mut()
            .par_chunks_mut(width)
            .enumerate()
            .map(|(y, out)| {
                let row = source.row(y as u32);
                let mut covered = IRect::EMPTY;
                for (x, slot) in out.iter_mut().enumerate() {
                    if difference(row[x], target) <= options.tolerance {
                        *slot = 255;
                        covered =
                            covered.union(&IRect::new(x as i32, y as i32, x as i32 + 1, y as i32 + 1));
                    }
                }
                covered
            })
            .reduce(|| IRect::EMPTY, |a, b| a.union(&b))
    };

    finish(mask, Some(covered), options.antialias)
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

    finish(mask, None, options.antialias)
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

    finish(mask, None, options.antialias)
}

/// Scanline flood fill.
///
/// Filling whole runs at a time rather than pushing every pixel keeps the
/// stack shallow; a naive four-way recursion overflows on large flat areas.
/// A horizontal stretch of matching pixels, `start..end`.
#[derive(Clone, Copy)]
struct Run {
    start: u32,
    end: u32,
}

/// Reduce one row to the stretches of it that match.
fn runs_of_row(row: &[Rgba8], target: Rgba8, tolerance: u8) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut x = 0usize;
    while x < row.len() {
        if difference(row[x], target) > tolerance {
            x += 1;
            continue;
        }
        let start = x;
        while x < row.len() && difference(row[x], target) <= tolerance {
            x += 1;
        }
        runs.push(Run { start: start as u32, end: x as u32 });
    }
    runs
}

/// Every run in `row` that touches `span` horizontally.
///
/// The runs are sorted and disjoint, so the first candidate can be found by
/// bisection and the rest follow immediately after it.
fn overlapping(runs: &[Run], span: Run) -> std::ops::Range<usize> {
    let first = runs.partition_point(|r| r.end <= span.start);
    let mut last = first;
    while last < runs.len() && runs[last].start < span.end {
        last += 1;
    }
    first..last
}

/// The connected region of matching pixels containing the seed.
///
/// Whether a pixel matches does not depend on the fill's progress — it is a
/// comparison against one colour — so the picture can be reduced to runs of
/// matching pixels a row at a time, in parallel, and the traversal then walks
/// runs rather than pixels. A flat row becomes one run, so the sequential part
/// shrinks from a hundred million steps to one per row.
///
/// Rows are reduced only when the fill reaches them. Computing them all up
/// front would be simpler and would make filling a small region on a large
/// canvas cost the whole canvas, which is the common case and was previously
/// free.
fn flood_fill(
    source: &PixelBuffer,
    mask: &mut MaskBuffer,
    seed_x: i32,
    seed_y: i32,
    target: Rgba8,
    tolerance: u8,
) -> IRect {
    use rayon::prelude::*;

    let (w, h) = (source.width() as i32, source.height() as i32);
    if seed_x < 0 || seed_y < 0 || seed_x >= w || seed_y >= h {
        return IRect::EMPTY;
    }

    let mut runs: Vec<Option<Vec<Run>>> = vec![None; h as usize];
    // Which runs the fill has claimed, one flag per run.
    let mut taken: Vec<Vec<bool>> = vec![Vec::new(); h as usize];

    // Reduce any of `rows` not already done, all at once.
    let reduce = |rows: &[i32], runs: &mut Vec<Option<Vec<Run>>>, taken: &mut Vec<Vec<bool>>| {
        let mut wanted: Vec<i32> = rows
            .iter()
            .copied()
            .filter(|y| *y >= 0 && *y < h && runs[*y as usize].is_none())
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        let done: Vec<(i32, Vec<Run>)> = wanted
            .par_iter()
            .map(|&y| (y, runs_of_row(source.row(y as u32), target, tolerance)))
            .collect();
        for (y, r) in done {
            taken[y as usize] = vec![false; r.len()];
            runs[y as usize] = Some(r);
        }
    };

    reduce(&[seed_y], &mut runs, &mut taken);
    let seed_run = {
        let row = runs[seed_y as usize].as_ref().expect("just reduced");
        match row.iter().position(|r| (r.start..r.end).contains(&(seed_x as u32))) {
            Some(i) => i,
            // The seed itself does not match, so nothing is selected.
            None => return IRect::EMPTY,
        }
    };
    taken[seed_y as usize][seed_run] = true;
    let mut frontier: Vec<(i32, usize)> = vec![(seed_y, seed_run)];

    while !frontier.is_empty() {
        // Everything the frontier could spread into, reduced in one parallel
        // step rather than one row at a time.
        let neighbours: Vec<i32> =
            frontier.iter().flat_map(|&(y, _)| [y - 1, y + 1]).collect();
        reduce(&neighbours, &mut runs, &mut taken);

        let mut next: Vec<(i32, usize)> = Vec::new();
        for &(y, i) in &frontier {
            let span = runs[y as usize].as_ref().expect("frontier rows are reduced")[i];
            for row in [y - 1, y + 1] {
                if row < 0 || row >= h {
                    continue;
                }
                let candidates = {
                    let there = runs[row as usize].as_ref().expect("just reduced");
                    overlapping(there, span)
                };
                for j in candidates {
                    if !taken[row as usize][j] {
                        taken[row as usize][j] = true;
                        next.push((row, j));
                    }
                }
            }
        }
        frontier = next;
    }

    // Writing is per row and independent, so it goes wide too. The extent is
    // accumulated here rather than rescanned afterwards: the fill knows
    // exactly which runs it claimed, and looking for them again in a hundred
    // megabytes of mostly-zero mask would be asking a question already answered.
    let width = mask.width() as usize;
    mask.as_bytes_mut()
        .par_chunks_mut(width)
        .enumerate()
        .map(|(y, out)| {
            let (Some(row), flags) = (&runs[y], &taken[y]) else { return IRect::EMPTY };
            let mut covered = IRect::EMPTY;
            for (r, &claimed) in row.iter().zip(flags) {
                if claimed {
                    out[r.start as usize..r.end as usize].fill(255);
                    covered = covered.union(&IRect::new(
                        r.start as i32,
                        y as i32,
                        r.end as i32,
                        y as i32 + 1,
                    ));
                }
            }
            covered
        })
        .reduce(|| IRect::EMPTY, |a, b| a.union(&b))
}

/// Turn a binary mask into a selection, softening the boundary if asked.
/// Wrap a finished coverage mask as a selection.
///
/// `covered` is the extent when the caller already knows it — a flood fill
/// does, having tracked the runs it claimed — so that the mask is not scanned
/// again to rediscover it. `None` asks for the scan.
fn finish(mask: MaskBuffer, covered: Option<IRect>, antialias: bool) -> Selection {
    let mut selection = match covered {
        Some(bounds) => Selection::from_mask_bounded(mask, bounds),
        None => Selection::from_mask(mask),
    };
    if antialias && !selection.is_empty() {
        // Just enough to take the staircase off a diagonal edge without
        // visibly moving it.
        selection.feather(1.0);
    }
    selection
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
