//! Every user-triggered operation, as data.
//!
//! Menus, keyboard shortcuts, toolbar buttons and panel controls all produce
//! an [`Action`] rather than mutating state where they are drawn. That keeps
//! mutation out of egui's closures — which would otherwise fight the borrow
//! checker — and puts every state change in one reviewable place.

use crate::tools::Tool;
use cshop_core::adjust::Adjustment;
use cshop_core::document::EditTarget;
use cshop_core::history::LayerProperty;
use cshop_core::layer::LayerId;
use cshop_core::selection::Selection;
use cshop_core::tree::LayerPos;
use std::path::PathBuf;

/// Something only the window itself can do.
///
/// With the platform's title bar switched off, moving, resizing, minimising
/// and maximising all become the application's job — but the `Window` lives in
/// the event loop, so the interface queues these and the loop carries them out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCommand {
    /// Begin an interactive move, as dragging a title bar does.
    StartDrag,
    /// Begin an interactive resize from an edge or corner.
    StartResize(ResizeEdge),
    Minimize,
    ToggleMaximize,
    Close,
}

/// Which edge or corner a resize drag started from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

impl ResizeEdge {
    /// The pointer shape that shows which way the edge moves.
    pub fn cursor(self) -> egui::CursorIcon {
        match self {
            ResizeEdge::North | ResizeEdge::South => egui::CursorIcon::ResizeVertical,
            ResizeEdge::East | ResizeEdge::West => egui::CursorIcon::ResizeHorizontal,
            ResizeEdge::NorthWest => egui::CursorIcon::ResizeNwSe,
            ResizeEdge::SouthEast => egui::CursorIcon::ResizeNwSe,
            ResizeEdge::NorthEast => egui::CursorIcon::ResizeNeSw,
            ResizeEdge::SouthWest => egui::CursorIcon::ResizeNeSw,
        }
    }
}

/// A fixed rotation or flip, from the Image > Transform menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformPreset {
    Rotate90Cw,
    Rotate90Ccw,
    Rotate180,
    FlipHorizontal,
    FlipVertical,
}

impl TransformPreset {
    pub fn name(self) -> &'static str {
        match self {
            TransformPreset::Rotate90Cw => "Rotate 90° Clockwise",
            TransformPreset::Rotate90Ccw => "Rotate 90° Counter Clockwise",
            TransformPreset::Rotate180 => "Rotate 180°",
            TransformPreset::FlipHorizontal => "Flip Horizontal",
            TransformPreset::FlipVertical => "Flip Vertical",
        }
    }

    pub const ALL: [TransformPreset; 5] = [
        TransformPreset::Rotate90Cw,
        TransformPreset::Rotate90Ccw,
        TransformPreset::Rotate180,
        TransformPreset::FlipHorizontal,
        TransformPreset::FlipVertical,
    ];
}

/// Which corner or edge a canvas resize grows from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    pub const GRID: [[Anchor; 3]; 3] = [
        [Anchor::TopLeft, Anchor::Top, Anchor::TopRight],
        [Anchor::Left, Anchor::Center, Anchor::Right],
        [Anchor::BottomLeft, Anchor::Bottom, Anchor::BottomRight],
    ];

    /// Fraction of the size change that lands before the content, per axis.
    pub fn weights(self) -> (f32, f32) {
        let x = match self {
            Anchor::TopLeft | Anchor::Left | Anchor::BottomLeft => 0.0,
            Anchor::Top | Anchor::Center | Anchor::Bottom => 0.5,
            _ => 1.0,
        };
        let y = match self {
            Anchor::TopLeft | Anchor::Top | Anchor::TopRight => 0.0,
            Anchor::Left | Anchor::Center | Anchor::Right => 0.5,
            _ => 1.0,
        };
        (x, y)
    }

    /// How far layers shift when the canvas goes from `from` to `to`.
    pub fn shift(self, from: (u32, u32), to: (u32, u32)) -> (i32, i32) {
        let (wx, wy) = self.weights();
        let dx = to.0 as f32 - from.0 as f32;
        let dy = to.1 as f32 - from.1 as f32;
        ((dx * wx).round() as i32, (dy * wy).round() as i32)
    }
}

/// Which of Select > Modify's operations to run, and by how much.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModifySelection {
    Feather(f32),
    Expand(u32),
    Contract(u32),
    Border(u32),
    Smooth(u32),
}

