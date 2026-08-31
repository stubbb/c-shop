//! Frames, as an order over the layers that are already there.
//!
//! # Why a timeline is not a new kind of document
//!
//! An animation is a stack of pictures shown one at a time instead of
//! composited together. Everything the editor already does to a layer —
//! painting, masks, adjustments, effects, blend modes — is what someone
//! animating wants to do to a frame, and none of it has to be taught anything
//! new if a frame simply *is* a layer.
//!
//! So a timeline holds an order and a set of durations, and showing a frame is
//! setting which layers are visible. The compositor was already doing that. A
//! document without a timeline is a still picture, which is what nearly all of
//! them are.
//!
//! # What it deliberately does not do
//!
//! It does not own the layers. Deleting a layer that a frame named leaves a
//! frame naming nothing, and that is handled by skipping it rather than by
//! making the layer un-deletable — a timeline is a way of looking at a
//! document, and a way of looking at something should not stop you editing it.

use crate::layer::LayerId;
use crate::tree::LayerTree;

/// One frame: which layer, and how long it is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub layer: LayerId,
    pub delay_ms: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Timeline {
    pub frames: Vec<Frame>,
    /// `0` loops forever, which is what most animations mean.
    pub loops: u16,
    /// Which frame is showing.
    pub current: usize,
}

impl Timeline {
    /// A timeline over the layers as they stand, bottom to top, each held for
    /// `delay_ms`.
    pub fn from_layers(tree: &LayerTree, delay_ms: u16) -> Timeline {
        let frames = tree
            .iter_all()
            .into_iter()
            .filter(|&id| tree.get(id).is_some_and(|l| l.pixels().is_some()))
            .map(|layer| Frame { layer, delay_ms })
            .collect();
        Timeline { frames, loops: 0, current: 0 }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// How long the whole thing runs, in milliseconds.
    pub fn duration_ms(&self) -> u32 {
        self.frames.iter().map(|f| f.delay_ms as u32).sum()
    }

    /// Show frame `n`: its layer visible, every other frame's hidden.
    ///
    /// Layers that are not frames are left alone, so a background or an
    /// adjustment above the animation keeps working across all of it.
    pub fn show(&mut self, n: usize, tree: &mut LayerTree) {
        if self.frames.is_empty() {
            return;
        }
        let n = n.min(self.frames.len() - 1);
        self.current = n;
        for (i, frame) in self.frames.iter().enumerate() {
            if let Some(layer) = tree.get_mut(frame.layer) {
                layer.visible = i == n;
            }
        }
    }

    /// The next frame round, and how long to wait before asking again.
    pub fn advance(&mut self, tree: &mut LayerTree) -> u16 {
        if self.frames.is_empty() {
            return 100;
        }
        let next = (self.current + 1) % self.frames.len();
        self.show(next, tree);
        self.frames[next].delay_ms.max(1)
    }

    /// Forget frames whose layers have gone.
    ///
    /// Called after anything that removes a layer. A frame pointing at nothing
    /// would show an empty picture, which looks like a bug in the animation
    /// rather than the deletion it actually is.
    pub fn prune(&mut self, tree: &LayerTree) {
        self.frames.retain(|f| tree.get(f.layer).is_some());
        if self.current >= self.frames.len() {
            self.current = self.frames.len().saturating_sub(1);
        }
    }

    /// Set every frame's duration at once.
    pub fn set_all_delays(&mut self, delay_ms: u16) {
        for f in &mut self.frames {
            f.delay_ms = delay_ms.max(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;
    use crate::document::{Background, Document};
    use crate::layer::{Layer, LayerKind};
    use crate::pixels::PixelBuffer;

    fn doc_with(n: usize) -> (Document, Vec<LayerId>) {
        let mut doc = Document::new("anim", 8, 8, Background::Transparent);
        let mut ids = vec![doc.tree.iter_all()[0]];
        for i in 1..n {
            let id = doc.tree.alloc_id();
            doc.tree.push(
                Layer::new(
                    id,
                    format!("Frame {i}"),
                    LayerKind::raster(PixelBuffer::filled(8, 8, Rgba8::opaque(i as u8 * 40, 0, 0))),
                ),
                None,
            );
            ids.push(id);
        }
        (doc, ids)
    }

    #[test]
    fn showing_a_frame_shows_one_layer_and_hides_the_others() {
        let (mut doc, ids) = doc_with(4);
        let mut t = Timeline::from_layers(&doc.tree, 100);
        assert_eq!(t.len(), 4);

        t.show(2, &mut doc.tree);
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(
                doc.tree.get(*id).unwrap().visible,
                i == 2,
                "layer {i} while frame 2 is showing"
            );
        }
    }

    #[test]
    fn advancing_wraps_round_and_says_how_long_to_wait() {
        let (mut doc, _) = doc_with(3);
        let mut t = Timeline::from_layers(&doc.tree, 80);
        t.show(2, &mut doc.tree);
        assert_eq!(t.advance(&mut doc.tree), 80);
        assert_eq!(t.current, 0, "it loops");
    }

    /// A timeline is a way of looking at a document, so it must not stop
    /// anyone editing one.
    #[test]
    fn deleting_a_layer_prunes_its_frame_rather_than_breaking() {
        let (mut doc, ids) = doc_with(4);
        let mut t = Timeline::from_layers(&doc.tree, 100);
        doc.tree.remove(ids[1]);
        t.prune(&doc.tree);
        assert_eq!(t.len(), 3);
        assert!(t.frames.iter().all(|f| doc.tree.get(f.layer).is_some()));
    }

    #[test]
    fn pruning_keeps_the_current_frame_in_range() {
        let (mut doc, ids) = doc_with(3);
        let mut t = Timeline::from_layers(&doc.tree, 100);
        t.show(2, &mut doc.tree);
        doc.tree.remove(ids[2]);
        t.prune(&doc.tree);
        assert!(t.current < t.len());
    }

    /// Layers that are not frames belong to every frame: a background under
    /// the animation, or an adjustment over it.
    #[test]
    fn a_layer_that_is_not_a_frame_is_left_alone() {
        let (mut doc, ids) = doc_with(3);
        let mut t = Timeline::from_layers(&doc.tree, 100);
        // Take the last layer out of the timeline and hide nothing about it.
        t.frames.retain(|f| f.layer != ids[2]);
        doc.tree.get_mut(ids[2]).unwrap().visible = true;

        t.show(0, &mut doc.tree);
        assert!(doc.tree.get(ids[2]).unwrap().visible, "not a frame, so not hidden");
        assert!(doc.tree.get(ids[0]).unwrap().visible);
        assert!(!doc.tree.get(ids[1]).unwrap().visible);
    }

    #[test]
    fn the_duration_is_the_sum_of_the_frames() {
        let (doc, _) = doc_with(4);
        let mut t = Timeline::from_layers(&doc.tree, 125);
        assert_eq!(t.duration_ms(), 500);
        t.set_all_delays(40);
        assert_eq!(t.duration_ms(), 160);
    }

    #[test]
    fn an_empty_timeline_does_nothing_rather_than_panicking() {
        let (mut doc, _) = doc_with(2);
        let mut t = Timeline::default();
        t.show(3, &mut doc.tree);
        assert_eq!(t.advance(&mut doc.tree), 100);
        assert!(t.is_empty());
    }
}
