//! Colour types and sRGB transfer functions.
//!
//! Two representations coexist deliberately:
//!
//! * [`Rgba8`] — 8-bit **straight** (non-premultiplied) alpha in the **sRGB**
//!   encoding. This is how pixels sit at rest in a document, and how PNG, JPEG
//!   and PSD store them, so loading and saving never has to touch every pixel.
//! * [`Rgba`] — `f32` straight alpha, used for colour maths.
//!
//! # Which space does `Rgba` hold?
//!
//! Whichever one the caller is working in, so read the call site:
//!
//! * **Document space** (sRGB-encoded, gamma ~2.2), via [`Rgba8::to_f32`] and
//!   [`Rgba::to_u8`]. This is the compositor's working space, because
//!   Established editors blend in the document's gamma-encoded space by
//!   default — their
//!   *Blend RGB Colors Using Gamma 1.00* option is off out of the box. Matching
//!   what those editors produce matters more here than physical
//!   correctness, so our blend modes take encoded values too.
//! * **Linear light**, via [`Rgba8::to_linear`] and [`Rgba::to_srgb8`]. Needed
//!   wherever the physics is the point: resampling, blurs, gradient
//!   interpolation, and the future linear-blending document option.

use bytemuck::{Pod, Zeroable};

/// 8-bit sRGB colour with straight alpha.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct Rgba8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba8 {
    pub const TRANSPARENT: Rgba8 = Rgba8::new(0, 0, 0, 0);
    pub const BLACK: Rgba8 = Rgba8::new(0, 0, 0, 255);
    pub const WHITE: Rgba8 = Rgba8::new(255, 255, 255, 255);

    #[inline]
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    #[inline]
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    #[inline]
    pub const fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    #[inline]
    pub const fn from_array(a: [u8; 4]) -> Self {
        Self { r: a[0], g: a[1], b: a[2], a: a[3] }
    }

    /// Parse `RRGGBB` or `RRGGBBAA`, with or without a leading `#`.
    pub fn from_hex(s: &str) -> Option<Rgba8> {
        let s = s.trim().trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
        match s.len() {
            6 => Some(Rgba8::opaque(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Rgba8::new(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        format!("{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// Normalise to `0..1` **without** applying a transfer function, keeping
    /// the value in the document's sRGB-encoded space. This is the conversion
    /// the compositor uses.
    #[inline]
    pub fn to_f32(self) -> Rgba {
        Rgba {
            r: self.r as f32 / 255.0,
            g: self.g as f32 / 255.0,
            b: self.b as f32 / 255.0,
            a: self.a as f32 / 255.0,
        }
    }

    /// Convert to linear-light float. Alpha is linear already and only scaled.
    #[inline]
    pub fn to_linear(self) -> Rgba {
        Rgba {
            r: srgb_to_linear(self.r as f32 / 255.0),
            g: srgb_to_linear(self.g as f32 / 255.0),
            b: srgb_to_linear(self.b as f32 / 255.0),
            a: self.a as f32 / 255.0,
        }
    }
}

/// Linear-light `f32` colour with straight alpha.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba::new(0.0, 0.0, 0.0, 0.0);
    pub const BLACK: Rgba = Rgba::new(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Rgba = Rgba::new(1.0, 1.0, 1.0, 1.0);

    #[inline]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    #[inline]
    pub const fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// Quantise to 8 bits **without** a transfer function; the inverse of
    /// [`Rgba8::to_f32`].
    #[inline]
    pub fn to_u8(self) -> Rgba8 {
        #[inline]
        fn q(v: f32) -> u8 {
            (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
        }
        Rgba8 { r: q(self.r), g: q(self.g), b: q(self.b), a: q(self.a) }
    }

    /// Convert linear light back to 8-bit sRGB, rounding to nearest.
    #[inline]
    pub fn to_srgb8(self) -> Rgba8 {
        #[inline]
        fn enc(v: f32) -> u8 {
            (linear_to_srgb(v).clamp(0.0, 1.0) * 255.0 + 0.5) as u8
        }
        Rgba8 {
            r: enc(self.r),
            g: enc(self.g),
            b: enc(self.b),
            a: (self.a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        }
    }

    /// Rec. 601 luma, which is what the non-separable blend modes and
    /// its Darker/Lighter Color comparisons use.
    #[inline]
    pub fn luma(self) -> f32 {
        0.30 * self.r + 0.59 * self.g + 0.11 * self.b
    }
}

/// sRGB electro-optical transfer function (encoded value -> linear light).
#[inline]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_449_936 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Inverse of [`srgb_to_linear`].
#[inline]
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// HSV in `[0,1]^3` from non-linear sRGB components, for the colour picker.
pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        (((g - b) / d) % 6.0) / 6.0
    } else if max == g {
        (((b - r) / d) + 2.0) / 6.0
    } else {
        (((r - g) / d) + 4.0) / 6.0
    };
    let h = if h < 0.0 { h + 1.0 } else { h };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

/// Inverse of [`rgb_to_hsv`].
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = (h.rem_euclid(1.0)) * 6.0;
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_roundtrip_is_exact_for_all_bytes() {
        for v in 0u8..=255 {
            let c = Rgba8::new(v, v, v, v);
            assert_eq!(c.to_linear().to_srgb8(), c, "byte {v} did not round-trip");
        }
    }

    #[test]
    fn transfer_endpoints() {
        assert!((srgb_to_linear(0.0)).abs() < 1e-6);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(Rgba8::from_hex("#FF8000"), Some(Rgba8::opaque(255, 128, 0)));
        assert_eq!(Rgba8::from_hex("ff800080"), Some(Rgba8::new(255, 128, 0, 128)));
        assert_eq!(Rgba8::from_hex("xyz"), None);
        assert_eq!(Rgba8::opaque(255, 128, 0).to_hex(), "FF8000");
    }

    #[test]
    fn document_space_roundtrip_is_exact() {
        for v in 0u8..=255 {
            let c = Rgba8::new(v, 255 - v, v, v);
            assert_eq!(c.to_f32().to_u8(), c);
        }
    }

    #[test]
    fn hsv_roundtrip() {
        for &(r, g, b) in &[(1.0, 0.0, 0.0), (0.2, 0.7, 0.4), (0.0, 0.0, 0.0), (1.0, 1.0, 1.0)] {
            let (h, s, v) = rgb_to_hsv(r, g, b);
            let (r2, g2, b2) = hsv_to_rgb(h, s, v);
            assert!((r - r2).abs() < 1e-4 && (g - g2).abs() < 1e-4 && (b - b2).abs() < 1e-4);
        }
    }
}
