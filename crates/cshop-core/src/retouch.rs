//! Dodge, burn and sponge: lightening, darkening and saturating by hand.
//!
//! The darkroom tools these are named after were pieces of card. Dodging held
//! one between the enlarger and the paper to keep light off a region, so it
//! came out lighter; burning was the opposite, a hole in the card letting extra
//! light through one part of the print. Both are still the fastest way to shape
//! a photograph, because the eye reads a picture by its tones long before it
//! reads what is in it.
//!
//! # Why a tonal range
//!
//! Lightening a whole region evenly flattens it. What a print usually needs is
//! for the *shadows* to open up while the highlights stay where they are, or
//! for the highlights to come down while the shadows hold. So each stroke
//! chooses a range and the effect falls off away from it — which is what makes
//! these tools different from painting with white at low opacity.
//!
//! The falloff is a bell centred on the range: full strength at its middle,
//! about an eighth of it half a range away, nothing at the far end. A bell
//! rather than a hard band because a band leaves a visible edge wherever a
//! gradient crosses its boundary, which is exactly where these tools are used.

use crate::color::Rgba;

/// Which part of the tonal scale a stroke acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tones {
    Shadows,
    #[default]
    Midtones,
    Highlights,
}

impl Tones {
    pub fn name(self) -> &'static str {
        match self {
            Tones::Shadows => "Shadows",
            Tones::Midtones => "Midtones",
            Tones::Highlights => "Highlights",
        }
    }

    /// Where on the scale this range is centred.
    fn centre(self) -> f32 {
        match self {
            Tones::Shadows => 0.0,
            Tones::Midtones => 0.5,
            Tones::Highlights => 1.0,
        }
    }

    /// How much of the effect a pixel of this luma receives, `0..=1`.
    ///
    /// The width is a quarter of the scale, so midtones reach a shadow at
    /// about an eighth strength and a black pixel not at all when the range
    /// is highlights.
    pub fn weight(self, luma: f32) -> f32 {
        const WIDTH: f32 = 0.25;
        let d = (luma.clamp(0.0, 1.0) - self.centre()) / WIDTH;
        (-0.5 * d * d).exp()
    }
}

/// Which of the three a stroke is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetouchKind {
    #[default]
    Dodge,
    Burn,
    /// Saturate, or — with `soak` off — desaturate.
    Sponge,
}

impl RetouchKind {
    pub fn name(self) -> &'static str {
        match self {
            RetouchKind::Dodge => "Dodge",
            RetouchKind::Burn => "Burn",
            RetouchKind::Sponge => "Sponge",
        }
    }
}

/// A whole retouching stroke's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Retouch {
    pub kind: RetouchKind,
    /// Ignored by the sponge, which acts on colour rather than tone.
    pub range: Tones,
    /// How much one full-coverage dab does, `0..=1`.
    pub exposure: f32,
    /// Sponge only: soak colour in rather than out.
    pub soak: bool,
}

impl Default for Retouch {
    fn default() -> Self {
        Self {
            kind: RetouchKind::Dodge,
            range: Tones::Midtones,
            // Low, because these tools are used by building up. A default that
            // does the whole job in one pass is a default that ruins a
            // photograph before anyone has found the slider.
            exposure: 0.15,
            soak: true,
        }
    }
}

