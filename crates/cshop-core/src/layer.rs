//! The layer model.
//!
//! A layer owns its pixels at an offset inside the document rather than a
//! full-document buffer, so a 200×200 logo on a 6000×4000 canvas costs 200×200.
//! Moving a layer is then just a change of [`Layer::offset`], with no pixel
//! work at all.

use crate::adjust::Adjustment;
use crate::shape::ShapeContent;
use crate::text::TextContent;
use crate::blend::BlendMode;
use crate::color::Rgba8;
use crate::geom::IRect;
use crate::mask::MaskBuffer;
use crate::pixels::PixelBuffer;

/// Stable handle for a layer. Never reused within a document, so stale ids
/// resolve to `None` instead of silently pointing at a different layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayerId(pub u64);

/// Which edits a layer refuses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayerLocks {
    /// Painting cannot change alpha; strokes are clipped to existing pixels.
    pub transparency: bool,
    /// Pixels are read-only.
    pub pixels: bool,
    /// The layer cannot be moved or transformed.
    pub position: bool,
    /// The layer cannot be deleted, reordered or nested.
    pub all: bool,
}

impl LayerLocks {
    pub fn any(&self) -> bool {
        self.transparency || self.pixels || self.position || self.all
    }

    pub fn blocks_pixels(&self) -> bool {
        self.pixels || self.all
    }

    pub fn blocks_move(&self) -> bool {
        self.position || self.all
    }
}

/// A raster layer mask: greyscale coverage that multiplies the layer's alpha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerMask {
    pub data: MaskBuffer,
    pub offset: (i32, i32),
    /// Unchecking this keeps the mask but stops it applying (shift-click).
    pub enabled: bool,
    /// Linked masks move together with the layer's pixels.
    pub linked: bool,
}

impl LayerMask {
    pub fn reveal_all(width: u32, height: u32) -> Self {
        Self {
            data: MaskBuffer::reveal_all(width, height),
            offset: (0, 0),
            enabled: true,
            linked: true,
        }
    }

    pub fn hide_all(width: u32, height: u32) -> Self {
        Self {
            data: MaskBuffer::hide_all(width, height),
            offset: (0, 0),
            enabled: true,
            linked: true,
        }
    }

    pub fn bounds(&self) -> IRect {
        IRect::at(self.offset.0, self.offset.1, self.data.width(), self.data.height())
    }
}

/// What a fill layer paints. Gradient and pattern arrive with the gradient
/// editor in a later phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FillStyle {
    Solid(Rgba8),
}

/// Layer content. Text, shape, adjustment and smart-object kinds extend this
/// enum in later phases; the compositor matches exhaustively so adding one is
/// a compile error until it is handled everywhere.
#[derive(Debug, Clone)]
pub enum LayerKind {
    Raster(PixelBuffer),
    /// Children are stored bottom-to-top, matching document order.
    Group { children: Vec<LayerId> },
    Fill(FillStyle),
    /// Recolours everything composited beneath it, without touching any
    /// pixels. Its mask, opacity and blend mode all apply as usual, so an
    /// adjustment can be limited to part of the image or blended back in.
    Adjustment(Adjustment),
    /// Re-editable type. Carries its own raster so that everything downstream
    /// — the compositor, masks, blend modes, filters — can treat it exactly
    /// like a raster layer.
    Text(Box<TextLayer>),
    /// A re-editable vector shape. Like type, it carries its own raster.
    Shape(Box<ShapeLayer>),
}

/// A shape layer: its geometry and style, and what those look like.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeLayer {
    content: ShapeContent,
    raster: PixelBuffer,
    /// Where the shape's box's top-left falls inside `raster`. Widening a
    /// stroke grows the raster; this is what keeps the shape still while it
    /// does.
    anchor: (i32, i32),
}

impl ShapeLayer {
    pub fn new(content: ShapeContent) -> Option<ShapeLayer> {
        let r = crate::shape::rasterize(&content)?;
        Some(ShapeLayer { content, raster: r.pixels, anchor: r.anchor })
    }

    pub fn content(&self) -> &ShapeContent {
        &self.content
    }

    pub fn raster(&self) -> &PixelBuffer {
        &self.raster
    }

    pub fn anchor(&self) -> (i32, i32) {
        self.anchor
    }

    /// Re-render with new geometry or style, returning how far
    /// [`Layer::offset`] must move to leave the shape where it was.
    pub fn set_content(&mut self, content: ShapeContent) -> (i32, i32) {
        let Some(r) = crate::shape::rasterize(&content) else { return (0, 0) };
        let delta = (self.anchor.0 - r.anchor.0, self.anchor.1 - r.anchor.1);
        self.content = content;
        self.raster = r.pixels;
        self.anchor = r.anchor;
        delta
    }
}

