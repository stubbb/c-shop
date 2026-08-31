//! Smoothing skin without smoothing eyes and hair.
//!
//! # Why an ordinary blur is wrong
//!
//! Skin has two kinds of variation on it. One is texture — pores, fine lines,
//! the grain of the photograph — which is small and everywhere. The other is
//! features: the edge of a lip, an eyelash, the line of a nostril. A blur
//! cannot tell them apart, so smoothing the first destroys the second and the
//! result is the plastic look that says "retouched" from across a room.
//!
//! # What tells them apart
//!
//! How *different* a neighbour is. Texture varies by a few levels; a feature
//! varies by fifty. So the smoothing is a weighted average that ignores
//! neighbours beyond a threshold — which is the surface blur this program
//! already has, and the reason it is the right tool is that its threshold is
//! exactly the distinction that matters here.
//!
//! # And why not all of it
//!
//! Removing texture entirely is what makes skin look like plastic, so some of
//! it goes back: the difference between the original and the smoothed version
//! is added back at a fraction. Skin with a little grain reads as skin.

use crate::color::Rgba;
use crate::filters::{Filter, FilterContext};
use crate::mask::MaskBuffer;
use crate::pixels::PixelBuffer;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinSmooth {
    /// How far the smoothing reaches, in pixels. Should be about the size of
    /// the texture being removed and well under the size of a feature.
    pub radius: f32,
    /// How different a neighbour may be and still be averaged in, `0..=1`.
    /// Low keeps everything but the finest grain; high starts eating features.
    pub threshold: f32,
    /// How much of the smoothed version to keep, `0..=1`.
    pub amount: f32,
    /// How much of the original texture to put back, `0..=1`. Some is what
    /// keeps skin looking like skin.
    pub texture: f32,
}

impl Default for SkinSmooth {
    fn default() -> Self {
        Self { radius: 6.0, threshold: 0.08, amount: 0.7, texture: 0.25 }
    }
}

impl SkinSmooth {
    /// Smooth inside `mask`, leaving everything outside it alone.
    ///
    /// `None` for the mask smooths the whole picture, which is what a
    /// selection-less call means.
    pub fn apply(&self, src: &PixelBuffer, mask: Option<&MaskBuffer>) -> PixelBuffer {
        if self.amount <= 0.0 || self.radius < 0.5 {
            return src.clone();
        }
        let smoothed = Filter::SurfaceBlur {
            radius: self.radius,
            threshold: self.threshold.clamp(0.0, 1.0),
        }
        .apply(src, &FilterContext::default());

        let mut out = src.clone();
        let amount = self.amount.clamp(0.0, 1.0);
        let texture = self.texture.clamp(0.0, 1.0);
        for y in 0..src.height() as i32 {
            for x in 0..src.width() as i32 {
                let k = match mask {
                    Some(m) => m.get(x, y) as f32 / 255.0,
                    None => 1.0,
                } * amount;
                if k <= 0.0 {
                    continue;
                }
                let a = src.get(x, y).to_f32();
                let b = smoothed.get(x, y).to_f32();
                // Toward the smoothed version, then a fraction of what was
                // removed put back.
                let mix = |a: f32, b: f32| {
                    let to = a + (b - a) * k;
                    (to + (a - b) * texture * k).clamp(0.0, 1.0)
                };
                out.set(
                    x,
                    y,
                    Rgba {
                        r: mix(a.r, b.r),
                        g: mix(a.g, b.g),
                        b: mix(a.b, b.b),
                        a: a.a,
                    }
                    .to_u8(),
                );
            }
        }
        out
    }
}

