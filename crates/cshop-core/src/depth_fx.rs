//! Effects that read how far away things are.
//!
//! The depth model already runs, and its answer is already available as a mask
//! and as the input to relighting. What follows from having it is a set of
//! effects that are impossible without it and ordinary with it: haze that
//! thickens with distance, a shallow depth of field applied to a photograph
//! that was not taken with one, and a shift of viewpoint.
//!
//! None of these needs a model of its own. They are filters that happen to
//! read a second picture — the depth — alongside the first.
//!
//! # What the depth is and is not
//!
//! One number per pixel, near at one and far at nothing, with no unit. It is a
//! guess made by a network from a single photograph, so it is smooth where the
//! world has a cliff and confident where it should not be. Every control here
//! is therefore a control over *how much*, and the honest answer to "how far
//! away is that, really?" is that nobody knows and it does not matter for any
//! of this.

use crate::color::{Rgba, Rgba8};
use crate::pixels::PixelBuffer;
use crate::relight::DepthMap;

/// Atmosphere: distant things fade toward the colour of the air between.
///
/// This is what makes a landscape read as deep. It is also nearly free — the
/// whole effect is one blend per pixel — and the reason it looks right is that
/// it is what the air actually does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fog {
    pub colour: Rgba8,
    /// How thick it gets at the far end, `0..=1`.
    pub density: f32,
    /// How far away it starts, as a depth. Nothing nearer than this is
    /// touched, which is what keeps a subject clear of it.
    pub start: f32,
    /// How sharply it comes on past that. Higher is more like a wall of
    /// weather and less like air.
    pub falloff: f32,
}

impl Default for Fog {
    fn default() -> Self {
        Self { colour: Rgba8::opaque(200, 210, 225), density: 0.6, start: 0.15, falloff: 1.6 }
    }
}

impl Fog {
    pub fn apply(&self, src: &PixelBuffer, depth: &DepthMap) -> PixelBuffer {
        let mut out = src.clone();
        let fog = self.colour.to_f32();
        let start = self.start.clamp(0.0, 0.999);
        for y in 0..src.height() as i32 {
            for x in 0..src.width() as i32 {
                // Depth is near-at-one, so distance is its complement.
                let far = 1.0 - sample(depth, src, x, y);
                let t = ((far - (1.0 - start)) / (start).max(1e-4)).clamp(0.0, 1.0);
                let t = 1.0 - (1.0 - t).powf(self.falloff.max(0.05));
                let k = (t * self.density.clamp(0.0, 1.0)).clamp(0.0, 1.0);
                if k <= 0.0 {
                    continue;
                }
                let c = src.get(x, y).to_f32();
                out.set(
                    x,
                    y,
                    Rgba {
                        r: c.r + (fog.r - c.r) * k,
                        g: c.g + (fog.g - c.g) * k,
                        b: c.b + (fog.b - c.b) * k,
                        a: c.a,
                    }
                    .to_u8(),
                );
            }
        }
        out
    }
}

/// A shallow depth of field, applied to a photograph that was not taken with
/// one.
///
/// # Why it is done in levels
///
/// Every pixel wants a different amount of blur, and a blur whose radius
/// changes per pixel cannot be separated into two passes — which is what makes
/// blurring cheap. So a few whole-picture blurs are made at fixed radii and
/// each pixel takes the two nearest and mixes between them. Six levels is
/// enough that the steps are invisible and few enough that the cost is six
/// blurs rather than one per radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Focus {
    /// The depth that is sharp.
    pub at: f32,
    /// How much of the depth around it stays sharp.
    pub range: f32,
    /// The largest blur, in pixels, at the far end of the depth.
    pub blur: f32,
    /// Blur things nearer than the focus as well. Off is the photograph a long
    /// lens takes; on is what a real one does.
    pub both_ways: bool,
}

impl Default for Focus {
    fn default() -> Self {
        Self { at: 0.7, range: 0.15, blur: 12.0, both_ways: true }
    }
}

/// How many blur levels are built. Enough that the steps do not show.
const LEVELS: usize = 6;

impl Focus {
    /// How out of focus a pixel at this depth is, `0..=1`.
    pub fn confusion(&self, depth: f32) -> f32 {
        let d = depth - self.at;
        let d = if self.both_ways { d.abs() } else { (-d).max(0.0) };
        ((d - self.range.max(0.0)) / (1.0 - self.range.max(0.0)).max(1e-4)).clamp(0.0, 1.0)
    }

