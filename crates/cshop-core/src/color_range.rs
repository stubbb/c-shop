//! Selecting by colour across the whole picture.
//!
//! # How this differs from the magic wand
//!
//! The wand answers "what is joined to this?"; it floods outward from a point
//! and stops where the colour changes. That is the right question for a sky
//! and the wrong one for *every* red thing in a photograph, which is not one
//! region and never will be.
//!
//! The other difference matters more. A wand selection is in or out: a pixel
//! either matched within the tolerance or it did not. Selecting by colour
//! returns a *matte* — partial coverage that falls away as the colour does — so
//! an edge where the colour is halfway between comes out halfway selected, and
//! whatever is done through the selection blends instead of stepping.
//!
//! # Fuzziness
//!
//! One control. At zero only an exact match is selected. Turning it up widens
//! the band and softens its edge together, because those are the same thing:
//! the band's edge *is* the falloff. What it is measured against depends on
//! what is being picked — distance from a sampled colour, position on the
//! tonal scale, or angle round the hue circle.

use crate::color::{rgb_to_hsv, Rgba8};
use crate::mask::MaskBuffer;
use crate::pixels::PixelBuffer;

/// What counts as "this colour".
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// Within reach of any of these, which is what the eyedroppers collect.
    /// Several colours are a union, so adding one can only ever select more.
    Sampled(Vec<Rgba8>),
    /// A band of the tonal scale, centred where it says.
    Shadows,
    Midtones,
    Highlights,
    /// A band of the hue circle. `centre` is in turns, `0` red, `1/3` green.
    Hue { centre: f32 },
}

