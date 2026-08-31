//! Smart objects: a layer that remembers the picture it was made from.
//!
//! # What it is for
//!
//! Scale a raster layer down to a quarter and back up again and you have a
//! quarter of a picture stretched over the original space. The pixels that
//! were thrown away on the first step do not come back on the second, and no
//! amount of care on the second step can help, because the information is
//! gone. Every editor that works directly on pixels has this property, and it
//! is why "resize once, at the end" is advice people have to be given.
//!
//! A smart object keeps the picture it was made from, at whatever size it
//! arrived, and treats the placement — scale, rotation, skew — as a setting
//! rather than an edit. Changing the placement re-renders from the original
//! every time, so the twentieth adjustment is exactly as good as the first.
//!
//! # How it fits the rest of the program
//!
//! Exactly the way type and shape layers do: it carries the raster its
//! placement currently produces, and everything downstream — the compositor,
//! masks, blend modes, effects, export — treats that as the layer's pixels
//! without knowing there is anything behind it. Nothing needed teaching about
//! smart objects except the two places that change the placement and the one
//! that reads the source back out.

use crate::geom::Vec2;
use crate::pixels::PixelBuffer;
use crate::resample::{self, Resampling};
use crate::transform::Transform;

/// The largest a placement may render to, so a mis-dragged handle asks for a
/// big picture rather than an impossible one.
const MAX_SIDE: u32 = 30_000;

#[derive(Debug, Clone, PartialEq)]
pub struct SmartObject {
    /// The picture as it arrived, never resampled. This is the whole point:
    /// nothing can be lost by a placement, because nothing is thrown away.
    source: PixelBuffer,
    /// Scale, rotation and skew about the source's own centre. Translation is
    /// the layer's `offset` and does not live here, so moving a smart object
    /// costs nothing and never re-renders it.
    placement: Transform,
    /// What the placement looks like.
    raster: PixelBuffer,
}

impl SmartObject {
    /// Wrap a picture, placed as it is.
    pub fn new(source: PixelBuffer) -> SmartObject {
        let raster = source.clone();
        SmartObject { source, placement: Transform::IDENTITY, raster }
    }

    pub fn source(&self) -> &PixelBuffer {
        &self.source
    }

    pub fn placement(&self) -> Transform {
        self.placement
    }

    pub fn raster(&self) -> &PixelBuffer {
        &self.raster
    }

    /// The size of the picture behind the placement.
    pub fn source_size(&self) -> (u32, u32) {
        (self.source.width(), self.source.height())
    }

    /// True when the placement is doing nothing, so the layer is showing its
    /// source at full size.
    pub fn is_untouched(&self) -> bool {
        self.placement == Transform::IDENTITY
    }

    /// Rebuild from the source at a new placement.
    ///
    /// Returns how far the raster's top-left moved, so the layer's own offset
    /// can absorb it and the picture stay where it was rather than drifting
    /// toward a corner every time it is scaled.
    pub fn place(&mut self, placement: Transform, filter: Resampling) -> (i32, i32) {
        let before = self.raster_origin();
        self.placement = placement;
        let (raster, origin) = self.render(filter);
        self.raster = raster;
        (origin.0 - before.0, origin.1 - before.1)
    }

    /// Re-render at the placement already set — after the source has been
    /// replaced, for instance.
    pub fn refresh(&mut self, filter: Resampling) {
        let (raster, _) = self.render(filter);
        self.raster = raster;
    }

    /// Replace the picture behind the placement, keeping the placement.
    pub fn set_source(&mut self, source: PixelBuffer, filter: Resampling) {
        self.source = source;
        self.refresh(filter);
    }

    /// Where the current raster's top-left sits relative to the source's, in
    /// the source's own coordinates.
    fn raster_origin(&self) -> (i32, i32) {
        let (w, h) = (self.source.width() as f32, self.source.height() as f32);
        let about = Transform::about(Vec2::new(w / 2.0, h / 2.0), self.placement);
        let bounds = about.transformed_bounds(crate::geom::IRect::new(0, 0, w as i32, h as i32));
        (bounds.x0, bounds.y0)
    }

