//! Colour adjustments.
//!
//! Each adjustment is a pure function from colour to colour. That is what lets
//! the same definition serve three purposes: a destructive command, a
//! non-destructive adjustment layer, and the GPU shader — with the CPU version
//! here as the reference the GPU is tested against.
//!
//! # How they reach the GPU
//!
//! Adjustments split into two families:
//!
//! * Those that act on each channel independently — Levels, Curves,
//!   Brightness/Contrast, Exposure, Posterize, Threshold, Invert — are baked
//!   into a 256-entry lookup table. One texture, one fetch, and the shader
//!   never needs to know which adjustment it is applying.
//! * Those that need the whole RGB triple — Hue/Saturation, Vibrance, Colour
//!   Balance, Black & White, Channel Mixer, Photo Filter — get a branch in the
//!   shader and a handful of uniform parameters.
//!
//! Gradient Map is a third case: a table indexed by luma rather than per
//!   channel.

use crate::color::{hsv_to_rgb, linear_to_srgb, rgb_to_hsv, srgb_to_linear, Rgba, Rgba8};
use rayon::prelude::*;
use crate::curve::Curve;

/// Per-channel input/output mapping for Levels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelsChannel {
    /// Input value mapped to black, `0..=1`.
    pub input_black: f32,
    /// Input value mapped to white, `0..=1`.
    pub input_white: f32,
    /// Midtone exponent; `1.0` is neutral, above brightens.
    pub gamma: f32,
    pub output_black: f32,
    pub output_white: f32,
}

impl Default for LevelsChannel {
    fn default() -> Self {
        Self {
            input_black: 0.0,
            input_white: 1.0,
            gamma: 1.0,
            output_black: 0.0,
            output_white: 1.0,
        }
    }
}

impl LevelsChannel {
    pub fn is_identity(&self) -> bool {
        self.input_black == 0.0
            && self.input_white == 1.0
            && (self.gamma - 1.0).abs() < 1e-6
            && self.output_black == 0.0
            && self.output_white == 1.0
    }

    pub fn apply(&self, v: f32) -> f32 {
        let span = (self.input_white - self.input_black).max(1e-4);
        let t = ((v - self.input_black) / span).clamp(0.0, 1.0);
        // The midtone slider is an exponent of 1/gamma.
        let t = if (self.gamma - 1.0).abs() < 1e-6 {
            t
        } else {
            t.powf(1.0 / self.gamma.clamp(0.01, 9.99))
        };
        (self.output_black + t * (self.output_white - self.output_black)).clamp(0.0, 1.0)
    }
}

/// A stop in a gradient map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    pub position: f32,
    pub color: Rgba8,
}

/// One colour adjustment.
#[derive(Debug, Clone, PartialEq)]
pub enum Adjustment {
    /// `brightness` and `contrast` both run `-1..=1`, `0` being neutral.
    BrightnessContrast { brightness: f32, contrast: f32 },
    /// Composite first, then the per-channel maps, which is the usual order.
    Levels { rgb: LevelsChannel, channels: [LevelsChannel; 3] },
    /// Index 0 is the composite curve; 1, 2 and 3 are red, green and blue.
    Curves { curves: [Curve; 4] },
    /// Stops in linear light, so `exposure` behaves like camera stops.
    Exposure { exposure: f32, offset: f32, gamma: f32 },
    /// `vibrance` favours less-saturated colours; `saturation` is uniform.
    Vibrance { vibrance: f32, saturation: f32 },
    /// `hue` in turns (`-0.5..=0.5`), the rest `-1..=1`.
    HueSaturation { hue: f32, saturation: f32, lightness: f32, colorize: bool },
    ColorBalance {
        shadows: [f32; 3],
        midtones: [f32; 3],
        highlights: [f32; 3],
        preserve_luminosity: bool,
    },
    /// Weights for reds, yellows, greens, cyans, blues and magentas.
    BlackAndWhite { weights: [f32; 6], tint: Option<Rgba8> },
    /// Rows are the output channels; each row is `[r, g, b, constant]`.
    ChannelMixer { matrix: [[f32; 4]; 3], monochrome: bool },
    PhotoFilter { color: Rgba8, density: f32, preserve_luminosity: bool },
    Invert,
    /// Number of levels per channel, `2..=255`.
    Posterize { levels: u32 },
    Threshold { level: f32 },
    GradientMap { stops: Vec<GradientStop> },
}

/// Which shader path an adjustment takes. The numbering is a contract with
/// `composite.wgsl`; never renumber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum AdjustKind {
    /// Per-channel lookup table.
    Lut = 0,
    /// Table indexed by luma.
    GradientMap = 1,
    HueSaturation = 2,
    Vibrance = 3,
    ColorBalance = 4,
    BlackAndWhite = 5,
    ChannelMixer = 6,
    PhotoFilter = 7,
}

