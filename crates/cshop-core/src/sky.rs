//! Replacing a sky.
//!
//! # What the pieces already do
//!
//! The labeller finds the sky. Compositing a new one into that mask is
//! ordinary. What makes a replaced sky look replaced is everything around
//! those two steps, and it is all here:
//!
//! * **The join.** A label map's boundary is approximate, and a hard edge
//!   along an approximate boundary is the giveaway. The mask is softened, and
//!   softened *inward* — into the sky rather than into the trees — because a
//!   little sky bleeding onto a branch reads as sky seen through it, and a
//!   little branch bleeding into the sky reads as a mistake.
//!
//! * **The horizon.** A replacement sky has its own horizon, and putting it
//!   anywhere but where the picture's horizon is makes a picture of two
//!   places. The scene's horizon is the bottom of its sky; the replacement is
//!   placed so its own lands there.
//!
//! * **The light.** A photograph taken under a grey sky, given a golden one,
//!   is a photograph of a grey day with a golden sky pasted on. The foreground
//!   has to take some of the new sky's colour, or nothing else matters.

use crate::color::{Rgba, Rgba8};
use crate::mask::MaskBuffer;
use crate::pixels::PixelBuffer;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyReplace {
    /// How far the mask is grown before it is softened, in pixels.
    ///
    /// A label map's boundary is approximate and, on a sky, it is
    /// approximately *inside* — the model is confident about the middle of a
    /// region and cautious at its edge. What is left behind is a pale fringe
    /// of the old sky round every silhouette, which is the most conspicuous
    /// way a replaced sky announces itself. Growing the mask a little before
    /// softening it trades that fringe for a pixel of new sky on the outermost
    /// branches, which is what sky seen through branches looks like anyway.
    pub grow: f32,
    /// How far the join is softened, in pixels.
    pub feather: f32,
    /// How much of the new sky's colour the foreground takes, `0..=1`. At zero
    /// the sky is pasted on; at one the scene is lit entirely by it, which is
    /// too much for anything but an overcast original.
    pub relight: f32,
    /// Move the replacement up or down, as a fraction of the picture's height.
    /// Positive lowers its horizon.
    pub offset: f32,
    /// Scale the replacement about its horizon.
    pub scale: f32,
}

impl Default for SkyReplace {
    fn default() -> Self {
        Self { grow: 2.0, feather: 2.0, relight: 0.35, offset: 0.0, scale: 1.0 }
    }
}

/// A sky made rather than photographed, for when there is no second picture.
///
/// Two colours and a gradient. It is not a photograph of a sky and does not
/// pretend to be; what it is for is a flat white overcast that needs *some*
/// tone in it, which is the commonest reason to replace one at all.
pub fn gradient(width: u32, height: u32, zenith: Rgba8, horizon: Rgba8) -> PixelBuffer {
    let mut out = PixelBuffer::new(width.max(1), height.max(1));
    let (a, b) = (zenith.to_f32(), horizon.to_f32());
    for y in 0..out.height() as i32 {
        // Squared, because the sky's colour changes faster near the horizon
        // than overhead — which is what the air between actually does.
        let t = (y as f32 / (out.height().max(2) - 1) as f32).clamp(0.0, 1.0);
        let t = t * t;
        for x in 0..out.width() as i32 {
            out.set(
                x,
                y,
                Rgba {
                    r: a.r + (b.r - a.r) * t,
                    g: a.g + (b.g - a.g) * t,
                    b: a.b + (b.b - a.b) * t,
                    a: 1.0,
                }
                .to_u8(),
            );
        }
    }
    out
}

/// Where the sky stops, as a fraction of the picture's height.
///
/// The lowest row that is still mostly sky. Not the lowest row containing
/// *any* sky, which on a picture with a gap between two buildings is the
/// bottom of the frame.
pub fn horizon(mask: &MaskBuffer) -> f32 {
    let (w, h) = (mask.width(), mask.height());
    if w == 0 || h == 0 {
        return 0.0;
    }
    let mut last = 0u32;
    for y in 0..h {
        let covered: u32 = (0..w as i32).map(|x| (mask.get(x, y as i32) > 128) as u32).sum();
        if covered * 2 > w {
            last = y;
        }
    }
    (last + 1) as f32 / h as f32
}

