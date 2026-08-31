//! A document: canvas metadata plus the layer tree.
//!
//! The document owns no undo state. History is a sibling
//! ([`crate::history::History`]) that mutates a document through commands,
//! which keeps borrows simple and lets a command hold a snapshot of the very
//! document it edits.

use crate::color::Rgba8;
use crate::geom::IRect;
use crate::layer::{Layer, LayerId, LayerKind};
use crate::pixels::PixelBuffer;
use crate::profile::Profile;
use crate::mask::MaskBuffer;
use crate::selection::Selection;
use crate::tree::LayerTree;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-unique document identity.
///
/// Layer ids restart at 1 in every document, so anything caching per-layer
/// state (notably the GPU texture cache) needs this to tell two documents
/// apart. Cloning a document mints a new id rather than copying one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocumentId(pub u64);

impl DocumentId {
    fn fresh() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        DocumentId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// What an edit invalidated, so the renderer can do the minimum work.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dirty {
    /// Layers whose pixels changed and must be re-uploaded to the GPU.
    pub layers: Vec<LayerId>,
    /// Document-space region that must be recomposited.
    pub rect: IRect,
    /// The tree itself changed (add/remove/reorder/reparent), so cached
    /// composites and the panel row list are stale.
    pub structure: bool,
}

impl Dirty {
    pub const NONE: Dirty = Dirty { layers: Vec::new(), rect: IRect::EMPTY, structure: false };

    /// Recomposite `rect`, but no pixel re-upload.
    pub fn region(rect: IRect) -> Self {
        Self { layers: Vec::new(), rect, structure: false }
    }

    /// One layer's pixels changed within `rect`.
    pub fn pixels(layer: LayerId, rect: IRect) -> Self {
        Self { layers: vec![layer], rect, structure: false }
    }

    /// The tree changed; everything in `rect` is stale.
    pub fn structural(rect: IRect) -> Self {
        Self { layers: Vec::new(), rect, structure: true }
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty() && self.rect.is_empty() && !self.structure
    }

    pub fn merge(&mut self, other: Dirty) {
        self.rect = self.rect.union(&other.rect);
        self.structure |= other.structure;
        for l in other.layers {
            if !self.layers.contains(&l) {
                self.layers.push(l);
            }
        }
    }
}

/// A stored selection, shown in the Channels panel.
///
/// Conventionally called alpha channels: greyscale planes that live beside the
/// colour channels and can be loaded back as a selection at any time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlphaChannel {
    pub name: String,
    pub data: MaskBuffer,
    /// Whether the channel is shown as an overlay on the canvas.
    pub visible: bool,
}

/// Whether painting affects a layer's pixels or its mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditTarget {
    #[default]
    Pixels,
    Mask,
}

/// How a new document's bottom layer starts out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    White,
    Transparent,
    Color(Rgba8),
}

/// An open image.
#[derive(Debug)]
pub struct Document {
    /// Identity for cache invalidation; see [`DocumentId`].
    pub id: DocumentId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Pixels per inch; carried through I/O and used by print-oriented sizing.
    pub dpi: f32,
    /// The working space: what the numbers in this document's pixels mean.
    ///
    /// Everything arriving is converted into it and everything leaving is
    /// converted out of it, so within the document there is only ever one
    /// answer to what a colour is. See [`crate::profile`].
    pub profile: Profile,
    pub tree: LayerTree,
    /// The layer that tools act on.
    pub active: Option<LayerId>,
    /// Layer multi-selection for bulk operations. Always contains `active`
    /// when set. Distinct from [`Document::selection`], which is the *pixel*
    /// selection — the convention is "selected layers" for one and "selection"
    /// for the other, and so do we.
    pub selected_layers: Vec<LayerId>,
    /// The pixel selection, if any.
    ///
    /// `None` means no selection, so the whole canvas is editable. That is not
    /// the same as an empty selection, which protects everything.
    pub selection: Option<Selection>,
    /// Where this document was loaded from or last saved to.
    pub path: Option<PathBuf>,
    /// Set on every edit, cleared on save; drives the unsaved-changes marker.
    pub modified: bool,
    /// Saved selections.
    pub channels: Vec<AlphaChannel>,
    /// Whether tools write to the active layer's pixels or its mask.
    pub edit_target: EditTarget,
    /// The last selection that was deselected, so Reselect can bring it back.
    pub last_selection: Option<Selection>,
    /// Lines to line things up against. They belong to the document rather
    /// than the view, because where something should sit is a property of the
    /// design and not of who is looking at it.
    pub guides: Vec<crate::guides::Guide>,
}