    pub fn apply(&self, src: &PixelBuffer, depth: &DepthMap) -> PixelBuffer {
        let radius = self.blur.max(0.0);
        if radius < 0.5 {
            return src.clone();
        }
        // The levels, each blurred a little more than the last.
        let plane = crate::filters::plane::Plane::from_pixels(src);
        let levels: Vec<PixelBuffer> = (0..LEVELS)
            .map(|i| {
                let r = radius * (i as f32 / (LEVELS - 1) as f32);
                if r < 0.5 {
                    src.clone()
                } else {
                    crate::filters::blur::gaussian(&plane, r).to_pixels()
                }
            })
            .collect();

        let mut out = src.clone();
        for y in 0..src.height() as i32 {
            for x in 0..src.width() as i32 {
                let c = self.confusion(sample(depth, src, x, y));
                let position = c * (LEVELS - 1) as f32;
                let lower = position.floor() as usize;
                let upper = (lower + 1).min(LEVELS - 1);
                let t = position - lower as f32;
                let a = levels[lower].get(x, y).to_f32();
                let b = levels[upper].get(x, y).to_f32();
                out.set(
                    x,
                    y,
                    Rgba {
                        r: a.r + (b.r - a.r) * t,
                        g: a.g + (b.g - a.g) * t,
                        b: a.b + (b.b - a.b) * t,
                        a: a.a + (b.a - a.a) * t,
                    }
                    .to_u8(),
                );
            }
        }
        out
    }
}

/// A shift of viewpoint: near things move further than far ones, which is what
/// parallax is.
///
/// # The hole
///
/// Moving the near things reveals what was behind them, and a photograph does
/// not know what was behind them. Every method of dealing with that is a
/// guess; this one stretches the far side of each gap across it, which is the
/// guess that is least often wrong — the thing behind an object is usually
/// more of whatever is beside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parallax {
    /// How far the nearest things move, in pixels. Negative goes the other
    /// way.
    pub shift: f32,
    /// Move vertically instead.
    pub vertical: bool,
}

impl Default for Parallax {
    fn default() -> Self {
        Self { shift: 12.0, vertical: false }
    }
}

impl Parallax {
    pub fn apply(&self, src: &PixelBuffer, depth: &DepthMap) -> PixelBuffer {
        let (w, h) = (src.width() as i32, src.height() as i32);
        let mut out = PixelBuffer::new(src.width(), src.height());
        let reach = self.shift.abs().ceil() as i32 + 1;

        // Asked backwards: for each pixel of the result, which pixel of the
        // photograph ends up here?
        //
        // The forward question — where does each pixel go — is the natural one
        // and gives a worse answer. Rounding sends two neighbouring pixels to
        // the same place and skips the one between, so a solid object comes
        // out with a comb of one-pixel holes through it, which then fill from
        // whatever happens to be beside them. Asking backwards fills every
        // pixel exactly once by construction.
        //
        // Where two pixels both land here, the nearer one wins, which is what
        // occlusion is.
        for y in 0..h {
            for x in 0..w {
                let mut best: Option<(f32, f32, i32)> = None;
                for k in -reach..=reach {
                    let sx = if self.vertical { x } else { x - k };
                    let sy = if self.vertical { y - k } else { y };
                    if sx < 0 || sy < 0 || sx >= w || sy >= h {
                        continue;
                    }
                    let d = sample(depth, src, sx, sy);
                    let lands = d * self.shift;
                    let miss = (lands - k as f32).abs();
                    if miss > 0.75 {
                        continue;
                    }
                    // Nearest first; among equals, the one that lands closest.
                    let better = match best {
                        None => true,
                        Some((bd, bm, _)) => d > bd + 1e-4 || ((d - bd).abs() <= 1e-4 && miss < bm),
                    };
                    if better {
                        best = Some((d, miss, if self.vertical { sy } else { sx }));
                    }
                }
                let c = match best {
                    Some((_, _, at)) => {
                        if self.vertical {
                            src.get(x, at)
                        } else {
                            src.get(at, y)
                        }
                    }
                    // Nothing lands here: a piece of background that was
                    // hidden and is not in the photograph at all. The nearest
                    // pixel that did land is the best guess there is.
                    None => nearest_along(src, depth, self, x, y, reach),
                };
                out.set(x, y, c);
            }
        }
        out
    }
}