/// The part of a detected person that is their head, as a fraction of the box.
///
/// A person detector gives a box round the whole body. The head is the top of
/// it, and how much of the top depends on how much of the body is in frame:
/// a portrait's box is mostly head, a full-length figure's box is mostly not.
/// The ratio of the box's height to its width says which — a tall narrow box
/// is a standing figure, a squarer one is a portrait.
pub fn head_of(box_width: f32, box_height: f32) -> f32 {
    if box_width <= 0.0 || box_height <= 0.0 {
        return 1.0;
    }
    let aspect = box_height / box_width;
    // A head is about one part in seven of a standing figure and most of a
    // head-and-shoulders. Interpolated between, and never more than all of it.
    (1.6 / aspect).clamp(0.14, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;

    /// A face: an even tone with fine grain on it, and one hard feature.
    fn face(w: u32, h: u32) -> PixelBuffer {
        let mut px = PixelBuffer::new(w, h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                // Grain of a few levels — texture.
                let n = ((x * 7 + y * 13) % 5) - 2;
                let v = (190 + n * 3).clamp(0, 255) as u8;
                px.set(x, y, Rgba8::opaque(v, (v as f32 * 0.82) as u8, (v as f32 * 0.74) as u8));
            }
        }
        // A feature: a dark line, like an eyelash.
        for x in 10..(w as i32 - 10) {
            px.set(x, (h / 2) as i32, Rgba8::opaque(30, 25, 25));
        }
        px
    }

    /// How much variation there is in a patch.
    fn grain(px: &PixelBuffer, x0: i32, y0: i32, n: i32) -> f32 {
        let v: Vec<f32> = (y0..y0 + n)
            .flat_map(|y| (x0..x0 + n).map(move |x| (x, y)))
            .map(|(x, y)| px.get(x, y).r as f32)
            .collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        (v.iter().map(|a| (a - mean).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn the_texture_goes_and_the_feature_stays() {
        let px = face(64, 64);
        let out = SkinSmooth { texture: 0.0, ..Default::default() }.apply(&px, None);

        let before = grain(&px, 8, 8, 10);
        let after = grain(&out, 8, 8, 10);
        assert!(after < before * 0.6, "the grain should go: {before:.2} to {after:.2}");

        // The line is still a line: dark against its neighbours.
        let on = out.get(32, 32).r as i32;
        let above = out.get(32, 28).r as i32;
        assert!(above - on > 100, "the feature should have survived: {on} against {above}");
    }

    /// Removing all the texture is what makes skin look like plastic, so some
    /// of it goes back.
    #[test]
    fn some_of_the_texture_is_put_back() {
        let px = face(48, 48);
        let flat = SkinSmooth { texture: 0.0, ..Default::default() }.apply(&px, None);
        let kept = SkinSmooth { texture: 0.5, ..Default::default() }.apply(&px, None);
        assert!(
            grain(&kept, 8, 8, 10) > grain(&flat, 8, 8, 10),
            "keeping texture should leave more of it"
        );
    }

    #[test]
    fn nothing_outside_the_mask_is_touched() {
        let px = face(48, 48);
        let mut mask = MaskBuffer::hide_all(48, 48);
        for y in 0..24 {
            for x in 0..48 {
                mask.set(x, y, 255);
            }
        }
        let out = SkinSmooth::default().apply(&px, Some(&mask));
        for y in 30..48 {
            for x in 0..48 {
                assert_eq!(out.get(x, y), px.get(x, y), "({x}, {y}) is outside the mask");
            }
        }
        assert_ne!(out.get(20, 8), px.get(20, 8), "and inside it something happened");
    }

    #[test]
    fn no_amount_is_no_change() {
        let px = face(32, 32);
        assert_eq!(SkinSmooth { amount: 0.0, ..Default::default() }.apply(&px, None).pixels(), px.pixels());
    }

    /// A person detector gives a box round the whole body, and how much of it
    /// is head depends on how much of the body is in frame.
    #[test]
    fn the_head_is_more_of_a_portrait_than_of_a_standing_figure() {
        let portrait = head_of(200.0, 240.0);
        let standing = head_of(120.0, 700.0);
        assert!(portrait > standing * 2.0, "{portrait:.2} against {standing:.2}");
        assert!((0.14..=1.0).contains(&standing));
        assert!((0.14..=1.0).contains(&portrait));
    }

    #[test]
    fn a_box_of_nothing_asks_for_all_of_it() {
        assert_eq!(head_of(0.0, 0.0), 1.0);
    }
}
