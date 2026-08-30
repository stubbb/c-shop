//! Lighting a photograph again, from a guess at its shape.
//!
//! A depth model turns a picture into a rough idea of how far away everything
//! in it is. That is not a model of the scene — it is one number per pixel,
//! with no notion of what is behind anything — but it is enough to do the one
//! thing photographers most often wish they could go back and change: where
//! the light was coming from.
//!
//! # From depth to shape
//!
//! Depth alone does not light anything; what a lamp responds to is which way a
//! surface *faces*. That is the gradient of the depth: where it changes
//! quickly across the frame the surface is turned away from the camera, and
//! where it is flat the surface faces us. So the normal at a pixel is
//! `(-dz/dx, -dz/dy, 1)` normalised, scaled by [`Relight::relief`] — which is
//! the honest knob, because the depth is relative and has no unit, so how much
//! shape a given change in it implies is a choice rather than a measurement.
//!
//! # What this is not
//!
//! It is not a physical relighting. Nothing here knows about shadows cast by
//! one object onto another, about how shiny anything is, or about what colour
//! the original light was. It shades the surfaces it can see by how they face
//! a new lamp, and leaves everything else to the ambient term. On a portrait
//! or a still life that is convincing; on a scene whose lighting is the
//! subject it will look like what it is.

use crate::color::{Rgba, Rgba8};
use crate::pixels::PixelBuffer;
use rayon::prelude::*;

/// A lamp, and how much of the picture's own light to keep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Relight {
    /// Where the lamp is, as a position on a circle around the picture:
    /// **0° is to the left**, and it goes round clockwise — 90° above, 180° to
    /// the right, 270° below.
    pub azimuth: f32,
    /// How high it is, from level with the subject to straight in front.
    pub elevation: f32,
    /// How strong the new lamp is.
    pub intensity: f32,
    /// How much of the picture survives where the lamp does not reach. At 1
    /// this only ever adds light; at 0 the unlit side goes black.
    pub ambient: f32,
    /// How much shape to read into the depth. Zero is a flat picture and the
    /// lamp does nothing; large values turn every gentle slope into a facet.
    pub relief: f32,
    /// The colour of the lamp.
    pub color: Rgba8,
}

impl Default for Relight {
    fn default() -> Self {
        Relight {
            azimuth: 135.0,
            elevation: 45.0,
            intensity: 0.6,
            ambient: 1.0,
            relief: 1.0,
            color: Rgba8::WHITE,
        }
    }
}

impl Relight {
    /// True when this would return the picture unchanged.
    pub fn is_identity(&self) -> bool {
        self.intensity == 0.0 && self.ambient >= 1.0
    }

    /// The direction the light comes *from*, in the frame's own axes: x to the
    /// right, y down, z out of the picture towards the viewer.
    ///
    /// A lamp at 0° is off to the left, so the direction light arrives from
    /// points left, which is negative x. Screen y grows downward, so a lamp
    /// above points negative y as well. Getting either of those backwards
    /// produces a picture that is merely lit from the wrong side, which a
    /// photograph will not complain about — hence the tests on a ramp, which
    /// will.
    fn direction(&self) -> [f32; 3] {
        let a = self.azimuth.to_radians();
        let e = self.elevation.clamp(-89.0, 89.0).to_radians();
        let (sa, ca) = a.sin_cos();
        let horizontal = e.cos();
        [-ca * horizontal, -sa * horizontal, e.sin().max(0.02)]
    }
}

/// Depth, one number a pixel, where 1 is nearest the camera.
#[derive(Debug, Clone)]
pub struct DepthMap {
    pub width: u32,
    pub height: u32,
    /// Row-major, 0 (far) to 1 (near).
    pub data: Vec<f32>,
}

impl DepthMap {
    pub fn new(width: u32, height: u32) -> DepthMap {
        DepthMap { width, height, data: vec![0.0; width as usize * height as usize] }
    }

    pub fn from_values(width: u32, height: u32, data: Vec<f32>) -> Option<DepthMap> {
        (data.len() == width as usize * height as usize)
            .then_some(DepthMap { width, height, data })
    }

