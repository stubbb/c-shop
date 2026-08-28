//! The layer tree: an arena of layers plus the ordering that defines the stack.
//!
//! Order convention throughout the codebase: **index 0 is the bottom** of the
//! stack, matching document and PSD order. The Layers panel iterates in reverse
//! because a layers panel shows the topmost layer first.

use crate::layer::{Layer, LayerId, LayerKind};
use ahash::AHashMap;

/// Where a layer sits relative to its siblings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerPos {
    /// `None` means the document root.
    pub parent: Option<LayerId>,
    /// Index among that parent's children, bottom-first.
    pub index: usize,
}

/// Arena of layers with parent/child links.
#[derive(Debug, Clone, Default)]
pub struct LayerTree {
    layers: AHashMap<LayerId, Layer>,
    root: Vec<LayerId>,
    next_id: u64,
}

impl LayerTree {
    pub fn new() -> Self {
        Self { layers: AHashMap::new(), root: Vec::new(), next_id: 1 }
    }

    /// Mint an id. Ids are never reused, so a stale [`LayerId`] resolves to
    /// `None` rather than aliasing a different layer.
    pub fn alloc_id(&mut self) -> LayerId {
        let id = LayerId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    pub fn get(&self, id: LayerId) -> Option<&Layer> {
        self.layers.get(&id)
    }

    pub fn get_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.get_mut(&id)
    }

    pub fn contains(&self, id: LayerId) -> bool {
        self.layers.contains_key(&id)
    }

    /// Children of `parent`, or the root list when `parent` is `None`.
    pub fn children(&self, parent: Option<LayerId>) -> &[LayerId] {
        match parent {
            None => &self.root,
            Some(p) => self.layers.get(&p).map(|l| l.children()).unwrap_or(&[]),
        }
    }

    pub fn root(&self) -> &[LayerId] {
        &self.root
    }

    /// Insert `layer` under `parent` at `index`, clamped to the sibling count.
    ///
    /// The layer's own `parent` field is overwritten, so callers cannot desync
    /// the two directions of the link.
    pub fn insert(&mut self, mut layer: Layer, parent: Option<LayerId>, index: usize) -> LayerId {
        let id = layer.id;
        layer.parent = parent;
        self.next_id = self.next_id.max(id.0 + 1);
        self.layers.insert(id, layer);

        let siblings = self.children_vec_mut(parent);
        let index = index.min(siblings.len());
        siblings.insert(index, id);
        id
    }

    /// Append to the top of `parent`'s children.
    pub fn push(&mut self, layer: Layer, parent: Option<LayerId>) -> LayerId {
        let n = self.children(parent).len();
        self.insert(layer, parent, n)
    }

    /// Detach `id` and its whole subtree.
    ///
    /// The returned layers are in **post-order** — every layer appears after
    /// all of its own descendants — and each group keeps its `children` list
    /// intact. Pass the result straight to [`LayerTree::restore`] to put it
    /// back; do not re-`insert` the layers one by one, or every group will
    /// gain a second copy of each child.
    pub fn remove(&mut self, id: LayerId) -> Vec<Layer> {
        let Some(layer) = self.layers.get(&id) else {
            return Vec::new();
        };
        let parent = layer.parent;
        let siblings = self.children_vec_mut(parent);
        if let Some(pos) = siblings.iter().position(|&s| s == id) {
            siblings.remove(pos);
        }

        let mut removed = Vec::new();
        self.take_subtree(id, &mut removed);
        removed
    }

    fn take_subtree(&mut self, id: LayerId, out: &mut Vec<Layer>) {
        let Some(layer) = self.layers.remove(&id) else {
            return;
        };
        for child in layer.children().to_vec() {
            self.take_subtree(child, out);
        }
        out.push(layer);
    }

    /// Put back a subtree taken by [`LayerTree::remove`], with `root` landing
    /// at `pos`.
    ///
    /// Parent and child links inside the subtree are already correct, so they
    /// are re-registered verbatim; only the root is threaded back into its
    /// parent's child list.
    pub fn restore(&mut self, layers: Vec<Layer>, root: LayerId, pos: LayerPos) {
        for layer in layers {
            self.next_id = self.next_id.max(layer.id.0 + 1);
            self.layers.insert(layer.id, layer);
        }
        if let Some(l) = self.layers.get_mut(&root) {
            l.parent = pos.parent;
        }
        let siblings = self.children_vec_mut(pos.parent);
        let index = pos.index.min(siblings.len());
        siblings.insert(index, root);
    }