impl Adjustment {
    /// Menu name.
    pub fn name(&self) -> &'static str {
        match self {
            Adjustment::BrightnessContrast { .. } => "Brightness/Contrast",
            Adjustment::Levels { .. } => "Levels",
            Adjustment::Curves { .. } => "Curves",
            Adjustment::Exposure { .. } => "Exposure",
            Adjustment::Vibrance { .. } => "Vibrance",
            Adjustment::HueSaturation { .. } => "Hue/Saturation",
            Adjustment::ColorBalance { .. } => "Color Balance",
            Adjustment::BlackAndWhite { .. } => "Black & White",
            Adjustment::ChannelMixer { .. } => "Channel Mixer",
            Adjustment::PhotoFilter { .. } => "Photo Filter",
            Adjustment::Invert => "Invert",
            Adjustment::Posterize { .. } => "Posterize",
            Adjustment::Threshold { .. } => "Threshold",
            Adjustment::GradientMap { .. } => "Gradient Map",
        }
    }

    /// Every adjustment at its neutral settings, for building menus.
    pub fn all_defaults() -> Vec<Adjustment> {
        vec![
            Adjustment::BrightnessContrast { brightness: 0.0, contrast: 0.0 },
            Adjustment::Levels {
                rgb: LevelsChannel::default(),
                channels: [LevelsChannel::default(); 3],
            },
            Adjustment::Curves { curves: Default::default() },
            Adjustment::Exposure { exposure: 0.0, offset: 0.0, gamma: 1.0 },
            Adjustment::Vibrance { vibrance: 0.0, saturation: 0.0 },
            Adjustment::HueSaturation {
                hue: 0.0,
                saturation: 0.0,
                lightness: 0.0,
                colorize: false,
            },
            Adjustment::ColorBalance {
                shadows: [0.0; 3],
                midtones: [0.0; 3],
                highlights: [0.0; 3],
                preserve_luminosity: true,
            },
            Adjustment::BlackAndWhite {
                weights: [0.4, 0.6, 0.4, 0.6, 0.2, 0.8],
                tint: None,
            },
            Adjustment::ChannelMixer {
                matrix: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
                monochrome: false,
            },
            Adjustment::PhotoFilter {
                color: Rgba8::opaque(236, 138, 0),
                density: 0.25,
                preserve_luminosity: true,
            },
            Adjustment::Invert,
            Adjustment::Posterize { levels: 4 },
            Adjustment::Threshold { level: 0.5 },
            Adjustment::GradientMap {
                stops: vec![
                    GradientStop { position: 0.0, color: Rgba8::BLACK },
                    GradientStop { position: 1.0, color: Rgba8::WHITE },
                ],
            },
        ]
    }

    /// Whether the adjustment has anything to configure.
    ///
    /// All but one do, and most of them are neutral at their defaults — a
    /// Curves with the identity curve changes nothing — so applying one
    /// without asking the user for settings first would be a no-op.
    pub fn has_settings(&self) -> bool {
        !matches!(self, Adjustment::Invert)
    }

    pub fn kind(&self) -> AdjustKind {
        match self {
            Adjustment::BrightnessContrast { .. }
            | Adjustment::Levels { .. }
            | Adjustment::Curves { .. }
            | Adjustment::Exposure { .. }
            | Adjustment::Invert
            | Adjustment::Posterize { .. }
            | Adjustment::Threshold { .. } => AdjustKind::Lut,
            Adjustment::GradientMap { .. } => AdjustKind::GradientMap,
            Adjustment::HueSaturation { .. } => AdjustKind::HueSaturation,
            Adjustment::Vibrance { .. } => AdjustKind::Vibrance,
            Adjustment::ColorBalance { .. } => AdjustKind::ColorBalance,
            Adjustment::BlackAndWhite { .. } => AdjustKind::BlackAndWhite,
            Adjustment::ChannelMixer { .. } => AdjustKind::ChannelMixer,
            Adjustment::PhotoFilter { .. } => AdjustKind::PhotoFilter,
        }
    }

    /// Bake the lookup table once, so it can be applied to many colours.
    ///
    /// Prefer this to [`Adjustment::apply`] for anything bigger than a single
    /// colour: the table-driven adjustments have to build a 256-entry table
    /// before they can read one entry from it, and doing that per pixel is
    /// what it sounds like.
    pub fn prepare(&self) -> Prepared<'_> {
        let lut = matches!(self.kind(), AdjustKind::Lut | AdjustKind::GradientMap)
            .then(|| self.bake_lut());
        Prepared { adjustment: self, lut }
    }

    /// Apply to a straight-alpha, sRGB-encoded colour. Alpha is never changed:
    /// an adjustment alters colour, not coverage.
    ///
    /// Bakes the lookup table on every call. For more than one colour, use
    /// [`Adjustment::prepare`].
    pub fn apply(&self, c: Rgba) -> Rgba {
        self.prepare().apply(c)
    }

    /// Bakes the lookup table on every call — see [`Adjustment::apply`].
    pub fn apply_rgb(&self, c: [f32; 3]) -> [f32; 3] {
        self.prepare().apply_rgb(c)
    }
}

/// An [`Adjustment`] with its lookup table already baked.
///
/// Holds a borrow of the adjustment rather than a copy so preparing is cheap
/// for the formula-driven adjustments, which have no table at all.
pub struct Prepared<'a> {
    adjustment: &'a Adjustment,
    /// Present for the two table-driven families; `None` for the ones that
    /// need the whole RGB triple and so cannot be tabulated per channel.
    lut: Option<[u8; 1024]>,
}

