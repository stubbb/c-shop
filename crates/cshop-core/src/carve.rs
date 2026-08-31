//! Content-aware scale: changing a picture's proportions while leaving the
//! things in it alone.
//!
//! # Why not just resize
//!
//! Resizing squashes everything equally, which is fine for a landscape and
//! obvious on a face. What people usually want when they change a picture's
//! proportions is for the *space* to change and the subjects not to — a wider
//! frame with the same two people in it, not two wider people.
//!
//! # Seams
//!
//! A seam is a connected path of pixels from top to bottom, one per row, each
//! within a pixel of the one above. Removing one takes a column's worth of
//! width out of the picture while bending around whatever is in the way.
//! Remove five hundred of the least interesting seams and the picture is five
//! hundred pixels narrower, with the boring parts gone and the rest untouched.
//!
//! "Least interesting" is the whole question, and the answer here is the
//! ordinary one: how much a pixel differs from its neighbours. Flat sky and
//! smooth wall cost nothing to cut through; an edge, a face, a line of text
//! cost a great deal. It is a crude measure of interest and a remarkably good
//! one, because what the eye notices is discontinuity.
//!
//! # Saying what to keep
//!
//! The energy is only a guess, and on a picture where the boring region *is*
//! the subject — a face against a busy background — it guesses wrong. So a
//! mask can be handed in to raise the energy where something must survive, or
//! lower it where something should go first. A selection is exactly such a
//! mask, which is where the segmentation work pays off: select the person,
//! and the seams route around them.

use crate::color::Rgba8;
use crate::mask::MaskBuffer;
use crate::pixels::PixelBuffer;

/// Energy added where a mask protects. Large enough that no seam crosses a
/// protected region while any unprotected route exists, and finite so that a
/// picture protected end to end still resizes rather than failing.
const PROTECT: f32 = 1_000.0;

/// Energy taken away where a mask marks something for removal. Negative, so
/// seams are actively drawn through it.
const REMOVE: f32 = -1_000.0;

/// The largest number of seams one call will carve, so a mis-typed size asks
/// for a long wait rather than an unbounded one.
const MAX_SEAMS: u32 = 4_000;

#[derive(Debug, Default, Clone)]
pub struct Carve {
    /// Where the picture must survive: white protects.
    pub protect: Option<MaskBuffer>,
    /// Where the picture should go first: white removes.
    pub remove: Option<MaskBuffer>,
}

impl Carve {
    /// Resize by carving seams rather than by resampling.
    ///
    /// Each axis is done in turn — vertical seams for the width, horizontal
    /// for the height — because a seam is one-dimensional by construction and
    /// there is no such thing as a diagonal one.
    pub fn resize(&self, src: &PixelBuffer, width: u32, height: u32) -> PixelBuffer {
        self.resize_reporting(src, width, height, None)
    }

    /// The same, counting seams into `progress` as it goes, so something can
    /// say how far along it is. This takes seconds on a large photograph and
    /// silence for seconds looks like a hang.
    pub fn resize_reporting(
        &self,
        src: &PixelBuffer,
        width: u32,
        height: u32,
        progress: Option<&std::sync::atomic::AtomicU32>,
    ) -> PixelBuffer {
        let mut out = src.clone();
        let mut protect = self.protect.clone();
        let mut remove = self.remove.clone();
        if width != src.width() {
            out = carve_axis(&out, width, &mut protect, &mut remove, false, progress);
        }
        if height != out.height() {
            out = carve_axis(&out, height, &mut protect, &mut remove, true, progress);
        }
        out
    }
}