    /// Current position of `id`, or `None` if it is not in the tree.
    pub fn position(&self, id: LayerId) -> Option<LayerPos> {
        let parent = self.layers.get(&id)?.parent;
        let index = self.children(parent).iter().position(|&s| s == id)?;
        Some(LayerPos { parent, index })
    }

    /// Reparent/reorder a single layer, keeping its subtree attached.
    ///
    /// Returns `false` — leaving the tree untouched — when the move is
    /// impossible: an unknown layer, a non-group destination, or dropping a
    /// group inside itself.
    pub fn move_to(&mut self, id: LayerId, parent: Option<LayerId>, index: usize) -> bool {
        if !self.layers.contains_key(&id) {
            return false;
        }
        if let Some(p) = parent {
            if !self.layers.get(&p).is_some_and(|l| l.kind.is_group()) {
                return false;
            }
            // A group cannot contain itself, directly or transitively.
            if p == id || self.is_ancestor(id, p) {
                return false;
            }
        }

        let old = self.position(id);
        let siblings = self.children_vec_mut(parent);

        // Compute the landing index before removal, then correct for the hole
        // that removal leaves behind when moving within one parent.
        let mut index = index.min(siblings.len());
        if let Some(old) = old {
            if old.parent == parent && old.index < index {
                index -= 1;
            }
        }

        if let Some(old) = old {
            let s = self.children_vec_mut(old.parent);
            if let Some(pos) = s.iter().position(|&x| x == id) {
                s.remove(pos);
            }
        }

        if let Some(l) = self.layers.get_mut(&id) {
            l.parent = parent;
        }
        let siblings = self.children_vec_mut(parent);
        let index = index.min(siblings.len());
        siblings.insert(index, id);
        true
    }

    /// `true` if `ancestor` is anywhere above `id` in the tree.
    pub fn is_ancestor(&self, ancestor: LayerId, id: LayerId) -> bool {
        let mut cur = self.layers.get(&id).and_then(|l| l.parent);
        while let Some(p) = cur {
            if p == ancestor {
                return true;
            }
            cur = self.layers.get(&p).and_then(|l| l.parent);
        }
        false
    }

    /// Chain of groups containing `id`, nearest parent first.
    pub fn ancestors(&self, id: LayerId) -> Vec<LayerId> {
        let mut out = Vec::new();
        let mut cur = self.layers.get(&id).and_then(|l| l.parent);
        while let Some(p) = cur {
            out.push(p);
            cur = self.layers.get(&p).and_then(|l| l.parent);
        }
        out
    }

    /// Nesting depth, used to indent rows in the Layers panel.
    pub fn depth(&self, id: LayerId) -> usize {
        self.ancestors(id).len()
    }

    /// A layer only renders when it and every enclosing group are visible.
    pub fn is_effectively_visible(&self, id: LayerId) -> bool {
        let Some(layer) = self.layers.get(&id) else {
            return false;
        };
        if !layer.contributes() {
            return false;
        }
        self.ancestors(id)
            .into_iter()
            .all(|p| self.layers.get(&p).is_some_and(|g| g.contributes()))
    }

    /// Every layer id, deepest-first within each subtree, bottom-to-top overall.
    pub fn iter_all(&self) -> Vec<LayerId> {
        let mut out = Vec::with_capacity(self.layers.len());
        self.collect(None, &mut out);
        out
    }

    fn collect(&self, parent: Option<LayerId>, out: &mut Vec<LayerId>) {
        for &id in self.children(parent) {
            out.push(id);
            if self.layers.get(&id).is_some_and(|l| l.kind.is_group()) {
                self.collect(Some(id), out);
            }
        }
    }

    /// Rows for the Layers panel: top-to-bottom, skipping the contents of
    /// collapsed groups. Each entry is `(id, depth)`.
    pub fn visible_rows(&self) -> Vec<(LayerId, usize)> {
        let mut out = Vec::new();
        self.collect_rows(None, 0, &mut out);
        out
    }