impl Prepared<'_> {
    /// Apply to a straight-alpha, sRGB-encoded colour. Alpha is untouched.
    pub fn apply(&self, c: Rgba) -> Rgba {
        let rgb = self.apply_rgb([c.r, c.g, c.b]);
        Rgba::new(rgb[0], rgb[1], rgb[2], c.a)
    }

    /// Apply across a whole buffer, in parallel.
    ///
    /// The table-driven adjustments never leave 8-bit here: the input byte is
    /// the table index, so the round trip through `f32` buys nothing.
    pub fn apply_buffer(&self, buffer: &mut [Rgba8]) {
        match &self.lut {
            Some(lut) if self.adjustment.kind() == AdjustKind::Lut => {
                buffer.par_iter_mut().for_each(|px| {
                    px.r = lut[px.r as usize * 4];
                    px.g = lut[px.g as usize * 4 + 1];
                    px.b = lut[px.b as usize * 4 + 2];
                });
            }
            _ => buffer.par_iter_mut().for_each(|px| {
                *px = self.apply(px.to_f32()).to_u8();
            }),
        }
    }

    pub fn apply_rgb(&self, c: [f32; 3]) -> [f32; 3] {
        match self.adjustment {
            // Everything table-driven shares one path, which also guarantees
            // the CPU and GPU agree: both read the same table.
            Adjustment::BrightnessContrast { .. }
            | Adjustment::Levels { .. }
            | Adjustment::Curves { .. }
            | Adjustment::Exposure { .. }
            | Adjustment::Invert
            | Adjustment::Posterize { .. }
            | Adjustment::Threshold { .. } => {
                let lut = self.lut.as_ref().expect("table-driven adjustment was prepared");
                let f = |v: f32, ch: usize| {
                    let i = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as usize;
                    lut[i * 4 + ch] as f32 / 255.0
                };
                [f(c[0], 0), f(c[1], 1), f(c[2], 2)]
            }

            Adjustment::GradientMap { .. } => {
                let lut = self.lut.as_ref().expect("gradient map was prepared");
                let luma = 0.30 * c[0] + 0.59 * c[1] + 0.11 * c[2];
                let i = (luma.clamp(0.0, 1.0) * 255.0 + 0.5) as usize;
                [
                    lut[i * 4] as f32 / 255.0,
                    lut[i * 4 + 1] as f32 / 255.0,
                    lut[i * 4 + 2] as f32 / 255.0,
                ]
            }

            Adjustment::HueSaturation { hue, saturation, lightness, colorize } => {
                hue_saturation(c, *hue, *saturation, *lightness, *colorize)
            }
            Adjustment::Vibrance { vibrance, saturation } => {
                vibrance_adjust(c, *vibrance, *saturation)
            }
            Adjustment::ColorBalance {
                shadows,
                midtones,
                highlights,
                preserve_luminosity,
            } => color_balance(c, *shadows, *midtones, *highlights, *preserve_luminosity),
            Adjustment::BlackAndWhite { weights, tint } => black_and_white(c, weights, *tint),
            Adjustment::ChannelMixer { matrix, monochrome } => {
                channel_mixer(c, matrix, *monochrome)
            }
            Adjustment::PhotoFilter { color, density, preserve_luminosity } => {
                photo_filter(c, *color, *density, *preserve_luminosity)
            }
        }
    }
}

impl Adjustment {
    /// `true` when the adjustment would leave every colour unchanged, so the
    /// compositor can skip its pass entirely.
    pub fn is_identity(&self) -> bool {
        match self {
            Adjustment::BrightnessContrast { brightness, contrast } => {
                brightness.abs() < 1e-6 && contrast.abs() < 1e-6
            }
            Adjustment::Levels { rgb, channels } => {
                rgb.is_identity() && channels.iter().all(|c| c.is_identity())
            }
            Adjustment::Curves { curves } => curves.iter().all(|c| c.is_identity()),
            Adjustment::Exposure { exposure, offset, gamma } => {
                exposure.abs() < 1e-6 && offset.abs() < 1e-6 && (gamma - 1.0).abs() < 1e-6
            }
            Adjustment::Vibrance { vibrance, saturation } => {
                vibrance.abs() < 1e-6 && saturation.abs() < 1e-6
            }
            Adjustment::HueSaturation { hue, saturation, lightness, colorize } => {
                !colorize && hue.abs() < 1e-6 && saturation.abs() < 1e-6 && lightness.abs() < 1e-6
            }
            Adjustment::ColorBalance { shadows, midtones, highlights, .. } => shadows
                .iter()
                .chain(midtones)
                .chain(highlights)
                .all(|v| v.abs() < 1e-6),
            Adjustment::ChannelMixer { matrix, monochrome } => {
                !monochrome
                    && matrix[0] == [1.0, 0.0, 0.0, 0.0]
                    && matrix[1] == [0.0, 1.0, 0.0, 0.0]
                    && matrix[2] == [0.0, 0.0, 1.0, 0.0]
            }
            Adjustment::PhotoFilter { density, .. } => density.abs() < 1e-6,
            // The rest always change something.
            _ => false,
        }
    }