/// A text layer: what it says, and what that looks like.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLayer {
    content: TextContent,
    raster: PixelBuffer,
    /// Where the text's anchor falls inside `raster`. Kept so that an edit
    /// which changes the raster's size can put the anchor back where it was
    /// rather than letting the text drift.
    anchor: (i32, i32),
    /// Where the layout box's top-left falls inside `raster`, which is the
    /// space caret positions are measured in.
    origin: (i32, i32),
}

impl TextLayer {
    /// `None` when the family cannot be loaded — an empty type layer is worse
    /// than none at all.
    pub fn new(content: TextContent) -> Option<TextLayer> {
        let r = crate::text::render(&content)?;
        Some(TextLayer { content, raster: r.pixels, anchor: r.anchor, origin: r.origin })
    }

    pub fn content(&self) -> &TextContent {
        &self.content
    }

    pub fn raster(&self) -> &PixelBuffer {
        &self.raster
    }

    pub fn anchor(&self) -> (i32, i32) {
        self.anchor
    }

    /// The layout box's top-left, in raster coordinates.
    pub fn layout_origin(&self) -> (i32, i32) {
        self.origin
    }

    /// Re-render with new content, returning how far [`Layer::offset`] must
    /// move to leave the anchor where it was.
    pub fn set_content(&mut self, content: TextContent) -> (i32, i32) {
        let Some(r) = crate::text::render(&content) else { return (0, 0) };
        let delta = (self.anchor.0 - r.anchor.0, self.anchor.1 - r.anchor.1);
        self.content = content;
        self.raster = r.pixels;
        self.anchor = r.anchor;
        self.origin = r.origin;
        delta
    }
}

impl LayerKind {
    pub fn is_group(&self) -> bool {
        matches!(self, LayerKind::Group { .. })
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            LayerKind::Raster(_) => "Raster",
            LayerKind::Group { .. } => "Group",
            LayerKind::Fill(_) => "Fill",
            LayerKind::Adjustment(_) => "Adjustment",
            LayerKind::Text(_) => "Type",
            LayerKind::Shape(_) => "Shape",
        }
    }

    /// Whether the layer alters what is beneath it rather than adding to it.
    pub fn is_adjustment(&self) -> bool {
        matches!(self, LayerKind::Adjustment(_))
    }
}

/// One entry in the layer stack.
#[derive(Debug, Clone)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub kind: LayerKind,
    pub visible: bool,
    /// `0..=1`. Scales the layer *and* its effects.
    pub opacity: f32,
    /// `0..=1`. Scales the layer's own pixels but not its effects — the
    /// distinction that makes a stroke-only layer possible.
    pub fill_opacity: f32,
    pub blend_mode: BlendMode,
    pub locks: LayerLocks,
    /// Clipped into the alpha of the first non-clipping layer below it.
    pub clipping: bool,
    /// Top-left of the layer's pixels in document space.
    pub offset: (i32, i32),
    pub mask: Option<LayerMask>,
    pub parent: Option<LayerId>,
    /// Group disclosure state in the Layers panel.
    pub expanded: bool,
    /// The locked bottom layer: opaque, unmovable, always at index 0.
    pub is_background: bool,
}