    fn render(&self, filter: Resampling) -> (PixelBuffer, (i32, i32)) {
        let (sw, sh) = (self.source.width(), self.source.height());
        let m = self.placement.m;

        // A plain scale is the common case by a long way, and `resize` handles
        // it better than a general warp does: reducing with an area-aware
        // filter takes in every source pixel, where point-sampling a warp
        // takes one in four and shimmers.
        let plain_scale = m[0][1].abs() < 1e-6
            && m[1][0].abs() < 1e-6
            && m[0][2].abs() < 1e-6
            && m[1][2].abs() < 1e-6
            && m[0][0] > 0.0
            && m[1][1] > 0.0;
        if plain_scale {
            let w = ((sw as f32 * m[0][0]).round() as u32).clamp(1, MAX_SIDE);
            let h = ((sh as f32 * m[1][1]).round() as u32).clamp(1, MAX_SIDE);
            let raster = resample::resize(&self.source, w, h, filter);
            // Scaled about the centre, so the top-left moves out by half of
            // whatever the size gained.
            let ox = ((sw as f32 - w as f32) / 2.0).round() as i32;
            let oy = ((sh as f32 - h as f32) / 2.0).round() as i32;
            return (raster, (ox, oy));
        }

        let about = Transform::about(Vec2::new(sw as f32 / 2.0, sh as f32 / 2.0), self.placement);
        let clip = crate::geom::IRect::new(
            -(MAX_SIDE as i32),
            -(MAX_SIDE as i32),
            MAX_SIDE as i32,
            MAX_SIDE as i32,
        );
        match resample::transform(&self.source, (0, 0), about, filter, Some(clip)) {
            Some((raster, origin)) => (raster, origin),
            // A placement that collapses to nothing. Keep a single pixel
            // rather than a zero-sized buffer nothing downstream expects.
            None => (PixelBuffer::new(1, 1), (0, 0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;

    fn detailed(w: u32, h: u32) -> PixelBuffer {
        let mut px = PixelBuffer::new(w, h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                // A fine checker, which is exactly what a downscale destroys.
                let on = (x / 2 + y / 2) % 2 == 0;
                px.set(x, y, if on { Rgba8::WHITE } else { Rgba8::BLACK });
            }
        }
        px
    }

    /// How much detail survives: the spread of the pixel values. A picture
    /// that has been through a quarter-size round trip is smooth grey.
    fn detail(px: &PixelBuffer) -> f32 {
        let v: Vec<f32> = px.pixels().iter().map(|p| p.r as f32).collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        (v.iter().map(|a| (a - mean).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn scaling_down_and_back_up_loses_nothing() {
        let source = detailed(128, 128);
        let mut smart = SmartObject::new(source.clone());

        smart.place(Transform::scale(0.25, 0.25), Resampling::Bilinear);
        assert_eq!(smart.raster().width(), 32);
        // What a raster layer would be left holding at this point.
        let flattened = smart.raster().clone();

        smart.place(Transform::IDENTITY, Resampling::Bilinear);
        assert_eq!(smart.raster().pixels(), source.pixels(), "back to exactly the picture");

        // And the comparison that makes the point: doing it to the pixels
        // themselves cannot come back.
        let stretched = resample::resize(&flattened, 128, 128, Resampling::Bilinear);
        assert!(
            detail(&stretched) < detail(&source) * 0.5,
            "a raster round trip should have lost most of the detail: {:.1} against {:.1}",
            detail(&stretched),
            detail(&source)
        );
    }

    #[test]
    fn a_placement_is_a_setting_and_not_an_edit() {
        let source = detailed(64, 64);
        let mut smart = SmartObject::new(source.clone());
        // Twenty changes of mind.
        for i in 1..=20 {
            let k = 0.2 + (i % 7) as f32 * 0.3;
            smart.place(Transform::scale(k, k), Resampling::Bilinear);
        }
        smart.place(Transform::scale(0.5, 0.5), Resampling::Bilinear);
        let after_twenty = smart.raster().clone();

        let mut fresh = SmartObject::new(source);
        fresh.place(Transform::scale(0.5, 0.5), Resampling::Bilinear);
        assert_eq!(
            after_twenty.pixels(),
            fresh.raster().pixels(),
            "the twentieth placement should be as good as the first"
        );
    }

    #[test]
    fn the_picture_stays_where_it_was_when_it_is_scaled() {
        let mut smart = SmartObject::new(detailed(100, 100));
        let shift = smart.place(Transform::scale(0.5, 0.5), Resampling::Bilinear);
        // Halving a 100px square about its centre moves the top-left in by 25.
        assert_eq!(shift, (25, 25), "so the layer's offset can cancel it out");
        let back = smart.place(Transform::IDENTITY, Resampling::Bilinear);
        assert_eq!(back, (-25, -25), "and the other way coming back");
    }

    #[test]
    fn rotation_grows_the_box_and_keeps_the_middle() {
        let mut smart = SmartObject::new(detailed(64, 64));
        smart.place(Transform::rotate(std::f32::consts::FRAC_PI_4), Resampling::Bilinear);
        let (w, h) = (smart.raster().width(), smart.raster().height());
        assert!(w > 84 && w < 96, "a 64px square turned 45 degrees is about 90 across: {w}");
        assert_eq!(w, h, "and square");
        // Still reversible, because the source was never touched.
        smart.place(Transform::IDENTITY, Resampling::Bilinear);
        assert_eq!(smart.raster().width(), 64);
    }

    #[test]
    fn a_collapsed_placement_does_not_produce_a_layer_of_nothing() {
        let mut smart = SmartObject::new(detailed(32, 32));
        smart.place(Transform::scale(0.0, 0.0), Resampling::Bilinear);
        assert!(smart.raster().width() >= 1 && smart.raster().height() >= 1);
        smart.place(Transform::IDENTITY, Resampling::Bilinear);
        assert_eq!(smart.raster().width(), 32, "and it comes back");
    }
}
