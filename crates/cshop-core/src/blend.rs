//! The 27 layer blend modes.
//!
//! Formulas operate on **document-space** (sRGB-encoded) straight-alpha
//! components in `0..1`, which is the conventional default — see the
//! module docs on [`crate::color`] for why we do not blend in linear light.
//!
//! The separable modes follow the W3C Compositing and Blending spec, which
//! agrees with what established editors produce; the modes that go beyond
//! that spec
//! (Linear Burn, Vivid Light, Divide, Darker Color, …) use the formulas
//! the PSD community has reverse-engineered.
//!
//! Every mode here is mirrored by a branch in `composite.wgsl`. The
//! discriminants are the contract between the two: `BlendMode as u32` is what
//! the shader switches on, so **never renumber them**.

use crate::color::Rgba;

/// How a layer combines with the backdrop beneath it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u32)]
pub enum BlendMode {
    #[default]
    Normal = 0,
    Dissolve = 1,

    Darken = 2,
    Multiply = 3,
    ColorBurn = 4,
    LinearBurn = 5,
    DarkerColor = 6,

    Lighten = 7,
    Screen = 8,
    ColorDodge = 9,
    LinearDodge = 10,
    LighterColor = 11,

    Overlay = 12,
    SoftLight = 13,
    HardLight = 14,
    VividLight = 15,
    LinearLight = 16,
    PinLight = 17,
    HardMix = 18,

    Difference = 19,
    Exclusion = 20,
    Subtract = 21,
    Divide = 22,

    Hue = 23,
    Saturation = 24,
    Color = 25,
    Luminosity = 26,

    /// Groups only: children composite directly against the backdrop outside
    /// the group rather than into an isolated buffer.
    PassThrough = 27,
}

impl BlendMode {
    /// Menu order, with `None` marking the conventional separator rules.
    pub const MENU: &'static [Option<BlendMode>] = &[
        Some(BlendMode::Normal),
        Some(BlendMode::Dissolve),
        None,
        Some(BlendMode::Darken),
        Some(BlendMode::Multiply),
        Some(BlendMode::ColorBurn),
        Some(BlendMode::LinearBurn),
        Some(BlendMode::DarkerColor),
        None,
        Some(BlendMode::Lighten),
        Some(BlendMode::Screen),
        Some(BlendMode::ColorDodge),
        Some(BlendMode::LinearDodge),
        Some(BlendMode::LighterColor),
        None,
        Some(BlendMode::Overlay),
        Some(BlendMode::SoftLight),
        Some(BlendMode::HardLight),
        Some(BlendMode::VividLight),
        Some(BlendMode::LinearLight),
        Some(BlendMode::PinLight),
        Some(BlendMode::HardMix),
        None,
        Some(BlendMode::Difference),
        Some(BlendMode::Exclusion),
        Some(BlendMode::Subtract),
        Some(BlendMode::Divide),
        None,
        Some(BlendMode::Hue),
        Some(BlendMode::Saturation),
        Some(BlendMode::Color),
        Some(BlendMode::Luminosity),
    ];

    /// Every real mode, excluding [`BlendMode::PassThrough`].
    pub fn all() -> impl Iterator<Item = BlendMode> {
        Self::MENU.iter().filter_map(|m| *m)
    }

    pub fn name(self) -> &'static str {
        use BlendMode::*;
        match self {
            Normal => "Normal",
            Dissolve => "Dissolve",
            Darken => "Darken",
            Multiply => "Multiply",
            ColorBurn => "Color Burn",
            LinearBurn => "Linear Burn",
            DarkerColor => "Darker Color",
            Lighten => "Lighten",
            Screen => "Screen",
            ColorDodge => "Color Dodge",
            LinearDodge => "Linear Dodge (Add)",
            LighterColor => "Lighter Color",
            Overlay => "Overlay",
            SoftLight => "Soft Light",
            HardLight => "Hard Light",
            VividLight => "Vivid Light",
            LinearLight => "Linear Light",
            PinLight => "Pin Light",
            HardMix => "Hard Mix",
            Difference => "Difference",
            Exclusion => "Exclusion",
            Subtract => "Subtract",
            Divide => "Divide",
            Hue => "Hue",
            Saturation => "Saturation",
            Color => "Color",
            Luminosity => "Luminosity",
            PassThrough => "Pass Through",
        }
    }

    /// The PSD four-character key, needed for `.psd` interop.
    pub fn psd_key(self) -> &'static [u8; 4] {
        use BlendMode::*;
        match self {
            Normal => b"norm",
            Dissolve => b"diss",
            Darken => b"dark",
            Multiply => b"mul ",
            ColorBurn => b"idiv",
            LinearBurn => b"lbrn",
            DarkerColor => b"dkCl",
            Lighten => b"lite",
            Screen => b"scrn",
            ColorDodge => b"div ",
            LinearDodge => b"lddg",
            LighterColor => b"lgCl",
            Overlay => b"over",
            SoftLight => b"sLit",
            HardLight => b"hLit",
            VividLight => b"vLit",
            LinearLight => b"lLit",
            PinLight => b"pLit",
            HardMix => b"hMix",
            Difference => b"diff",
            Exclusion => b"smud",
            Subtract => b"fsub",
            Divide => b"fdiv",
            Hue => b"hue ",
            Saturation => b"sat ",
            Color => b"colr",
            Luminosity => b"lum ",
            PassThrough => b"pass",
        }
    }

    pub fn from_psd_key(key: &[u8; 4]) -> Option<BlendMode> {
        BlendMode::all()
            .chain(std::iter::once(BlendMode::PassThrough))
            .find(|m| m.psd_key() == key)
    }

}