    /// Bake the adjustment into a 256-entry RGBA table.
    ///
    /// For [`AdjustKind::Lut`] each entry holds that input level mapped through
    /// the red, green and blue channels. For [`AdjustKind::GradientMap`] the
    /// index is luma and the entry is the resulting colour. Returns an identity
    /// ramp for adjustments that use a shader formula instead.
    pub fn bake_lut(&self) -> [u8; 1024] {
        let mut lut = [0u8; 1024];
        for i in 0..256 {
            let v = i as f32 / 255.0;
            let out = match self {
                Adjustment::BrightnessContrast { brightness, contrast } => {
                    let c = brightness_contrast(v, *brightness, *contrast);
                    [c, c, c]
                }
                Adjustment::Levels { rgb, channels } => {
                    // Composite first, then per channel, as the composite map is applied
                    // before the per-channel ones.
                    let base = rgb.apply(v);
                    [
                        channels[0].apply(base),
                        channels[1].apply(base),
                        channels[2].apply(base),
                    ]
                }
                Adjustment::Curves { curves } => {
                    let base = curves[0].eval(v);
                    [curves[1].eval(base), curves[2].eval(base), curves[3].eval(base)]
                }
                Adjustment::Exposure { exposure, offset, gamma } => {
                    let c = exposure_adjust(v, *exposure, *offset, *gamma);
                    [c, c, c]
                }
                Adjustment::Invert => [1.0 - v; 3],
                Adjustment::Posterize { levels } => {
                    let n = (*levels).clamp(2, 255) as f32;
                    // Quantise to `levels` evenly spaced output values.
                    let c = ((v * n).floor().min(n - 1.0)) / (n - 1.0);
                    [c.clamp(0.0, 1.0); 3]
                }
                Adjustment::Threshold { level } => {
                    let c = if v >= *level { 1.0 } else { 0.0 };
                    [c; 3]
                }
                Adjustment::GradientMap { stops } => gradient_at(stops, v),
                // Formula-based adjustments get an identity ramp so a stale
                // table can never tint the image.
                _ => [v; 3],
            };
            lut[i * 4] = (out[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            lut[i * 4 + 1] = (out[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            lut[i * 4 + 2] = (out[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            lut[i * 4 + 3] = 255;
        }
        lut
    }

    /// Uniform parameters for the shader's formula branch.
    pub fn gpu_params(&self) -> [[f32; 4]; 4] {
        let mut p = [[0.0f32; 4]; 4];
        match self {
            Adjustment::HueSaturation { hue, saturation, lightness, colorize } => {
                p[0] = [*hue, *saturation, *lightness, if *colorize { 1.0 } else { 0.0 }];
            }
            Adjustment::Vibrance { vibrance, saturation } => {
                p[0] = [*vibrance, *saturation, 0.0, 0.0];
            }
            Adjustment::ColorBalance {
                shadows,
                midtones,
                highlights,
                preserve_luminosity,
            } => {
                p[0] = [shadows[0], shadows[1], shadows[2], 0.0];
                p[1] = [midtones[0], midtones[1], midtones[2], 0.0];
                p[2] = [highlights[0], highlights[1], highlights[2], 0.0];
                p[3] = [if *preserve_luminosity { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
            }
            Adjustment::BlackAndWhite { weights, tint } => {
                p[0] = [weights[0], weights[1], weights[2], weights[3]];
                p[1] = [weights[4], weights[5], 0.0, 0.0];
                match tint {
                    Some(c) => {
                        let t = c.to_f32();
                        p[2] = [t.r, t.g, t.b, 1.0];
                    }
                    None => p[2] = [0.0, 0.0, 0.0, 0.0],
                }
            }
            Adjustment::ChannelMixer { matrix, monochrome } => {
                p[0] = matrix[0];
                p[1] = matrix[1];
                p[2] = matrix[2];
                p[3] = [if *monochrome { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
            }
            Adjustment::PhotoFilter { color, density, preserve_luminosity } => {
                let c = color.to_f32();
                p[0] = [c.r, c.g, c.b, *density];
                p[1] = [if *preserve_luminosity { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
            }
            _ => {}
        }
        p
    }
}

// ---------------------------------------------------------------------------
// Individual adjustments
// ---------------------------------------------------------------------------

/// Brightness as an offset, contrast as a slope about mid-grey.
///
/// The slope comes from `tan`, which gives a smooth, symmetric response over
/// the whole slider range: `-1` collapses to flat grey and `+1` approaches a
/// hard threshold, with no discontinuity in between.
fn brightness_contrast(v: f32, brightness: f32, contrast: f32) -> f32 {
    let v = v + brightness * 0.5;
    let slope = ((contrast.clamp(-0.999, 0.999) + 1.0) * std::f32::consts::FRAC_PI_4).tan();
    ((v - 0.5) * slope + 0.5).clamp(0.0, 1.0)
}

/// Exposure in stops.
///
/// Doubling the light means doubling *linear* intensity, so this is one of the
/// few adjustments that has to leave the document's gamma-encoded space, do its
/// work, and come back.
fn exposure_adjust(v: f32, exposure: f32, offset: f32, gamma: f32) -> f32 {
    let linear = srgb_to_linear(v.clamp(0.0, 1.0));
    let scaled = linear * 2f32.powf(exposure) + offset;
    let gamma = gamma.clamp(0.01, 9.99);
    let corrected = if (gamma - 1.0).abs() < 1e-6 {
        scaled
    } else {
        scaled.max(0.0).powf(1.0 / gamma)
    };
    linear_to_srgb(corrected.clamp(0.0, 1.0))
}

/// Colour at `t` along a gradient.
fn gradient_at(stops: &[GradientStop], t: f32) -> [f32; 3] {
    if stops.is_empty() {
        return [t; 3];
    }
    let t = t.clamp(0.0, 1.0);
    let mut sorted: Vec<&GradientStop> = stops.iter().collect();
    sorted.sort_by(|a, b| a.position.total_cmp(&b.position));

    if t <= sorted[0].position {
        let c = sorted[0].color.to_f32();
        return [c.r, c.g, c.b];
    }
    if t >= sorted[sorted.len() - 1].position {
        let c = sorted[sorted.len() - 1].color.to_f32();
        return [c.r, c.g, c.b];
    }
    for w in sorted.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t >= a.position && t <= b.position {
            let span = (b.position - a.position).max(1e-6);
            let f = (t - a.position) / span;
            let (ca, cb) = (a.color.to_f32(), b.color.to_f32());
            return [
                ca.r + (cb.r - ca.r) * f,
                ca.g + (cb.g - ca.g) * f,
                ca.b + (cb.b - ca.b) * f,
            ];
        }
    }
    [t; 3]
}

fn hue_saturation(
    c: [f32; 3],
    hue: f32,
    saturation: f32,
    lightness: f32,
    colorize: bool,
) -> [f32; 3] {
    let (h, s, v) = rgb_to_hsv(c[0], c[1], c[2]);
    let (h, s) = if colorize {
        // Colorize replaces the hue outright and drives saturation from the
        // slider rather than scaling what was there. Clamp *after* halving:
        // clamping first pins the slider's top half to a single value.
        (hue.rem_euclid(1.0), ((saturation + 1.0) * 0.5).clamp(0.0, 1.0))
    } else {
        (h + hue, (s * (1.0 + saturation)).clamp(0.0, 1.0))
    };

    let (r, g, b) = hsv_to_rgb(h, s, v);
    // Lightness lifts toward white or drops toward black, as the
    // slider does, rather than scaling the value.
    let out = if lightness >= 0.0 {
        let t = lightness.clamp(0.0, 1.0);
        [r + (1.0 - r) * t, g + (1.0 - g) * t, b + (1.0 - b) * t]
    } else {
        let t = (1.0 + lightness).clamp(0.0, 1.0);
        [r * t, g * t, b * t]
    };
    [out[0].clamp(0.0, 1.0), out[1].clamp(0.0, 1.0), out[2].clamp(0.0, 1.0)]
}

/// Saturation weighted toward the colours that have least of it.
fn vibrance_adjust(c: [f32; 3], vibrance: f32, saturation: f32) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let sat = max - min;

    // Already-saturated colours get proportionally less, which is what stops
    // skin tones going orange when the slider is pushed.
    let amount = saturation + vibrance * (1.0 - sat);
    let luma = 0.30 * c[0] + 0.59 * c[1] + 0.11 * c[2];
    let f = 1.0 + amount;
    [
        (luma + (c[0] - luma) * f).clamp(0.0, 1.0),
        (luma + (c[1] - luma) * f).clamp(0.0, 1.0),
        (luma + (c[2] - luma) * f).clamp(0.0, 1.0),
    ]
}

fn color_balance(
    c: [f32; 3],
    shadows: [f32; 3],
    midtones: [f32; 3],
    highlights: [f32; 3],
    preserve_luminosity: bool,
) -> [f32; 3] {
    let luma_before = 0.30 * c[0] + 0.59 * c[1] + 0.11 * c[2];
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        let v = c[i];
        // Overlapping weights so a shift in one range fades into the next
        // instead of banding at the boundaries.
        let w_shadow = (1.0 - v * 2.0).clamp(0.0, 1.0);
        let w_high = ((v - 0.5) * 2.0).clamp(0.0, 1.0);
        let w_mid = (1.0 - w_shadow - w_high).clamp(0.0, 1.0);

        let shift =
            shadows[i] * w_shadow + midtones[i] * w_mid + highlights[i] * w_high;
        out[i] = (v + shift * 0.5).clamp(0.0, 1.0);
    }

    if preserve_luminosity {
        let luma_after = 0.30 * out[0] + 0.59 * out[1] + 0.11 * out[2];
        let delta = luma_before - luma_after;
        for v in &mut out {
            *v = (*v + delta).clamp(0.0, 1.0);
        }
    }
    out
}

/// Greyscale conversion weighted by where each pixel's hue falls.
fn black_and_white(c: [f32; 3], weights: &[f32; 6], tint: Option<Rgba8>) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let (h, _, _) = rgb_to_hsv(c[0], c[1], c[2]);

    // Blend between the two sliders either side of the hue, so the result is
    // continuous as a colour rotates through the wheel.
    let sector = h * 6.0;
    let i = sector.floor() as usize % 6;
    let f = sector - sector.floor();
    // The sliders are ordered red, yellow, green, cyan, blue, magenta, which is
    // the order hue passes through them.
    let weight = weights[i] * (1.0 - f) + weights[(i + 1) % 6] * f;

    let chroma = max - min;
    // Weight only the coloured part; the grey component passes through.
    let grey = (min + chroma * weight).clamp(0.0, 1.0);

    match tint {
        Some(t) => {
            let t = t.to_f32();
            // Multiply the tint by the grey level, keeping its own brightness.
            let scale = grey / 0.5_f32.max(1e-4);
            let _ = scale;
            [
                (t.r * grey * 2.0).clamp(0.0, 1.0),
                (t.g * grey * 2.0).clamp(0.0, 1.0),
                (t.b * grey * 2.0).clamp(0.0, 1.0),
            ]
        }
        None => [grey; 3],
    }
}

fn channel_mixer(c: [f32; 3], matrix: &[[f32; 4]; 3], monochrome: bool) -> [f32; 3] {
    let mix = |row: &[f32; 4]| {
        (c[0] * row[0] + c[1] * row[1] + c[2] * row[2] + row[3]).clamp(0.0, 1.0)
    };
    if monochrome {
        // The red row drives every output channel.
        [mix(&matrix[0]); 3]
    } else {
        [mix(&matrix[0]), mix(&matrix[1]), mix(&matrix[2])]
    }
}

fn photo_filter(
    c: [f32; 3],
    color: Rgba8,
    density: f32,
    preserve_luminosity: bool,
) -> [f32; 3] {
    let f = color.to_f32();
    let d = density.clamp(0.0, 1.0);
    let luma_before = 0.30 * c[0] + 0.59 * c[1] + 0.11 * c[2];

    // A filter absorbs light, so it multiplies rather than blends.
    let mut out = [
        c[0] * (1.0 - d + d * f.r * 2.0),
        c[1] * (1.0 - d + d * f.g * 2.0),
        c[2] * (1.0 - d + d * f.b * 2.0),
    ];
    for v in &mut out {
        *v = v.clamp(0.0, 1.0);
    }

    if preserve_luminosity {
        let luma_after = 0.30 * out[0] + 0.59 * out[1] + 0.11 * out[2];
        if luma_after > 1e-4 {
            let scale = luma_before / luma_after;
            for v in &mut out {
                *v = (*v * scale).clamp(0.0, 1.0);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Preparing must not change the result, only the cost of getting it.
    /// `apply` is defined in terms of `prepare`, so this mostly guards
    /// `apply_buffer`, which takes a separate 8-bit route for LUT kinds.
    #[test]
    fn the_prepared_path_matches_apply() {
        let mut cases = Adjustment::all_defaults();
        // Defaults are neutral for most adjustments, which would let a broken
        // fast path pass by doing nothing. Add settings that actually bite.
        let mut curves: [Curve; 4] = Default::default();
        curves[0] = Curve::new(vec![(0.0, 0.1), (0.4, 0.15), (0.75, 0.9), (1.0, 0.95)]);
        cases.push(Adjustment::Curves { curves });
        cases.push(Adjustment::Posterize { levels: 5 });
        cases.push(Adjustment::Threshold { level: 0.42 });
        cases.push(Adjustment::Invert);

        for adj in cases {
            let mut buffer: Vec<Rgba8> = (0..=255u8)
                .map(|v| Rgba8::new(v, 255 - v, v.wrapping_mul(7), v))
                .collect();
            let want: Vec<Rgba8> =
                buffer.iter().map(|px| adj.apply(px.to_f32()).to_u8()).collect();
            adj.prepare().apply_buffer(&mut buffer);
            assert_eq!(buffer, want, "{} disagrees when prepared", adj.name());
        }
    }


    const GREY: [f32; 3] = [0.5, 0.5, 0.5];

    fn close(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < eps)
    }

    #[test]
    fn neutral_settings_leave_colours_alone() {
        let samples = [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], GREY, [0.8, 0.2, 0.4]];
        for adj in Adjustment::all_defaults() {
            // Only the adjustments that report themselves neutral must be
            // exactly transparent; the rest are expected to change things.
            if !adj.is_identity() {
                continue;
            }
            for s in samples {
                assert!(
                    close(adj.apply_rgb(s), s, 1.0 / 255.0 + 1e-4),
                    "{} altered {s:?} into {:?}",
                    adj.name(),
                    adj.apply_rgb(s)
                );
            }
        }
    }

    #[test]
    fn every_adjustment_stays_in_gamut_and_finite() {
        let samples = [
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            GREY,
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.03, 0.97, 0.5],
        ];
        for adj in Adjustment::all_defaults() {
            for s in samples {
                let out = adj.apply_rgb(s);
                for v in out {
                    assert!(v.is_finite(), "{} produced a non-finite value", adj.name());
                    assert!(
                        (-1e-4..=1.0 + 1e-4).contains(&v),
                        "{} produced {v} from {s:?}",
                        adj.name()
                    );
                }
            }
        }
    }

    #[test]
    fn adjustments_never_touch_alpha() {
        for adj in Adjustment::all_defaults() {
            let c = Rgba::new(0.4, 0.6, 0.2, 0.37);
            assert_eq!(adj.apply(c).a, 0.37, "{} changed alpha", adj.name());
        }
    }

    #[test]
    fn invert_is_an_involution() {
        let adj = Adjustment::Invert;
        for s in [[0.0, 0.25, 1.0], GREY, [0.9, 0.1, 0.6]] {
            assert!(close(adj.apply_rgb(adj.apply_rgb(s)), s, 2.0 / 255.0));
        }
    }

    #[test]
    fn brightness_and_contrast_move_in_the_expected_direction() {
        let up = Adjustment::BrightnessContrast { brightness: 0.4, contrast: 0.0 };
        assert!(up.apply_rgb(GREY)[0] > 0.6);
        let down = Adjustment::BrightnessContrast { brightness: -0.4, contrast: 0.0 };
        assert!(down.apply_rgb(GREY)[0] < 0.4);

        // Contrast pivots about mid-grey, so grey itself does not move.
        let more = Adjustment::BrightnessContrast { brightness: 0.0, contrast: 0.5 };
        assert!((more.apply_rgb(GREY)[0] - 0.5).abs() < 2.0 / 255.0);
        assert!(more.apply_rgb([0.7, 0.7, 0.7])[0] > 0.7, "highlights get brighter");
        assert!(more.apply_rgb([0.3, 0.3, 0.3])[0] < 0.3, "shadows get darker");

        // Full negative contrast collapses everything to mid-grey.
        let flat = Adjustment::BrightnessContrast { brightness: 0.0, contrast: -1.0 };
        assert!(close(flat.apply_rgb([0.9, 0.1, 0.5]), GREY, 3.0 / 255.0));
    }

    #[test]
    fn levels_remap_the_input_range() {
        let rgb = LevelsChannel { input_black: 0.25, input_white: 0.75, ..Default::default() };
        let adj = Adjustment::Levels { rgb, channels: [LevelsChannel::default(); 3] };

        assert!(adj.apply_rgb([0.25; 3])[0] < 2.0 / 255.0, "the black point maps to black");
        assert!(adj.apply_rgb([0.75; 3])[0] > 253.0 / 255.0, "the white point maps to white");
        assert!(close(adj.apply_rgb(GREY), GREY, 2.0 / 255.0), "the midpoint stays put");
        assert_eq!(adj.apply_rgb([0.1; 3])[0], 0.0, "below the black point clips");
    }

    #[test]
    fn levels_gamma_brightens_midtones_only() {
        let rgb = LevelsChannel { gamma: 2.0, ..Default::default() };
        let adj = Adjustment::Levels { rgb, channels: [LevelsChannel::default(); 3] };
        assert!(adj.apply_rgb(GREY)[0] > 0.65, "midtones lift");
        assert!(adj.apply_rgb([0.0; 3])[0] < 1.0 / 255.0, "black stays black");
        assert!(adj.apply_rgb([1.0; 3])[0] > 254.0 / 255.0, "white stays white");
    }

    #[test]
    fn a_per_channel_level_tints_the_image() {
        let mut channels = [LevelsChannel::default(); 3];
        channels[0].output_white = 0.5;
        let adj = Adjustment::Levels { rgb: LevelsChannel::default(), channels };
        let out = adj.apply_rgb([1.0, 1.0, 1.0]);
        assert!((out[0] - 0.5).abs() < 2.0 / 255.0, "red is halved");
        assert!(out[1] > 0.99 && out[2] > 0.99, "green and blue are untouched");
    }

    #[test]
    fn curves_apply_the_composite_then_the_channels() {
        let mut curves: [Curve; 4] = Default::default();
        curves[0] = Curve::new(vec![(0.0, 0.0), (0.5, 0.75), (1.0, 1.0)]);
        let adj = Adjustment::Curves { curves };
        assert!((adj.apply_rgb(GREY)[0] - 0.75).abs() < 2.0 / 255.0);

        let mut curves: [Curve; 4] = Default::default();
        curves[1] = Curve::new(vec![(0.0, 0.0), (1.0, 0.5)]);
        let adj = Adjustment::Curves { curves };
        let out = adj.apply_rgb([1.0, 1.0, 1.0]);
        assert!((out[0] - 0.5).abs() < 2.0 / 255.0, "only red is halved");
        assert!(out[1] > 0.99);
    }

    #[test]
    fn exposure_doubles_linear_intensity_per_stop() {
        let adj = Adjustment::Exposure { exposure: 1.0, offset: 0.0, gamma: 1.0 };
        let out = adj.apply_rgb(GREY)[0];
        // One stop doubles the *linear* value, not the encoded one.
        let expected = linear_to_srgb((srgb_to_linear(0.5) * 2.0).min(1.0));
        assert!((out - expected).abs() < 2.0 / 255.0, "got {out}, expected {expected}");
        // Doubling linear light is well under doubling the encoded value:
        // mid-grey goes to about 0.69, not 1.0.
        assert!(out > 0.65 && out < 0.72, "one stop from mid-grey should land near 0.69, got {out}");

        let down = Adjustment::Exposure { exposure: -1.0, offset: 0.0, gamma: 1.0 };
        assert!(down.apply_rgb(GREY)[0] < 0.4);
    }

    #[test]
    fn posterize_quantises_to_the_requested_levels() {
        let adj = Adjustment::Posterize { levels: 2 };
        let mut seen: Vec<u8> = (0..=255)
            .map(|i| (adj.apply_rgb([i as f32 / 255.0; 3])[0] * 255.0).round() as u8)
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen, vec![0, 255], "two levels means black and white only");

        let adj = Adjustment::Posterize { levels: 4 };
        let mut seen: Vec<u8> = (0..=255)
            .map(|i| (adj.apply_rgb([i as f32 / 255.0; 3])[0] * 255.0).round() as u8)
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn threshold_produces_only_black_and_white() {
        let adj = Adjustment::Threshold { level: 0.5 };
        assert_eq!(adj.apply_rgb([0.49; 3]), [0.0; 3]);
        assert_eq!(adj.apply_rgb([0.51; 3]), [1.0; 3]);
    }

    #[test]
    fn a_gradient_map_replaces_colour_by_luma() {
        let adj = Adjustment::GradientMap {
            stops: vec![
                GradientStop { position: 0.0, color: Rgba8::opaque(0, 0, 255) },
                GradientStop { position: 1.0, color: Rgba8::opaque(255, 255, 0) },
            ],
        };
        // Black maps to the first stop, white to the last.
        assert!(close(adj.apply_rgb([0.0; 3]), [0.0, 0.0, 1.0], 2.0 / 255.0));
        assert!(close(adj.apply_rgb([1.0; 3]), [1.0, 1.0, 0.0], 2.0 / 255.0));
        // Two colours with the same luma map to the same output.
        let a = adj.apply_rgb([0.59, 0.0, 0.0]);
        let b = adj.apply_rgb([0.0, 0.3, 0.0]);
        assert!(close(a, b, 0.15), "similar luma should give similar output");
    }

    #[test]
    fn hue_rotation_moves_around_the_wheel() {
        let adj = Adjustment::HueSaturation {
            hue: 1.0 / 3.0,
            saturation: 0.0,
            lightness: 0.0,
            colorize: false,
        };
        // Red rotated by a third of a turn is green.
        let out = adj.apply_rgb([1.0, 0.0, 0.0]);
        assert!(out[1] > 0.9 && out[0] < 0.1, "expected green, got {out:?}");
    }

    #[test]
    fn colorize_maps_the_saturation_slider_across_its_whole_range() {
        // Clamping before halving would flatten everything above zero to the
        // same saturation.
        let at = |sat: f32| {
            let adj = Adjustment::HueSaturation {
                hue: 0.0,
                saturation: sat,
                lightness: 0.0,
                colorize: true,
            };
            let o = adj.apply_rgb([1.0, 1.0, 1.0]);
            o[0].max(o[1]).max(o[2]) - o[0].min(o[1]).min(o[2])
        };
        assert!(at(0.5) > at(0.0) + 0.05, "the upper half of the slider must still do something");
        assert!(at(1.0) > at(0.5) + 0.05);
    }

    #[test]
    fn saturation_extremes_grey_out_and_intensify() {
        let down = Adjustment::HueSaturation {
            hue: 0.0,
            saturation: -1.0,
            lightness: 0.0,
            colorize: false,
        };
        let out = down.apply_rgb([0.9, 0.3, 0.2]);
        assert!(
            (out[0] - out[1]).abs() < 1e-3 && (out[1] - out[2]).abs() < 1e-3,
            "fully desaturated should be grey, got {out:?}"
        );

        let up = Adjustment::HueSaturation {
            hue: 0.0,
            saturation: 1.0,
            lightness: 0.0,
            colorize: false,
        };
        let out = up.apply_rgb([0.6, 0.5, 0.5]);
        assert!(out[0] - out[1] > 0.1, "saturation should widen the spread");
    }

    #[test]
    fn lightness_lifts_toward_white_and_drops_toward_black() {
        let up = Adjustment::HueSaturation {
            hue: 0.0,
            saturation: 0.0,
            lightness: 1.0,
            colorize: false,
        };
        assert!(close(up.apply_rgb([0.2, 0.4, 0.6]), [1.0; 3], 1e-3));

        let down = Adjustment::HueSaturation {
            hue: 0.0,
            saturation: 0.0,
            lightness: -1.0,
            colorize: false,
        };
        assert!(close(down.apply_rgb([0.2, 0.4, 0.6]), [0.0; 3], 1e-3));
    }

    #[test]
    fn vibrance_favours_the_least_saturated_colours() {
        let adj = Adjustment::Vibrance { vibrance: 0.6, saturation: 0.0 };
        let spread = |c: [f32; 3]| {
            let o = adj.apply_rgb(c);
            (o[0].max(o[1]).max(o[2]) - o[0].min(o[1]).min(o[2]))
                - (c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]))
        };
        let muted = spread([0.55, 0.5, 0.45]);
        let vivid = spread([1.0, 0.1, 0.05]);
        assert!(muted > 0.0, "a muted colour should gain saturation");
        assert!(muted > vivid, "a vivid colour should gain less than a muted one");
    }

    #[test]
    fn black_and_white_produces_grey_and_respects_its_sliders() {
        let adj = Adjustment::BlackAndWhite { weights: [0.4, 0.6, 0.4, 0.6, 0.2, 0.8], tint: None };
        let out = adj.apply_rgb([0.9, 0.2, 0.2]);
        assert!(
            (out[0] - out[1]).abs() < 1e-4 && (out[1] - out[2]).abs() < 1e-4,
            "output must be neutral, got {out:?}"
        );

        // Raising the red slider must brighten a red subject.
        let bright = Adjustment::BlackAndWhite { weights: [1.5, 0.6, 0.4, 0.6, 0.2, 0.8], tint: None };
        assert!(bright.apply_rgb([0.9, 0.2, 0.2])[0] > out[0]);
    }

    #[test]
    fn channel_mixer_swaps_channels() {
        let adj = Adjustment::ChannelMixer {
            matrix: [[0.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
            monochrome: false,
        };
        assert!(close(adj.apply_rgb([1.0, 0.0, 0.5]), [0.0, 1.0, 0.5], 1e-5));
    }

    #[test]
    fn channel_mixer_monochrome_drives_every_channel_from_one_row() {
        let adj = Adjustment::ChannelMixer {
            matrix: [[0.4, 0.4, 0.2, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
            monochrome: true,
        };
        let out = adj.apply_rgb([1.0, 0.5, 0.0]);
        assert!(close(out, [out[0]; 3], 1e-6), "monochrome output must be neutral");
    }

    #[test]
    fn color_balance_shifts_and_can_preserve_luminosity() {
        let adj = Adjustment::ColorBalance {
            shadows: [0.0; 3],
            midtones: [0.5, 0.0, -0.5],
            highlights: [0.0; 3],
            preserve_luminosity: false,
        };
        let out = adj.apply_rgb(GREY);
        assert!(out[0] > 0.5 && out[2] < 0.5, "should warm the midtones, got {out:?}");

        let preserving = Adjustment::ColorBalance {
            shadows: [0.0; 3],
            midtones: [0.5, 0.0, -0.5],
            highlights: [0.0; 3],
            preserve_luminosity: true,
        };
        let before = 0.30 * 0.5 + 0.59 * 0.5 + 0.11 * 0.5;
        let o = preserving.apply_rgb(GREY);
        let after = 0.30 * o[0] + 0.59 * o[1] + 0.11 * o[2];
        assert!((before - after).abs() < 0.02, "luminosity drifted: {before} -> {after}");
    }

    #[test]
    fn a_photo_filter_tints_toward_its_colour() {
        let adj = Adjustment::PhotoFilter {
            color: Rgba8::opaque(255, 100, 0),
            density: 0.8,
            preserve_luminosity: false,
        };
        let out = adj.apply_rgb(GREY);
        assert!(out[0] > out[2], "a warm filter should leave more red than blue");
    }

    #[test]
    fn adjustments_that_are_neutral_by_default_all_offer_settings() {
        // Otherwise the menu would apply them and appear to do nothing.
        for adjustment in Adjustment::all_defaults() {
            if adjustment.is_identity() {
                assert!(
                    adjustment.has_settings(),
                    "{} is neutral by default but has nothing to configure",
                    adjustment.name()
                );
            }
        }
    }

    #[test]
    fn the_only_adjustment_without_settings_does_something_on_its_own() {
        for adjustment in Adjustment::all_defaults() {
            if !adjustment.has_settings() {
                assert!(
                    !adjustment.is_identity(),
                    "{} has no settings and no effect",
                    adjustment.name()
                );
            }
        }
    }

    #[test]
    fn identity_detection_matches_actual_behaviour() {
        // A mismatch here means the compositor either skips a pass it should
        // run, or runs one it could skip.
        let cases: Vec<(Adjustment, bool)> = vec![
            (Adjustment::BrightnessContrast { brightness: 0.0, contrast: 0.0 }, true),
            (Adjustment::BrightnessContrast { brightness: 0.1, contrast: 0.0 }, false),
            (Adjustment::Curves { curves: Default::default() }, true),
            (Adjustment::Invert, false),
            (
                Adjustment::HueSaturation {
                    hue: 0.0,
                    saturation: 0.0,
                    lightness: 0.0,
                    colorize: true,
                },
                false,
            ),
        ];
        for (adj, expected) in cases {
            assert_eq!(adj.is_identity(), expected, "{}", adj.name());
            if expected {
                let s = [0.3, 0.6, 0.9];
                assert!(close(adj.apply_rgb(s), s, 1.0 / 255.0 + 1e-4));
            }
        }
    }

    #[test]
    fn the_baked_table_agrees_with_direct_application() {
        // The GPU reads the table; the CPU reference reads it too, so this is
        // really checking that the table is built from the same maths.
        let adj = Adjustment::Levels {
            rgb: LevelsChannel { input_black: 0.1, input_white: 0.9, gamma: 1.4, ..Default::default() },
            channels: [LevelsChannel::default(); 3],
        };
        let lut = adj.bake_lut();
        for i in [0usize, 40, 128, 200, 255] {
            let v = i as f32 / 255.0;
            let expected = adj.apply_rgb([v; 3])[0];
            let from_lut = lut[i * 4] as f32 / 255.0;
            assert!((expected - from_lut).abs() < 1e-6, "mismatch at {i}");
        }
    }

    #[test]
    fn formula_adjustments_bake_an_identity_table() {
        // A stale table must never tint the image if a formula adjustment ever
        // takes the table path by mistake.
        for adj in Adjustment::all_defaults() {
            if adj.kind() == AdjustKind::Lut || adj.kind() == AdjustKind::GradientMap {
                continue;
            }
            let lut = adj.bake_lut();
            for i in [0usize, 77, 255] {
                assert_eq!(lut[i * 4], i as u8, "{} baked a non-identity table", adj.name());
            }
        }
    }

    #[test]
    fn gpu_kinds_are_stable() {
        // The shader switches on these numbers.
        assert_eq!(AdjustKind::Lut as u32, 0);
        assert_eq!(AdjustKind::GradientMap as u32, 1);
        assert_eq!(AdjustKind::HueSaturation as u32, 2);
        assert_eq!(AdjustKind::PhotoFilter as u32, 7);
    }
}