impl Layer {
    /// A layer with sensible defaults; callers override what they need.
    pub fn new(id: LayerId, name: impl Into<String>, kind: LayerKind) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            visible: true,
            opacity: 1.0,
            fill_opacity: 1.0,
            blend_mode: BlendMode::Normal,
            locks: LayerLocks::default(),
            clipping: false,
            offset: (0, 0),
            mask: None,
            parent: None,
            expanded: true,
            is_background: false,
        }
    }

    pub fn raster(id: LayerId, name: impl Into<String>, pixels: PixelBuffer) -> Self {
        Self::new(id, name, LayerKind::Raster(pixels))
    }

    pub fn group(id: LayerId, name: impl Into<String>) -> Self {
        Self::new(id, name, LayerKind::Group { children: Vec::new() })
    }

    /// `None` when the text cannot be rendered, which means no usable font.
    pub fn text_layer(id: LayerId, content: TextContent) -> Option<Self> {
        let name = content.layer_name();
        let text = TextLayer::new(content)?;
        Some(Self::new(id, name, LayerKind::Text(Box::new(text))))
    }

    /// `None` when the shape would be degenerate.
    pub fn shape_layer(id: LayerId, content: ShapeContent) -> Option<Self> {
        let name = content.layer_name();
        let shape = ShapeLayer::new(content)?;
        Some(Self::new(id, name, LayerKind::Shape(Box::new(shape))))
    }

    pub fn adjustment(id: LayerId, adjustment: Adjustment) -> Self {
        Self::new(id, adjustment.name(), LayerKind::Adjustment(adjustment))
    }

    pub fn adjustment_settings(&self) -> Option<&Adjustment> {
        match &self.kind {
            LayerKind::Adjustment(a) => Some(a),
            _ => None,
        }
    }

    /// `true` when the layer would leave the composite unchanged, so the
    /// compositor can drop its pass.
    pub fn is_no_op(&self) -> bool {
        match &self.kind {
            LayerKind::Adjustment(a) => a.is_identity(),
            _ => false,
        }
    }

    pub fn pixels(&self) -> Option<&PixelBuffer> {
        match &self.kind {
            LayerKind::Raster(p) => Some(p),
            LayerKind::Text(t) => Some(&t.raster),
            LayerKind::Shape(s) => Some(&s.raster),
            _ => None,
        }
    }

    /// The type layer, if this is one.
    pub fn text(&self) -> Option<&TextLayer> {
        match &self.kind {
            LayerKind::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn text_mut(&mut self) -> Option<&mut TextLayer> {
        match &mut self.kind {
            LayerKind::Text(t) => Some(t),
            _ => None,
        }
    }

    /// The shape layer, if this is one.
    pub fn shape(&self) -> Option<&ShapeLayer> {
        match &self.kind {
            LayerKind::Shape(s) => Some(s),
            _ => None,
        }
    }

    pub fn shape_mut(&mut self) -> Option<&mut ShapeLayer> {
        match &mut self.kind {
            LayerKind::Shape(s) => Some(s),
            _ => None,
        }
    }

    /// True for the kinds that are drawn from a description rather than from
    /// pixels, and so cannot be painted on until they are rasterised.
    pub fn is_vector(&self) -> bool {
        matches!(self.kind, LayerKind::Text(_) | LayerKind::Shape(_))
    }

    /// Only true raster layers are writable. Type has to be rasterised
    /// first, as in any layered editor, or an edit would be thrown away the
    /// next time the text was re-rendered.
    pub fn pixels_mut(&mut self) -> Option<&mut PixelBuffer> {
        match &mut self.kind {
            LayerKind::Raster(p) => Some(p),
            _ => None,
        }
    }

    pub fn children(&self) -> &[LayerId] {
        match &self.kind {
            LayerKind::Group { children } => children,
            _ => &[],
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Vec<LayerId>> {
        match &mut self.kind {
            LayerKind::Group { children } => Some(children),
            _ => None,
        }
    }

    /// The layer's extent in document space. Groups and fill layers have no
    /// intrinsic bounds, so they report [`IRect::EMPTY`]; a group's real extent
    /// is the union of its children, which only the tree can compute.
    pub fn bounds(&self) -> IRect {
        match &self.kind {
            LayerKind::Raster(p) => IRect::at(self.offset.0, self.offset.1, p.width(), p.height()),
            LayerKind::Text(t) => {
                IRect::at(self.offset.0, self.offset.1, t.raster.width(), t.raster.height())
            }
            LayerKind::Shape(s) => {
                IRect::at(self.offset.0, self.offset.1, s.raster.width(), s.raster.height())
            }
            // Groups, fills and adjustments have no intrinsic extent: a fill
            // and an adjustment both cover the whole canvas, and a group's
            // extent is the union of its children.
            LayerKind::Group { .. } | LayerKind::Fill(_) | LayerKind::Adjustment(_) => {
                IRect::EMPTY
            }
        }
    }

    /// Whether this layer contributes anything to the composite. Checked before
    /// a layer is uploaded or drawn.
    pub fn contributes(&self) -> bool {
        self.visible && self.opacity > 0.0
    }

    /// Effective alpha for the layer's own pixels.
    pub fn effective_alpha(&self) -> f32 {
        (self.opacity * self.fill_opacity).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_bounds_follow_the_offset() {
        let mut l = Layer::raster(LayerId(1), "L", PixelBuffer::new(10, 20));
        assert_eq!(l.bounds(), IRect::new(0, 0, 10, 20));
        l.offset = (5, -3);
        assert_eq!(l.bounds(), IRect::new(5, -3, 15, 17));
    }

    #[test]
    fn groups_have_no_intrinsic_bounds() {
        assert!(Layer::group(LayerId(1), "G").bounds().is_empty());
    }

    #[test]
    fn invisible_or_transparent_layers_do_not_contribute() {
        let mut l = Layer::raster(LayerId(1), "L", PixelBuffer::new(4, 4));
        assert!(l.contributes());
        l.visible = false;
        assert!(!l.contributes());
        l.visible = true;
        l.opacity = 0.0;
        assert!(!l.contributes());
    }

    #[test]
    fn fill_opacity_multiplies_into_effective_alpha() {
        let mut l = Layer::raster(LayerId(1), "L", PixelBuffer::new(1, 1));
        l.opacity = 0.5;
        l.fill_opacity = 0.5;
        assert!((l.effective_alpha() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn locks_gate_the_right_operations() {
        let mut k = LayerLocks::default();
        assert!(!k.any());
        k.transparency = true;
        assert!(k.any() && !k.blocks_pixels() && !k.blocks_move());
        k = LayerLocks { all: true, ..Default::default() };
        assert!(k.blocks_pixels() && k.blocks_move());
    }
}