// ---------------------------------------------------------------------------
// Separable channel functions
// ---------------------------------------------------------------------------

#[inline]
fn b_multiply(cb: f32, cs: f32) -> f32 {
    cb * cs
}

#[inline]
fn b_screen(cb: f32, cs: f32) -> f32 {
    cb + cs - cb * cs
}

#[inline]
fn b_color_burn(cb: f32, cs: f32) -> f32 {
    if cb >= 1.0 {
        1.0
    } else if cs <= 0.0 {
        0.0
    } else {
        1.0 - (1.0f32).min((1.0 - cb) / cs)
    }
}

#[inline]
fn b_color_dodge(cb: f32, cs: f32) -> f32 {
    if cb <= 0.0 {
        0.0
    } else if cs >= 1.0 {
        1.0
    } else {
        (1.0f32).min(cb / (1.0 - cs))
    }
}

#[inline]
fn b_hard_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        b_multiply(cb, 2.0 * cs)
    } else {
        b_screen(cb, 2.0 * cs - 1.0)
    }
}

#[inline]
fn b_soft_light(cb: f32, cs: f32) -> f32 {
    // W3C formulation; the smooth variant.
    if cs <= 0.5 {
        cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
    } else {
        let d = if cb <= 0.25 {
            ((16.0 * cb - 12.0) * cb + 4.0) * cb
        } else {
            cb.max(0.0).sqrt()
        };
        cb + (2.0 * cs - 1.0) * (d - cb)
    }
}

