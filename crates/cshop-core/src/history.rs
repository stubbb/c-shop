//! Undo/redo and the History panel.
//!
//! Every mutation goes through a [`Command`], which knows how to apply and
//! revert itself. Raster commands store only the *before* pixels of the region
//! they touch, so a brush stroke costs its bounding box rather than a copy of
//! the whole layer.

use crate::document::{Dirty, Document};
use crate::geom::IRect;
use crate::layer::{Layer, LayerId, LayerMask, Surface};
use crate::mask::MaskBuffer;
use crate::color::Rgba8;
use crate::pixels::PixelBuffer;
use crate::selection::{CompressedSelection, Selection};
use crate::tree::LayerPos;

/// A reversible edit.
///
/// `apply` may be called more than once (redo), so implementations must be
/// idempotent with respect to their stored state: capture what you need on the
/// first apply and reuse it afterwards.
pub trait Command: std::fmt::Debug + std::any::Any + Send {
    /// Label shown in the History panel, e.g. "Brush Tool".
    fn name(&self) -> String;
    fn apply(&mut self, doc: &mut Document) -> Dirty;
    fn revert(&mut self, doc: &mut Document) -> Dirty;

    /// Merge a subsequent command into this one, returning `true` if absorbed.
    ///
    /// Lets a run of slider drags collapse into a single history entry instead
    /// of flooding the panel.
    fn merge(&mut self, _next: &dyn Command) -> bool {
        false
    }

    /// Roughly how much this entry holds, so the history can bound itself by
    /// memory rather than only by how many entries there are.
    ///
    /// Most commands are a handful of fields and can leave this at zero. The
    /// ones that matter keep whole images: on a 6000x6000 document a single
    /// full-canvas fill holds 275 MB of before and after, and a hundred of
    /// those is more memory than any machine has.
    fn memory_bytes(&self) -> u64 {
        0
    }
}

/// Undo stack with a bounded depth.
/// Bytes a pixel buffer occupies.
fn pixel_bytes(px: &PixelBuffer) -> u64 {
    px.width() as u64 * px.height() as u64 * 4
}

fn mask_bytes(mask: &LayerMask) -> u64 {
    mask.data.width() as u64 * mask.data.height() as u64
}

/// Bytes a whole layer occupies, counting its pixels and its mask.
fn layer_bytes(layer: &Layer) -> u64 {
    let pixels = layer.pixels().map_or(0, pixel_bytes);
    let mask = layer.mask.as_ref().map_or(0, mask_bytes);
    pixels + mask
}

/// A pixel buffer as the undo stack keeps it.
///
/// Filling, clearing, and flattening onto a flat background all leave a region
/// of one colour behind, and keeping four bytes per pixel to say so is most of
/// what a large document's history weighs. Photographic content does not
/// compress cheaply and is kept as it is; the check that tells them apart
/// stops at the first pixel that differs.
#[derive(Debug)]
enum Stored {
    Uniform { width: u32, height: u32, color: Rgba8 },
    Raw(PixelBuffer),
}

impl Stored {
    fn of(px: PixelBuffer) -> Stored {
        match uniform_colour(&px) {
            Some(color) => {
                Stored::Uniform { width: px.width(), height: px.height(), color }
            }
            None => Stored::Raw(px),
        }
    }

    fn bytes(&self) -> u64 {
        match self {
            Stored::Uniform { .. } => 0,
            Stored::Raw(px) => pixel_bytes(px),
        }
    }

    /// Write this back into `dst` with its top-left at `(x, y)`.
    ///
    /// A uniform region is written as a fill rather than being expanded into a
    /// buffer first, so undoing one costs nothing either.
    fn paste_into(&self, dst: &mut PixelBuffer, x: i32, y: i32) {
        match self {
            Stored::Raw(px) => dst.paste(px, x, y),
            Stored::Uniform { width, height, color } => {
                dst.fill_rect(IRect::at(x, y, *width, *height), *color);
            }
        }
    }
}

/// The single colour of a buffer, if it has one.
fn uniform_colour(px: &PixelBuffer) -> Option<Rgba8> {
    use rayon::prelude::*;
    let first = *px.pixels().first()?;
    // `all` short-circuits, so photographic content stops almost immediately.
    px.pixels().par_iter().all(|p| *p == first).then_some(first)
}

/// How much the undo stack may hold before its oldest entries are dropped.
///
/// A count alone cannot bound it: an entry is a few bytes for a rename and
/// hundreds of megabytes for a fill across a large canvas, so a limit of two
/// hundred entries is either far too small or tens of gigabytes.
pub const DEFAULT_MEMORY_BUDGET: u64 = 2 << 30;

#[derive(Debug)]
pub struct History {
    entries: Vec<Box<dyn Command>>,
    /// Number of entries currently applied; everything at or past this index
    /// is redoable.
    cursor: usize,
    limit: usize,
    /// Ceiling on what the entries may hold between them.
    budget: u64,
    /// Label for the implicit state at cursor 0, e.g. "Open" or "New".
    origin: String,
    /// Entries dropped to stay inside the budget, so the panel can say so
    /// rather than leaving the oldest step to vanish unexplained.
    forgotten: usize,
}

impl History {
    pub fn new(origin: impl Into<String>) -> Self {
        Self {
            entries: Vec::new(),
            cursor: 0,
            limit: 200,
            budget: DEFAULT_MEMORY_BUDGET,
            origin: origin.into(),
            forgotten: 0,
        }
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit.max(1);
        self
    }

    pub fn with_budget(mut self, bytes: u64) -> Self {
        self.budget = bytes;
        self
    }

    /// What the entries hold between them.
    pub fn memory_bytes(&self) -> u64 {
        self.entries.iter().map(|c| c.memory_bytes()).sum()
    }

    /// How many steps have been dropped to stay inside the budget.
    pub fn forgotten(&self) -> usize {
        self.forgotten
    }

    /// Drop the oldest entries until the stack fits.
    ///
    /// One entry is always kept, however large. A single undo that cannot be
    /// afforded is still better than none, and refusing to record the edit at
    /// all would leave the document changed with no way back.
    fn trim(&mut self) {
        let mut excess = self.entries.len().saturating_sub(self.limit);
        let mut held: u64 = self.memory_bytes();
        while self.entries.len() - excess > 1 && held > self.budget {
            held -= self.entries[excess].memory_bytes();
            excess += 1;
        }
        if excess > 0 {
            self.entries.drain(0..excess);
            self.cursor = self.cursor.saturating_sub(excess);
            self.forgotten += excess;
        }
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Applied-entry count. Also the History panel's selected row, offset by
    /// the origin row at index 0.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.entries.len()
    }

    pub fn undo_name(&self) -> Option<String> {
        self.cursor.checked_sub(1).and_then(|i| self.entries.get(i)).map(|c| c.name())
    }

    pub fn redo_name(&self) -> Option<String> {
        self.entries.get(self.cursor).map(|c| c.name())
    }

    /// Entry labels, oldest first, for the History panel.
    /// The name of one state, counting the opened document as state zero —
    /// the same numbering the history panel and `jump_to` use.
    pub fn label_at(&self, state: usize) -> String {
        match state.checked_sub(1) {
            None => self.origin.clone(),
            Some(i) => self.entries.get(i).map_or_else(|| self.origin.clone(), |c| c.name()),
        }
    }

    pub fn labels(&self) -> Vec<String> {
        self.entries.iter().map(|c| c.name()).collect()
    }

    /// Apply `cmd` and record it, discarding any redo tail.
    pub fn apply(&mut self, doc: &mut Document, mut cmd: Box<dyn Command>) -> Dirty {
        let dirty = cmd.apply(doc);
        doc.modified = true;

        // A new edit invalidates the redo branch.
        self.entries.truncate(self.cursor);

        // Offer the previous entry a chance to absorb this one.
        if let Some(prev) = self.entries.last_mut() {
            if prev.merge(cmd.as_ref()) {
                return dirty;
            }
        }

        self.entries.push(cmd);
        self.cursor = self.entries.len();
        // Everything is applied, so trimming the oldest moves the cursor with
        // it rather than leaving it pointing past the end.
        self.trim();
        dirty
    }

    pub fn undo(&mut self, doc: &mut Document) -> Option<Dirty> {
        if !self.can_undo() {
            return None;
        }
        self.cursor -= 1;
        let dirty = self.entries[self.cursor].revert(doc);
        doc.modified = true;
        doc.prune_selection();
        Some(dirty)
    }

    pub fn redo(&mut self, doc: &mut Document) -> Option<Dirty> {
        if !self.can_redo() {
            return None;
        }
        let dirty = self.entries[self.cursor].apply(doc);
        self.cursor += 1;
        doc.modified = true;
        doc.prune_selection();
        Some(dirty)
    }

    /// Jump to an arbitrary history state by walking one step at a time, which
    /// is what clicking a row in the History panel does.
    pub fn jump_to(&mut self, doc: &mut Document, target: usize) -> Dirty {
        let target = target.min(self.entries.len());
        let mut dirty = Dirty::NONE;
        while self.cursor > target {
            if let Some(d) = self.undo(doc) {
                dirty.merge(d);
            } else {
                break;
            }
        }
        while self.cursor < target {
            if let Some(d) = self.redo(doc) {
                dirty.merge(d);
            } else {
                break;
            }
        }
        dirty
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Insert a layer at a position.
#[derive(Debug)]
pub struct AddLayer {
    layer: Option<Layer>,
    pos: LayerPos,
    id: LayerId,
    label: String,
}

impl AddLayer {
    pub fn new(layer: Layer, pos: LayerPos, label: impl Into<String>) -> Self {
        Self { id: layer.id, layer: Some(layer), pos, label: label.into() }
    }
}

impl Command for AddLayer {
    fn memory_bytes(&self) -> u64 {
        self.layer.as_ref().map_or(0, layer_bytes)
    }

    fn name(&self) -> String {
        self.label.clone()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        let Some(layer) = self.layer.take() else {
            return Dirty::NONE;
        };
        let bounds = layer.bounds();
        doc.tree.insert(layer, self.pos.parent, self.pos.index);
        doc.select(Some(self.id));
        Dirty::structural(if bounds.is_empty() { doc.bounds() } else { bounds })
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let bounds = doc.tree.get(self.id).map(|l| l.bounds()).unwrap_or(IRect::EMPTY);
        let mut removed = doc.tree.remove(self.id);
        // remove() is post-order, so the layer itself is last.
        self.layer = removed.pop();
        doc.prune_selection();
        Dirty::structural(if bounds.is_empty() { doc.bounds() } else { bounds })
    }
}

/// Several commands that undo as one step.
///
/// Layer via Copy both adds a layer and drops the selection. Left as two
/// entries the first undo would hand back the selection and leave the new
/// layer sitting there, which is not what the one gesture promised.
pub struct Compound {
    steps: Vec<Box<dyn Command>>,
    label: String,
}

impl Compound {
    pub fn new(label: impl Into<String>, steps: Vec<Box<dyn Command>>) -> Self {
        Self { steps, label: label.into() }
    }
}

impl std::fmt::Debug for Compound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compound").field("label", &self.label).field("steps", &self.steps.len()).finish()
    }
}

impl Command for Compound {
    fn name(&self) -> String {
        self.label.clone()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        let mut dirty = Dirty::NONE;
        for step in &mut self.steps {
            dirty.merge(step.apply(doc));
        }
        dirty
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        // Reverse order, or a step would undo against a document its
        // predecessor has not yet restored.
        let mut dirty = Dirty::NONE;
        for step in self.steps.iter_mut().rev() {
            dirty.merge(step.revert(doc));
        }
        dirty
    }
}

/// Remove a layer and its subtree.
#[derive(Debug)]
pub struct DeleteLayer {
    id: LayerId,
    /// Post-order, exactly as [`crate::tree::LayerTree::remove`] returns it.
    removed: Vec<Layer>,
    pos: Option<LayerPos>,
}

