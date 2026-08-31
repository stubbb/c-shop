//! Layer states: a set of visibilities, positions and styles, remembered by
//! name.
//!
//! # What it is for
//!
//! Two versions of the same design usually differ by very little — this
//! headline instead of that one, the logo in the corner rather than the
//! middle, the price banner on or off. Keeping them as two documents means
//! every change that is *not* the difference has to be made twice, and one of
//! them is always slightly behind.
//!
//! A layer state records what each layer was doing, not what it contained, so
//! switching between states leaves every pixel alone. Edit the picture and both
//! versions have the edit; switch the state and you see the other version of
//! the same picture.
//!
//! # What it deliberately does not remember
//!
//! Pixels, masks, the tree's shape, or which layers exist. A state is a set of
//! settings on the layers that are there now — so a layer added since a state
//! was saved keeps whatever it is doing, rather than vanishing because a state
//! from before it existed did not mention it. Applying a state is a small,
//! predictable change, and one that can be undone in a single step.

use crate::blend::BlendMode;
use crate::effects::LayerEffects;
use crate::layer::LayerId;

/// What one layer was doing when a state was taken.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerSetting {
    pub id: LayerId,
    pub visible: bool,
    pub offset: (i32, i32),
    pub opacity: f32,
    pub fill_opacity: f32,
    pub blend_mode: BlendMode,
    pub clipping: bool,
    pub effects: LayerEffects,
    /// Whether the layer's mask was applying, if it had one.
    pub mask_enabled: bool,
}

/// A named set of them.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerState {
    pub name: String,
    pub layers: Vec<LayerSetting>,
}

impl LayerState {
    /// Take the state of every layer in the tree.
    pub fn capture(tree: &crate::tree::LayerTree, name: impl Into<String>) -> LayerState {
        let layers = tree
            .iter_all()
            .into_iter()
            .filter_map(|id| {
                let l = tree.get(id)?;
                Some(LayerSetting {
                    id,
                    visible: l.visible,
                    offset: l.offset,
                    opacity: l.opacity,
                    fill_opacity: l.fill_opacity,
                    blend_mode: l.blend_mode,
                    clipping: l.clipping,
                    effects: l.effects,
                    mask_enabled: l.mask.as_ref().is_some_and(|m| m.enabled),
                })
            })
            .collect();
        LayerState { name: name.into(), layers }
    }

    /// Put it back. Layers the state does not mention are left alone, and
    /// settings for layers that have since gone are ignored.
    pub fn apply(&self, tree: &mut crate::tree::LayerTree) {
        for s in &self.layers {
            let Some(l) = tree.get_mut(s.id) else { continue };
            l.visible = s.visible;
            l.offset = s.offset;
            l.opacity = s.opacity;
            l.fill_opacity = s.fill_opacity;
            l.blend_mode = s.blend_mode;
            l.clipping = s.clipping;
            l.effects = s.effects;
            if let Some(m) = &mut l.mask {
                m.enabled = s.mask_enabled;
            }
        }
    }