impl Clone for Document {
    /// A cloned document is a *different* document, so it gets a fresh
    /// [`DocumentId`]. Sharing one would let the clone's layers collide with
    /// the original's entries in any per-layer cache.
    fn clone(&self) -> Self {
        Self {
            id: DocumentId::fresh(),
            name: self.name.clone(),
            width: self.width,
            height: self.height,
            dpi: self.dpi,
            profile: self.profile.clone(),
            tree: self.tree.clone(),
            active: self.active,
            selected_layers: self.selected_layers.clone(),
            selection: self.selection.clone(),
            path: self.path.clone(),
            modified: self.modified,
            channels: self.channels.clone(),
            edit_target: self.edit_target,
            last_selection: self.last_selection.clone(),
            guides: self.guides.clone(),
        }
    }
}

impl Document {
    /// Create a document with a single bottom layer.
    ///
    /// A `White` or `Color` background becomes a locked Background layer, the
    /// conventional; a transparent one becomes an ordinary "Layer 1".
    pub fn new(name: impl Into<String>, width: u32, height: u32, background: Background) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let mut tree = LayerTree::new();
        let id = tree.alloc_id();

        let mut layer = match background {
            Background::Transparent => {
                Layer::raster(id, "Layer 1", PixelBuffer::new(width, height))
            }
            Background::White => Layer::raster(
                id,
                "Background",
                PixelBuffer::filled(width, height, Rgba8::WHITE),
            ),
            Background::Color(c) => {
                Layer::raster(id, "Background", PixelBuffer::filled(width, height, c))
            }
        };
        if !matches!(background, Background::Transparent) {
            layer.is_background = true;
            layer.locks.position = true;
        }
        tree.push(layer, None);