impl DeleteLayer {
    pub fn new(id: LayerId) -> Self {
        Self { id, removed: Vec::new(), pos: None }
    }
}

impl Command for DeleteLayer {
    fn memory_bytes(&self) -> u64 {
        self.removed.iter().map(layer_bytes).sum()
    }

    fn name(&self) -> String {
        "Delete Layer".to_string()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        self.pos = doc.tree.position(self.id);
        self.removed = doc.tree.remove(self.id);
        if let Some(pos) = self.pos {
            let next = doc.tree.neighbour_after_removal(pos);
            doc.select(next);
        }
        doc.prune_selection();
        Dirty::structural(doc.bounds())
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some(pos) = self.pos else {
            return Dirty::NONE;
        };
        // Links inside the subtree survived removal, so restore() re-registers
        // them wholesale. Re-inserting layer by layer would duplicate every
        // group's children.
        doc.tree.restore(std::mem::take(&mut self.removed), self.id, pos);
        doc.select(Some(self.id));
        Dirty::structural(doc.bounds())
    }
}

/// Reorder or reparent a layer.
#[derive(Debug)]
pub struct MoveLayer {
    id: LayerId,
    to: LayerPos,
    from: Option<LayerPos>,
}

impl MoveLayer {
    pub fn new(id: LayerId, to: LayerPos) -> Self {
        Self { id, to, from: None }
    }
}

impl Command for MoveLayer {
    fn name(&self) -> String {
        "Reorder Layer".to_string()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        self.from = doc.tree.position(self.id);
        doc.tree.move_to(self.id, self.to.parent, self.to.index);
        Dirty::structural(doc.bounds())
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        if let Some(from) = self.from {
            doc.tree.move_to(self.id, from.parent, from.index);
        }
        Dirty::structural(doc.bounds())
    }
}

/// Nudge or drag a layer's pixels without touching them.
#[derive(Debug)]
pub struct OffsetLayer {
    id: LayerId,
    delta: (i32, i32),
}

impl OffsetLayer {
    pub fn new(id: LayerId, delta: (i32, i32)) -> Self {
        Self { id, delta }
    }

    fn shift(&self, doc: &mut Document, dx: i32, dy: i32) -> Dirty {
        let Some(layer) = doc.tree.get_mut(self.id) else {
            return Dirty::NONE;
        };
        let before = layer.bounds();
        layer.offset.0 += dx;
        layer.offset.1 += dy;
        if let Some(mask) = &mut layer.mask {
            if mask.linked {
                mask.offset.0 += dx;
                mask.offset.1 += dy;
            }
        }
        let after = layer.bounds();
        // Both the vacated and the newly covered area need recompositing.
        Dirty::region(before.union(&after))
    }
}

impl Command for OffsetLayer {
    fn name(&self) -> String {
        "Move Layer".to_string()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        self.shift(doc, self.delta.0, self.delta.1)
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        self.shift(doc, -self.delta.0, -self.delta.1)
    }

    fn merge(&mut self, next: &dyn Command) -> bool {
        // Consecutive nudges of one layer collapse into a single undo step.
        let Some(next) = (next as &dyn std::any::Any).downcast_ref::<OffsetLayer>() else {
            return false;
        };
        if next.id != self.id {
            return false;
        }
        self.delta.0 += next.delta.0;
        self.delta.1 += next.delta.1;
        true
    }
}

/// Change one scalar or flag on a layer.
///
/// Kept as a single command with an enum payload so the whole Layers panel
/// needs one code path rather than a command type per control.
#[derive(Debug, Clone, PartialEq)]
pub enum LayerProperty {
    Name(String),
    Visible(bool),
    Opacity(f32),
    FillOpacity(f32),
    Blend(crate::blend::BlendMode),
    Clipping(bool),
    Expanded(bool),
    LockTransparency(bool),
    LockPixels(bool),
    LockPosition(bool),
    LockAll(bool),
}

impl LayerProperty {
    fn label(&self) -> &'static str {
        match self {
            LayerProperty::Name(_) => "Rename Layer",
            LayerProperty::Visible(_) => "Toggle Visibility",
            LayerProperty::Opacity(_) => "Layer Opacity",
            LayerProperty::FillOpacity(_) => "Fill Opacity",
            LayerProperty::Blend(_) => "Blend Mode",
            LayerProperty::Clipping(_) => "Clipping Mask",
            LayerProperty::Expanded(_) => "Expand Group",
            LayerProperty::LockTransparency(_)
            | LayerProperty::LockPixels(_)
            | LayerProperty::LockPosition(_)
            | LayerProperty::LockAll(_) => "Lock Layer",
        }
    }

    /// Purely a UI affordance, so it should not clutter the History panel.
    fn is_cosmetic(&self) -> bool {
        matches!(self, LayerProperty::Expanded(_))
    }

    /// Read the current value so it can be restored on undo.
    fn read(&self, layer: &Layer) -> LayerProperty {
        match self {
            LayerProperty::Name(_) => LayerProperty::Name(layer.name.clone()),
            LayerProperty::Visible(_) => LayerProperty::Visible(layer.visible),
            LayerProperty::Opacity(_) => LayerProperty::Opacity(layer.opacity),
            LayerProperty::FillOpacity(_) => LayerProperty::FillOpacity(layer.fill_opacity),
            LayerProperty::Blend(_) => LayerProperty::Blend(layer.blend_mode),
            LayerProperty::Clipping(_) => LayerProperty::Clipping(layer.clipping),
            LayerProperty::Expanded(_) => LayerProperty::Expanded(layer.expanded),
            LayerProperty::LockTransparency(_) => {
                LayerProperty::LockTransparency(layer.locks.transparency)
            }
            LayerProperty::LockPixels(_) => LayerProperty::LockPixels(layer.locks.pixels),
            LayerProperty::LockPosition(_) => LayerProperty::LockPosition(layer.locks.position),
            LayerProperty::LockAll(_) => LayerProperty::LockAll(layer.locks.all),
        }
    }

    fn write(&self, layer: &mut Layer) {
        match self.clone() {
            LayerProperty::Name(v) => layer.name = v,
            LayerProperty::Visible(v) => layer.visible = v,
            LayerProperty::Opacity(v) => layer.opacity = v.clamp(0.0, 1.0),
            LayerProperty::FillOpacity(v) => layer.fill_opacity = v.clamp(0.0, 1.0),
            LayerProperty::Blend(v) => layer.blend_mode = v,
            LayerProperty::Clipping(v) => layer.clipping = v,
            LayerProperty::Expanded(v) => layer.expanded = v,
            LayerProperty::LockTransparency(v) => layer.locks.transparency = v,
            LayerProperty::LockPixels(v) => layer.locks.pixels = v,
            LayerProperty::LockPosition(v) => layer.locks.position = v,
            LayerProperty::LockAll(v) => layer.locks.all = v,
        }
    }

    /// Same field on the same layer, so a slider drag can collapse.
    fn same_field(&self, other: &LayerProperty) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

#[derive(Debug)]
pub struct SetLayerProperty {
    id: LayerId,
    to: LayerProperty,
    from: Option<LayerProperty>,
}

impl SetLayerProperty {
    pub fn new(id: LayerId, to: LayerProperty) -> Self {
        Self { id, to, from: None }
    }

    fn dirty_for(&self, doc: &Document) -> Dirty {
        if self.to.is_cosmetic() {
            return Dirty::NONE;
        }
        let bounds = doc.tree.get(self.id).map(|l| l.bounds()).unwrap_or(IRect::EMPTY);
        // Groups and fill layers have no intrinsic bounds, and clipping affects
        // neighbours, so fall back to the whole canvas.
        if bounds.is_empty() || matches!(self.to, LayerProperty::Clipping(_)) {
            Dirty::region(doc.bounds())
        } else {
            Dirty::region(bounds)
        }
    }
}

impl Command for SetLayerProperty {
    fn name(&self) -> String {
        self.to.label().to_string()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        let Some(layer) = doc.tree.get_mut(self.id) else {
            return Dirty::NONE;
        };
        if self.from.is_none() {
            self.from = Some(self.to.read(layer));
        }
        self.to.write(layer);
        self.dirty_for(doc)
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let (Some(from), Some(layer)) = (&self.from, doc.tree.get_mut(self.id)) else {
            return Dirty::NONE;
        };
        from.write(layer);
        self.dirty_for(doc)
    }

    fn merge(&mut self, next: &dyn Command) -> bool {
        let Some(next) = (next as &dyn std::any::Any).downcast_ref::<SetLayerProperty>() else {
            return false;
        };
        if next.id != self.id || !next.to.same_field(&self.to) {
            return false;
        }
        // Keep the original "from" so undo jumps back to before the drag began.
        self.to = next.to.clone();
        true
    }
}

/// Replace a rectangular region of a raster layer's pixels.
///
/// This is the workhorse behind fills, clears, filters and committed brush
/// strokes. Only `rect` is stored, in both directions.
#[derive(Debug)]
pub struct ReplacePixels {
    id: LayerId,
    rect: IRect,
    after: Stored,
    before: Option<Stored>,
    label: String,
}

impl ReplacePixels {
    /// `after` must be exactly `rect`-sized.
    pub fn new(id: LayerId, rect: IRect, after: PixelBuffer, label: impl Into<String>) -> Self {
        Self { id, rect, after: Stored::of(after), before: None, label: label.into() }
    }

    fn paste(&self, doc: &mut Document, src: &Stored) -> Dirty {
        let Some(layer) = doc.tree.get_mut(self.id) else {
            return Dirty::NONE;
        };
        let offset = layer.offset;
        let Some(pixels) = layer.pixels_mut() else {
            return Dirty::NONE;
        };
        // `rect` is document space; the buffer is layer-local.
        src.paste_into(pixels, self.rect.x0 - offset.0, self.rect.y0 - offset.1);
        Dirty::pixels(self.id, self.rect)
    }
}

impl Command for ReplacePixels {
    fn memory_bytes(&self) -> u64 {
        self.after.bytes() + self.before.as_ref().map_or(0, Stored::bytes)
    }

    fn name(&self) -> String {
        self.label.clone()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.before.is_none() {
            let Some(layer) = doc.tree.get(self.id) else {
                return Dirty::NONE;
            };
            let offset = layer.offset;
            let Some(pixels) = layer.pixels() else {
                return Dirty::NONE;
            };
            self.before =
                Some(Stored::of(pixels.copy_rect(self.rect.translate(-offset.0, -offset.1))));
        }
        let after = std::mem::replace(&mut self.after, Stored::Raw(PixelBuffer::new(0, 0)));
        let dirty = self.paste(doc, &after);
        self.after = after;
        dirty
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some(before) = self.before.take() else {
            return Dirty::NONE;
        };
        let dirty = self.paste(doc, &before);
        self.before = Some(before);
        dirty
    }
}

/// Replace a layer's pixels and position wholesale — the result of a
/// transform, a crop, or anything else that changes a layer's geometry.
/// Give a smart object a new placement.
///
/// The counterpart of [`ReplaceLayerPixels`] for a layer that does not need
/// its pixels replaced: the placement is a setting, so a transform on a smart
/// object records the setting and re-renders from the source. Undo puts the
/// old setting back and re-renders again, which is why this holds no pixels
/// at all and costs the history nothing.
#[derive(Debug)]
pub struct PlaceSmart {
    id: LayerId,
    to: (crate::transform::Transform, (i32, i32)),
    before: Option<(crate::transform::Transform, (i32, i32))>,
    /// A linked mask still has to be resampled; it has no source to re-render
    /// from.
    after_mask: Option<Option<LayerMask>>,
    before_mask: Option<Option<LayerMask>>,
    filter: crate::resample::Resampling,
    label: String,
}