impl ModifySelection {
    pub fn label(self) -> &'static str {
        match self {
            ModifySelection::Feather(_) => "Feather Selection",
            ModifySelection::Expand(_) => "Expand Selection",
            ModifySelection::Contract(_) => "Contract Selection",
            ModifySelection::Border(_) => "Border Selection",
            ModifySelection::Smooth(_) => "Smooth Selection",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    // --- documents ---
    NewDocument,
    ShowOpenDialog,
    OpenPath(PathBuf),
    Save,
    ShowSaveAsDialog,
    /// Save to this path. `deep` writes sixteen bits a channel, which only
    /// PNG and TIFF can hold — see [`cshop_core::color::Rgba16`].
    SavePath { path: PathBuf, deep: bool },
    CloseDocument(usize),
    SelectDocument(usize),

    // --- history ---
    Undo,
    Redo,
    /// Jump to a state in the History panel; `0` is the document as opened.
    HistoryJump(usize),
    /// Mark a history state as what the History Brush paints from.
    SetHistorySource(usize),

    // --- tools and colour ---
    SelectTool(Tool),
    SwapColors,
    ResetColors,

    // --- view ---
    ZoomIn,
    ZoomOut,
    ZoomFit,
    ZoomActual,
    TogglePanels,

    // --- layers ---
    NewLayer,
    NewGroup,
    DeleteLayer,
    DuplicateLayer,
    /// Layer via Copy (Ctrl+J): with a selection, lift just the
    /// selected pixels onto a new layer; without one, copy the whole layer.
    LayerViaCopy,
    MergeDown,
    /// Turn the active layer's clipping mask on or off.
    ToggleClippingMask,
    /// Move the active layer within its parent. `i32::MAX` and `i32::MIN` mean
    /// all the way to the top and the bottom.
    ReorderActiveLayer(i32),
    /// Select the layer this many rows above (positive) or below the active
    /// one, as Alt+[ and Alt+] do.
    StepActiveLayer(i32),
    FlattenImage,
    SelectLayer(LayerId),
    SetLayerProperty(LayerId, LayerProperty),
    MoveLayer(LayerId, LayerPos),
    /// Nudge the active layer by whole pixels.
    NudgeLayer(i32, i32),

    // --- editing ---
    /// Fill with the foreground or background swatch, as the Backspace family
    /// of shortcuts does. `preserve_transparency` keeps the layer's existing
    /// alpha, which is what the Shift variants add.
    FillSwatch { background: bool, preserve_transparency: bool },
    /// Open Edit > Fill.
    /// Copy the selection from the active layer.
    Copy,
    /// Copy the selection from everything visible, not just the active layer.
    CopyMerged,
    /// Copy, then clear what was copied.
    Cut,
    /// Paste onto a new layer, centred on the view.
    Paste,
    /// Paste onto a new layer at the coordinates it was copied from.
    PasteInPlace,
    ShowFillDialog,
    /// Start a new type layer at a document point. `wrap` gives the paragraph
    /// box width when the tool was dragged rather than clicked.
    BeginText { at: cshop_core::geom::Vec2, wrap: Option<f32> },
    /// Re-open an existing type layer for editing.
    EditTextLayer(LayerId),
    TextInput(crate::text_tool::TextInput),
    /// Put the caret at a document point within the type being edited.
    TextCaretAt(cshop_core::geom::Vec2),
    CommitText,
    CancelText,
    /// Turn the active type or shape layer into ordinary pixels.
    RasterizeLayer,
    /// Wrap the active layer's pixels so its placement stops being an edit.
    ConvertToSmartObject,
    /// Open the Layer Style dialog for the active layer.
    ShowLayerStyle,
    /// Apply a set of effects to a layer.
    SetLayerEffects(LayerId, Box<cshop_core::effects::LayerEffects>),
    /// Remove every effect from a layer.
    ClearLayerEffects(LayerId),
    /// Create a shape layer from a drag, in document space.
    DrawShape { from: cshop_core::geom::Vec2, to: cshop_core::geom::Vec2, from_centre: bool, constrain: bool },
    /// Turn the Pen tool's draft into a shape layer. Closed paths get a fill,
    /// open ones only a stroke.
    FinishPath { closed: bool },
    /// Throw the Pen tool's draft away.
    CancelPath,
    /// Remove the anchors the Direct Selection tool has selected.
    DeletePathAnchors,
    /// Open the Segment Object window.
    ShowSegment,
    /// Ask the detector what is in the picture, for that window's list.
    SegmentDetect,
    /// Run the segmenter for the points collected so far and show the result
    /// as the selection.
    SegmentPreview,
    /// Put the selection back as it was before the window opened.
    SegmentCancel,
    /// Combine the selected shape layers into one path with this operation.
    CombineShapes(cshop_core::path::BoolOp),
    /// Step the brush diameter one notch, as `[` and `]` do.
    StepBrushSize(i32),
    /// Step the brush hardness one notch, as Shift+`[` and Shift+`]` do.
    StepBrushHardness(i32),
    /// Set the painting opacity outright, as the digit keys do.
    SetBrushOpacity(f32),
    /// Open one of the Select > Modify dialogs.
    ShowModifyDialog(crate::chrome::ModifyKind),
    /// Fill the selection, or the whole layer, with a chosen colour.
    FillWith {
        color: cshop_core::color::Rgba8,
        mode: cshop_core::blend::BlendMode,
        opacity: f32,
        preserve_transparency: bool,
    },
    /// Open the colour picker for one of the two swatches.
    ShowColorPicker(crate::dialogs::PickerTarget),
    SetColor {
        target: crate::dialogs::PickerTarget,
        color: cshop_core::color::Rgba8,
    },
    ClearLayer,

    // --- selections ---
    SelectAll,
    Deselect,
    Reselect,
    InverseSelection,
    /// Replace the selection outright; used by the selection tools once a drag
    /// finishes. Boxed because a `Selection` owns a document-sized mask and
    /// would otherwise make every `Action` that large.
    SetSelection(Box<Selection>, &'static str),
    ModifySelection(ModifySelection),
    GrowSelection,
    SimilarSelection,
    ToggleQuickMask,
    SaveSelectionAsChannel,
    LoadChannelAsSelection(usize),
    DeleteChannel(usize),
    ToggleChannelVisible(usize),

    // --- adjustments ---
    /// Add a non-destructive adjustment layer above the active layer.
    AddAdjustmentLayer(Box<Adjustment>),
    /// Open the dialog for a destructive adjustment.
    ShowAdjustmentDialog(Box<Adjustment>),
    /// Apply an adjustment straight to the active layer's pixels.
    ApplyAdjustment(Box<Adjustment>),
    /// Retune the active adjustment layer.
    SetAdjustment(Box<Adjustment>),

    // --- transform ---
    /// Start Free Transform on the active layer.
    BeginTransform,
    /// Apply the transform in progress.
    CommitTransform,
    CancelTransform,
    /// Rotate or flip the active layer by a fixed amount.
    TransformPreset(TransformPreset),
    /// Crop the document to the rectangle currently being dragged.
    CommitCrop,
    CancelCrop,
    ShowImageSize,
    /// Open the colour-profile window.
    /// Open the relight window and start working out the depth.
    ShowRelight,
    /// Re-light the picture with the lamp where it is now.
    RelightPreview,
    /// Commit what is on the canvas as one history entry.
    RelightKeep,
    /// Put the original back.
    RelightCancel,
    /// Make what is selected disappear, inventing what was behind it.
    FillInSelection,
    /// Open the separate-by-content window and start looking.
    ShowSeparate,
    /// Make a layer from each kind of thing that was ticked.
    RunSeparate,
    /// Open the upscale window.
    /// Show or hide the rulers, the guides, the grid; turn snapping off.
    ToggleRulers,
    ToggleGuides,
    ToggleGrid,
    ToggleSnap,
    /// Put a guide across the document at a given place.
    AddGuide { vertical: bool, at: f32 },
    ClearGuides,
    /// Forget the list of recently opened files.
    ClearRecent,
    ShowUpscale,
    /// Run the enlarger over every raster layer, on a worker thread.
    RunUpscale,
    /// Open the noise-removal window on the active layer.
    ShowDenoise,
    /// Start the model on the selected region, on a worker thread.
    RunDenoise,
    /// Re-blend the model's answer at the strength now chosen.
    DenoiseRestrength,
    /// Commit what is on the canvas as one history entry.
    DenoiseKeep,
    /// Put the original pixels back.
    DenoiseCancel,
    /// Open the lens-correction window on the active layer.
    ShowLens,
    /// Run the agreed corrections over the whole layer, on a worker thread.
    ApplyLens,
    ShowColorProfile,
    /// Change the document's working space. `None` means the built-in sRGB.
    /// `convert` chooses between rewriting the pixels and relabelling them —
    /// see [`cshop_core::profile`].
    SetColorProfile { path: Option<std::path::PathBuf>, convert: bool },
    /// Move every raster layer to eight or sixteen bits a channel.
    SetDepth(u8),
    ShowCanvasSize,
    ResizeImage { width: u32, height: u32, filter: cshop_core::resample::Resampling },
    /// Find the sky and put a different one in.
    ReplaceSky,
    /// Smooth skin without smoothing eyes and hair.
    RetouchSkin,
    /// Open the window for effects that read the depth.
    ShowDepthFx,
    /// Re-render what it is showing.
    PreviewDepthFx,
    /// Keep it.
    KeepDepthFx,
    /// Throw it away and put the layer back.
    CancelDepthFx,

    /// Show one frame of the animation.
    ShowFrame(usize),
    /// Start or stop playing it.
    TogglePlayback,
    /// Make a timeline out of the layers that are there, or take it away.
    ToggleTimeline,
    /// Set every frame's duration.
    SetFrameDelay(u16),
    /// Set one frame's duration.
    SetOneFrameDelay(usize, u16),

    /// Move every layer onto the bottom one, by finding what they have in
    /// common. See [`cshop_core::align`].
    AlignLayers { motion: cshop_core::align::Motion },
    /// Align, then average — which is how noise is removed by stacking.
    StackLayers,

    /// Start bending the active layer: a mesh over it, or pins in it.
    BeginWarp { puppet: bool },
    CommitWarp,
    CancelWarp,
    /// Resize by carving seams rather than by resampling, so what is in the
    /// picture keeps its proportions. See [`cshop_core::carve`].
    ContentAwareScale { width: u32, height: u32, protect_selection: bool },
    ResizeCanvas { width: u32, height: u32, anchor: Anchor },
    /// Crop the canvas to the current selection.
    CropToSelection,

    /// Open the rename dialog for a layer.
    RenameLayer(LayerId),

    // --- filters ---
    /// Open the dialog for a filter, seeded with its default settings.
    ShowFilterDialog(Box<cshop_core::filters::Filter>),
    /// Apply a filter to the active layer, restricted to the selection.
    ApplyFilter(Box<cshop_core::filters::Filter>),
    /// Re-run the last filter with the settings it was used with.
    RepeatLastFilter,
    /// Attach a filter to the active layer rather than running it into the
    /// pixels. See [`cshop_core::smart_filters`].
    AttachFilter(Box<cshop_core::filters::Filter>),
    /// Take one off again.
    RemoveAttachedFilter(usize),
    /// Switch one on or off, keeping its settings.
    ToggleAttachedFilter(usize),
    /// Switch the whole stack on or off.
    ToggleAttachedFilters,
    /// Reopen the filter window on one that is already attached.
    EditAttachedFilter(usize),
    /// Put new settings on one that is already attached.
    ReplaceAttachedFilter(usize, Box<cshop_core::filters::Filter>),
    /// How much of one slot's result to keep, `0..=1`.
    SetAttachedFilterOpacity(usize, f32),
    /// Run the whole stack into the pixels, so it stops being editable.
    ApplyAttachedFilters,

    // --- masks ---
    AddLayerMask { hide_all: bool },
    AddLayerMaskFromSelection { invert: bool },
    /// Mask the active layer by how far away everything in it is. Needs the
    /// vision pack, so it runs on a worker thread.
    AddLayerMaskFromDepth { invert: bool },
    /// Open the window that selects a colour wherever it appears.
    ShowColorRange,
    /// Make that selection.
    ApplyColorRange(Box<cshop_core::color_range::ColorRange>),
    /// Open the window that fits a selection's edge to the picture.
    ShowRefineEdge,
    /// Refine it.
    ApplyRefineEdge(Box<cshop_core::refine::RefineEdge>),
    /// Turn the path being drawn into a selection.
    SelectionFromPath,
    /// Trace the selection's outline as an editable path.
    PathFromSelection,

    /// Remember what every layer is doing, under a name.
    SaveLayerState(String),
    /// Show a saved one.
    ApplyLayerState(usize),
    /// Replace one with what the layers are doing now.
    UpdateLayerState(usize),
    /// Forget one.
    DeleteLayerState(usize),

    /// Turn the path being drawn with the Pen into a mask on the active layer,
    /// keeping the path so the edge stays exact. See [`cshop_core::layer`].
    AddVectorMask { invert: bool },
    /// Turn the active layer into a mask on the one below it, consuming it.
    LayerToMask,
    /// Load the active layer's mask as the selection.
    SelectionFromMask,
    DeleteLayerMask,
    ApplyLayerMask,
    ToggleMaskEnabled,
    SetEditTarget(EditTarget),

    /// Abandon a selection gesture in progress.
    CancelDrag,
    /// Close and commit a polygonal lasso.
    CloseDrag,

    // --- shell ---
    CloseDialog,
    Quit,
}

impl Action {
    pub fn fill_foreground(preserve_transparency: bool) -> Action {
        Action::FillSwatch { background: false, preserve_transparency }
    }

    pub fn fill_background(preserve_transparency: bool) -> Action {
        Action::FillSwatch { background: true, preserve_transparency }
    }
}