/// Blend a single channel for the separable modes. Non-separable modes are
/// handled in [`blend_rgb`] and are passed through unchanged here.
#[inline]
pub fn blend_channel(mode: BlendMode, cb: f32, cs: f32) -> f32 {
    use BlendMode::*;
    match mode {
        Normal | Dissolve | PassThrough => cs,
        Darken => cb.min(cs),
        Multiply => b_multiply(cb, cs),
        ColorBurn => b_color_burn(cb, cs),
        LinearBurn => cb + cs - 1.0,
        Lighten => cb.max(cs),
        Screen => b_screen(cb, cs),
        ColorDodge => b_color_dodge(cb, cs),
        LinearDodge => cb + cs,
        // Overlay is Hard Light with the operands swapped.
        Overlay => b_hard_light(cs, cb),
        SoftLight => b_soft_light(cb, cs),
        HardLight => b_hard_light(cb, cs),
        VividLight => {
            if cs <= 0.5 {
                b_color_burn(cb, 2.0 * cs)
            } else {
                b_color_dodge(cb, 2.0 * cs - 1.0)
            }
        }
        LinearLight => cb + 2.0 * cs - 1.0,
        PinLight => {
            if cs <= 0.5 {
                cb.min(2.0 * cs)
            } else {
                cb.max(2.0 * cs - 1.0)
            }
        }
        HardMix => {
            if cb + cs >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        Difference => (cb - cs).abs(),
        Exclusion => cb + cs - 2.0 * cb * cs,
        Subtract => cb - cs,
        Divide => {
            if cs <= 0.0 {
                1.0
            } else {
                (1.0f32).min(cb / cs)
            }
        }
        // Handled by blend_rgb.
        DarkerColor | LighterColor | Hue | Saturation | Color | Luminosity => cs,
    }
}

// ---------------------------------------------------------------------------
// Non-separable helpers (W3C "SetLum" / "SetSat" family)
// ---------------------------------------------------------------------------

#[inline]
fn lum(c: [f32; 3]) -> f32 {
    0.30 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

/// Pull any out-of-gamut channel back by desaturating toward the luminosity,
/// which preserves the perceived brightness.
fn clip_color(mut c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    if n < 0.0 {
        let d = l - n;
        if d > 1e-6 {
            for v in &mut c {
                *v = l + (*v - l) * l / d;
            }
        } else {
            c = [l; 3];
        }
    }
    if x > 1.0 {
        let d = x - l;
        if d > 1e-6 {
            for v in &mut c {
                *v = l + (*v - l) * (1.0 - l) / d;
            }
        } else {
            c = [l; 3];
        }
    }
    c
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

#[inline]
fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

/// Rescale the mid/max channels so the colour has saturation `s`, keeping the
/// channel ordering intact.
fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    // Index of the smallest, middle and largest components.
    let (mut imin, mut imid, mut imax) = (0usize, 1usize, 2usize);
    if c[imin] > c[imid] {
        std::mem::swap(&mut imin, &mut imid);
    }
    if c[imid] > c[imax] {
        std::mem::swap(&mut imid, &mut imax);
    }
    if c[imin] > c[imid] {
        std::mem::swap(&mut imin, &mut imid);
    }

    let mut out = [0.0f32; 3];
    if c[imax] > c[imin] {
        out[imid] = (c[imid] - c[imin]) * s / (c[imax] - c[imin]);
        out[imax] = s;
    }
    out[imin] = 0.0;
    out
}

/// Blend an RGB triple, dispatching separable modes per channel and handling
/// the six whole-colour modes directly.
pub fn blend_rgb(mode: BlendMode, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    use BlendMode::*;
    match mode {
        Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        Color => set_lum(cs, lum(cb)),
        Luminosity => set_lum(cb, lum(cs)),
        DarkerColor => {
            if lum(cs) < lum(cb) {
                cs
            } else {
                cb
            }
        }
        LighterColor => {
            if lum(cs) > lum(cb) {
                cs
            } else {
                cb
            }
        }
        _ => [
            blend_channel(mode, cb[0], cs[0]),
            blend_channel(mode, cb[1], cs[1]),
            blend_channel(mode, cb[2], cs[2]),
        ],
    }
}

/// Full source-over composite of `src` onto `backdrop` with a blend mode and
/// an extra coverage factor (layer opacity × mask).
///
/// Both inputs and the result use **straight** alpha. This is the W3C formula:
/// where the backdrop is transparent the blend result fades back to the plain
/// source colour, which is what stops blend modes from producing black halos
/// against empty areas.
pub fn composite(mode: BlendMode, backdrop: Rgba, src: Rgba, coverage: f32) -> Rgba {
    let asrc = (src.a * coverage).clamp(0.0, 1.0);
    if asrc <= 0.0 {
        return backdrop;
    }
    let ab = backdrop.a;
    let cb = [backdrop.r, backdrop.g, backdrop.b];
    let cs = [src.r, src.g, src.b];

    let blended = blend_rgb(mode, cb, cs);
    // Weight the blended colour by how opaque the backdrop actually is.
    let mixed = [
        cs[0] + ab * (blended[0] - cs[0]),
        cs[1] + ab * (blended[1] - cs[1]),
        cs[2] + ab * (blended[2] - cs[2]),
    ];

    let ao = asrc + ab * (1.0 - asrc);
    if ao <= 0.0 {
        return Rgba::TRANSPARENT;
    }
    // Premultiplied source-over, then back to straight alpha.
    let co = |m: f32, b: f32| (asrc * m + ab * (1.0 - asrc) * b) / ao;
    Rgba::new(co(mixed[0], cb[0]), co(mixed[1], cb[1]), co(mixed[2], cb[2]), ao)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    #[test]
    fn discriminants_are_stable() {
        // The shader switches on these; renumbering silently corrupts output.
        assert_eq!(BlendMode::Normal as u32, 0);
        assert_eq!(BlendMode::Multiply as u32, 3);
        assert_eq!(BlendMode::Overlay as u32, 12);
        assert_eq!(BlendMode::Difference as u32, 19);
        assert_eq!(BlendMode::Luminosity as u32, 26);
        assert_eq!(BlendMode::PassThrough as u32, 27);
    }

    #[test]
    fn menu_covers_every_mode_exactly_once() {
        let modes: Vec<_> = BlendMode::all().collect();
        assert_eq!(modes.len(), 27, "expected 27 blend modes, got {}", modes.len());
        let mut seen = modes.clone();
        seen.sort_by_key(|m| *m as u32);
        seen.dedup();
        assert_eq!(seen.len(), 27, "menu lists a mode twice");
    }

    #[test]
    fn psd_keys_are_unique_and_round_trip() {
        let mut keys: Vec<_> = BlendMode::all().map(|m| *m.psd_key()).collect();
        let n = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), n, "two modes share a PSD key");
        for m in BlendMode::all() {
            assert_eq!(BlendMode::from_psd_key(m.psd_key()), Some(m));
        }
    }

    #[test]
    fn separable_reference_values() {
        assert!(close(blend_channel(BlendMode::Multiply, 0.5, 0.5), 0.25));
        assert!(close(blend_channel(BlendMode::Screen, 0.5, 0.5), 0.75));
        assert!(close(blend_channel(BlendMode::Difference, 0.2, 0.7), 0.5));
        assert!(close(blend_channel(BlendMode::Exclusion, 0.5, 0.5), 0.5));
        assert!(close(blend_channel(BlendMode::LinearBurn, 0.5, 0.5), 0.0));
        assert!(close(blend_channel(BlendMode::LinearDodge, 0.5, 0.25), 0.75));
        // Overlay is Hard Light with operands swapped.
        for &(cb, cs) in &[(0.2, 0.8), (0.9, 0.1), (0.5, 0.5)] {
            assert!(close(
                blend_channel(BlendMode::Overlay, cb, cs),
                blend_channel(BlendMode::HardLight, cs, cb)
            ));
        }
    }

    #[test]
    fn degenerate_dodge_and_burn_are_finite() {
        assert!(close(blend_channel(BlendMode::ColorDodge, 0.5, 1.0), 1.0));
        assert!(close(blend_channel(BlendMode::ColorDodge, 0.0, 1.0), 0.0));
        assert!(close(blend_channel(BlendMode::ColorBurn, 0.5, 0.0), 0.0));
        assert!(close(blend_channel(BlendMode::ColorBurn, 1.0, 0.0), 1.0));
        assert!(close(blend_channel(BlendMode::Divide, 0.5, 0.0), 1.0));
        for m in BlendMode::all() {
            for &(cb, cs) in &[(0.0, 0.0), (1.0, 1.0), (0.0, 1.0), (1.0, 0.0)] {
                assert!(blend_channel(m, cb, cs).is_finite(), "{m:?} produced a non-finite value");
            }
        }
    }

    #[test]
    fn normal_over_opaque_backdrop_is_a_lerp() {
        let bd = Rgba::new(0.0, 0.0, 0.0, 1.0);
        let src = Rgba::new(1.0, 1.0, 1.0, 1.0);
        let out = composite(BlendMode::Normal, bd, src, 0.5);
        assert!(close(out.r, 0.5) && close(out.a, 1.0));
    }

    #[test]
    fn zero_coverage_leaves_backdrop_untouched() {
        let bd = Rgba::new(0.25, 0.5, 0.75, 0.6);
        for m in BlendMode::all() {
            let out = composite(m, bd, Rgba::WHITE, 0.0);
            assert_eq!(out, bd, "{m:?} altered the backdrop at zero coverage");
        }
    }

    #[test]
    fn blending_onto_transparency_keeps_the_source_colour() {
        // The classic bug this guards: Multiply over empty space going black.
        let src = Rgba::new(0.8, 0.4, 0.2, 1.0);
        for m in BlendMode::all() {
            let out = composite(m, Rgba::TRANSPARENT, src, 1.0);
            assert!(
                close(out.r, src.r) && close(out.g, src.g) && close(out.b, src.b),
                "{m:?} over transparency gave {out:?}"
            );
            assert!(close(out.a, 1.0));
        }
    }

    #[test]
    fn luminosity_preserves_backdrop_hue() {
        let bd = [0.8, 0.2, 0.2];
        let out = blend_rgb(BlendMode::Luminosity, bd, [0.5, 0.5, 0.5]);
        assert!(close(lum(out), 0.5), "luminosity should adopt the source luma");
        assert!(out[0] > out[1] && out[0] > out[2], "hue ordering should survive");
    }

    #[test]
    fn color_mode_adopts_backdrop_luma() {
        let out = blend_rgb(BlendMode::Color, [0.5, 0.5, 0.5], [0.9, 0.1, 0.1]);
        assert!(close(lum(out), 0.5));
    }

    #[test]
    fn non_separable_results_stay_in_gamut() {
        let samples = [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.9, 0.1, 0.4], [0.2, 0.7, 0.3]];
        for m in [BlendMode::Hue, BlendMode::Saturation, BlendMode::Color, BlendMode::Luminosity] {
            for cb in &samples {
                for cs in &samples {
                    for v in blend_rgb(m, *cb, *cs) {
                        assert!((-EPS..=1.0 + EPS).contains(&v), "{m:?} produced {v}");
                    }
                }
            }
        }
    }
}