impl PlaceSmart {
    pub fn new(
        id: LayerId,
        placement: crate::transform::Transform,
        offset: (i32, i32),
        mask: Option<LayerMask>,
        filter: crate::resample::Resampling,
        label: impl Into<String>,
    ) -> Self {
        Self {
            id,
            to: (placement, offset),
            before: None,
            after_mask: Some(mask),
            before_mask: None,
            filter,
            label: label.into(),
        }
    }

    fn put(&self, doc: &mut Document, to: (crate::transform::Transform, (i32, i32))) -> Dirty {
        let bounds = doc.bounds();
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        layer.offset = to.1;
        if let Some(smart) = layer.smart_mut() {
            smart.place(to.0, self.filter);
        }
        Dirty { layers: vec![self.id], rect: bounds, structure: true }
    }
}

impl Command for PlaceSmart {
    fn name(&self) -> String {
        self.label.clone()
    }

    fn memory_bytes(&self) -> u64 {
        // A placement is nine numbers. The mask, if one came along, is not.
        self.before_mask.as_ref().and_then(|m| m.as_ref()).map_or(0, mask_bytes)
            + self.after_mask.as_ref().and_then(|m| m.as_ref()).map_or(0, mask_bytes)
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.before.is_none() {
            let Some(layer) = doc.tree.get(self.id) else { return Dirty::NONE };
            let Some(smart) = layer.smart() else { return Dirty::NONE };
            self.before = Some((smart.placement(), layer.offset));
            self.before_mask = Some(layer.mask.clone());
        }
        let dirty = self.put(doc, self.to);
        if let (Some(mask), Some(layer)) = (&self.after_mask, doc.tree.get_mut(self.id)) {
            layer.mask = mask.clone();
        }
        dirty
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some(before) = self.before else { return Dirty::NONE };
        let dirty = self.put(doc, before);
        if let (Some(mask), Some(layer)) = (&self.before_mask, doc.tree.get_mut(self.id)) {
            layer.mask = mask.clone();
        }
        dirty
    }
}

#[derive(Debug)]
pub struct ReplaceLayerPixels {
    id: LayerId,
    after: Option<(PixelBuffer, (i32, i32))>,
    before: Option<(PixelBuffer, (i32, i32))>,
    /// The mask moves with the layer when it is linked.
    after_mask: Option<Option<LayerMask>>,
    before_mask: Option<Option<LayerMask>>,
    label: String,
}

impl ReplaceLayerPixels {
    pub fn new(
        id: LayerId,
        pixels: PixelBuffer,
        offset: (i32, i32),
        mask: Option<LayerMask>,
        label: impl Into<String>,
    ) -> Self {
        Self {
            id,
            after: Some((pixels, offset)),
            before: None,
            after_mask: Some(mask),
            before_mask: None,
            label: label.into(),
        }
    }

    fn swap(&self, doc: &mut Document, pixels: &PixelBuffer, offset: (i32, i32)) -> Dirty {
        let bounds = doc.bounds();
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        layer.offset = offset;
        layer.kind = crate::layer::LayerKind::Raster(Surface::Eight(pixels.clone()));
        // The layer changed size, so the cache has to rebuild its texture.
        Dirty { layers: vec![self.id], rect: bounds, structure: true }
    }
}

impl Command for ReplaceLayerPixels {
    fn memory_bytes(&self) -> u64 {
        let px = |slot: &Option<(PixelBuffer, (i32, i32))>| {
            slot.as_ref().map_or(0, |(p, _)| pixel_bytes(p))
        };
        let mk = |slot: &Option<Option<LayerMask>>| {
            slot.as_ref().and_then(|m| m.as_ref()).map_or(0, mask_bytes)
        };
        px(&self.after) + px(&self.before) + mk(&self.after_mask) + mk(&self.before_mask)
    }

    fn name(&self) -> String {
        self.label.clone()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.before.is_none() {
            let Some(layer) = doc.tree.get(self.id) else { return Dirty::NONE };
            let Some(px) = layer.pixels() else { return Dirty::NONE };
            self.before = Some((px.clone(), layer.offset));
            self.before_mask = Some(layer.mask.clone());
        }
        let Some((pixels, offset)) = self.after.take() else { return Dirty::NONE };
        let dirty = self.swap(doc, &pixels, offset);
        if let Some(mask) = self.after_mask.clone() {
            if let Some(layer) = doc.tree.get_mut(self.id) {
                layer.mask = mask;
            }
        }
        self.after = Some((pixels, offset));
        dirty
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some((pixels, offset)) = self.before.clone() else { return Dirty::NONE };
        let dirty = self.swap(doc, &pixels, offset);
        if let Some(mask) = self.before_mask.clone() {
            if let Some(layer) = doc.tree.get_mut(self.id) {
                layer.mask = mask;
            }
        }
        dirty
    }
}

/// Change the canvas size without touching any pixels.
///
/// Layers keep their content and simply move, which is what Canvas Size does:
/// growing reveals more empty space, shrinking crops the view but leaves the
/// layers intact.
#[derive(Debug)]
pub struct ResizeCanvas {
    width: u32,
    height: u32,
    /// How far every layer shifts, from the anchor the user chose.
    shift: (i32, i32),
    before: Option<(u32, u32)>,
}

impl ResizeCanvas {
    pub fn new(width: u32, height: u32, shift: (i32, i32)) -> Self {
        Self { width: width.max(1), height: height.max(1), shift, before: None }
    }
}

impl Command for ResizeCanvas {
    fn name(&self) -> String {
        "Canvas Size".to_string()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.before.is_none() {
            self.before = Some((doc.width, doc.height));
        }
        doc.width = self.width;
        doc.height = self.height;
        for id in doc.tree.iter_all() {
            if let Some(layer) = doc.tree.get_mut(id) {
                layer.offset.0 += self.shift.0;
                layer.offset.1 += self.shift.1;
                if let Some(mask) = &mut layer.mask {
                    mask.offset.0 += self.shift.0;
                    mask.offset.1 += self.shift.1;
                }
            }
        }
        // A selection sized to the old canvas no longer means anything.
        doc.set_selection(None);
        Dirty::structural(doc.bounds())
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some((w, h)) = self.before else { return Dirty::NONE };
        doc.width = w;
        doc.height = h;
        for id in doc.tree.iter_all() {
            if let Some(layer) = doc.tree.get_mut(id) {
                layer.offset.0 -= self.shift.0;
                layer.offset.1 -= self.shift.1;
                if let Some(mask) = &mut layer.mask {
                    mask.offset.0 -= self.shift.0;
                    mask.offset.1 -= self.shift.1;
                }
            }
        }
        Dirty::structural(doc.bounds())
    }
}

/// Resample the whole document to new dimensions.
///
/// Every raster layer and mask is resampled, so this snapshots them all. That
/// is expensive in memory, but Image Size is a deliberate, occasional
/// operation and an exact undo is worth more than the bytes.
/// One layer's pixels, position and mask, as they were before a resize.
type LayerSnapshot = (LayerId, crate::layer::Surface, (i32, i32), Option<LayerMask>);

#[derive(Debug)]
pub struct ResizeImage {
    width: u32,
    height: u32,
    filter: crate::resample::Resampling,
    before: Option<(u32, u32, Vec<LayerSnapshot>)>,
}

impl ResizeImage {
    pub fn new(width: u32, height: u32, filter: crate::resample::Resampling) -> Self {
        Self { width: width.max(1), height: height.max(1), filter, before: None }
    }
}

impl Command for ResizeImage {
    fn memory_bytes(&self) -> u64 {
        self.before.as_ref().map_or(0, |(_, _, snaps)| {
            snaps
                .iter()
                .map(|(_, surface, _, mask)| {
                    surface.bytes() + mask.as_ref().map_or(0, mask_bytes)
                })
                .sum()
        })
    }

    fn name(&self) -> String {
        "Image Size".to_string()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.before.is_none() {
            let snapshot = doc
                .tree
                .iter_all()
                .into_iter()
                .filter_map(|id| {
                    let layer = doc.tree.get(id)?;
                    let surface = match &layer.kind {
                        crate::layer::LayerKind::Raster(s) => s.clone(),
                        // A vector layer's raster is a rendering of it; the
                        // resize redraws those from their own description.
                        _ => crate::layer::Surface::Eight(layer.pixels()?.clone()),
                    };
                    Some((id, surface, layer.offset, layer.mask.clone()))
                })
                .collect();
            self.before = Some((doc.width, doc.height, snapshot));
        }

        let sx = self.width as f64 / doc.width.max(1) as f64;
        let sy = self.height as f64 / doc.height.max(1) as f64;

        for id in doc.tree.iter_all() {
            let Some(layer) = doc.tree.get_mut(id) else { continue };
            let offset = layer.offset;
            // Scale the layer's own size and position by the same factors, so
            // a layer that filled the canvas still does.
            let new_offset = (
                (offset.0 as f64 * sx).round() as i32,
                (offset.1 as f64 * sy).round() as i32,
            );
            if let Some(px) = layer.pixels() {
                let w = ((px.width() as f64 * sx).round() as u32).max(1);
                let h = ((px.height() as f64 * sy).round() as u32).max(1);
                let resized = crate::resample::resize(px, w, h, self.filter);
                layer.kind = crate::layer::LayerKind::Raster(Surface::Eight(resized));
            }
            layer.offset = new_offset;

            if let Some(mask) = &mut layer.mask {
                let w = ((mask.data.width() as f64 * sx).round() as u32).max(1);
                let h = ((mask.data.height() as f64 * sy).round() as u32).max(1);
                // A mask that knows the path it came from is drawn again at
                // the new size rather than resampled, so its edge stays as
                // exact as it was. This is what a vector mask is *for*: a
                // painted one softens a little on every resize and there is
                // nothing to be done about it, because a picture of an edge
                // is all it has.
                if let Some(path) = mask.path.clone() {
                    let scaled = scale_path(&path, sx as f32, sy as f32);
                    mask.data = crate::layer::mask_from_path(&scaled, w, h, false);
                    mask.path = Some(Box::new(scaled));
                    mask.offset = (0, 0);
                } else {
                    mask.data = resize_mask(&mask.data, w, h);
                    mask.offset = (
                        (mask.offset.0 as f64 * sx).round() as i32,
                        (mask.offset.1 as f64 * sy).round() as i32,
                    );
                }
            }
        }

        doc.width = self.width;
        doc.height = self.height;
        doc.set_selection(None);
        Dirty::structural(doc.bounds())
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some((w, h, snapshot)) = self.before.clone() else { return Dirty::NONE };
        doc.width = w;
        doc.height = h;
        for (id, surface, offset, mask) in snapshot {
            if let Some(layer) = doc.tree.get_mut(id) {
                layer.kind = crate::layer::LayerKind::Raster(surface);
                layer.offset = offset;
                layer.mask = mask;
            }
        }
        Dirty::structural(doc.bounds())
    }
}

/// Swap one layer's pixels for a better version of the same picture.
///
/// Written for enlarging, where an ordinary resize runs first to move the
/// canvas, the offsets, the masks and the vector layers, and this then puts
/// the model's pixels in where the resize left stretched ones. It reads the
/// layer as it finds it rather than being told, precisely so that it can run
/// *after* something else has changed the geometry.
#[derive(Debug)]
pub struct UpscaleLayer {
    id: LayerId,
    after: PixelBuffer,
    before: Option<PixelBuffer>,
}

impl UpscaleLayer {
    pub fn new(id: LayerId, after: PixelBuffer) -> Self {
        Self { id, after, before: None }
    }
}

impl Command for UpscaleLayer {
    fn name(&self) -> String {
        "Upscale".to_string()
    }

