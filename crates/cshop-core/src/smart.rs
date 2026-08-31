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
//!
//! # Why the picture lives in the document and not in the layer
//!
//! The obvious arrangement is a layer that owns its source. It is simpler, and
//! it makes one thing impossible: a *linked* smart object, where several
//! layers show the same picture and changing that picture changes all of them.
//! A logo placed in four corners is one picture used four times, and correcting
//! it should be one correction.
//!
//! So the pictures live in a [`SourceStore`] on the document and a smart layer
//! holds a [`SourceId`]. Sharing is then the default rather than a feature: two
//! layers on one id *are* linked, with nothing to keep in step, and the saved
//! file holds the picture once however many places it appears.
//!
//! The cost is that a smart object can no longer render itself — it needs the
//! store to find its picture — so the operations that re-render live on
//! [`crate::document::Document`], which can lend out the store and the layer at
//! the same time. That is the whole of the awkwardness, and it is confined to
//! the three that re-render.

use crate::geom::Vec2;
use crate::pixels::PixelBuffer;
use crate::resample::{self, Resampling};
use crate::transform::Transform;

/// The largest a placement may render to, so a mis-dragged handle asks for a
/// big picture rather than an impossible one.
const MAX_SIDE: u32 = 30_000;

/// Names one picture in a document's [`SourceStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(pub u32);

/// A picture one or more smart objects are placements of.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// As it arrived, never resampled. This is the whole point: nothing can be
    /// lost by a placement, because nothing is thrown away.
    pub pixels: PixelBuffer,
    /// What to call it — the file it came from, or the layer it was made out
    /// of. Shown where a document's sources are listed.
    pub name: String,
}

/// Every picture the smart objects in one document were made from.
///
/// Kept in insertion order rather than a map, because the order it is written
/// to a file in should not depend on how a hasher felt about the numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceStore {
    entries: Vec<(SourceId, Source)>,
    next: u32,
}

impl SourceStore {
    /// Take a picture in, and say what it is now called.
    pub fn add(&mut self, pixels: PixelBuffer, name: impl Into<String>) -> SourceId {
        let id = SourceId(self.next);
        self.next += 1;
        self.entries.push((id, Source { pixels, name: name.into() }));
        id
    }

    /// Put a picture back under an identifier it already had — for loading a
    /// file, where the references were written against those numbers.
    pub fn restore(&mut self, id: SourceId, source: Source) {
        self.next = self.next.max(id.0 + 1);
        match self.entries.iter_mut().find(|(at, _)| *at == id) {
            Some(slot) => slot.1 = source,
            None => self.entries.push((id, source)),
        }
    }

    pub fn get(&self, id: SourceId) -> Option<&Source> {
        self.entries.iter().find(|(at, _)| *at == id).map(|(_, s)| s)
    }

    pub fn pixels(&self, id: SourceId) -> Option<&PixelBuffer> {
        self.get(id).map(|s| &s.pixels)
    }

    /// Swap one picture for another, handing back what was there.
    ///
    /// Only the store is changed. Every layer placing this source is now
    /// showing a stale rendering, which is why the only caller is
    /// [`crate::document::Document::replace_source`] — it re-renders them.
    pub fn replace(&mut self, id: SourceId, pixels: PixelBuffer) -> Option<PixelBuffer> {
        let slot = self.entries.iter_mut().find(|(at, _)| *at == id)?;
        Some(std::mem::replace(&mut slot.1.pixels, pixels))
    }

    pub fn remove(&mut self, id: SourceId) -> Option<Source> {
        let at = self.entries.iter().position(|(other, _)| *other == id)?;
        Some(self.entries.remove(at).1)
    }