impl Retouch {
    /// Apply this to one pixel at the given coverage.
    ///
    /// `pixel` is straight (not premultiplied) linear-ish sRGB in `0..=1`, as
    /// everything in the paint engine is. Alpha is never changed: these tools
    /// shape what is there and do not add or remove any of it. The effect is
    /// scaled by alpha as well, so a stroke over a half-transparent edge does
    /// not lighten it as if it were solid and leave a bright fringe.
    pub fn apply(&self, pixel: Rgba, coverage: f32) -> Rgba {
        let amount = (self.exposure * coverage * pixel.a).clamp(0.0, 1.0);
        if amount <= 0.0 {
            return pixel;
        }
        let luma = pixel.luma().clamp(0.0, 1.0);
        match self.kind {
            RetouchKind::Dodge => {
                let a = amount * self.range.weight(luma);
                // Toward white, proportionally to how far each channel still
                // has to go — so a channel already at 1 cannot move and the
                // hue holds instead of drifting.
                Rgba {
                    r: pixel.r + (1.0 - pixel.r) * a,
                    g: pixel.g + (1.0 - pixel.g) * a,
                    b: pixel.b + (1.0 - pixel.b) * a,
                    a: pixel.a,
                }
            }
            RetouchKind::Burn => {
                let a = amount * self.range.weight(luma);
                Rgba {
                    r: pixel.r * (1.0 - a),
                    g: pixel.g * (1.0 - a),
                    b: pixel.b * (1.0 - a),
                    a: pixel.a,
                }
            }
            RetouchKind::Sponge => {
                // Away from or toward the pixel's own grey. Soaking can push a
                // channel past the ends, so the result is clamped rather than
                // allowed to wrap.
                let k = if self.soak { 1.0 + amount * 2.0 } else { 1.0 - amount };
                Rgba {
                    r: (luma + (pixel.r - luma) * k).clamp(0.0, 1.0),
                    g: (luma + (pixel.g - luma) * k).clamp(0.0, 1.0),
                    b: (luma + (pixel.b - luma) * k).clamp(0.0, 1.0),
                    a: pixel.a,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(v: f32) -> Rgba {
        Rgba { r: v, g: v, b: v, a: 1.0 }
    }

    #[test]
    fn each_range_peaks_where_it_says_it_does() {
        assert!((Tones::Shadows.weight(0.0) - 1.0).abs() < 1e-6);
        assert!((Tones::Midtones.weight(0.5) - 1.0).abs() < 1e-6);
        assert!((Tones::Highlights.weight(1.0) - 1.0).abs() < 1e-6);
        // And falls away from it, rather than stopping at an edge.
        assert!(Tones::Shadows.weight(0.5) < 0.2);
        assert!(Tones::Shadows.weight(1.0) < 0.01);
        assert!(Tones::Highlights.weight(0.0) < 0.01);
    }

    #[test]
    fn dodge_lightens_and_burn_darkens_within_the_range() {
        let d = Retouch { kind: RetouchKind::Dodge, range: Tones::Midtones, exposure: 0.5, soak: true };
        let b = Retouch { kind: RetouchKind::Burn, ..d };
        let mid = grey(0.5);
        assert!(d.apply(mid, 1.0).r > mid.r, "dodge lightens");
        assert!(b.apply(mid, 1.0).r < mid.r, "burn darkens");
    }

    #[test]
    fn a_range_leaves_the_other_end_alone() {
        let d = Retouch { kind: RetouchKind::Dodge, range: Tones::Highlights, exposure: 1.0, soak: true };
        let dark = grey(0.02);
        let moved = (d.apply(dark, 1.0).r - dark.r).abs();
        assert!(moved < 0.01, "highlights dodging moved a shadow by {moved}");
        let light = grey(0.95);
        assert!(d.apply(light, 1.0).r > light.r + 0.01, "but it moves a highlight");
    }

    #[test]
    fn neither_can_leave_the_scale() {
        for kind in [RetouchKind::Dodge, RetouchKind::Burn, RetouchKind::Sponge] {
            for range in [Tones::Shadows, Tones::Midtones, Tones::Highlights] {
                let r = Retouch { kind, range, exposure: 1.0, soak: true };
                for step in 0..=20 {
                    let v = step as f32 / 20.0;
                    let out = r.apply(Rgba { r: v, g: 1.0 - v, b: 0.5, a: 1.0 }, 1.0);
                    for c in [out.r, out.g, out.b, out.a] {
                        assert!((0.0..=1.0).contains(&c), "{kind:?} {range:?} at {v} gave {c}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_sponge_moves_colour_and_not_tone() {
        let out = Retouch { kind: RetouchKind::Sponge, exposure: 0.5, soak: false, ..Default::default() }
            .apply(Rgba { r: 0.8, g: 0.2, b: 0.2, a: 1.0 }, 1.0);
        let (before, after) = (0.8 - 0.2, out.r - out.g);
        assert!(after < before, "wringing it out should narrow the channels");
        // Desaturating fully would land on the grey; halfway should not.
        assert!(after > 0.0, "and not overshoot into the other direction");

        let soaked = Retouch { kind: RetouchKind::Sponge, exposure: 0.5, soak: true, ..Default::default() }
            .apply(Rgba { r: 0.6, g: 0.4, b: 0.4, a: 1.0 }, 1.0);
        assert!(soaked.r - soaked.g > 0.2, "soaking should widen them");
    }

    #[test]
    fn nothing_happens_where_the_brush_did_not_reach() {
        let r = Retouch { exposure: 1.0, ..Default::default() };
        assert_eq!(r.apply(grey(0.5), 0.0), grey(0.5));
        // Nor on a transparent pixel, which has no tone to shape.
        let clear = Rgba { r: 0.5, g: 0.5, b: 0.5, a: 0.0 };
        assert_eq!(r.apply(clear, 1.0), clear);
    }
}