    fn memory_bytes(&self) -> u64 {
        pixel_bytes(&self.after) + self.before.as_ref().map_or(0, pixel_bytes)
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        let Some(pixels) = layer.pixels_mut() else { return Dirty::NONE };
        // A model asked for one size can come back a pixel out on a rounding
        // boundary. The resize has already decided what this layer is, so
        // that decision wins.
        let (w, h) = (pixels.width(), pixels.height());
        if self.after.width() != w || self.after.height() != h {
            self.after = crate::resample::resize(
                &self.after,
                w,
                h,
                crate::resample::Resampling::Lanczos3,
            );
        }
        if self.before.is_none() {
            self.before = Some(pixels.clone());
        }
        *pixels = self.after.clone();
        Dirty::pixels(self.id, layer.bounds())
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some(before) = self.before.clone() else { return Dirty::NONE };
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        let Some(pixels) = layer.pixels_mut() else { return Dirty::NONE };
        *pixels = before;
        Dirty::pixels(self.id, layer.bounds())
    }
}

/// Change what a document's numbers mean, and optionally the numbers.
///
/// The two things this can do are worth keeping apart, because they are
/// opposites and both are sometimes right:
///
/// * **Assign** leaves every pixel alone and changes the profile. Nothing is
///   recomputed; the picture looks different, because the same numbers are now
///   being read as a different set of colours. This is the repair for a file
///   that arrived labelled wrongly, or labelled not at all.
/// * **Convert** rewrites every pixel so that it looks the same in the new
///   space as it did in the old one. The numbers change so that the colours
///   need not.
///
/// Converting is not free and not lossless: a colour the new space cannot
/// reach is clipped to its nearest neighbour, and at eight bits a channel the
/// journey costs precision even where nothing is clipped. Undo keeps the whole
/// document, which is why this reports its size honestly.
/// What one layer held before a conversion.
///
/// Not simply "its pixels": a type layer's pixels are a rendering of its text,
/// and putting them back as pixels would undo a conversion by turning the
/// type into a picture of itself.
#[derive(Debug, Clone)]
enum Unconverted {
    Raster(Surface),
    Fill(crate::color::Rgba8),
    Text(Box<crate::text::TextContent>),
    Shape(Box<crate::shape::ShapeContent>),
}

/// Move every raster layer in the document to eight or sixteen bits a channel.
///
/// Widening invents nothing and can always be undone by narrowing again.
/// Narrowing throws away what eight bits cannot hold, so this keeps the whole
/// of every layer it touched: undo is exact, at the cost of the memory, which
/// is what the history's budget is for.
#[derive(Debug)]
pub struct SetDepth {
    to: u8,
    before: Vec<(LayerId, Surface)>,
    taken: bool,
}

impl SetDepth {
    /// `bits` is 8 or 16; anything else is treated as 8.
    pub fn new(bits: u8) -> Self {
        Self { to: if bits == 16 { 16 } else { 8 }, before: Vec::new(), taken: false }
    }
}

impl Command for SetDepth {
    fn name(&self) -> String {
        format!("{} Bits a Channel", self.to)
    }

    fn memory_bytes(&self) -> u64 {
        self.before.iter().map(|(_, s)| s.bytes()).sum()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if !self.taken {
            self.before = doc
                .tree
                .iter_all()
                .into_iter()
                .filter_map(|id| {
                    let s = doc.tree.get(id)?.surface()?;
                    (s.depth() != self.to).then(|| (id, s.clone()))
                })
                .collect();
            self.taken = true;
        }
        for (id, _) in &self.before {
            let Some(layer) = doc.tree.get_mut(*id) else { continue };
            if let Some(s) = layer.surface_mut() {
                *s = s.at_depth(self.to);
            }
        }
        Dirty::structural(doc.bounds())
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        for (id, was) in &self.before {
            let Some(layer) = doc.tree.get_mut(*id) else { continue };
            if let Some(s) = layer.surface_mut() {
                *s = was.clone();
            }
        }
        Dirty::structural(doc.bounds())
    }
}

#[derive(Debug)]
pub struct SetProfile {
    to: crate::profile::Profile,
    convert: bool,
    before: Option<(crate::profile::Profile, Vec<(LayerId, Unconverted)>)>,
}

impl SetProfile {
    /// Change the meaning, leave the pixels.
    pub fn assign(to: crate::profile::Profile) -> Self {
        Self { to, convert: false, before: None }
    }

    /// Change the pixels, keep the appearance.
    pub fn convert(to: crate::profile::Profile) -> Self {
        Self { to, convert: true, before: None }
    }
}

impl Command for SetProfile {
    fn name(&self) -> String {
        if self.convert { "Convert to Profile".into() } else { "Assign Profile".into() }
    }

    fn memory_bytes(&self) -> u64 {
        self.before.as_ref().map_or(0, |(_, snaps)| {
            snaps
                .iter()
                .map(|(_, held)| match held {
                    Unconverted::Raster(s) => s.bytes(),
                    _ => 0,
                })
                .sum()
        })
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.before.is_none() {
            // Assigning touches nothing, so it has nothing to remember.
            let snapshot = if self.convert {
                doc.tree
                    .iter_all()
                    .into_iter()
                    .filter_map(|id| {
                        let layer = doc.tree.get(id)?;
                        let held = match &layer.kind {
                            crate::layer::LayerKind::Raster(px) => {
                                Unconverted::Raster(px.clone())
                            }
                            crate::layer::LayerKind::Fill(
                                crate::layer::FillStyle::Solid(c),
                            ) => Unconverted::Fill(*c),
                            crate::layer::LayerKind::Text(t) => {
                                Unconverted::Text(Box::new(t.content().clone()))
                            }
                            crate::layer::LayerKind::Shape(sh) => {
                                Unconverted::Shape(Box::new(sh.content().clone()))
                            }
                            _ => return None,
                        };
                        Some((id, held))
                    })
                    .collect()
            } else {
                Vec::new()
            };
            self.before = Some((doc.profile.clone(), snapshot));
        }

        if self.convert && !doc.profile.same_transform(&self.to) {
            let from = doc.profile.clone();
            let intent = crate::profile::RenderingIntent::RelativeColorimetric;
            let recolour = |c: &mut crate::color::Rgba8| {
                let mut one = [*c];
                if from.convert_rgba8(&self.to, &mut one, intent).is_ok() {
                    *c = one[0];
                }
            };
            for id in doc.tree.iter_all() {
                let Some(layer) = doc.tree.get_mut(id) else { continue };
                // A deep layer is converted at its own depth. Narrowing it to
                // eight for the transform and widening it back would throw
                // away exactly the bits it was kept deep for.
                let converted = match layer.surface_mut() {
                    Some(crate::layer::Surface::Eight(px)) => {
                        from.convert_rgba8(&self.to, px.pixels_mut(), intent)
                    }
                    Some(crate::layer::Surface::Sixteen(px)) => {
                        from.convert_rgba16(&self.to, px.pixels_mut(), intent)
                    }
                    None => Ok(()),
                };
                if let Err(e) = converted {
                    // Leave the pixels alone rather than half-converted.
                    log::warn!("colour conversion failed on one layer: {e}");
                }
                // A vector layer keeps the colour it was drawn from, and the
                // next re-render would put the old one back. So those are
                // converted at the source and redrawn, which also spares them
                // the converted-raster-of-a-converted-colour they would get
                // otherwise. These are the only four places in a document
                // where a colour lives outside a raster.
                match &mut layer.kind {
                    crate::layer::LayerKind::Fill(crate::layer::FillStyle::Solid(c)) => {
                        recolour(c)
                    }
                    crate::layer::LayerKind::Text(t) => {
                        let mut content = t.content().clone();
                        recolour(&mut content.style.color);
                        t.set_content(content);
                    }
                    crate::layer::LayerKind::Shape(sh) => {
                        let mut content = sh.content().clone();
                        if let Some(c) = content.style.fill.as_mut() {
                            recolour(c);
                        }
                        if let Some(c) = content.style.stroke.as_mut() {
                            recolour(c);
                        }
                        sh.set_content(content);
                    }
                    _ => {}
                }
            }
        }
        doc.profile = self.to.clone();
        Dirty::structural(doc.bounds())
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some((profile, snapshot)) = self.before.clone() else { return Dirty::NONE };
        doc.profile = profile;
        for (id, held) in snapshot {
            let Some(layer) = doc.tree.get_mut(id) else { continue };
            match held {
                Unconverted::Raster(s) => layer.kind = crate::layer::LayerKind::Raster(s),
                Unconverted::Fill(c) => {
                    layer.kind =
                        crate::layer::LayerKind::Fill(crate::layer::FillStyle::Solid(c))
                }
                Unconverted::Text(content) => {
                    if let Some(t) = layer.text_mut() {
                        t.set_content(*content);
                    }
                }
                Unconverted::Shape(content) => {
                    if let Some(sh) = layer.shape_mut() {
                        sh.set_content(*content);
                    }
                }
            }
        }
        Dirty::structural(doc.bounds())
    }
}

/// Resize a coverage mask by resampling it as a greyscale image.
/// Every anchor and handle multiplied through, so a path scales with the
/// document it is drawn in.
fn scale_path(path: &crate::path::PathShape, sx: f32, sy: f32) -> crate::path::PathShape {
    let mut out = path.clone();
    for part in &mut out.parts {
        for sub in &mut part.subpaths {
            for a in &mut sub.anchors {
                let go = |v: &mut crate::geom::Vec2| {
                    v.x *= sx;
                    v.y *= sy;
                };
                // A handle is a control point in the same space as the
                // anchor, not an offset from it, so all three scale the same
                // way about the origin and none of them needs re-centring.
                go(&mut a.at);
                go(&mut a.in_handle);
                go(&mut a.out_handle);
            }
        }
    }
    out
}

fn resize_mask(mask: &MaskBuffer, width: u32, height: u32) -> MaskBuffer {
    let mut as_pixels = PixelBuffer::new(mask.width(), mask.height());
    for y in 0..mask.height() as i32 {
        for x in 0..mask.width() as i32 {
            let v = mask.get(x, y);
            as_pixels.set(x, y, crate::color::Rgba8::new(v, v, v, 255));
        }
    }
    let resized =
        crate::resample::resize(&as_pixels, width, height, crate::resample::Resampling::Bilinear);
    let mut out = MaskBuffer::hide_all(width, height);
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            out.set(x, y, resized.get(x, y).r);
        }
    }
    out
}

/// Swap one layer kind for another, keeping everything else about the layer.
///
/// The general form of [`RasterizeLayer`], for the direction that goes the
/// other way: wrapping pixels in a smart object, where the picture on screen
/// does not change and only what can be done to it next does.
#[derive(Debug)]
pub struct ReplaceLayerKind {
    id: LayerId,
    to: Option<crate::layer::LayerKind>,
    was: Option<crate::layer::LayerKind>,
    label: String,
}

impl ReplaceLayerKind {
    pub fn new(id: LayerId, to: crate::layer::LayerKind, label: impl Into<String>) -> Self {
        Self { id, to: Some(to), was: None, label: label.into() }
    }

    fn swap(&mut self, doc: &mut Document, into: bool) -> Dirty {
        let slot = if into { &mut self.to } else { &mut self.was };
        let Some(next) = slot.take() else { return Dirty::NONE };
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        let previous = std::mem::replace(&mut layer.kind, next);
        if into {
            self.was = Some(previous);
        } else {
            self.to = Some(previous);
        }
        Dirty::structural(layer.bounds())
    }
}

impl Command for ReplaceLayerKind {
    fn name(&self) -> String {
        self.label.clone()
    }

    fn memory_bytes(&self) -> u64 {
        let held = |k: &Option<crate::layer::LayerKind>| match k {
            Some(crate::layer::LayerKind::Smart(s)) => {
                let (w, h) = s.source_size();
                w as u64 * h as u64 * 4
            }
            Some(crate::layer::LayerKind::Raster(s)) => s.bytes(),
            _ => 0,
        };
        held(&self.to) + held(&self.was)
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        self.swap(doc, true)
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        self.swap(doc, false)
    }
}