    /// Whether the tree currently matches this state, so the panel can show
    /// which one is showing.
    pub fn matches(&self, tree: &crate::tree::LayerTree) -> bool {
        self.layers.iter().all(|s| {
            tree.get(s.id).is_some_and(|l| {
                l.visible == s.visible
                    && l.offset == s.offset
                    && l.opacity == s.opacity
                    && l.fill_opacity == s.fill_opacity
                    && l.blend_mode == s.blend_mode
                    && l.clipping == s.clipping
                    && l.effects == s.effects
                    && l.mask.as_ref().is_none_or(|m| m.enabled == s.mask_enabled)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;
    use crate::document::{Background, Document};
    use crate::layer::{Layer, LayerKind};
    use crate::pixels::PixelBuffer;

    fn doc_with_two() -> (Document, LayerId, LayerId) {
        let mut doc = Document::new("d", 32, 32, Background::Transparent);
        let a = doc.tree.alloc_id();
        doc.tree.push(
            Layer::new(a, "A", LayerKind::raster(PixelBuffer::filled(8, 8, Rgba8::WHITE))),
            None,
        );
        let b = doc.tree.alloc_id();
        doc.tree.push(
            Layer::new(b, "B", LayerKind::raster(PixelBuffer::filled(8, 8, Rgba8::BLACK))),
            None,
        );
        (doc, a, b)
    }

    #[test]
    fn a_state_puts_back_what_it_took() {
        let (mut doc, a, b) = doc_with_two();
        doc.tree.get_mut(a).unwrap().visible = true;
        doc.tree.get_mut(b).unwrap().visible = false;
        doc.tree.get_mut(a).unwrap().offset = (5, 7);
        let first = LayerState::capture(&doc.tree, "A showing");

        // The other version.
        doc.tree.get_mut(a).unwrap().visible = false;
        doc.tree.get_mut(b).unwrap().visible = true;
        doc.tree.get_mut(a).unwrap().offset = (0, 0);
        let second = LayerState::capture(&doc.tree, "B showing");

        first.apply(&mut doc.tree);
        assert!(doc.tree.get(a).unwrap().visible);
        assert!(!doc.tree.get(b).unwrap().visible);
        assert_eq!(doc.tree.get(a).unwrap().offset, (5, 7));

        second.apply(&mut doc.tree);
        assert!(!doc.tree.get(a).unwrap().visible);
        assert!(doc.tree.get(b).unwrap().visible);
        assert_eq!(doc.tree.get(a).unwrap().offset, (0, 0));
    }

    /// The point of remembering settings rather than pixels: an edit made
    /// after a state was saved is still there when the state comes back.
    #[test]
    fn switching_states_leaves_the_pixels_alone() {
        let (mut doc, a, _b) = doc_with_two();
        let state = LayerState::capture(&doc.tree, "as it was");

        doc.tree.get_mut(a).unwrap().kind =
            LayerKind::raster(PixelBuffer::filled(8, 8, Rgba8::opaque(200, 30, 30)));
        state.apply(&mut doc.tree);
        assert_eq!(
            doc.tree.get(a).unwrap().pixels().unwrap().get(0, 0),
            Rgba8::opaque(200, 30, 30),
            "the edit survives the state coming back"
        );
    }

    /// A layer added since a state was saved should keep doing what it is
    /// doing, not disappear because an older state never heard of it.
    #[test]
    fn a_state_says_nothing_about_layers_it_never_saw() {
        let (mut doc, _a, _b) = doc_with_two();
        let state = LayerState::capture(&doc.tree, "before");

        let c = doc.tree.alloc_id();
        doc.tree.push(
            Layer::new(c, "C", LayerKind::raster(PixelBuffer::new(4, 4))),
            None,
        );
        doc.tree.get_mut(c).unwrap().visible = true;
        doc.tree.get_mut(c).unwrap().opacity = 0.4;

        state.apply(&mut doc.tree);
        assert!(doc.tree.get(c).unwrap().visible, "and it is left alone");
        assert!((doc.tree.get(c).unwrap().opacity - 0.4).abs() < 1e-6);
    }

    #[test]
    fn a_state_knows_whether_it_is_the_one_showing() {
        let (mut doc, a, _b) = doc_with_two();
        let state = LayerState::capture(&doc.tree, "now");
        assert!(state.matches(&doc.tree));
        doc.tree.get_mut(a).unwrap().opacity = 0.5;
        assert!(!state.matches(&doc.tree));
        state.apply(&mut doc.tree);
        assert!(state.matches(&doc.tree));
    }

    /// A state from a document whose layers have since been deleted must not
    /// take the program down with it.
    #[test]
    fn a_state_that_mentions_a_missing_layer_is_harmless() {
        let (mut doc, a, _b) = doc_with_two();
        let state = LayerState::capture(&doc.tree, "before the delete");
        doc.tree.remove(a);
        state.apply(&mut doc.tree);
        assert!(doc.tree.get(a).is_none());
    }
}