    fn collect_rows(&self, parent: Option<LayerId>, depth: usize, out: &mut Vec<(LayerId, usize)>) {
        for &id in self.children(parent).iter().rev() {
            out.push((id, depth));
            let expanded = self.layers.get(&id).is_some_and(|l| l.kind.is_group() && l.expanded);
            if expanded {
                self.collect_rows(Some(id), depth + 1, out);
            }
        }
    }

    /// Pick a sensible layer to select after `removed` disappears: the sibling
    /// that took its place, else the one below, else the parent group.
    pub fn neighbour_after_removal(&self, pos: LayerPos) -> Option<LayerId> {
        let siblings = self.children(pos.parent);
        if siblings.is_empty() {
            return pos.parent;
        }
        Some(siblings[pos.index.min(siblings.len() - 1)])
    }

    fn children_vec_mut(&mut self, parent: Option<LayerId>) -> &mut Vec<LayerId> {
        match parent {
            None => &mut self.root,
            Some(p) => match self.layers.get_mut(&p).map(|l| &mut l.kind) {
                Some(LayerKind::Group { children }) => children,
                // Inserting under a non-group is a caller bug; falling back to
                // the root keeps the layer reachable instead of leaking it.
                _ => {
                    log::warn!("insert under non-group layer {p:?}; falling back to root");
                    &mut self.root
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::PixelBuffer;

    fn tree_with_layers(n: usize) -> (LayerTree, Vec<LayerId>) {
        let mut t = LayerTree::new();
        let ids = (0..n)
            .map(|i| {
                let id = t.alloc_id();
                t.push(Layer::raster(id, format!("L{i}"), PixelBuffer::new(2, 2)), None);
                id
            })
            .collect();
        (t, ids)
    }

    #[test]
    fn push_stacks_bottom_to_top() {
        let (t, ids) = tree_with_layers(3);
        assert_eq!(t.root(), &ids[..]);
        // The panel shows the last-added layer first.
        let rows: Vec<_> = t.visible_rows().into_iter().map(|(id, _)| id).collect();
        assert_eq!(rows, vec![ids[2], ids[1], ids[0]]);
    }

    #[test]
    fn ids_are_never_reused() {
        let (mut t, ids) = tree_with_layers(2);
        t.remove(ids[0]);
        let fresh = t.alloc_id();
        assert!(!ids.contains(&fresh));
        assert!(t.get(ids[0]).is_none());
    }

    #[test]
    fn removing_a_group_takes_its_subtree() {
        let mut t = LayerTree::new();
        let g = t.alloc_id();
        t.push(Layer::group(g, "G"), None);
        let a = t.alloc_id();
        t.push(Layer::raster(a, "A", PixelBuffer::new(2, 2)), Some(g));
        let inner = t.alloc_id();
        t.push(Layer::group(inner, "Inner"), Some(g));
        let b = t.alloc_id();
        t.push(Layer::raster(b, "B", PixelBuffer::new(2, 2)), Some(inner));

        assert_eq!(t.len(), 4);
        let removed = t.remove(g);
        assert_eq!(removed.len(), 4);
        assert_eq!(t.len(), 0);

        // Post-order: every group appears after all of its descendants, which
        // is what makes a reversed walk safe.
        let order: Vec<_> = removed.iter().map(|l| l.id).collect();
        let pos = |id: LayerId| order.iter().position(|&x| x == id).unwrap();
        assert!(pos(b) < pos(inner), "child must precede its group");
        assert!(pos(inner) < pos(g) && pos(a) < pos(g));
        assert_eq!(*order.last().unwrap(), g, "the removed root comes last");
    }

    #[test]
    fn restore_puts_a_subtree_back_without_duplicating_links() {
        let mut t = LayerTree::new();
        let g = t.alloc_id();
        t.push(Layer::group(g, "G"), None);
        let a = t.alloc_id();
        t.push(Layer::raster(a, "A", PixelBuffer::new(2, 2)), Some(g));
        let inner = t.alloc_id();
        t.push(Layer::group(inner, "Inner"), Some(g));
        let b = t.alloc_id();
        t.push(Layer::raster(b, "B", PixelBuffer::new(2, 2)), Some(inner));

        let pos = t.position(g).unwrap();
        let removed = t.remove(g);
        t.restore(removed, g, pos);

        assert_eq!(t.len(), 4);
        assert_eq!(t.children(None), &[g]);
        assert_eq!(t.children(Some(g)), &[a, inner], "children must not be duplicated");
        assert_eq!(t.children(Some(inner)), &[b]);
        assert_eq!(t.get(b).unwrap().parent, Some(inner));
        assert_eq!(t.depth(b), 2);
    }

    #[test]
    fn move_into_group_updates_both_links() {
        let mut t = LayerTree::new();
        let g = t.alloc_id();
        t.push(Layer::group(g, "G"), None);
        let a = t.alloc_id();
        t.push(Layer::raster(a, "A", PixelBuffer::new(2, 2)), None);

        assert!(t.move_to(a, Some(g), 0));
        assert_eq!(t.get(a).unwrap().parent, Some(g));
        assert_eq!(t.children(Some(g)), &[a]);
        assert_eq!(t.root(), &[g]);
        assert_eq!(t.depth(a), 1);
    }

    #[test]
    fn a_group_cannot_be_moved_into_itself() {
        let mut t = LayerTree::new();
        let outer = t.alloc_id();
        t.push(Layer::group(outer, "Outer"), None);
        let inner = t.alloc_id();
        t.push(Layer::group(inner, "Inner"), Some(outer));

        assert!(!t.move_to(outer, Some(outer), 0));
        assert!(!t.move_to(outer, Some(inner), 0));
        assert_eq!(t.get(outer).unwrap().parent, None);
    }

    #[test]
    fn move_into_a_non_group_is_rejected() {
        let (mut t, ids) = tree_with_layers(2);
        assert!(!t.move_to(ids[0], Some(ids[1]), 0));
        assert_eq!(t.root().len(), 2);
    }

    #[test]
    fn reordering_within_a_parent_lands_on_the_intended_index() {
        // Dragging L0 to the top must yield [L1, L2, L0], not [L1, L0, L2]:
        // removal shifts every later index down by one.
        let (mut t, ids) = tree_with_layers(3);
        assert!(t.move_to(ids[0], None, 3));
        assert_eq!(t.root(), &[ids[1], ids[2], ids[0]]);

        let (mut t, ids) = tree_with_layers(3);
        assert!(t.move_to(ids[2], None, 0));
        assert_eq!(t.root(), &[ids[2], ids[0], ids[1]]);

        // A move onto its own position is a no-op.
        let (mut t, ids) = tree_with_layers(3);
        assert!(t.move_to(ids[1], None, 1));
        assert_eq!(t.root(), &[ids[0], ids[1], ids[2]]);
    }

    #[test]
    fn hidden_groups_hide_their_children() {
        let mut t = LayerTree::new();
        let g = t.alloc_id();
        t.push(Layer::group(g, "G"), None);
        let a = t.alloc_id();
        t.push(Layer::raster(a, "A", PixelBuffer::new(2, 2)), Some(g));

        assert!(t.is_effectively_visible(a));
        t.get_mut(g).unwrap().visible = false;
        assert!(!t.is_effectively_visible(a));
        t.get_mut(g).unwrap().visible = true;
        t.get_mut(g).unwrap().opacity = 0.0;
        assert!(!t.is_effectively_visible(a));
    }

    #[test]
    fn collapsed_groups_hide_their_rows() {
        let mut t = LayerTree::new();
        let g = t.alloc_id();
        t.push(Layer::group(g, "G"), None);
        let a = t.alloc_id();
        t.push(Layer::raster(a, "A", PixelBuffer::new(2, 2)), Some(g));

        assert_eq!(t.visible_rows(), vec![(g, 0), (a, 1)]);
        t.get_mut(g).unwrap().expanded = false;
        assert_eq!(t.visible_rows(), vec![(g, 0)]);
        // iter_all always walks everything, collapsed or not.
        assert_eq!(t.iter_all().len(), 2);
    }

    #[test]
    fn selection_falls_back_sensibly_after_delete() {
        let (mut t, ids) = tree_with_layers(3);
        let pos = t.position(ids[1]).unwrap();
        t.remove(ids[1]);
        // The layer that slid into the vacated slot.
        assert_eq!(t.neighbour_after_removal(pos), Some(ids[2]));
    }
}