/// Turn a vector layer — type or a shape — into ordinary pixels.
///
/// The raster is the one the layer was already showing, so rasterising changes
/// nothing on screen; it only changes what can be done to the layer next.
#[derive(Debug)]
pub struct RasterizeLayer {
    id: LayerId,
    pixels: Option<crate::pixels::PixelBuffer>,
    /// What it was, so undo can put it back.
    was: Option<crate::layer::LayerKind>,
    label: String,
}

impl RasterizeLayer {
    pub fn new(
        id: LayerId,
        pixels: crate::pixels::PixelBuffer,
        label: impl Into<String>,
    ) -> Self {
        Self { id, pixels: Some(pixels), was: None, label: label.into() }
    }
}

impl Command for RasterizeLayer {
    fn memory_bytes(&self) -> u64 {
        self.pixels.as_ref().map_or(0, pixel_bytes)
    }

    fn name(&self) -> String {
        self.label.clone()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        let Some(pixels) = self.pixels.take() else { return Dirty::NONE };
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        self.was = Some(std::mem::replace(&mut layer.kind, crate::layer::LayerKind::Raster(Surface::Eight(pixels))));
        // Nothing moves and nothing changes colour; only the layer's kind.
        Dirty::structural(layer.bounds())
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some(was) = self.was.take() else { return Dirty::NONE };
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        if let crate::layer::LayerKind::Raster(s) = std::mem::replace(&mut layer.kind, was) {
            // Rasterising made this from a vector layer, so it is eight-bit by
            // construction; taking it back keeps it that way.
            self.pixels = s.eight().cloned();
        }
        Dirty::structural(layer.bounds())
    }
}

/// Resize by carving seams, with the carving already done.
///
/// The work happens off the main thread and arrives here finished, because
/// eight seconds on a twelve-megapixel photograph is not something to do with
/// the interface stopped. What is left is the same shape as any other edit:
/// put the new pixels in, remember the old, and undo by putting them back —
/// which it has to hold, since removing a seam is not something an inverse can
/// undo.
#[derive(Debug)]
pub struct CarvedResize {
    to: Option<Vec<(LayerId, PixelBuffer)>>,
    size: (u32, u32),
    before: Option<Box<Undone>>,
}

impl CarvedResize {
    pub fn new(layers: Vec<(LayerId, PixelBuffer)>, width: u32, height: u32) -> Self {
        Self { to: Some(layers), size: (width, height), before: None }
    }
}

impl Command for CarvedResize {
    fn name(&self) -> String {
        "Content-Aware Scale".into()
    }

    fn memory_bytes(&self) -> u64 {
        self.before.as_ref().map_or(0, |b| {
            b.layers
                .iter()
                .map(|(_, kind, _, mask)| {
                    let px = match kind {
                        crate::layer::LayerKind::Raster(s) => s.bytes(),
                        _ => 0,
                    };
                    px + mask.as_ref().map_or(0, mask_bytes)
                })
                .sum()
        })
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        let Some(carved) = self.to.take() else { return Dirty::NONE };
        if self.before.is_none() {
            let layers = doc
                .tree
                .iter_all()
                .into_iter()
                .filter_map(|id| {
                    let l = doc.tree.get(id)?;
                    Some((id, l.kind.clone(), l.offset, l.mask.clone()))
                })
                .collect();
            self.before = Some(Box::new(Undone {
                size: (doc.width, doc.height),
                layers,
                selection: doc.selection.as_ref().map(|s| s.compress()),
            }));
        }
        for (id, px) in carved {
            let Some(layer) = doc.tree.get_mut(id) else { continue };
            layer.kind = crate::layer::LayerKind::Raster(Surface::Eight(px));
            layer.offset = (0, 0);
            // A mask cannot be carved along with the layer — the seams are
            // chosen from the picture, and a mask has no picture — so it is
            // resampled, which is what it would have got from a plain resize.
            if let Some(mask) = &mut layer.mask {
                mask.data = resize_mask(&mask.data, self.size.0, self.size.1);
                mask.offset = (0, 0);
                mask.path = None;
            }
        }
        doc.width = self.size.0;
        doc.height = self.size.1;
        doc.set_selection(None);
        Dirty::structural(doc.bounds())
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some(before) = self.before.take() else { return Dirty::NONE };
        let mut carved = Vec::new();
        doc.width = before.size.0;
        doc.height = before.size.1;
        for (id, kind, offset, mask) in before.layers {
            if let Some(layer) = doc.tree.get_mut(id) {
                if let crate::layer::LayerKind::Raster(s) =
                    std::mem::replace(&mut layer.kind, kind)
                {
                    if let Some(px) = s.eight() {
                        carved.push((id, px.clone()));
                    }
                }
                layer.offset = offset;
                layer.mask = mask;
            }
        }
        self.to = Some(carved);
        doc.set_selection(before.selection.map(|c| c.restore()));
        Dirty::structural(doc.bounds())
    }
}

/// Straighten a photographed rectangle and crop to it in one step.
///
/// An ordinary crop is a canvas resize: nothing is resampled, because nothing
/// moves. This one is not — the four corners the user dragged are the corners
/// of something rectangular in the world, and putting it back means undoing
/// the projection that made it a quadrilateral. Every layer goes through the
/// same projective transform, so they stay registered with each other.
///
/// The size to land on comes from the quad's own edges: the average of the two
/// horizontal edges and of the two vertical ones. A photographed rectangle's
/// far edge is shorter than its near one, and taking either alone would either
/// stretch or squash the result; the average keeps the pixel count about right
/// while the transform sorts out the shape.
#[derive(Debug)]
pub struct PerspectiveCrop {
    corners: [crate::geom::Vec2; 4],
    width: u32,
    height: u32,
    filter: crate::resample::Resampling,
    /// Everything the layers were, since a projective warp cannot be undone by
    /// applying its inverse — the samples it dropped are not recoverable.
    before: Option<Box<Undone>>,
}

/// One layer as it was: what it held, where it was, and what was masking it.
type LayerBefore = (LayerId, crate::layer::LayerKind, (i32, i32), Option<LayerMask>);

/// The document as it was before an edit that resamples everything.
///
/// Held whole, because neither a projective warp nor a carved seam can be
/// undone by applying an inverse: both throw samples away, and the samples are
/// what undo has to give back.
#[derive(Debug)]
pub struct Undone {
    size: (u32, u32),
    layers: Vec<LayerBefore>,
    selection: Option<CompressedSelection>,
}

impl PerspectiveCrop {
    /// `corners` are in document space, top-left then clockwise.
    pub fn new(
        corners: [crate::geom::Vec2; 4],
        filter: crate::resample::Resampling,
    ) -> Option<PerspectiveCrop> {
        let side = |a: crate::geom::Vec2, b: crate::geom::Vec2| a.distance(b);
        let w = (side(corners[0], corners[1]) + side(corners[3], corners[2])) / 2.0;
        let h = (side(corners[0], corners[3]) + side(corners[1], corners[2])) / 2.0;
        let (width, height) = (w.round() as u32, h.round() as u32);
        if width < 1 || height < 1 || width > 30_000 || height > 30_000 {
            return None;
        }
        // Edge lengths alone do not say the four points enclose anything: four
        // in a row measure a perfectly reasonable rectangle. Ask for the
        // transform here, where refusing costs nothing, rather than finding
        // out in `apply` and leaving an undo step that did nothing.
        let target = IRect::new(0, 0, width as i32, height as i32);
        crate::transform::Transform::from_quad(target, corners)?.invert()?;
        Some(PerspectiveCrop { corners, width, height, filter, before: None })
    }

    /// The size the result will be.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Command for PerspectiveCrop {
    fn name(&self) -> String {
        "Perspective Crop".into()
    }

    fn memory_bytes(&self) -> u64 {
        self.before.as_ref().map_or(0, |b| {
            b.layers
                .iter()
                .map(|(_, kind, _, mask)| {
                    let px = match kind {
                        crate::layer::LayerKind::Raster(s) => s.bytes(),
                        _ => 0,
                    };
                    px + mask.as_ref().map_or(0, mask_bytes)
                })
                .sum()
        })
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        // Target rect -> the quad, then inverted: what the resampler wants is
        // the map from where the pixels are to where they should go.
        let target = IRect::new(0, 0, self.width as i32, self.height as i32);
        let Some(to_quad) = crate::transform::Transform::from_quad(target, self.corners) else {
            return Dirty::NONE;
        };
        let Some(matrix) = to_quad.invert() else { return Dirty::NONE };

        if self.before.is_none() {
            let layers = doc
                .tree
                .iter_all()
                .into_iter()
                .filter_map(|id| {
                    let l = doc.tree.get(id)?;
                    Some((id, l.kind.clone(), l.offset, l.mask.clone()))
                })
                .collect();
            self.before = Some(Box::new(Undone {
                size: (doc.width, doc.height),
                layers,
                selection: doc.selection.as_ref().map(|s| s.compress()),
            }));
        }

        let clip = target;
        for id in doc.tree.iter_all() {
            let Some(layer) = doc.tree.get_mut(id) else { continue };
            let offset = layer.offset;
            if let Some(px) = layer.pixels() {
                match crate::resample::transform(px, offset, matrix, self.filter, Some(clip)) {
                    Some((moved, at)) => {
                        layer.kind = crate::layer::LayerKind::Raster(Surface::Eight(moved));
                        layer.offset = at;
                    }
                    // Straightened out of the frame entirely: an empty layer
                    // rather than a stale one in the wrong place.
                    None => {
                        layer.kind =
                            crate::layer::LayerKind::Raster(Surface::Eight(PixelBuffer::new(1, 1)));
                        layer.offset = (0, 0);
                    }
                }
            }
            if let Some(mask) = layer.mask.take() {
                let as_pixels = mask_as_pixels(&mask.data);
                if let Some((moved, at)) =
                    crate::resample::transform(&as_pixels, mask.offset, matrix, self.filter, Some(clip))
                {
                    layer.mask = Some(LayerMask {
                        data: pixels_as_mask(&moved),
                        offset: at,
                        enabled: mask.enabled,
                        linked: mask.linked,
                        // The path is in the old document's coordinates and a
                        // projective warp is not something a path survives, so
                        // what comes out is the drawing rather than the plan.
                        path: None,
                    });
                }
            }
        }

        doc.width = self.width;
        doc.height = self.height;
        doc.set_selection(None);
        Dirty::structural(doc.bounds())
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some(before) = self.before.take() else { return Dirty::NONE };
        doc.width = before.size.0;
        doc.height = before.size.1;
        for (id, kind, offset, mask) in before.layers {
            let Some(layer) = doc.tree.get_mut(id) else { continue };
            layer.kind = kind;
            layer.offset = offset;
            layer.mask = mask;
        }
        doc.set_selection(before.selection.map(|c| c.restore()));
        Dirty::structural(doc.bounds())
    }
}

/// A coverage mask as an image, so the resampler can move it. Carried in alpha
/// so premultiplied filtering treats hidden areas as absent rather than black.
fn mask_as_pixels(mask: &MaskBuffer) -> PixelBuffer {
    let mut out = PixelBuffer::new(mask.width(), mask.height());
    for y in 0..mask.height() as i32 {
        for x in 0..mask.width() as i32 {
            let v = mask.get(x, y);
            out.set(x, y, Rgba8::new(255, 255, 255, v));
        }
    }
    out
}

fn pixels_as_mask(px: &PixelBuffer) -> MaskBuffer {
    let mut out = MaskBuffer::hide_all(px.width(), px.height());
    for y in 0..px.height() as i32 {
        for x in 0..px.width() as i32 {
            out.set(x, y, px.get(x, y).a);
        }
    }
    out
}