/// Put a new sky in.
pub fn replace(
    scene: &PixelBuffer,
    sky: &MaskBuffer,
    new_sky: &PixelBuffer,
    how: SkyReplace,
) -> PixelBuffer {
    let (w, h) = (scene.width(), scene.height());
    if w == 0 || h == 0 || new_sky.width() == 0 || new_sky.height() == 0 {
        return scene.clone();
    }

    // Grown, then softened inward: a little sky on a branch reads as sky seen
    // through it, and a little branch in the sky reads as a mistake.
    let grown = grow_mask(sky, how.grow);
    let soft = feather_inward(&grown, how.feather);

    // The replacement is placed so its horizon lands on the scene's — the
    // original's, not the grown one's, since growing moved it down a little.
    let line = (horizon(sky) + how.offset).clamp(0.0, 1.0);
    let scale = how.scale.clamp(0.1, 10.0);

    // What the sky was and what it is now, for the light.
    let old_light = mean_under(scene, &soft);
    let new_light = mean_of(new_sky);

    let mut out = scene.clone();
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let k = soft.get(x, y) as f32 / 255.0;
            let here = scene.get(x, y).to_f32();

            // The foreground takes some of the new sky's colour. Weighted by
            // how bright the pixel already is: a lit surface takes the light's
            // colour and a shadow keeps its own, which is what makes this read
            // as light rather than as a filter.
            let lit = if how.relight > 0.0 && k < 1.0 {
                let weight = how.relight.clamp(0.0, 1.0) * here.luma().clamp(0.0, 1.0);
                let shift = |c: f32, from: f32, to: f32| {
                    let ratio = if from > 1e-3 { to / from } else { 1.0 };
                    (c * (1.0 + (ratio - 1.0) * weight)).clamp(0.0, 1.0)
                };
                Rgba {
                    r: shift(here.r, old_light.r, new_light.r),
                    g: shift(here.g, old_light.g, new_light.g),
                    b: shift(here.b, old_light.b, new_light.b),
                    a: here.a,
                }
            } else {
                here
            };

            if k <= 0.0 {
                out.set(x, y, lit.to_u8());
                continue;
            }
            let s = sample_sky(new_sky, x, y, w, h, line, scale).to_f32();
            out.set(
                x,
                y,
                Rgba {
                    r: lit.r + (s.r - lit.r) * k,
                    g: lit.g + (s.g - lit.g) * k,
                    b: lit.b + (s.b - lit.b) * k,
                    a: here.a,
                }
                .to_u8(),
            );
        }
    }
    out
}

/// The replacement's pixel for a scene pixel, with its horizon on the scene's.
fn sample_sky(
    sky: &PixelBuffer,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    line: f32,
    scale: f32,
) -> Rgba8 {
    // The replacement's own horizon is taken to be its bottom edge, which is
    // what a photograph of a sky has.
    let sh = sky.height() as f32;
    let sw = sky.width() as f32;
    let scene_line = line * h as f32;
    let v = (y as f32 - scene_line) / scale + sh;
    let u = (x as f32 - w as f32 / 2.0) / scale * (sw / w.max(1) as f32) + sw / 2.0;
    sky.get(
        (u.round() as i32).clamp(0, sky.width() as i32 - 1),
        (v.round() as i32).clamp(0, sky.height() as i32 - 1),
    )
}

/// Spread a mask outward by `radius` pixels.
fn grow_mask(mask: &MaskBuffer, radius: f32) -> MaskBuffer {
    let r = radius.round().max(0.0) as i32;
    if r == 0 {
        return mask.clone();
    }
    let (w, h) = (mask.width(), mask.height());
    let mut out = MaskBuffer::hide_all(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut most = 0u8;
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dy * dy > r * r {
                        continue;
                    }
                    most = most.max(mask.get(x + dx, y + dy));
                }
            }
            out.set(x, y, most);
        }
    }
    out
}