        Self {
            id: DocumentId::fresh(),
            name: name.into(),
            width,
            height,
            dpi: 72.0,
            profile: Profile::srgb(),
            tree,
            active: Some(id),
            selected_layers: vec![id],
            selection: None,
            path: None,
            modified: false,
            channels: Vec::new(),
            edit_target: EditTarget::Pixels,
            last_selection: None,
            guides: Vec::new(),
        }
    }

    /// The one deep layer this document *is*, when that is all it is.
    ///
    /// Compositing happens on the GPU in half-float, which carries about
    /// eleven bits — better than eight and short of sixteen, and short of it
    /// because wgpu will not allow a sixteen-bit unorm texture as a colour
    /// attachment. So a document that needs no compositing should not be
    /// composited: a photograph opened, adjusted destructively and exported is
    /// one layer from beginning to end, and this is the path that keeps every
    /// one of its bits.
    ///
    /// Every condition here is one that would make the answer differ from the
    /// layer's own pixels. Anything else — a second layer, a mask, an effect,
    /// a blend mode, an opacity — has to go through the compositor, and gives
    /// up those bits knowingly.
    /// How many bits a channel this document's rasters hold.
    ///
    /// Sixteen if any one of them does: a document is as deep as its deepest
    /// layer, because that is the depth an export has to be written at for
    /// nothing to be lost.
    pub fn depth(&self) -> u8 {
        let deep = self
            .tree
            .iter_all()
            .into_iter()
            .filter_map(|id| self.tree.get(id)?.surface())
            .any(|s| s.depth() == 16);
        if deep { 16 } else { 8 }
    }

    pub fn single_deep_layer(&self) -> Option<&crate::pixels::DeepBuffer> {
        let ids = self.tree.iter_all();
        let [only] = ids[..] else { return None };
        let layer = self.tree.get(only)?;
        if !layer.visible
            || layer.mask.is_some()
            || layer.effects.any()
            || layer.opacity < 1.0
            || layer.fill_opacity < 1.0
            || layer.blend_mode != crate::blend::BlendMode::Normal
            || layer.offset != (0, 0)
        {
            return None;
        }
        let crate::layer::LayerKind::Raster(crate::layer::Surface::Sixteen(px)) = &layer.kind
        else {
            return None;
        };
        (px.width() == self.width && px.height() == self.height).then_some(px)
    }

    /// Wrap a decoded image as a single-layer document.
    pub fn from_image(name: impl Into<String>, pixels: PixelBuffer) -> Self {
        let (width, height) = (pixels.width().max(1), pixels.height().max(1));
        let mut tree = LayerTree::new();
        let id = tree.alloc_id();
        let mut layer = Layer::raster(id, "Background", pixels);
        layer.is_background = true;
        layer.locks.position = true;
        tree.push(layer, None);

        Self {
            id: DocumentId::fresh(),
            name: name.into(),
            width,
            height,
            dpi: 72.0,
            profile: Profile::srgb(),
            tree,
            active: Some(id),
            selected_layers: vec![id],
            selection: None,
            path: None,
            modified: false,
            channels: Vec::new(),
            edit_target: EditTarget::Pixels,
            last_selection: None,
            guides: Vec::new(),
        }
    }

    #[inline]
    pub fn bounds(&self) -> IRect {
        IRect::from_size(self.width, self.height)
    }

    /// Region an edit may touch: the selection's bounds when one exists,
    /// otherwise the whole canvas.
    pub fn editable_bounds(&self) -> IRect {
        match &self.selection {
            Some(s) => s.bounds(),
            None => self.bounds(),
        }
    }

    /// `true` when the pixel may be edited, i.e. there is no selection or the
    /// pixel is at least partly inside it.
    pub fn is_editable(&self, x: i32, y: i32) -> bool {
        match &self.selection {
            Some(s) => s.coverage(x, y) > 0,
            None => true,
        }
    }

    /// Selection coverage at a pixel, as a `0..=1` multiplier. Tools scale
    /// their own coverage by this.
    #[inline]
    pub fn selection_coverage(&self, x: i32, y: i32) -> f32 {
        match &self.selection {
            Some(s) => s.coverage(x, y) as f32 / 255.0,
            None => 1.0,
        }
    }

    /// Replace the pixel selection, dropping it entirely when the new one is
    /// empty. Keeping an empty selection would silently protect the whole
    /// document, which is never what a user means by deselecting.
    ///
    /// A selection that is on its way out is remembered so Reselect can bring
    /// it back.
    pub fn set_selection(&mut self, selection: Option<Selection>) {
        let next = match selection {
            Some(s) if s.is_empty() => None,
            other => other,
        };
        if next.is_none() {
            if let Some(previous) = self.selection.take() {
                self.last_selection = Some(previous);
            }
        }
        self.selection = next;
    }

    /// Whether the active layer has a mask that tools could target.
    pub fn active_has_mask(&self) -> bool {
        self.active_layer().is_some_and(|l| l.mask.is_some())
    }

    /// Editing a mask only makes sense while the active layer has one, so this
    /// falls back to the pixels rather than silently discarding edits.
    pub fn effective_edit_target(&self) -> EditTarget {
        if self.edit_target == EditTarget::Mask && self.active_has_mask() {
            EditTarget::Mask
        } else {
            EditTarget::Pixels
        }
    }

    /// Add a saved selection, named "Alpha 1", "Alpha 2" by convention.
    pub fn add_channel(&mut self, data: MaskBuffer) -> usize {
        let n = self
            .channels
            .iter()
            .filter_map(|c| c.name.strip_prefix("Alpha ").and_then(|n| n.parse::<u32>().ok()))
            .max()
            .unwrap_or(0)
            + 1;
        self.channels.push(AlphaChannel { name: format!("Alpha {n}"), data, visible: false });
        self.channels.len() - 1
    }

    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    pub fn active_layer(&self) -> Option<&Layer> {
        self.active.and_then(|id| self.tree.get(id))
    }

    pub fn active_layer_mut(&mut self) -> Option<&mut Layer> {
        match self.active {
            Some(id) => self.tree.get_mut(id),
            None => None,
        }
    }

    /// Select a single layer, replacing any multi-selection.
    pub fn select(&mut self, id: Option<LayerId>) {
        self.active = id.filter(|&id| self.tree.contains(id));
        self.selected_layers = self.active.into_iter().collect();
    }

    /// Add `id` to the multi-selection and make it active.
    pub fn select_add(&mut self, id: LayerId) {
        if !self.tree.contains(id) {
            return;
        }
        if !self.selected_layers.contains(&id) {
            self.selected_layers.push(id);
        }
        self.active = Some(id);
    }

    /// Drop layers that no longer exist, then re-point `active` if needed.
    ///
    /// Called after any structural change, so the UI never holds a dangling id.
    /// A document that still has layers always ends up with one selected —
    /// otherwise undoing past a layer's creation would leave the tools with
    /// nothing to act on.
    pub fn prune_selection(&mut self) {
        self.selected_layers.retain(|&id| self.tree.contains(id));
        if !self.active.is_some_and(|id| self.tree.contains(id)) {
            self.active = self.selected_layers.last().copied();
        }
        if self.active.is_none() {
            self.active = self.tree.root().last().copied();
        }
        if let Some(active) = self.active {
            if !self.selected_layers.contains(&active) {
                self.selected_layers.push(active);
            }
        }
    }

    /// A unique "Layer N" name, continuing past whatever already exists so
    /// duplicates never collide.
    pub fn next_layer_name(&self) -> String {
        let highest = self
            .tree
            .iter_all()
            .into_iter()
            .filter_map(|id| self.tree.get(id))
            .filter_map(|l| l.name.strip_prefix("Layer ").and_then(|n| n.parse::<u32>().ok()))
            .max()
            .unwrap_or(0);
        format!("Layer {}", highest + 1)
    }

    /// Union of every layer's extent, clamped to nothing when the document is
    /// empty. Used by Trim and by "fit to content".
    pub fn content_bounds(&self) -> IRect {
        self.tree
            .iter_all()
            .into_iter()
            .filter_map(|id| self.tree.get(id))
            .fold(IRect::EMPTY, |acc, l| acc.union(&l.bounds()))
    }

    /// Rough in-memory cost of the layer pixels, for the status bar.
    pub fn memory_bytes(&self) -> u64 {
        self.tree
            .iter_all()
            .into_iter()
            .filter_map(|id| self.tree.get(id))
            .map(|l| {
                let px = match &l.kind {
                    LayerKind::Raster(p) => p.width() as u64 * p.height() as u64 * 4,
                    _ => 0,
                };
                let mask = l
                    .mask
                    .as_ref()
                    .map(|m| m.data.width() as u64 * m.data.height() as u64)
                    .unwrap_or(0);
                px + mask
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_background_is_locked_and_opaque() {
        let d = Document::new("Untitled-1", 64, 48, Background::White);
        let l = d.active_layer().unwrap();
        assert_eq!(l.name, "Background");
        assert!(l.is_background && l.locks.blocks_move());
        assert_eq!(l.pixels().unwrap().get(0, 0), Rgba8::WHITE);
    }

    #[test]
    fn transparent_background_is_an_ordinary_layer() {
        let d = Document::new("Untitled-1", 8, 8, Background::Transparent);
        let l = d.active_layer().unwrap();
        assert_eq!(l.name, "Layer 1");
        assert!(!l.is_background && !l.locks.any());
    }

    #[test]
    fn zero_sized_documents_are_clamped() {
        let d = Document::new("x", 0, 0, Background::Transparent);
        assert_eq!((d.width, d.height), (1, 1));
    }

    #[test]
    fn pruning_always_leaves_something_selected() {
        let mut d = Document::new("d", 8, 8, Background::White);
        let first = d.active.unwrap();
        d.active = None;
        d.selected_layers.clear();
        d.prune_selection();
        assert_eq!(d.active, Some(first), "a document with layers must have an active one");
        assert_eq!(d.selected_layers, vec![first]);
    }

    #[test]
    fn pruning_an_empty_document_selects_nothing() {
        let mut d = Document::new("d", 8, 8, Background::White);
        let id = d.active.unwrap();
        d.tree.remove(id);
        d.prune_selection();
        assert_eq!(d.active, None);
        assert!(d.selected_layers.is_empty());
    }

    #[test]
    fn selection_is_pruned_after_removal() {
        let mut d = Document::new("d", 8, 8, Background::White);
        let first = d.active.unwrap();
        let id = d.tree.alloc_id();
        d.tree.push(Layer::raster(id, "L", PixelBuffer::new(4, 4)), None);
        d.select_add(id);
        assert_eq!(d.selected_layers.len(), 2);

        d.tree.remove(id);
        d.prune_selection();
        assert_eq!(d.selected_layers, vec![first]);
        assert_eq!(d.active, Some(first));
    }

    #[test]
    fn selecting_an_unknown_layer_clears_the_selection() {
        let mut d = Document::new("d", 8, 8, Background::White);
        d.select(Some(LayerId(9999)));
        assert_eq!(d.active, None);
        assert!(d.selected_layers.is_empty());
    }

    #[test]
    fn generated_names_do_not_collide() {
        let mut d = Document::new("d", 8, 8, Background::Transparent);
        assert_eq!(d.next_layer_name(), "Layer 2");
        let id = d.tree.alloc_id();
        d.tree.push(Layer::raster(id, "Layer 7", PixelBuffer::new(1, 1)), None);
        assert_eq!(d.next_layer_name(), "Layer 8");
    }

    #[test]
    fn content_bounds_span_every_layer() {
        let mut d = Document::new("d", 100, 100, Background::Transparent);
        let id = d.tree.alloc_id();
        let mut l = Layer::raster(id, "L", PixelBuffer::new(10, 10));
        l.offset = (120, 130);
        d.tree.push(l, None);
        assert_eq!(d.content_bounds(), IRect::new(0, 0, 130, 140));
    }

    #[test]
    fn documents_have_distinct_identities() {
        let a = Document::new("a", 4, 4, Background::White);
        let b = Document::new("b", 4, 4, Background::White);
        assert_ne!(a.id, b.id);
        // A clone is a separate document, so it must not share the identity.
        assert_ne!(a.clone().id, a.id);
    }

    #[test]
    fn no_selection_means_everything_is_editable() {
        let d = Document::new("d", 16, 16, Background::White);
        assert!(!d.has_selection());
        assert!(d.is_editable(0, 0));
        assert!(d.is_editable(15, 15));
        assert_eq!(d.editable_bounds(), IRect::new(0, 0, 16, 16));
        assert_eq!(d.selection_coverage(8, 8), 1.0);
    }

    #[test]
    fn a_selection_restricts_where_edits_land() {
        let mut d = Document::new("d", 16, 16, Background::White);
        d.set_selection(Some(Selection::from_rect(
            16,
            16,
            crate::selection::Rectf { x0: 4.0, y0: 4.0, x1: 12.0, y1: 12.0 },
            false,
        )));
        assert!(d.has_selection());
        assert!(d.is_editable(8, 8));
        assert!(!d.is_editable(0, 0));
        assert_eq!(d.editable_bounds(), IRect::new(4, 4, 12, 12));
        assert_eq!(d.selection_coverage(8, 8), 1.0);
        assert_eq!(d.selection_coverage(0, 0), 0.0);
    }

    #[test]
    fn an_empty_selection_is_stored_as_no_selection() {
        // Otherwise deselecting would leave the document entirely protected.
        let mut d = Document::new("d", 16, 16, Background::White);
        d.set_selection(Some(Selection::empty(16, 16)));
        assert!(!d.has_selection());
        assert!(d.is_editable(8, 8));
    }

    #[test]
    fn deselecting_remembers_the_previous_selection() {
        let mut d = Document::new("d", 16, 16, Background::White);
        let s = Selection::from_rect(
            16,
            16,
            crate::selection::Rectf { x0: 2.0, y0: 2.0, x1: 8.0, y1: 8.0 },
            false,
        );
        d.set_selection(Some(s));
        d.set_selection(None);
        assert!(!d.has_selection());
        assert_eq!(
            d.last_selection.as_ref().unwrap().bounds(),
            IRect::new(2, 2, 8, 8),
            "Reselect needs the discarded selection"
        );
    }

    #[test]
    fn the_mask_edit_target_falls_back_when_there_is_no_mask() {
        let mut d = Document::new("d", 16, 16, Background::White);
        d.edit_target = EditTarget::Mask;
        assert!(!d.active_has_mask());
        assert_eq!(d.effective_edit_target(), EditTarget::Pixels);

        let id = d.active.unwrap();
        d.tree.get_mut(id).unwrap().mask = Some(crate::layer::LayerMask::reveal_all(16, 16));
        assert_eq!(d.effective_edit_target(), EditTarget::Mask);
    }

    #[test]
    fn alpha_channels_get_unique_names() {
        let mut d = Document::new("d", 8, 8, Background::White);
        d.add_channel(MaskBuffer::hide_all(8, 8));
        d.add_channel(MaskBuffer::hide_all(8, 8));
        assert_eq!(d.channels[0].name, "Alpha 1");
        assert_eq!(d.channels[1].name, "Alpha 2");

        d.channels.remove(0);
        d.add_channel(MaskBuffer::hide_all(8, 8));
        assert_eq!(d.channels[1].name, "Alpha 3", "names continue past what exists");
    }

    #[test]
    fn dirty_merges_regions_and_dedups_layers() {
        let mut a = Dirty::pixels(LayerId(1), IRect::at(0, 0, 4, 4));
        a.merge(Dirty::pixels(LayerId(1), IRect::at(10, 10, 2, 2)));
        a.merge(Dirty::structural(IRect::EMPTY));
        assert_eq!(a.layers, vec![LayerId(1)]);
        assert_eq!(a.rect, IRect::new(0, 0, 12, 12));
        assert!(a.structure);
        assert!(!a.is_empty());
        assert!(Dirty::NONE.is_empty());
    }
}