/// Switch the document to a saved layer state, or change the list of them.
///
/// One command for all of it — applying, saving, renaming, deleting — because
/// every one of them is "the states and the settings were this, now they are
/// that", and a state holds settings rather than pixels, so remembering both
/// sides costs almost nothing.
#[derive(Debug)]
pub struct SetLayerStates {
    to: Vec<crate::states::LayerState>,
    /// The state to switch to as part of this, if any.
    show: Option<usize>,
    before: Option<(Vec<crate::states::LayerState>, crate::states::LayerState)>,
    label: String,
}

impl SetLayerStates {
    pub fn new(
        to: Vec<crate::states::LayerState>,
        show: Option<usize>,
        label: impl Into<String>,
    ) -> Self {
        Self { to, show, before: None, label: label.into() }
    }
}

impl Command for SetLayerStates {
    fn name(&self) -> String {
        self.label.clone()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.before.is_none() {
            // What the layers are doing right now, so undo can put them back
            // whether or not this command switches states.
            self.before = Some((
                doc.states.clone(),
                crate::states::LayerState::capture(&doc.tree, "before"),
            ));
        }
        doc.states = self.to.clone();
        if let Some(state) = self.show.and_then(|i| doc.states.get(i)).cloned() {
            state.apply(&mut doc.tree);
        }
        Dirty::structural(doc.bounds())
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some((states, was)) = self.before.clone() else { return Dirty::NONE };
        doc.states = states;
        was.apply(&mut doc.tree);
        Dirty::structural(doc.bounds())
    }
}

/// Change a layer's attached filters.
///
/// The whole stack at once rather than one slot, because every operation on it
/// — adding, removing, reordering, switching one off — is a small edit to a
/// small structure, and one entry per operation is what someone undoing would
/// expect. The stack holds settings and a mask, not pixels, so keeping both
/// the old and the new costs nothing worth counting.
#[derive(Debug)]
pub struct SetSmartFilters {
    id: LayerId,
    to: crate::smart_filters::SmartFilters,
    from: Option<crate::smart_filters::SmartFilters>,
    label: String,
}

impl SetSmartFilters {
    pub fn new(
        id: LayerId,
        to: crate::smart_filters::SmartFilters,
        label: impl Into<String>,
    ) -> Self {
        Self { id, to, from: None, label: label.into() }
    }

    fn write(&self, doc: &mut Document, value: crate::smart_filters::SmartFilters) -> Dirty {
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        let before = layer.render_bounds();
        layer.filters = value;
        Dirty::pixels(self.id, before.union(&layer.render_bounds()))
    }
}

impl Command for SetSmartFilters {
    fn name(&self) -> String {
        self.label.clone()
    }

    fn memory_bytes(&self) -> u64 {
        let mask = |f: &crate::smart_filters::SmartFilters| {
            f.mask.as_ref().map_or(0, |m| m.width() as u64 * m.height() as u64)
        };
        mask(&self.to) + self.from.as_ref().map_or(0, mask)
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.from.is_none() {
            self.from = doc.tree.get(self.id).map(|l| l.filters.clone());
        }
        self.write(doc, self.to.clone())
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some(from) = self.from.clone() else { return Dirty::NONE };
        self.write(doc, from)
    }
}

/// Change a layer's effects.
///
/// Merges with itself so that dragging a slider in the Layer Style dialog is
/// one history entry rather than one per frame.
#[derive(Debug)]
pub struct SetLayerEffects {
    id: LayerId,
    to: crate::effects::LayerEffects,
    from: Option<crate::effects::LayerEffects>,
}

impl SetLayerEffects {
    pub fn new(id: LayerId, to: crate::effects::LayerEffects) -> Self {
        Self { id, to, from: None }
    }

    fn write(&self, doc: &mut Document, value: crate::effects::LayerEffects) -> Dirty {
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        let before = layer.render_bounds();
        layer.effects = value;
        // Both the old reach and the new one have to be redrawn.
        Dirty::pixels(self.id, before.union(&layer.render_bounds()))
    }
}

impl Command for SetLayerEffects {
    fn name(&self) -> String {
        "Layer Style".into()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.from.is_none() {
            self.from = doc.tree.get(self.id).map(|l| l.effects);
        }
        self.write(doc, self.to)
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some(from) = self.from else { return Dirty::NONE };
        self.write(doc, from)
    }

    fn merge(&mut self, next: &dyn Command) -> bool {
        let Some(next) = (next as &dyn std::any::Any).downcast_ref::<SetLayerEffects>() else {
            return false;
        };
        if next.id != self.id {
            return false;
        }
        self.to = next.to;
        true
    }
}

/// Replace a shape layer's geometry or style.
///
/// Editing a shape is live, exactly as editing type is, so this is what turns
/// a run of option changes into the single undo step the user expects.
#[derive(Debug)]
pub struct SetShapeContent {
    id: LayerId,
    to: crate::shape::ShapeContent,
    to_offset: (i32, i32),
    from: Option<(crate::shape::ShapeContent, (i32, i32))>,
    label: String,
    /// Which run of edits this belongs to, if any.
    ///
    /// Dragging an anchor produces one of these per frame; without folding
    /// them the history would fill with a hundred identical-looking steps and
    /// undo would walk back through the drag a pixel at a time. An identifier
    /// rather than a flag, so that the first edit of a run can be marked too
    /// — and so two drags in quick succession stay two steps.
    run: Option<u64>,
}

impl SetShapeContent {
    pub fn new(
        id: LayerId,
        to: crate::shape::ShapeContent,
        to_offset: (i32, i32),
        label: impl Into<String>,
    ) -> Self {
        Self { id, to, to_offset, from: None, label: label.into(), run: None }
    }

    /// Mark this edit as part of a run — one drag — so the run is one step.
    pub fn in_run(mut self, run: u64) -> Self {
        self.run = Some(run);
        self
    }

    fn write(
        &self,
        doc: &mut Document,
        content: &crate::shape::ShapeContent,
        offset: (i32, i32),
    ) -> Dirty {
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        let before = layer.bounds();
        let Some(shape) = layer.shape_mut() else { return Dirty::NONE };
        shape.set_content(content.clone());
        layer.offset = offset;
        Dirty::pixels(self.id, before.union(&layer.bounds()))
    }
}

impl Command for SetShapeContent {
    /// Fold a later edit of the same shape into this one.
    ///
    /// Only while both are marked as part of a run: two deliberate edits
    /// should stay two steps, however quickly they follow each other.
    fn merge(&mut self, next: &dyn Command) -> bool {
        let Some(run) = self.run else { return false };
        let Some(other) = (next as &dyn std::any::Any).downcast_ref::<SetShapeContent>() else {
            return false;
        };
        if other.run != Some(run) || other.id != self.id {
            return false;
        }
        // Keep this entry's `from`, which is the state before the drag began,
        // and take the newest `to`.
        self.to = other.to.clone();
        self.to_offset = other.to_offset;
        true
    }

    fn name(&self) -> String {
        self.label.clone()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.from.is_none() {
            self.from =
                doc.tree.get(self.id).and_then(|l| l.shape().map(|s| (s.content().clone(), l.offset)));
        }
        let (to, offset) = (self.to.clone(), self.to_offset);
        self.write(doc, &to, offset)
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some((from, offset)) = self.from.clone() else { return Dirty::NONE };
        self.write(doc, &from, offset)
    }
}

/// Replace a type layer's content.
///
/// Editing type is live — every keystroke re-renders the layer straight away —
/// so this is what turns a whole editing session into the single undo step
/// editors record when the type is committed.
#[derive(Debug)]
pub struct SetTextContent {
    id: LayerId,
    to: crate::text::TextContent,
    /// Layer offset that goes with `to`, since re-rendering moves the raster
    /// when the text grows leftwards or upwards.
    to_offset: (i32, i32),
    from: Option<(crate::text::TextContent, (i32, i32))>,
    label: String,
}

impl SetTextContent {
    pub fn new(
        id: LayerId,
        to: crate::text::TextContent,
        to_offset: (i32, i32),
        label: impl Into<String>,
    ) -> Self {
        Self { id, to, to_offset, from: None, label: label.into() }
    }

    fn write(
        &self,
        doc: &mut Document,
        content: &crate::text::TextContent,
        offset: (i32, i32),
    ) -> Dirty {
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        let before = layer.bounds();
        let Some(text) = layer.text_mut() else { return Dirty::NONE };
        text.set_content(content.clone());
        layer.offset = offset;
        layer.name = content.layer_name();
        // Both where it was and where it now is have to be redrawn.
        Dirty::pixels(self.id, before.union(&layer.bounds()))
    }
}

impl Command for SetTextContent {
    fn name(&self) -> String {
        self.label.clone()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.from.is_none() {
            self.from = doc
                .tree
                .get(self.id)
                .and_then(|l| l.text().map(|t| (t.content().clone(), l.offset)));
        }
        let (to, offset) = (self.to.clone(), self.to_offset);
        self.write(doc, &to, offset)
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some((from, offset)) = self.from.clone() else { return Dirty::NONE };
        self.write(doc, &from, offset)
    }
}

/// Retune an adjustment layer's settings.
///
/// Merges with itself, so dragging a slider is one history entry rather than
/// one per frame.
#[derive(Debug)]
pub struct SetAdjustment {
    id: LayerId,
    to: crate::adjust::Adjustment,
    from: Option<crate::adjust::Adjustment>,
}

impl SetAdjustment {
    pub fn new(id: LayerId, to: crate::adjust::Adjustment) -> Self {
        Self { id, to, from: None }
    }

    fn write(&self, doc: &mut Document, value: &crate::adjust::Adjustment) -> Dirty {
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        if !layer.kind.is_adjustment() {
            return Dirty::NONE;
        }
        layer.kind = crate::layer::LayerKind::Adjustment(value.clone());
        // The layer's baked table has to be re-uploaded, and everything the
        // adjustment covers recomposited.
        Dirty::pixels(self.id, doc.bounds())
    }
}

impl Command for SetAdjustment {
    fn name(&self) -> String {
        self.to.name().to_string()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.from.is_none() {
            self.from = doc.tree.get(self.id).and_then(|l| l.adjustment_settings()).cloned();
        }
        let to = self.to.clone();
        self.write(doc, &to)
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some(from) = self.from.clone() else { return Dirty::NONE };
        self.write(doc, &from)
    }

    fn merge(&mut self, next: &dyn Command) -> bool {
        let Some(next) = (next as &dyn std::any::Any).downcast_ref::<SetAdjustment>() else {
            return false;
        };
        if next.id != self.id
            || std::mem::discriminant(&next.to) != std::mem::discriminant(&self.to)
        {
            return false;
        }
        // Keep the original "from" so undo jumps back to before the drag.
        self.to = next.to.clone();
        true
    }
}

// ---------------------------------------------------------------------------
// Selection commands
// ---------------------------------------------------------------------------

/// Change the pixel selection.
///
/// Selections are stored compressed — only the covered region — so an ordinary
/// marquee costs a few kilobytes of history rather than a full-canvas mask.
#[derive(Debug)]
pub struct SetSelection {
    to: Option<CompressedSelection>,
    from: Option<Option<CompressedSelection>>,
    label: String,
}

impl SetSelection {
    pub fn new(to: Option<&Selection>, label: impl Into<String>) -> Self {
        Self { to: to.map(|s| s.compress()), from: None, label: label.into() }
    }

    /// Deselect everything.
    pub fn deselect() -> Self {
        Self { to: None, from: None, label: "Deselect".into() }
    }

    fn write(doc: &mut Document, value: &Option<CompressedSelection>) {
        doc.set_selection(value.as_ref().map(|c| c.restore()));
    }
}