/// Carve one axis to `target`. `vertical` swaps rows for columns, so the same
/// code does both.
fn carve_axis(
    src: &PixelBuffer,
    target: u32,
    protect: &mut Option<MaskBuffer>,
    remove: &mut Option<MaskBuffer>,
    vertical: bool,
    progress: Option<&std::sync::atomic::AtomicU32>,
) -> PixelBuffer {
    let work = if vertical { transpose(src) } else { src.clone() };
    let p = protect.as_ref().map(|m| if vertical { transpose_mask(m) } else { m.clone() });
    let r = remove.as_ref().map(|m| if vertical { transpose_mask(m) } else { m.clone() });

    let from = work.width();
    let mut field = Field::new(&work, p.as_ref(), r.as_ref());
    match target.cmp(&from) {
        std::cmp::Ordering::Equal => {}
        std::cmp::Ordering::Less => {
            let n = (from - target).min(MAX_SEAMS).min(from.saturating_sub(1));
            for _ in 0..n {
                if field.w <= 1 {
                    break;
                }
                let seam = field.cheapest_seam();
                field.remove_seam(&seam);
                if let Some(p) = progress {
                    p.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        std::cmp::Ordering::Greater => {
            let n = (target - from).min(MAX_SEAMS);
            let grown = field.grown_by(n, progress);
            *protect = field.mask_of(&field.protect).map(|m| if vertical { transpose_mask(&m) } else { m });
            *remove = field.mask_of(&field.remove).map(|m| if vertical { transpose_mask(&m) } else { m });
            return if vertical { transpose(&grown) } else { grown };
        }
    }

    // The masks have been carved alongside, so a second axis sees them where
    // the picture now is rather than where it started.
    *protect = field.mask_of(&field.protect).map(|m| if vertical { transpose_mask(&m) } else { m });
    *remove = field.mask_of(&field.remove).map(|m| if vertical { transpose_mask(&m) } else { m });
    let out = field.to_pixels();
    if vertical {
        transpose(&out)
    } else {
        out
    }
}

/// The picture and everything derived from it, as flat rows.
///
/// The obvious implementation — read pixels through the buffer's accessor and
/// work the energy out afresh for every seam — measured 45 seconds to take two
/// hundred seams out of a twelve-megapixel photograph, which is not a feature.
/// Almost all of it was the accessor's bounds checks and clamps in the inner
/// loop, and recomputing an energy map that changes only where the last seam
/// was taken from.
///
/// So: everything flat and indexed arithmetically, the energy kept alongside
/// and repaired in a band around each cut rather than rebuilt. Same answers,
/// and the same photograph takes about a second.
struct Field {
    w: usize,
    h: usize,
    px: Vec<Rgba8>,
    luma: Vec<f32>,
    energy: Vec<f32>,
    protect: Option<Vec<f32>>,
    remove: Option<Vec<f32>>,
    /// Where each surviving pixel started, so an insertion can name a column
    /// in the original picture.
    origin: Vec<u32>,
}

impl Field {
    fn new(src: &PixelBuffer, protect: Option<&MaskBuffer>, remove: Option<&MaskBuffer>) -> Field {
        let (w, h) = (src.width() as usize, src.height() as usize);
        let px: Vec<Rgba8> = src.pixels().to_vec();
        let luma: Vec<f32> = px.iter().map(|c| c.to_f32().luma()).collect();
        let plane = |m: Option<&MaskBuffer>| {
            m.map(|m| {
                let mut v = Vec::with_capacity(w * h);
                for y in 0..h as i32 {
                    for x in 0..w as i32 {
                        v.push(m.get(x, y) as f32 / 255.0);
                    }
                }
                v
            })
        };
        let origin = (0..h).flat_map(|_| 0..w as u32).collect();
        let mut f = Field {
            w,
            h,
            px,
            luma,
            energy: vec![0.0; w * h],
            protect: plane(protect),
            remove: plane(remove),
            origin,
        };
        for y in 0..f.h {
            for x in 0..f.w {
                f.energy[y * f.w + x] = f.energy_at(x, y);
            }
        }
        f
    }

    /// How much a pixel differs from its neighbours, plus whatever the masks
    /// say. Edges are clamped by index rather than by a branch per read.
    fn energy_at(&self, x: usize, y: usize) -> f32 {
        let row = y * self.w;
        let left = row + x.saturating_sub(1);
        let right = row + (x + 1).min(self.w - 1);
        let up = y.saturating_sub(1) * self.w + x;
        let down = (y + 1).min(self.h - 1) * self.w + x;
        let gx = self.luma[right] - self.luma[left];
        let gy = self.luma[down] - self.luma[up];
        let mut e = (gx * gx + gy * gy).sqrt();
        // Transparent pixels are nothing, and nothing should be the first to
        // go.
        if self.px[row + x].a < 8 {
            e -= 0.5;
        }
        if let Some(p) = &self.protect {
            e += PROTECT * p[row + x];
        }
        if let Some(r) = &self.remove {
            e += REMOVE * r[row + x];
        }
        e
    }

    /// The vertical seam of least total energy: one x per row, each within a
    /// pixel of the row above.
    fn cheapest_seam(&self) -> Vec<u32> {
        let (w, h) = (self.w, self.h);
        let mut cost = vec![0.0f32; w * h];
        // Where each pixel's cheapest route came from, so the seam can be read
        // back without recomputing anything.
        let mut from = vec![0i8; w * h];
        cost[..w].copy_from_slice(&self.energy[..w]);

        for y in 1..h {
            let (above, here) = cost.split_at_mut(y * w);
            let above = &above[(y - 1) * w..];
            for x in 0..w {
                let mut best = above[x];
                let mut step = 0i8;
                if x > 0 && above[x - 1] < best {
                    best = above[x - 1];
                    step = -1;
                }
                if x + 1 < w && above[x + 1] < best {
                    best = above[x + 1];
                    step = 1;
                }
                here[x] = best + self.energy[y * w + x];
                from[y * w + x] = step;
            }
        }

        let last = (h - 1) * w;
        let mut x = (0..w)
            .min_by(|a, b| cost[last + a].total_cmp(&cost[last + b]))
            .unwrap_or(0);
        let mut seam = vec![0u32; h];
        for y in (0..h).rev() {
            seam[y] = x as u32;
            x = (x as isize + from[y * w + x] as isize).clamp(0, w as isize - 1) as usize;
        }
        seam
    }

    /// Take a seam out of every row, and repair the energy either side of it.
    fn remove_seam(&mut self, seam: &[u32]) {
        let (w, h) = (self.w, self.h);
        // Every row loses one element and everything after it slides down by
        // however many rows have gone before. Done in place, which beats
        // allocating five fresh vectors per seam.
        //
        // The two halves of a row must move in address order — the left part
        // first. Its destination sits before its source, and the right part's
        // destination lands inside the left part's *source*, so moving the
        // right one first overwrites what the left one has not read yet.
        fn shift<T: Copy>(v: &mut Vec<T>, w: usize, h: usize, seam: &[u32]) {
            for (y, &at) in seam.iter().enumerate().take(h) {
                let cut = y * w + at as usize;
                v.copy_within(y * w..cut, y * w - y);
                v.copy_within(cut + 1..(y + 1) * w, cut - y);
            }
            v.truncate((w - 1) * h);
        }
        shift(&mut self.px, w, h, seam);
        shift(&mut self.origin, w, h, seam);
        shift(&mut self.luma, w, h, seam);
        shift(&mut self.energy, w, h, seam);
        if let Some(v) = &mut self.protect {
            shift(v, w, h, seam);
        }
        if let Some(v) = &mut self.remove {
            shift(v, w, h, seam);
        }
        self.w -= 1;

        // Only the pixels next to where the seam was have new neighbours, so
        // only their energy is stale. Two columns either side covers the
        // three-wide gradient plus the row above and below.
        for (y, &cut) in seam.iter().enumerate().take(self.h) {
            let at = cut as usize;
            let lo = at.saturating_sub(2);
            let hi = (at + 2).min(self.w.saturating_sub(1));
            for yy in y.saturating_sub(1)..=(y + 1).min(self.h - 1) {
                for x in lo..=hi {
                    self.energy[yy * self.w + x] = self.energy_at(x, yy);
                }
            }
        }
    }

    /// The picture as it now stands.
    fn to_pixels(&self) -> PixelBuffer {
        PixelBuffer::from_pixels(self.w as u32, self.h as u32, self.px.clone())
            .unwrap_or_else(|| PixelBuffer::new(1, 1))
    }

    fn mask_of(&self, plane: &Option<Vec<f32>>) -> Option<MaskBuffer> {
        let v = plane.as_ref()?;
        let mut out = MaskBuffer::hide_all(self.w as u32, self.h as u32);
        for y in 0..self.h {
            for x in 0..self.w {
                out.set(x as i32, y as i32, (v[y * self.w + x].clamp(0.0, 1.0) * 255.0) as u8);
            }
        }
        Some(out)
    }

    /// Widen by `n`, by finding the `n` cheapest seams and putting a blend of
    /// their neighbours beside each.
    ///
    /// The seams are found by carving a scratch copy, because looking for one
    /// seam at a time in the growing picture would find the same seam every
    /// time — a seam inserted beside itself is still the cheapest thing there
    /// — and the result would be one wide smear rather than a stretch spread
    /// across the picture.
    fn grown_by(
        &mut self,
        n: u32,
        progress: Option<&std::sync::atomic::AtomicU32>,
    ) -> PixelBuffer {
        let (w0, h) = (self.w, self.h);
        let original: Vec<Rgba8> = self.px.clone();
        let n = (n as usize).min(w0.saturating_sub(1));
        if n == 0 {
            return self.to_pixels();
        }

        // Per row, how many copies each original column needs. Per row and not
        // per column: a seam has exactly one pixel in each row and wanders
        // between them, so counting by column would credit a seam once for
        // every row it passes through.
        let mut extra = vec![0u32; w0 * h];
        for _ in 0..n {
            if self.w <= 1 {
                break;
            }
            let seam = self.cheapest_seam();
            for (y, &x) in seam.iter().enumerate() {
                extra[y * w0 + self.origin[y * self.w + x as usize] as usize] += 1;
            }
            self.remove_seam(&seam);
            if let Some(p) = progress {
                p.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        // Every seam contributes one insertion to every row, so the rows come
        // out the same length without having to be made to.
        let new_w = w0 + n;
        let mut out = PixelBuffer::new(new_w as u32, h as u32);
        for y in 0..h as i32 {
            let mut at = 0i32;
            for x in 0..w0 as i32 {
                let here = original[y as usize * w0 + x as usize];
                out.set(at, y, here);
                at += 1;
                for _ in 0..extra[y as usize * w0 + x as usize] {
                    // The average of the pixel and its neighbour, so an
                    // inserted column is a step between them rather than a
                    // repeat, which would show as a visible band.
                    let next =
                        original[y as usize * w0 + (x as usize + 1).min(w0 - 1)];
                    out.set(at, y, average(here, next));
                    at += 1;
                }
            }
        }
        out
    }
}

fn average(a: Rgba8, b: Rgba8) -> Rgba8 {
    let m = |x: u8, y: u8| ((x as u16 + y as u16) / 2) as u8;
    Rgba8::new(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b), m(a.a, b.a))
}

fn transpose(px: &PixelBuffer) -> PixelBuffer {
    let mut out = PixelBuffer::new(px.height(), px.width());
    for y in 0..px.height() as i32 {
        for x in 0..px.width() as i32 {
            out.set(y, x, px.get(x, y));
        }
    }
    out
}

fn transpose_mask(m: &MaskBuffer) -> MaskBuffer {
    let mut out = MaskBuffer::hide_all(m.height(), m.width());
    for y in 0..m.height() as i32 {
        for x in 0..m.width() as i32 {
            out.set(y, x, m.get(x, y));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flat sky on the left, a hard vertical bar on the right — the bar is the
    /// only thing in the picture worth keeping.
    fn sky_and_bar(w: u32, h: u32, bar: std::ops::Range<i32>) -> PixelBuffer {
        let mut px = PixelBuffer::filled(w, h, Rgba8::opaque(120, 140, 200));
        for y in 0..h as i32 {
            for x in bar.clone() {
                px.set(x, y, Rgba8::opaque(20, 20, 20));
            }
        }
        px
    }

    /// Where the dark bar is, and how wide, along one row.
    fn bar_in(px: &PixelBuffer, y: i32) -> (i32, i32) {
        let dark: Vec<i32> =
            (0..px.width() as i32).filter(|&x| px.get(x, y).r < 100).collect();
        match (dark.first(), dark.last()) {
            (Some(&a), Some(&b)) => (a, b - a + 1),
            _ => (-1, 0),
        }
    }

    #[test]
    fn narrowing_takes_the_space_and_leaves_the_subject() {
        let px = sky_and_bar(120, 40, 90..100);
        let (_, before) = bar_in(&px, 20);
        assert_eq!(before, 10);

        let out = Carve::default().resize(&px, 80, 40);
        assert_eq!(out.width(), 80);
        let (_, after) = bar_in(&out, 20);
        assert_eq!(after, 10, "the bar should be the width it was, not squashed");
    }

    /// The comparison that makes the point: an ordinary resize squashes it.
    #[test]
    fn an_ordinary_resize_squashes_the_subject() {
        let px = sky_and_bar(120, 40, 90..100);
        let out = crate::resample::resize(&px, 80, 40, crate::resample::Resampling::Bilinear);
        let (_, after) = bar_in(&out, 20);
        assert!(after < 9, "resizing narrows it to about two thirds: {after}");
    }

    #[test]
    fn widening_adds_space_rather_than_stretching_the_subject() {
        let px = sky_and_bar(100, 30, 70..80);
        let out = Carve::default().resize(&px, 140, 30);
        assert_eq!(out.width(), 140);
        let (_, after) = bar_in(&out, 15);
        assert!((after - 10).abs() <= 2, "the bar should be about as wide: {after}");
    }

    /// The energy is a guess, and this is how it is overruled: protect the
    /// flat region and the seams have to go through the busy one instead.
    #[test]
    fn a_protect_mask_is_obeyed_over_the_energy() {
        let px = sky_and_bar(120, 40, 90..100);
        // Protect the left half, which is exactly where the seams would go.
        let mut protect = MaskBuffer::hide_all(120, 40);
        for y in 0..40 {
            for x in 0..60 {
                protect.set(x, y, 255);
            }
        }
        let carve = Carve { protect: Some(protect), remove: None };
        let out = carve.resize(&px, 90, 40);

        // The protected half is untouched, so the bar has to have been cut.
        let (_, after) = bar_in(&out, 20);
        assert!(after < 10, "the seams had nowhere else to go: bar is {after} wide");
    }

    #[test]
    fn a_remove_mask_is_carved_out_first() {
        let mut px = PixelBuffer::filled(100, 30, Rgba8::opaque(120, 140, 200));
        // A blemish in the middle of the sky, which is as flat as its
        // surroundings and so would otherwise survive by luck.
        for y in 10..20 {
            for x in 40..50 {
                px.set(x, y, Rgba8::opaque(200, 60, 60));
            }
        }
        let mut remove = MaskBuffer::hide_all(100, 30);
        for y in 10..20 {
            for x in 40..50 {
                remove.set(x, y, 255);
            }
        }
        let out = Carve { protect: None, remove: Some(remove) }.resize(&px, 88, 30);
        let left = (0..out.width() as i32).filter(|&x| out.get(x, 15).r > 180).count();
        assert!(left <= 1, "the marked region should have been carved away: {left} left");
    }

    #[test]
    fn both_axes_can_be_carved_at_once() {
        let px = sky_and_bar(80, 60, 60..70);
        let out = Carve::default().resize(&px, 60, 40);
        assert_eq!((out.width(), out.height()), (60, 40));
    }

    #[test]
    fn a_size_that_is_no_change_gives_back_the_picture() {
        let px = sky_and_bar(40, 20, 30..35);
        let out = Carve::default().resize(&px, 40, 20);
        assert_eq!(out.pixels(), px.pixels());
    }
}