/// Soften a mask's edge into the covered side only.
///
/// An ordinary feather spreads both ways and pulls the foreground into the
/// sky, which is the one direction that shows.
fn feather_inward(mask: &MaskBuffer, radius: f32) -> MaskBuffer {
    let r = radius.round().max(0.0) as i32;
    if r == 0 {
        return mask.clone();
    }
    let (w, h) = (mask.width(), mask.height());
    let mut out = MaskBuffer::hide_all(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if mask.get(x, y) == 0 {
                continue;
            }
            // How far into the covered region this pixel is, up to the radius.
            let mut near = r + 1;
            for dy in -r..=r {
                for dx in -r..=r {
                    if mask.get(x + dx, y + dy) < 128 {
                        let d = ((dx * dx + dy * dy) as f32).sqrt().round() as i32;
                        near = near.min(d);
                    }
                }
            }
            let k = (near as f32 / (r as f32 + 1.0)).clamp(0.0, 1.0);
            let base = mask.get(x, y) as f32 / 255.0;
            out.set(x, y, (base * k * 255.0) as u8);
        }
    }
    out
}

fn mean_under(px: &PixelBuffer, mask: &MaskBuffer) -> Rgba {
    let (mut acc, mut n) = (Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }, 0.0f32);
    for y in 0..px.height() as i32 {
        for x in 0..px.width() as i32 {
            let k = mask.get(x, y) as f32 / 255.0;
            if k < 0.5 {
                continue;
            }
            let c = px.get(x, y).to_f32();
            acc.r += c.r;
            acc.g += c.g;
            acc.b += c.b;
            n += 1.0;
        }
    }
    if n == 0.0 {
        return Rgba { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };
    }
    Rgba { r: acc.r / n, g: acc.g / n, b: acc.b / n, a: 1.0 }
}