    pub fn ids(&self) -> impl Iterator<Item = SourceId> + '_ {
        self.entries.iter().map(|(id, _)| *id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (SourceId, &Source)> {
        self.entries.iter().map(|(id, s)| (*id, s))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes held, for the memory readout.
    pub fn bytes(&self) -> u64 {
        self.entries.iter().map(|(_, s)| s.pixels.as_bytes().len() as u64).sum()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SmartObject {
    /// Which picture in the document's store this is a placement of.
    source: SourceId,
    /// Scale, rotation and skew about the source's own centre. Translation is
    /// the layer's `offset` and does not live here, so moving a smart object
    /// costs nothing and never re-renders it.
    placement: Transform,
    /// What the placement looks like.
    raster: PixelBuffer,
}

impl SmartObject {
    /// A placement of `source`, doing nothing to it yet.
    pub fn new(source: SourceId, store: &SourceStore) -> SmartObject {
        let raster = store.pixels(source).cloned().unwrap_or_else(|| PixelBuffer::new(1, 1));
        SmartObject { source, placement: Transform::IDENTITY, raster }
    }

    pub fn source(&self) -> SourceId {
        self.source
    }

    /// Point this placement at a different picture, keeping the placement.
    ///
    /// Not a re-render: the caller has the store and does that. Two layers
    /// pointed at one source are linked by that alone, so this is also how a
    /// layer is unlinked — by being pointed at a copy.
    pub fn set_source(&mut self, source: SourceId) {
        self.source = source;
    }

    pub fn placement(&self) -> Transform {
        self.placement
    }

    pub fn raster(&self) -> &PixelBuffer {
        &self.raster
    }

    /// The size of the picture behind the placement.
    pub fn source_size(&self, store: &SourceStore) -> (u32, u32) {
        store.pixels(self.source).map_or((1, 1), |p| (p.width(), p.height()))
    }

    /// Rebuild from the source at a new placement.
    ///
    /// Returns how far the raster's top-left moved, so the layer's own offset
    /// can absorb it and the picture stay where it was rather than drifting
    /// toward a corner every time it is scaled.
    pub fn place(
        &mut self,
        placement: Transform,
        filter: Resampling,
        store: &SourceStore,
    ) -> (i32, i32) {
        let before = self.raster_origin(store);
        self.placement = placement;
        let (raster, origin) = self.render(filter, store);
        self.raster = raster;
        (origin.0 - before.0, origin.1 - before.1)
    }

    /// Re-render at the placement already set — after the source has been
    /// replaced, for instance.
    pub fn refresh(&mut self, filter: Resampling, store: &SourceStore) {
        let (raster, _) = self.render(filter, store);
        self.raster = raster;
    }

    /// Where the current raster's top-left sits relative to the source's, in
    /// the source's own coordinates.
    fn raster_origin(&self, store: &SourceStore) -> (i32, i32) {
        let (sw, sh) = self.source_size(store);
        let (w, h) = (sw as f32, sh as f32);
        let about = Transform::about(Vec2::new(w / 2.0, h / 2.0), self.placement);
        let bounds = about.transformed_bounds(crate::geom::IRect::new(0, 0, w as i32, h as i32));
        (bounds.x0, bounds.y0)
    }

    fn render(&self, filter: Resampling, store: &SourceStore) -> (PixelBuffer, (i32, i32)) {
        let Some(source) = store.pixels(self.source) else {
            // The store lost it, which should not happen and must not take the
            // window with it if it does.
            return (PixelBuffer::new(1, 1), (0, 0));
        };
        let (sw, sh) = (source.width(), source.height());
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
            let raster = resample::resize(source, w, h, filter);
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
        match resample::transform(source, (0, 0), about, filter, Some(clip)) {
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

    /// A store with one picture in it, and its identifier.
    fn stored(pixels: PixelBuffer) -> (SourceStore, SourceId) {
        let mut store = SourceStore::default();
        let id = store.add(pixels, "test");
        (store, id)
    }

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
        let (store, id) = stored(source.clone());
        let mut smart = SmartObject::new(id, &store);

        smart.place(Transform::scale(0.25, 0.25), Resampling::Bilinear, &store);
        assert_eq!(smart.raster().width(), 32);
        // What a raster layer would be left holding at this point.
        let flattened = smart.raster().clone();

        smart.place(Transform::IDENTITY, Resampling::Bilinear, &store);
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
        let (store, id) = stored(source);
        let mut smart = SmartObject::new(id, &store);
        // Twenty changes of mind.
        for i in 1..=20 {
            let k = 0.2 + (i % 7) as f32 * 0.3;
            smart.place(Transform::scale(k, k), Resampling::Bilinear, &store);
        }
        smart.place(Transform::scale(0.5, 0.5), Resampling::Bilinear, &store);
        let after_twenty = smart.raster().clone();

        let mut fresh = SmartObject::new(id, &store);
        fresh.place(Transform::scale(0.5, 0.5), Resampling::Bilinear, &store);
        assert_eq!(
            after_twenty.pixels(),
            fresh.raster().pixels(),
            "the twentieth placement should be as good as the first"
        );
    }

    #[test]
    fn the_picture_stays_where_it_was_when_it_is_scaled() {
        let (store, id) = stored(detailed(100, 100));
        let mut smart = SmartObject::new(id, &store);
        let shift = smart.place(Transform::scale(0.5, 0.5), Resampling::Bilinear, &store);
        // Halving a 100px square about its centre moves the top-left in by 25.
        assert_eq!(shift, (25, 25), "so the layer's offset can cancel it out");
        let back = smart.place(Transform::IDENTITY, Resampling::Bilinear, &store);
        assert_eq!(back, (-25, -25), "and the other way coming back");
    }

    #[test]
    fn rotation_grows_the_box_and_keeps_the_middle() {
        let (store, id) = stored(detailed(64, 64));
        let mut smart = SmartObject::new(id, &store);
        smart.place(Transform::rotate(std::f32::consts::FRAC_PI_4), Resampling::Bilinear, &store);
        let (w, h) = (smart.raster().width(), smart.raster().height());
        assert!(w > 84 && w < 96, "a 64px square turned 45 degrees is about 90 across: {w}");
        assert_eq!(w, h, "and square");
        // Still reversible, because the source was never touched.
        smart.place(Transform::IDENTITY, Resampling::Bilinear, &store);
        assert_eq!(smart.raster().width(), 64);
    }

    #[test]
    fn a_collapsed_placement_does_not_produce_a_layer_of_nothing() {
        let (store, id) = stored(detailed(32, 32));
        let mut smart = SmartObject::new(id, &store);
        smart.place(Transform::scale(0.0, 0.0), Resampling::Bilinear, &store);
        assert!(smart.raster().width() >= 1 && smart.raster().height() >= 1);
        smart.place(Transform::IDENTITY, Resampling::Bilinear, &store);
        assert_eq!(smart.raster().width(), 32, "and it comes back");
    }
}