/// The colour of the nearest pixel that does land somewhere, searched outward
/// from `(x, y)` — used only where a shift has uncovered something the
/// photograph never saw.
fn nearest_along(
    src: &PixelBuffer,
    depth: &DepthMap,
    shift: &Parallax,
    x: i32,
    y: i32,
    reach: i32,
) -> Rgba8 {
    let (w, h) = (src.width() as i32, src.height() as i32);
    let mut best: Option<(f32, Rgba8)> = None;
    for k in 1..=reach * 2 {
        for sign in [-1i32, 1] {
            let (sx, sy) = if shift.vertical {
                (x, y + sign * k)
            } else {
                (x + sign * k, y)
            };
            if sx < 0 || sy < 0 || sx >= w || sy >= h {
                continue;
            }
            let d = sample(depth, src, sx, sy);
            // The far side, because a hole is background a near thing was
            // covering and the background is the far side of it.
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, src.get(sx, sy)));
            }
        }
        if best.is_some() {
            break;
        }
    }
    best.map(|(_, c)| c).unwrap_or_else(|| src.get(x, y))
}

/// The depth at a picture pixel, when the two are different sizes — which they
/// usually are, since the model runs at its own resolution.
#[inline]
fn sample(depth: &DepthMap, src: &PixelBuffer, x: i32, y: i32) -> f32 {
    if depth.width == src.width() && depth.height == src.height() {
        return depth.at(x, y);
    }
    let sx = x as f32 * depth.width as f32 / src.width().max(1) as f32;
    let sy = y as f32 * depth.height as f32 / src.height().max(1) as f32;
    depth.at(sx as i32, sy as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture and a depth that runs from far at the left to near at the
    /// right.
    fn scene(w: u32, h: u32) -> (PixelBuffer, DepthMap) {
        let mut px = PixelBuffer::new(w, h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                // A checker, so blur has something to remove.
                let on = (x / 4 + y / 4) % 2 == 0;
                px.set(x, y, if on { Rgba8::WHITE } else { Rgba8::opaque(20, 20, 20) });
            }
        }
        let mut data = vec![0.0f32; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                data[(y * w + x) as usize] = x as f32 / (w - 1) as f32;
            }
        }
        (px, DepthMap::from_values(w, h, data).unwrap())
    }

    fn spread(px: &PixelBuffer, x0: i32, x1: i32, y: i32) -> f32 {
        let v: Vec<f32> = (x0..x1).map(|x| px.get(x, y).r as f32).collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        (v.iter().map(|a| (a - mean).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn fog_thickens_with_distance_and_leaves_the_near_alone() {
        let (px, depth) = scene(64, 16);
        let out = Fog { density: 0.9, start: 0.4, ..Default::default() }.apply(&px, &depth);
        // Far is on the left, where the fog should be thick.
        assert!(spread(&out, 2, 10, 8) < spread(&px, 2, 10, 8) * 0.6, "the far end is fogged");
        assert!(
            (spread(&out, 54, 62, 8) - spread(&px, 54, 62, 8)).abs() < 1.0,
            "and the near end is not"
        );
    }

    #[test]
    fn fog_with_no_density_changes_nothing() {
        let (px, depth) = scene(32, 8);
        let out = Fog { density: 0.0, ..Default::default() }.apply(&px, &depth);
        assert_eq!(out.pixels(), px.pixels());
    }

    /// The whole point: what is at the focus stays sharp and what is not does
    /// not.
    #[test]
    fn focus_keeps_its_depth_sharp_and_blurs_the_rest() {
        let (px, depth) = scene(96, 24);
        // Focus on the near end, at depth 1.
        let out = Focus { at: 1.0, range: 0.1, blur: 10.0, both_ways: true }
            .apply(&px, &depth);
        let near = spread(&out, 86, 94, 12);
        let far = spread(&out, 2, 10, 12);
        assert!(near > far * 2.0, "near should be sharp and far soft: {near:.1} against {far:.1}");
        assert!(
            (near - spread(&px, 86, 94, 12)).abs() < 8.0,
            "and the sharp part should be about as sharp as it was"
        );
    }

    #[test]
    fn the_circle_of_confusion_is_nothing_at_the_focus_and_grows_away_from_it() {
        let f = Focus { at: 0.5, range: 0.1, blur: 10.0, both_ways: true };
        assert_eq!(f.confusion(0.5), 0.0);
        assert_eq!(f.confusion(0.55), 0.0, "inside the range it is still sharp");
        assert!(f.confusion(0.9) > 0.0);
        assert!(f.confusion(0.1) > 0.0, "and both ways, since a real lens does");
    }

    #[test]
    fn focusing_one_way_leaves_the_nearer_things_sharp() {
        let f = Focus { at: 0.5, range: 0.05, blur: 8.0, both_ways: false };
        assert_eq!(f.confusion(0.9), 0.0, "nearer than the focus stays sharp");
        assert!(f.confusion(0.1) > 0.0, "and further does not");
    }

    #[test]
    fn no_blur_is_no_change() {
        let (px, depth) = scene(32, 8);
        let out = Focus { blur: 0.0, ..Default::default() }.apply(&px, &depth);
        assert_eq!(out.pixels(), px.pixels());
    }

    /// Near things move further than far ones. That is the whole definition.
    #[test]
    fn parallax_moves_the_near_more_than_the_far() {
        let (mut px, mut depth) = scene(64, 16);
        // A marker near and a marker far, so the movement can be measured.
        for y in 0..16 {
            for x in 0..64 {
                px.set(x, y, Rgba8::opaque(40, 40, 40));
            }
        }
        for y in 6..10 {
            px.set(8, y, Rgba8::WHITE); // far
            // Far enough from the right edge that a shift of eight keeps it
            // in the picture.
            px.set(48, y, Rgba8::WHITE); // near
        }
        for y in 0..16u32 {
            for x in 0..64u32 {
                depth.data[(y * 64 + x) as usize] = if x < 32 { 0.0 } else { 1.0 };
            }
        }
        let out = Parallax { shift: 8.0, vertical: false }.apply(&px, &depth);
        let bright = |from: i32, to: i32| (from..to).find(|&x| out.get(x, 8).r > 200);
        assert_eq!(bright(0, 30), Some(8), "the far marker did not move");
        assert_eq!(bright(40, 64), Some(48 + 8), "and the near one moved by the shift");
    }

    /// A solid object has to come out solid. Asked forwards — where does each
    /// pixel go — rounding sends two neighbours to the same place and skips
    /// the one between, so an object comes out with a comb of one-pixel holes
    /// through it that fill from whatever is beside them. On a photograph that
    /// shows as streaks along every depth cliff, which is how this was found.
    #[test]
    fn a_solid_object_comes_out_solid() {
        let (w, h) = (80u32, 24u32);
        let mut px = PixelBuffer::filled(w, h, Rgba8::WHITE);
        let mut data = vec![0.0f32; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                // A near block in the middle, on a far background — and its
                // depth varies slightly across it, which is what produces the
                // holes when the mapping is done forwards.
                let near = (24..56).contains(&x);
                if near {
                    px.set(x as i32, y as i32, Rgba8::opaque(20, 20, 20));
                }
                data[(y * w + x) as usize] =
                    if near { 0.80 + (x % 8) as f32 * 0.004 } else { 0.02 };
            }
        }
        let depth = DepthMap::from_values(w, h, data).unwrap();
        let out = Parallax { shift: 17.0, vertical: false }.apply(&px, &depth);

        // Somewhere in the middle of where the block landed, every pixel
        // should be the block's colour — no white showing through.
        let holes = (34..62)
            .filter(|&x| out.get(x, 12).r > 128)
            .count();
        assert_eq!(holes, 0, "{holes} pixels of background showing through a solid object");
    }

    #[test]
    fn parallax_fills_what_it_uncovers() {
        let (px, depth) = scene(48, 12);
        let out = Parallax { shift: 6.0, vertical: false }.apply(&px, &depth);
        // Nothing transparent: every hole was filled with something.
        assert!(
            out.pixels().iter().all(|c| c.a == 255),
            "a shift should leave no holes"
        );
    }

    /// The depth model runs at its own resolution, so the two pictures are
    /// usually different sizes.
    #[test]
    fn a_depth_of_a_different_size_still_lines_up() {
        let (px, _) = scene(64, 32);
        let small = DepthMap::from_values(
            16,
            8,
            (0..16 * 8).map(|i| (i % 16) as f32 / 15.0).collect(),
        )
        .unwrap();
        let out = Fog { density: 0.9, start: 0.5, ..Default::default() }.apply(&px, &small);
        assert!(spread(&out, 2, 10, 16) < spread(&px, 2, 10, 16), "still fogged at the far end");
    }
}