    #[inline]
    pub fn at(&self, x: i32, y: i32) -> f32 {
        let x = x.clamp(0, self.width as i32 - 1) as usize;
        let y = y.clamp(0, self.height as i32 - 1) as usize;
        self.data[y * self.width as usize + x]
    }

    /// Stretch the values to fill 0..1, which is what makes `relief` mean the
    /// same thing on a picture of a room and a picture of a face.
    pub fn normalised(mut self) -> DepthMap {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for &v in &self.data {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        let span = (hi - lo).max(1e-6);
        for v in &mut self.data {
            *v = (*v - lo) / span;
        }
        self
    }

    /// The surface normal at a pixel, from how fast the depth is changing.
    ///
    /// Sampled two pixels either side rather than one: depth from a model is
    /// smooth but not clean, and the wider stencil is markedly steadier
    /// without losing anything a photograph's shading would show.
    #[inline]
    pub fn normal_at(&self, x: i32, y: i32, relief: f32) -> [f32; 3] {
        let dx = (self.at(x + 2, y) - self.at(x - 2, y)) * 0.25;
        let dy = (self.at(x, y + 2) - self.at(x, y - 2)) * 0.25;
        // The scale turns a unitless depth change into something comparable
        // with one pixel of distance across the frame.
        let s = relief * self.width.min(self.height) as f32 * 0.05;
        let n = [-dx * s, -dy * s, 1.0];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
        [n[0] / len, n[1] / len, n[2] / len]
    }
}

/// Light a picture again.
///
/// Alpha is untouched: a lamp changes how much light comes off a surface, not
/// whether the surface is there.
pub fn apply(src: &PixelBuffer, depth: &DepthMap, lamp: Relight) -> PixelBuffer {
    let mut out = src.clone();
    let width = src.width() as usize;
    let dir = lamp.direction();
    let tint = lamp.color.to_f32();

    out.pixels_mut().par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let n = depth.normal_at(x as i32, y as i32, lamp.relief);
            let lambert = (n[0] * dir[0] + n[1] * dir[1] + n[2] * dir[2]).max(0.0);
            let c = px.to_f32();
            // Ambient keeps the picture; the lamp adds to it. Multiplying
            // rather than replacing is what keeps a photograph looking like
            // one — the surfaces keep their own colour and only their
            // brightness answers to the light.
            let gain = |ch: f32| {
                (lamp.ambient + lamp.intensity * lambert * ch).max(0.0)
            };
            *px = Rgba::new(
                c.r * gain(tint.r),
                c.g * gain(tint.g),
                c.b * gain(tint.b),
                c.a,
            )
            .to_u8();
        }
    });
    out
}

/// Depth as coverage, for use as a layer mask.
///
/// Near is revealed and far is hidden, which is the way round that makes the
/// obvious edit obvious: mask a layer by its own depth and what is close to
/// the camera survives. `invert` is for the other half of that — fog, a
/// darkened background, anything that should build with distance.
pub fn to_mask(depth: &DepthMap, invert: bool) -> crate::mask::MaskBuffer {
    let mut out = crate::mask::MaskBuffer::hide_all(depth.width, depth.height);
    for y in 0..depth.height as i32 {
        for x in 0..depth.width as i32 {
            let v = depth.at(x, y).clamp(0.0, 1.0);
            let v = if invert { 1.0 - v } else { v };
            out.set(x, y, (v * 255.0 + 0.5) as u8);
        }
    }
    out
}

/// Depth as a picture, for looking at or keeping as a layer.
///
/// Near is white, which is the way round everyone reads a depth map even
/// though the number it comes from usually runs the other way.
pub fn to_pixels(depth: &DepthMap) -> PixelBuffer {
    let mut out = PixelBuffer::new(depth.width, depth.height);
    for y in 0..depth.height as i32 {
        for x in 0..depth.width as i32 {
            let v = (depth.at(x, y).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            out.set(x, y, Rgba8::opaque(v, v, v));
        }
    }
    out
}