fn mean_of(px: &PixelBuffer) -> Rgba {
    let (mut acc, mut n) = (Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }, 0.0f32);
    for c in px.pixels() {
        let c = c.to_f32();
        acc.r += c.r;
        acc.g += c.g;
        acc.b += c.b;
        n += 1.0;
    }
    if n == 0.0 {
        return Rgba { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };
    }
    Rgba { r: acc.r / n, g: acc.g / n, b: acc.b / n, a: 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scene: grey sky above, dark ground below.
    fn scene(w: u32, h: u32, line: u32) -> (PixelBuffer, MaskBuffer) {
        let mut px = PixelBuffer::new(w, h);
        let mut mask = MaskBuffer::hide_all(w, h);
        for y in 0..h {
            for x in 0..w {
                let sky = y < line;
                px.set(
                    x as i32,
                    y as i32,
                    if sky { Rgba8::opaque(200, 200, 205) } else { Rgba8::opaque(60, 70, 50) },
                );
                if sky {
                    mask.set(x as i32, y as i32, 255);
                }
            }
        }
        (px, mask)
    }

    #[test]
    fn the_sky_is_replaced_and_the_ground_is_not() {
        let (px, mask) = scene(40, 40, 20);
        let new = gradient(40, 40, Rgba8::opaque(30, 60, 180), Rgba8::opaque(220, 160, 90));
        let out = replace(
            &px,
            &mask,
            &new,
            SkyReplace { relight: 0.0, feather: 0.0, grow: 0.0, ..Default::default() },
        );

        let up = out.get(20, 4);
        assert!(up.b > up.r, "the sky above is the new blue: {up:?}");
        let down = out.get(20, 34);
        assert_eq!(down, px.get(20, 34), "and the ground is untouched");
    }

    /// The horizon is where the sky stops, not the bottom of the frame.
    #[test]
    fn the_horizon_is_found_where_the_sky_ends() {
        let (_, mask) = scene(40, 40, 20);
        let line = horizon(&mask);
        assert!((line - 0.5).abs() < 0.05, "half way down: {line}");
    }

    /// A gap between two buildings has sky in it all the way down, and the
    /// horizon is still where most of the sky stops.
    #[test]
    fn a_gap_of_sky_does_not_move_the_horizon() {
        let (_, mut mask) = scene(40, 40, 20);
        for y in 20..40 {
            for x in 18..22 {
                mask.set(x, y, 255);
            }
        }
        let line = horizon(&mask);
        assert!((line - 0.5).abs() < 0.05, "a narrow gap is not the horizon: {line}");
    }

    /// A photograph taken under a grey sky, given a golden one, has to take
    /// some of its colour or it is a grey day with a golden sky pasted on.
    #[test]
    fn the_foreground_takes_the_new_skys_light() {
        let (px, mask) = scene(40, 40, 20);
        let golden = gradient(40, 40, Rgba8::opaque(240, 180, 90), Rgba8::opaque(250, 200, 130));
        let lit = replace(&px, &mask, &golden, SkyReplace { relight: 0.8, ..Default::default() });
        let pasted = replace(&px, &mask, &golden, SkyReplace { relight: 0.0, ..Default::default() });

        let warmth = |c: Rgba8| c.r as f32 - c.b as f32;
        let ground = (30, 34);
        assert!(
            warmth(lit.get(ground.0, ground.1)) > warmth(pasted.get(ground.0, ground.1)) + 3.0,
            "the ground should have warmed: {:?} against {:?}",
            lit.get(ground.0, ground.1),
            pasted.get(ground.0, ground.1)
        );
    }

    /// Softening has to go into the sky, not into the trees: sky on a branch
    /// reads as sky seen through it, and a branch in the sky reads as a
    /// mistake.
    #[test]
    fn the_join_is_softened_into_the_sky_only() {
        let (_, mask) = scene(40, 40, 20);
        let soft = feather_inward(&mask, 3.0);
        // Below the line the mask was empty and must stay empty.
        for y in 20..24 {
            assert_eq!(soft.get(20, y), 0, "row {y} is not sky and must not become any");
        }
        // Just above the line it has been softened.
        assert!(soft.get(20, 19) < 255, "the edge is soft");
        assert_eq!(soft.get(20, 4), 255, "and well inside it is not");
    }

    /// The label boundary usually falls just inside the sky, leaving a pale
    /// fringe of the old one round every silhouette — the most conspicuous way
    /// a replaced sky announces itself.
    #[test]
    fn growing_the_mask_takes_the_old_skys_fringe_with_it() {
        let (px, mask) = scene(40, 40, 20);
        // A mask that stops two pixels short, as a label map does.
        let mut shy = MaskBuffer::hide_all(40, 40);
        for y in 0..18 {
            for x in 0..40 {
                shy.set(x, y, 255);
            }
        }
        let new = gradient(40, 40, Rgba8::opaque(30, 60, 180), Rgba8::opaque(40, 90, 200));
        let plain = SkyReplace { relight: 0.0, feather: 0.0, grow: 0.0, ..Default::default() };
        let without = replace(&px, &shy, &new, plain);
        let with = replace(&px, &shy, &new, SkyReplace { grow: 2.0, ..plain });

        // Rows 18 and 19 are old sky the label missed.
        assert!(without.get(20, 19).r > 150, "the fringe is there without growing");
        assert!(with.get(20, 19).b > with.get(20, 19).r, "and gone with it");
        let _ = mask;
    }

    #[test]
    fn a_generated_sky_is_darker_overhead_than_at_the_horizon() {
        let sky = gradient(8, 64, Rgba8::opaque(20, 60, 180), Rgba8::opaque(220, 220, 230));
        assert!(sky.get(4, 2).b > sky.get(4, 2).r, "blue overhead");
        assert!(sky.get(4, 62).r > sky.get(4, 2).r, "and pale at the horizon");
    }

    #[test]
    fn a_scene_with_no_sky_is_left_alone() {
        let (px, _) = scene(20, 20, 0);
        let empty = MaskBuffer::hide_all(20, 20);
        let new = gradient(20, 20, Rgba8::opaque(0, 0, 255), Rgba8::WHITE);
        let out = replace(
            &px,
            &empty,
            &new,
            SkyReplace { relight: 0.0, grow: 0.0, ..Default::default() },
        );
        assert_eq!(out.pixels(), px.pixels());
    }
}