impl Command for SetSelection {
    fn memory_bytes(&self) -> u64 {
        let c = |s: &Option<CompressedSelection>| s.as_ref().map_or(0, |v| v.memory_bytes());
        c(&self.to) + self.from.as_ref().map_or(0, c)
    }

    fn name(&self) -> String {
        self.label.clone()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.from.is_none() {
            self.from = Some(doc.selection.as_ref().map(|s| s.compress()));
        }
        Self::write(doc, &self.to);
        // Only the marching ants move; no pixels changed, so nothing needs
        // recompositing.
        Dirty::NONE
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        if let Some(from) = &self.from {
            let from = from.clone();
            Self::write(doc, &from);
        }
        Dirty::NONE
    }
}

// ---------------------------------------------------------------------------
// Layer mask commands
// ---------------------------------------------------------------------------

/// Attach a mask to a layer.
#[derive(Debug)]
pub struct AddLayerMask {
    id: LayerId,
    mask: Option<LayerMask>,
    label: String,
}

impl AddLayerMask {
    pub fn new(id: LayerId, mask: LayerMask, label: impl Into<String>) -> Self {
        Self { id, mask: Some(mask), label: label.into() }
    }
}

impl Command for AddLayerMask {
    fn name(&self) -> String {
        self.label.clone()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        let Some(mask) = self.mask.take() else { return Dirty::NONE };
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        layer.mask = Some(mask);
        Dirty::pixels(self.id, doc.bounds())
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        self.mask = layer.mask.take();
        Dirty::pixels(self.id, doc.bounds())
    }
}

/// Remove a layer's mask, optionally baking it into the pixels first.
#[derive(Debug)]
pub struct RemoveLayerMask {
    id: LayerId,
    /// `true` for Apply Mask, `false` for Delete Mask.
    apply: bool,
    mask: Option<LayerMask>,
    /// Pixels before the mask was baked in, for undo.
    pixels: Option<PixelBuffer>,
}

impl RemoveLayerMask {
    pub fn new(id: LayerId, apply: bool) -> Self {
        Self { id, apply, mask: None, pixels: None }
    }
}

impl Command for RemoveLayerMask {
    fn name(&self) -> String {
        if self.apply { "Apply Layer Mask".into() } else { "Delete Layer Mask".into() }
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        let bounds = doc.bounds();
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        let Some(mask) = layer.mask.take() else { return Dirty::NONE };

        if self.apply {
            let offset = layer.offset;
            if let Some(px) = layer.pixels_mut() {
                if self.pixels.is_none() {
                    self.pixels = Some(px.clone());
                }
                // Multiply the mask into the alpha channel, which is what
                // makes the result identical to what was on screen.
                for y in 0..px.height() as i32 {
                    for x in 0..px.width() as i32 {
                        let doc_x = x + offset.0 - mask.offset.0;
                        let doc_y = y + offset.1 - mask.offset.1;
                        let m = mask.data.get(doc_x, doc_y) as u32;
                        let mut c = px.get(x, y);
                        c.a = ((c.a as u32 * m) / 255) as u8;
                        px.set(x, y, c);
                    }
                }
            }
        }
        self.mask = Some(mask);
        Dirty::pixels(self.id, bounds)
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let bounds = doc.bounds();
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        layer.mask = self.mask.take();
        if let (true, Some(before)) = (self.apply, self.pixels.take()) {
            if let Some(px) = layer.pixels_mut() {
                *px = before;
            }
        }
        Dirty::pixels(self.id, bounds)
    }
}

/// Replace a rectangle of a layer mask — the mask equivalent of
/// [`ReplacePixels`], used for painting on and filling masks.
#[derive(Debug)]
pub struct ReplaceMaskPixels {
    id: LayerId,
    rect: IRect,
    after: MaskBuffer,
    before: Option<MaskBuffer>,
    label: String,
}

impl ReplaceMaskPixels {
    pub fn new(id: LayerId, rect: IRect, after: MaskBuffer, label: impl Into<String>) -> Self {
        Self { id, rect, after, before: None, label: label.into() }
    }

    fn paste(&self, doc: &mut Document, src: &MaskBuffer) -> Dirty {
        let Some(layer) = doc.tree.get_mut(self.id) else { return Dirty::NONE };
        let Some(mask) = &mut layer.mask else { return Dirty::NONE };
        let (ox, oy) = mask.offset;
        for y in 0..src.height() as i32 {
            for x in 0..src.width() as i32 {
                mask.data.set(
                    self.rect.x0 - ox + x,
                    self.rect.y0 - oy + y,
                    src.get(x, y),
                );
            }
        }
        Dirty::pixels(self.id, self.rect)
    }
}

impl Command for ReplaceMaskPixels {
    fn memory_bytes(&self) -> u64 {
        let m = |b: &MaskBuffer| b.width() as u64 * b.height() as u64;
        m(&self.after) + self.before.as_ref().map_or(0, m)
    }

    fn name(&self) -> String {
        self.label.clone()
    }

    fn apply(&mut self, doc: &mut Document) -> Dirty {
        if self.before.is_none() {
            let Some(layer) = doc.tree.get(self.id) else { return Dirty::NONE };
            let Some(mask) = &layer.mask else { return Dirty::NONE };
            self.before =
                Some(mask.data.copy_rect(self.rect.translate(-mask.offset.0, -mask.offset.1)));
        }
        let after = std::mem::replace(&mut self.after, MaskBuffer::new(0, 0, 0));
        let dirty = self.paste(doc, &after);
        self.after = after;
        dirty
    }