impl Pick {
    pub fn name(&self) -> &'static str {
        match self {
            Pick::Sampled(_) => "Sampled Colours",
            Pick::Shadows => "Shadows",
            Pick::Midtones => "Midtones",
            Pick::Highlights => "Highlights",
            Pick::Hue { .. } => "Hue",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColorRange {
    pub pick: Pick,
    /// `0..=1`. How far past an exact match still counts, and how softly.
    pub fuzziness: f32,
    pub invert: bool,
}

impl Default for ColorRange {
    fn default() -> Self {
        Self { pick: Pick::Sampled(Vec::new()), fuzziness: 0.16, invert: false }
    }
}

impl ColorRange {
    /// How selected one colour is, `0..=1`.
    pub fn coverage(&self, c: Rgba8) -> f32 {
        let v = match &self.pick {
            Pick::Sampled(colours) => colours
                .iter()
                .map(|&t| self.falloff(distance(c, t)))
                .fold(0.0f32, f32::max),
            Pick::Shadows => self.band(luma(c), 0.0),
            Pick::Midtones => self.band(luma(c), 0.5),
            Pick::Highlights => self.band(luma(c), 1.0),
            Pick::Hue { centre } => {
                let (h, s, _) = rgb_to_hsv(
                    c.r as f32 / 255.0,
                    c.g as f32 / 255.0,
                    c.b as f32 / 255.0,
                );
                // A grey pixel has no hue to speak of, and treating its
                // arbitrary one as a match is how selecting "the reds" ends up
                // including half the shadows.
                let d = (h - centre).rem_euclid(1.0);
                let d = d.min(1.0 - d) * 2.0;
                self.falloff(d) * s.min(1.0)
            }
        };
        // Transparent pixels have no colour worth matching, whatever the
        // numbers in them say.
        let v = v * (c.a as f32 / 255.0);
        if self.invert {
            1.0 - v
        } else {
            v
        }
    }

    /// Coverage as a whole matte over an image.
    pub fn matte(&self, src: &PixelBuffer) -> MaskBuffer {
        use rayon::prelude::*;
        let (w, h) = (src.width(), src.height());
        let mut out = MaskBuffer::hide_all(w, h);
        let rows: Vec<Vec<u8>> = (0..h)
            .into_par_iter()
            .map(|y| {
                src.row(y)
                    .iter()
                    .map(|&c| (self.coverage(c) * 255.0 + 0.5) as u8)
                    .collect()
            })
            .collect();
        for (y, row) in rows.iter().enumerate() {
            for (x, &v) in row.iter().enumerate() {
                out.set(x as i32, y as i32, v);
            }
        }
        out
    }

    /// Full inside the band, falling to nothing at its edge.
    ///
    /// The inner half is a plateau. Without one, nothing but an exact match is
    /// ever *fully* selected — a colour a shade off comes out at 96%, and
    /// filling through the selection leaves a faint veil over what was meant
    /// to be solid. The shoulder is a smoothstep, because a straight ramp
    /// leaves a visible crease where it meets the plateau. It is the same
    /// shape the brush's hardness uses, for the same reason.
    fn falloff(&self, d: f32) -> f32 {
        let f = self.fuzziness.clamp(0.0, 1.0);
        if f <= 0.0 {
            return if d <= 1e-6 { 1.0 } else { 0.0 };
        }
        let inner = f * 0.5;
        if d <= inner {
            return 1.0;
        }
        let t = ((d - inner) / (f - inner).max(1e-4)).clamp(0.0, 1.0);
        1.0 - t * t * (3.0 - 2.0 * t)
    }

    /// A tonal band. Fuzziness sets its width, so the one control means the
    /// same thing whichever way the picking is done.
    fn band(&self, v: f32, centre: f32) -> f32 {
        // Doubled, because a tonal band is measured along a scale rather than
        // as a distance between two colours, and half the scale is a
        // reasonable widest band.
        let width = (self.fuzziness.clamp(0.0, 1.0) * 2.0).max(1e-4);
        let t = ((v - centre).abs() / width).clamp(0.0, 1.0);
        1.0 - t * t * (3.0 - 2.0 * t)
    }
}

/// Largest difference across the channels, `0..=1` — the same measure the
/// magic wand uses, so a tolerance means the same thing in both.
fn distance(a: Rgba8, b: Rgba8) -> f32 {
    let d = |x: u8, y: u8| x.abs_diff(y) as f32;
    d(a.r, b.r).max(d(a.g, b.g)).max(d(a.b, b.b)) / 255.0
}

fn luma(c: Rgba8) -> f32 {
    (0.30 * c.r as f32 + 0.59 * c.g as f32 + 0.11 * c.b as f32) / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stripes() -> PixelBuffer {
        // Three vertical bands: red, a red that is slightly off, and blue.
        let mut px = PixelBuffer::new(30, 4);
        for y in 0..4 {
            for x in 0..30 {
                let c = match x / 10 {
                    0 => Rgba8::opaque(200, 30, 30),
                    1 => Rgba8::opaque(190, 45, 40),
                    _ => Rgba8::opaque(30, 30, 200),
                };
                px.set(x, y, c);
            }
        }
        px
    }

    #[test]
    fn an_exact_match_is_selected_and_a_different_colour_is_not() {
        let px = stripes();
        let r = ColorRange {
            pick: Pick::Sampled(vec![Rgba8::opaque(200, 30, 30)]),
            fuzziness: 0.0,
            invert: false,
        };
        let m = r.matte(&px);
        assert_eq!(m.get(5, 2), 255, "the colour that was sampled");
        assert_eq!(m.get(15, 2), 0, "the near miss, with no fuzziness");
        assert_eq!(m.get(25, 2), 0, "and the blue");
    }

    /// The difference from the wand: the answer is a matte, not a verdict.
    #[test]
    fn fuzziness_selects_partly_rather_than_not_at_all() {
        let px = stripes();
        let at = |f: f32| {
            ColorRange {
                pick: Pick::Sampled(vec![Rgba8::opaque(200, 30, 30)]),
                fuzziness: f,
                invert: false,
            }
            .matte(&px)
            .get(15, 2)
        };
        // The near miss is 15 levels away, so 15/255 ≈ 0.06 of the scale.
        assert_eq!(at(0.0), 0);
        // Fifteen levels is 0.06 of the scale, past the plateau of a 0.08
        // band but inside its shoulder.
        let partial = at(0.08);
        assert!(partial > 0 && partial < 255, "partly selected: {partial}");
        assert_eq!(at(0.5), 255, "and fully, once the band is wide enough");
    }

    #[test]
    fn several_sampled_colours_are_a_union() {
        let px = stripes();
        let mut r = ColorRange {
            pick: Pick::Sampled(vec![Rgba8::opaque(200, 30, 30)]),
            fuzziness: 0.0,
            invert: false,
        };
        assert_eq!(r.matte(&px).get(25, 2), 0);
        if let Pick::Sampled(v) = &mut r.pick {
            v.push(Rgba8::opaque(30, 30, 200));
        }
        let m = r.matte(&px);
        assert_eq!(m.get(5, 2), 255, "the first is still selected");
        assert_eq!(m.get(25, 2), 255, "and now the second as well");
    }

    #[test]
    fn inverting_swaps_every_level_and_not_just_the_ends() {
        let px = stripes();
        let base = ColorRange {
            pick: Pick::Sampled(vec![Rgba8::opaque(200, 30, 30)]),
            fuzziness: 0.08,
            invert: false,
        };
        let mut flipped = base.clone();
        flipped.invert = true;
        for x in [5, 15, 25] {
            let a = base.matte(&px).get(x, 2) as i32;
            let b = flipped.matte(&px).get(x, 2) as i32;
            assert!((a + b - 255).abs() <= 1, "at {x}: {a} and {b} should sum to 255");
        }
    }

    #[test]
    fn the_tonal_bands_land_where_they_say() {
        let mut px = PixelBuffer::new(3, 1);
        px.set(0, 0, Rgba8::opaque(5, 5, 5));
        px.set(1, 0, Rgba8::opaque(128, 128, 128));
        px.set(2, 0, Rgba8::opaque(250, 250, 250));
        let at = |pick: Pick, x: i32| {
            ColorRange { pick, fuzziness: 0.3, invert: false }.matte(&px).get(x, 0)
        };
        assert!(at(Pick::Shadows, 0) > 200 && at(Pick::Shadows, 2) < 20);
        assert!(at(Pick::Highlights, 2) > 200 && at(Pick::Highlights, 0) < 20);
        assert!(at(Pick::Midtones, 1) > 200);
    }

    /// Grey has an arbitrary hue, and letting it match is how "select the
    /// reds" ends up selecting half the shadows as well.
    #[test]
    fn a_hue_band_ignores_colours_that_have_no_hue() {
        let mut px = PixelBuffer::new(2, 1);
        px.set(0, 0, Rgba8::opaque(220, 40, 40)); // red
        px.set(1, 0, Rgba8::opaque(128, 128, 128)); // grey
        let m = ColorRange { pick: Pick::Hue { centre: 0.0 }, fuzziness: 0.3, invert: false }
            .matte(&px);
        assert!(m.get(0, 0) > 200, "the red is in the band");
        assert!(m.get(1, 0) < 20, "and the grey is not, whatever its hue says");
    }

    #[test]
    fn transparent_pixels_are_not_selected_by_the_colour_they_are_not_showing() {
        let mut px = PixelBuffer::new(2, 1);
        px.set(0, 0, Rgba8::opaque(200, 30, 30));
        px.set(1, 0, Rgba8::new(200, 30, 30, 0));
        let m = ColorRange {
            pick: Pick::Sampled(vec![Rgba8::opaque(200, 30, 30)]),
            fuzziness: 0.1,
            invert: false,
        }
        .matte(&px);
        assert_eq!(m.get(0, 0), 255);
        assert_eq!(m.get(1, 0), 0);
    }
}