    fn revert(&mut self, doc: &mut Document) -> Dirty {
        let Some(before) = self.before.take() else { return Dirty::NONE };
        let dirty = self.paste(doc, &before);
        self.before = Some(before);
        dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::BlendMode;
    use crate::color::Rgba8;
    use crate::document::Background;

    fn doc() -> Document {
        Document::new("test", 32, 32, Background::White)
    }

    #[test]
    fn undo_redo_walks_the_stack() {
        let mut d = doc();
        let mut h = History::new("Open");
        assert!(!h.can_undo() && !h.can_redo());

        let id = d.tree.alloc_id();
        let layer = Layer::raster(id, "New", PixelBuffer::new(8, 8));
        h.apply(&mut d, Box::new(AddLayer::new(layer, LayerPos { parent: None, index: 1 }, "New Layer")));

        assert_eq!(d.tree.len(), 2);
        assert!(h.can_undo());
        assert_eq!(h.undo_name().as_deref(), Some("New Layer"));

        h.undo(&mut d);
        assert_eq!(d.tree.len(), 1);
        assert!(h.can_redo());

        h.redo(&mut d);
        assert_eq!(d.tree.len(), 2);
        assert!(d.tree.contains(id));
    }

    #[test]
    fn a_new_edit_discards_the_redo_tail() {
        let mut d = doc();
        let mut h = History::new("Open");
        for _ in 0..2 {
            let id = d.tree.alloc_id();
            h.apply(
                &mut d,
                Box::new(AddLayer::new(
                    Layer::raster(id, "L", PixelBuffer::new(4, 4)),
                    LayerPos { parent: None, index: 1 },
                    "New Layer",
                )),
            );
        }
        h.undo(&mut d);
        h.undo(&mut d);
        assert!(h.can_redo());

        let id = d.tree.alloc_id();
        h.apply(
            &mut d,
            Box::new(AddLayer::new(
                Layer::raster(id, "Fresh", PixelBuffer::new(4, 4)),
                LayerPos { parent: None, index: 1 },
                "New Layer",
            )),
        );
        assert!(!h.can_redo());
        assert_eq!(h.labels().len(), 1);
    }

    #[test]
    fn deleting_a_group_restores_the_whole_subtree() {
        let mut d = doc();
        let g = d.tree.alloc_id();
        d.tree.push(Layer::group(g, "Group 1"), None);
        let child = d.tree.alloc_id();
        d.tree.push(Layer::raster(child, "Child", PixelBuffer::new(4, 4)), Some(g));

        let mut h = History::new("Open");
        h.apply(&mut d, Box::new(DeleteLayer::new(g)));
        assert!(!d.tree.contains(g) && !d.tree.contains(child));

        h.undo(&mut d);
        assert!(d.tree.contains(g) && d.tree.contains(child));
        assert_eq!(d.tree.get(child).unwrap().parent, Some(g));
        assert_eq!(d.tree.children(Some(g)), &[child]);
    }

    #[test]
    fn deleted_layer_returns_to_its_original_index() {
        let mut d = doc();
        let mut ids = vec![d.active.unwrap()];
        for i in 0..3 {
            let id = d.tree.alloc_id();
            d.tree.push(Layer::raster(id, format!("L{i}"), PixelBuffer::new(4, 4)), None);
            ids.push(id);
        }
        let mut h = History::new("Open");
        h.apply(&mut d, Box::new(DeleteLayer::new(ids[2])));
        h.undo(&mut d);
        assert_eq!(d.tree.root(), &ids[..]);
    }

    #[test]
    fn replace_pixels_restores_exactly_what_it_overwrote() {
        let mut d = doc();
        let id = d.active.unwrap();
        let rect = IRect::at(4, 4, 8, 8);
        let patch = PixelBuffer::filled(8, 8, Rgba8::BLACK);

        let mut h = History::new("Open");
        h.apply(&mut d, Box::new(ReplacePixels::new(id, rect, patch, "Fill")));
        let px = d.tree.get(id).unwrap().pixels().unwrap();
        assert_eq!(px.get(5, 5), Rgba8::BLACK);
        assert_eq!(px.get(0, 0), Rgba8::WHITE);

        h.undo(&mut d);
        assert_eq!(d.tree.get(id).unwrap().pixels().unwrap().get(5, 5), Rgba8::WHITE);
        h.redo(&mut d);
        assert_eq!(d.tree.get(id).unwrap().pixels().unwrap().get(5, 5), Rgba8::BLACK);
    }

    #[test]
    fn replace_pixels_accounts_for_the_layer_offset() {
        let mut d = doc();
        let id = d.tree.alloc_id();
        let mut layer = Layer::raster(id, "L", PixelBuffer::filled(8, 8, Rgba8::WHITE));
        layer.offset = (10, 10);
        d.tree.push(layer, None);

        let mut h = History::new("Open");
        // Document-space rect that maps to the layer's own (0,0).
        let rect = IRect::at(10, 10, 2, 2);
        h.apply(
            &mut d,
            Box::new(ReplacePixels::new(id, rect, PixelBuffer::filled(2, 2, Rgba8::BLACK), "Fill")),
        );
        assert_eq!(d.tree.get(id).unwrap().pixels().unwrap().get(0, 0), Rgba8::BLACK);
        h.undo(&mut d);
        assert_eq!(d.tree.get(id).unwrap().pixels().unwrap().get(0, 0), Rgba8::WHITE);
    }

    #[test]
    fn slider_drags_collapse_into_one_entry() {
        let mut d = doc();
        let id = d.active.unwrap();
        let mut h = History::new("Open");
        for v in [0.9f32, 0.8, 0.7, 0.6] {
            h.apply(&mut d, Box::new(SetLayerProperty::new(id, LayerProperty::Opacity(v))));
        }
        assert_eq!(h.labels().len(), 1, "each drag step should not be its own entry");
        assert!((d.tree.get(id).unwrap().opacity - 0.6).abs() < 1e-6);

        h.undo(&mut d);
        assert!((d.tree.get(id).unwrap().opacity - 1.0).abs() < 1e-6, "undo goes to pre-drag value");
    }

    #[test]
    fn different_fields_do_not_collapse_together() {
        let mut d = doc();
        let id = d.active.unwrap();
        let mut h = History::new("Open");
        h.apply(&mut d, Box::new(SetLayerProperty::new(id, LayerProperty::Opacity(0.5))));
        h.apply(&mut d, Box::new(SetLayerProperty::new(id, LayerProperty::Blend(BlendMode::Multiply))));
        assert_eq!(h.labels(), vec!["Layer Opacity", "Blend Mode"]);
    }

    #[test]
    fn nudges_collapse_but_undo_fully() {
        let mut d = doc();
        let id = d.tree.alloc_id();
        d.tree.push(Layer::raster(id, "L", PixelBuffer::new(4, 4)), None);
        let mut h = History::new("Open");
        for _ in 0..5 {
            h.apply(&mut d, Box::new(OffsetLayer::new(id, (1, 2))));
        }
        assert_eq!(h.labels().len(), 1);
        assert_eq!(d.tree.get(id).unwrap().offset, (5, 10));
        h.undo(&mut d);
        assert_eq!(d.tree.get(id).unwrap().offset, (0, 0));
    }

    #[test]
    fn jump_to_reaches_any_state_in_both_directions() {
        let mut d = doc();
        let id = d.active.unwrap();
        let mut h = History::new("Open");
        for v in [0.9f32, 0.5, 0.2] {
            h.apply(&mut d, Box::new(SetLayerProperty::new(id, LayerProperty::Opacity(v))));
            // Break the merge run so each becomes its own entry.
            h.apply(&mut d, Box::new(SetLayerProperty::new(id, LayerProperty::Visible(true))));
        }
        let total = h.labels().len();
        h.jump_to(&mut d, 0);
        assert_eq!(h.cursor(), 0);
        assert!((d.tree.get(id).unwrap().opacity - 1.0).abs() < 1e-6);

        h.jump_to(&mut d, total);
        assert_eq!(h.cursor(), total);
        assert!((d.tree.get(id).unwrap().opacity - 0.2).abs() < 1e-6);
    }

    #[test]
    fn the_stack_is_bounded() {
        let mut d = doc();
        let mut h = History::new("Open").with_limit(4);
        for i in 0..10 {
            let id = d.tree.alloc_id();
            h.apply(
                &mut d,
                Box::new(AddLayer::new(
                    Layer::raster(id, format!("L{i}"), PixelBuffer::new(2, 2)),
                    LayerPos { parent: None, index: 1 },
                    "New Layer",
                )),
            );
        }
        assert_eq!(h.labels().len(), 4);
        assert_eq!(h.cursor(), 4);
    }

    #[test]
    fn canvas_resize_moves_layers_without_resampling() {
        let mut d = Document::new("d", 100, 100, Background::White);
        let id = d.active.unwrap();
        let before = d.tree.get(id).unwrap().pixels().unwrap().clone();

        let mut h = History::new("Open");
        // Grow to 200x200 anchored in the centre: layers shift by 50.
        h.apply(&mut d, Box::new(ResizeCanvas::new(200, 200, (50, 50))));
        assert_eq!((d.width, d.height), (200, 200));
        assert_eq!(d.tree.get(id).unwrap().offset, (50, 50));
        assert!(
            d.tree.get(id).unwrap().pixels().unwrap() == &before,
            "canvas size must not touch pixels"
        );

        h.undo(&mut d);
        assert_eq!((d.width, d.height), (100, 100));
        assert_eq!(d.tree.get(id).unwrap().offset, (0, 0));
    }

    #[test]
    fn image_resize_scales_every_layer_and_undoes() {
        let mut d = Document::new("d", 100, 50, Background::White);
        let id = d.active.unwrap();
        d.tree.get_mut(id).unwrap().mask =
            Some(LayerMask::reveal_all(100, 50));

        let mut h = History::new("Open");
        h.apply(&mut d, Box::new(ResizeImage::new(50, 25, crate::resample::Resampling::Bilinear)));

        assert_eq!((d.width, d.height), (50, 25));
        let layer = d.tree.get(id).unwrap();
        assert_eq!((layer.pixels().unwrap().width(), layer.pixels().unwrap().height()), (50, 25));
        assert_eq!(layer.mask.as_ref().unwrap().data.width(), 50);

        h.undo(&mut d);
        assert_eq!((d.width, d.height), (100, 50));
        let layer = d.tree.get(id).unwrap();
        assert_eq!(layer.pixels().unwrap().width(), 100, "undo restores the original pixels");
        assert_eq!(layer.mask.as_ref().unwrap().data.width(), 100);
    }

    #[test]
    fn image_resize_keeps_an_offset_layer_in_proportion() {
        let mut d = Document::new("d", 200, 200, Background::Transparent);
        let id = d.tree.alloc_id();
        let mut layer = Layer::raster(id, "L", PixelBuffer::new(50, 50));
        layer.offset = (100, 60);
        d.tree.push(layer, None);

        let mut h = History::new("Open");
        h.apply(&mut d, Box::new(ResizeImage::new(100, 100, crate::resample::Resampling::Bilinear)));
        let layer = d.tree.get(id).unwrap();
        assert_eq!(layer.offset, (50, 30), "the offset should halve with the canvas");
        assert_eq!(layer.pixels().unwrap().width(), 25);
    }

    #[test]
    fn replacing_layer_pixels_round_trips() {
        let mut d = doc();
        let id = d.active.unwrap();
        let mut h = History::new("Open");

        let replacement = PixelBuffer::filled(10, 10, Rgba8::BLACK);
        h.apply(
            &mut d,
            Box::new(ReplaceLayerPixels::new(id, replacement, (5, 7), None, "Free Transform")),
        );
        let layer = d.tree.get(id).unwrap();
        assert_eq!(layer.offset, (5, 7));
        assert_eq!(layer.pixels().unwrap().width(), 10);

        h.undo(&mut d);
        let layer = d.tree.get(id).unwrap();
        assert_eq!(layer.offset, (0, 0));
        assert_eq!(layer.pixels().unwrap().width(), 32);
    }

    #[test]
    fn adjustment_edits_collapse_into_one_entry() {
        use crate::adjust::Adjustment;
        let mut d = doc();
        let id = d.tree.alloc_id();
        d.tree.push(
            Layer::adjustment(id, Adjustment::BrightnessContrast { brightness: 0.0, contrast: 0.0 }),
            None,
        );

        let mut h = History::new("Open");
        for v in [0.1f32, 0.2, 0.3] {
            h.apply(
                &mut d,
                Box::new(SetAdjustment::new(
                    id,
                    Adjustment::BrightnessContrast { brightness: v, contrast: 0.0 },
                )),
            );
        }
        assert_eq!(h.labels(), vec!["Brightness/Contrast"], "a drag is one entry");

        let settings = d.tree.get(id).unwrap().adjustment_settings().unwrap().clone();
        assert_eq!(settings, Adjustment::BrightnessContrast { brightness: 0.3, contrast: 0.0 });

        h.undo(&mut d);
        let settings = d.tree.get(id).unwrap().adjustment_settings().unwrap().clone();
        assert_eq!(
            settings,
            Adjustment::BrightnessContrast { brightness: 0.0, contrast: 0.0 },
            "undo returns to before the drag"
        );
    }

    #[test]
    fn different_adjustments_do_not_merge() {
        use crate::adjust::Adjustment;
        let mut d = doc();
        let id = d.tree.alloc_id();
        d.tree.push(Layer::adjustment(id, Adjustment::Invert), None);

        let mut h = History::new("Open");
        h.apply(&mut d, Box::new(SetAdjustment::new(id, Adjustment::Posterize { levels: 4 })));
        h.apply(&mut d, Box::new(SetAdjustment::new(id, Adjustment::Threshold { level: 0.5 })));
        assert_eq!(h.labels(), vec!["Posterize", "Threshold"]);
    }

    #[test]
    fn selection_changes_undo_cleanly() {
        use crate::selection::Rectf;
        let mut d = doc();
        let mut h = History::new("Open");
        assert!(!d.has_selection());

        let s = Selection::from_rect(32, 32, Rectf { x0: 4.0, y0: 4.0, x1: 20.0, y1: 20.0 }, false);
        h.apply(&mut d, Box::new(SetSelection::new(Some(&s), "Rectangular Marquee")));
        assert_eq!(d.selection.as_ref().unwrap().bounds(), IRect::new(4, 4, 20, 20));

        h.apply(&mut d, Box::new(SetSelection::deselect()));
        assert!(!d.has_selection());

        h.undo(&mut d);
        assert_eq!(d.selection.as_ref().unwrap().bounds(), IRect::new(4, 4, 20, 20));

        h.undo(&mut d);
        assert!(!d.has_selection(), "undoing the first selection restores no selection");
    }

    #[test]
    fn adding_and_removing_a_layer_mask_round_trips() {
        let mut d = doc();
        let id = d.active.unwrap();
        let mut h = History::new("Open");

        h.apply(
            &mut d,
            Box::new(AddLayerMask::new(id, LayerMask::reveal_all(32, 32), "Add Layer Mask")),
        );
        assert!(d.tree.get(id).unwrap().mask.is_some());

        h.undo(&mut d);
        assert!(d.tree.get(id).unwrap().mask.is_none());

        h.redo(&mut d);
        assert!(d.tree.get(id).unwrap().mask.is_some());

        h.apply(&mut d, Box::new(RemoveLayerMask::new(id, false)));
        assert!(d.tree.get(id).unwrap().mask.is_none());
        h.undo(&mut d);
        assert!(d.tree.get(id).unwrap().mask.is_some());
    }

    #[test]
    fn applying_a_mask_bakes_it_into_the_alpha() {
        let mut d = doc();
        let id = d.active.unwrap();
        let mut mask = LayerMask::reveal_all(32, 32);
        mask.data.fill_rect(IRect::new(0, 0, 16, 32), 0);
        d.tree.get_mut(id).unwrap().mask = Some(mask);

        let mut h = History::new("Open");
        h.apply(&mut d, Box::new(RemoveLayerMask::new(id, true)));

        let layer = d.tree.get(id).unwrap();
        assert!(layer.mask.is_none(), "the mask is consumed");
        assert_eq!(layer.pixels().unwrap().get(4, 4).a, 0, "the hidden half became transparent");
        assert_eq!(layer.pixels().unwrap().get(20, 4).a, 255);

        h.undo(&mut d);
        let layer = d.tree.get(id).unwrap();
        assert!(layer.mask.is_some(), "undo restores the mask");
        assert_eq!(layer.pixels().unwrap().get(4, 4).a, 255, "and the original pixels");
    }

    #[test]
    fn mask_edits_undo_exactly() {
        let mut d = doc();
        let id = d.active.unwrap();
        d.tree.get_mut(id).unwrap().mask = Some(LayerMask::reveal_all(32, 32));

        let mut h = History::new("Open");
        let rect = IRect::at(4, 4, 8, 8);
        h.apply(
            &mut d,
            Box::new(ReplaceMaskPixels::new(id, rect, MaskBuffer::new(8, 8, 0), "Brush Tool")),
        );
        let mask = d.tree.get(id).unwrap().mask.as_ref().unwrap();
        assert_eq!(mask.data.get(6, 6), 0);
        assert_eq!(mask.data.get(0, 0), 255);

        h.undo(&mut d);
        assert_eq!(d.tree.get(id).unwrap().mask.as_ref().unwrap().data.get(6, 6), 255);
    }

    #[test]
    fn moving_a_layer_is_reversible() {
        let mut d = doc();
        let mut ids = vec![d.active.unwrap()];
        for i in 0..2 {
            let id = d.tree.alloc_id();
            d.tree.push(Layer::raster(id, format!("L{i}"), PixelBuffer::new(2, 2)), None);
            ids.push(id);
        }
        let mut h = History::new("Open");
        h.apply(&mut d, Box::new(MoveLayer::new(ids[0], LayerPos { parent: None, index: 3 })));
        assert_eq!(d.tree.root(), &[ids[1], ids[2], ids[0]]);
        h.undo(&mut d);
        assert_eq!(d.tree.root(), &ids[..]);
    }
}
