//! Application state and the top-level layout.

use crate::canvas;
use crate::chrome;
use crate::commands::{Action, ModifySelection, TransformPreset, WindowCommand};
use crate::transform_tool::{ActiveCrop, ActiveTransform};
use crate::dialogs::{BrowserMode, Dialog, FileBrowser, NewDocument};
use crate::doc_view::DocView;
use crate::panels;
use crate::theme::Palette;
use crate::tools::Tool;
use cshop_core::color::Rgba8;
use cshop_core::document::{Dirty, Document, EditTarget};
use cshop_core::geom::{IRect, Vec2};
use cshop_core::adjust::Adjustment;
use cshop_core::history::{
    AddLayer, AddLayerMask, DeleteLayer, MoveLayer, OffsetLayer, RemoveLayerMask,
    ReplaceLayerPixels, ReplaceMaskPixels, ReplacePixels, ResizeCanvas, ResizeImage,
    SetAdjustment, SetLayerProperty, SetSelection,
};
use cshop_core::layer::{Layer, LayerId, LayerKind, LayerMask};
use cshop_core::mask::MaskBuffer;
use cshop_core::paint::{Brush, Clip, PaintMode, Stroke, StrokeSource};
use cshop_core::snapshot::Snapshot;
use cshop_core::pixels::PixelBuffer;
use cshop_core::selection::{Selection, SelectionMode};
use cshop_core::tree::LayerPos;
use cshop_core::wand::{self, WandOptions};
use cshop_gpu::compositor::Compositor;
use cshop_gpu::context::GpuContext;
use std::path::PathBuf;

/// What a stroke is writing into, with the buffer as it was when the stroke
/// began.
///
/// Painting re-derives from the snapshot each frame rather than compounding
/// onto the live buffer, which is what lets the preview match the committed
/// result and makes the undo entry exact.
pub enum StrokeTarget {
    Pixels(Snapshot<Rgba8>),
    /// The active layer's mask.
    Mask(Snapshot<u8>),
    /// The selection itself, while Quick Mask is on.
    QuickMask(Snapshot<u8>),
}

/// Where a stroke takes its colour from. See [`CShopApp::begin_stroke_from`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StrokeFrom {
    /// The foreground colour, as an ordinary brush.
    Colour,
    /// Elsewhere in the picture, as the Clone Stamp.
    Clone,
    /// The picture underneath, filtered — the Blur and Sharpen brushes.
    Filter(cshop_core::paint::BrushFilter),
    /// Texture from elsewhere and tone from where it lands — the Healing
    /// Brush, whose source is set by Alt-clicking as the Clone Stamp's is.
    Heal,
    /// The same, with the source found rather than given — Spot Healing.
    HealSpot,
    /// The layer as it was at a marked history state — the History Brush.
    History,
}

/// A smudge in progress.
///
/// Kept apart from [`ActiveStroke`] because smudging is not a stroke: it
/// cannot accumulate coverage and composite once, since what each dab lays
/// down depends on what the one before it picked up. See
/// [`cshop_core::paint::Smudge`].
pub struct ActiveSmudge {
    layer: LayerId,
    smudge: cshop_core::paint::Smudge,
    snapshot: Snapshot<Rgba8>,
    /// Where the pointer was last, so the next segment knows what it will
    /// touch and can take its undo snapshot before writing.
    last: Vec2,
}

/// A stroke in progress.
pub struct ActiveStroke {
    layer: LayerId,
    stroke: Stroke,
    target: StrokeTarget,
    /// The tool that began the stroke, so the History entry names it even if
    /// the tool is switched before the pointer is released.
    tool: Tool,
}

/// Where a new layer goes: directly above the active one, in the same group.
fn new_layer_pos(view: &crate::doc_view::DocView) -> LayerPos {
    view.doc
        .active
        .and_then(|a| view.doc.tree.position(a))
        .map(|p| LayerPos { parent: p.parent, index: p.index + 1 })
        .unwrap_or(LayerPos { parent: None, index: view.doc.tree.root().len() })
}

/// Diameters that `[` and `]` step through.
const BRUSH_SIZES: &[f32] = &[
    1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 13.0, 16.0, 20.0, 25.0, 30.0, 36.0, 45.0, 55.0, 65.0,
    80.0, 100.0, 125.0, 150.0, 200.0, 250.0, 300.0, 400.0, 500.0, 650.0, 800.0, 1000.0, 1250.0,
    1600.0, 2000.0,
];

pub struct CShopApp {
    pub gpu: GpuContext,
    compositor: Compositor,

    pub docs: Vec<DocView>,
    pub active: Option<usize>,

    pub tool: Tool,

    /// What the canvas shows besides the picture, and whether things catch on
    /// it. View settings rather than document ones — except the guides
    /// themselves, which belong to the document.
    pub show_rulers: bool,
    pub show_guides: bool,
    pub show_grid: bool,
    pub snap: bool,
    pub grid_spacing: f32,
    /// The guide currently being dragged, by index.
    pub dragging_guide: Option<usize>,
    /// What is remembered between runs, and the files opened so far.
    pub settings: crate::settings::Settings,

    pub foreground: Rgba8,
    pub background: Rgba8,
    pub brush: Brush,
    /// Settings the Type tool starts a new layer with, and applies to the one
    /// being edited.
    pub text_style: cshop_core::text::TextStyle,
    /// The type layer currently being edited, if any.
    pub text_edit: Option<crate::text_tool::TextEdit>,
    /// Copy, cut and paste, inside the editor and with the rest of the desktop.
    pub clipboard: crate::clipboard::Clipboard,
    /// Geometry and style the Shape tool draws with, and applies to the shape
    /// layer being edited.
    pub shape_kind: cshop_core::shape::ShapeKind,
    pub shape_style: cshop_core::shape::ShapeStyle,
    /// The shape layer the options bar was last loaded from, so selecting one
    /// adopts its settings instead of overwriting them.
    shape_synced: Option<LayerId>,
    /// Where a Type-tool drag began, for sizing a paragraph box.
    pub drag_start: Option<Vec2>,
    /// The path the Pen tool is laying down, before it becomes a layer.
    pub pen: Option<PenDraft>,
    /// Which anchors of a path layer are selected, and what is being dragged.
    pub path_edit: PathEdit,
    /// Counter behind [`CShopApp::next_edit_run`].
    edit_run: u64,
    /// The selection as it stood before the Segment window opened, so that
    /// cancelling puts it back rather than leaving a half-chosen mask.
    segment_before: Option<cshop_core::selection::Selection>,
    /// A segmentation running on another thread.
    ///
    /// The models take about a second, which is a long time to hold the frame:
    /// with the work on this thread the window freezes and nothing can say it
    /// is busy, least of all a spinner that never gets drawn.
    segment_job: Option<std::sync::mpsc::Receiver<SegmentOutcome>>,
    /// The full-resolution lens pass, while it runs. See [`CShopApp::poll_lens`].
    lens_job: Option<std::sync::mpsc::Receiver<LensOutcome>>,
    /// The denoising run, while it works its way through the tiles.
    denoise_job: Option<std::sync::mpsc::Receiver<Result<PixelBuffer, String>>>,
    /// The depth model, while it looks at the picture.
    depth_job: Option<std::sync::mpsc::Receiver<Result<cshop_core::relight::DepthMap, String>>>,
    /// Set while the depth being worked out is destined for a mask rather than
    /// for the Relight window: the layer to put it on, and which way round.
    depth_for_mask: Option<(LayerId, bool)>,
    /// The hole being filled in, while the model works on it.
    #[allow(clippy::type_complexity)]
    inpaint_job: Option<std::sync::mpsc::Receiver<Result<(LayerId, PixelBuffer), String>>>,
    /// The map the labeller wrote, kept between looking and separating.
    separate_map: Option<PixelBuffer>,
    /// The labeller, while it looks: what it found and the map it wrote.
    #[allow(clippy::type_complexity)]
    separate_job:
        Option<std::sync::mpsc::Receiver<Result<(Vec<crate::vision::Region>, PixelBuffer), String>>>,
    /// The enlargement, and the size and scale it was started for.
    #[allow(clippy::type_complexity)]
    upscale_job:
        Option<std::sync::mpsc::Receiver<Result<((u32, u32), f32, Vec<(LayerId, PixelBuffer)>), String>>>,
    /// This frame's clock, for anything that animates without a widget of its
    /// own — the type caret's blink.
    pub now: f64,

    /// How the next selection combines with the current one.
    pub selection_mode: SelectionMode,
    /// Feather radius applied to marquee and lasso selections, in pixels.
    pub selection_feather: f32,
    pub selection_antialias: bool,
    pub wand: WandOptions,
    /// Sample the composited image rather than the active layer.
    pub sample_all_layers: bool,
    /// Quick Mask shows the selection as a paintable red overlay.
    pub quick_mask: bool,

    /// Paint Bucket settings.
    pub bucket: cshop_core::fill::BucketOptions,
    /// Dodge, Burn and Sponge settings. One set between the three, because
    /// they are one tool with three signs and the range is the control that
    /// matters most.
    pub retouch: cshop_core::retouch::Retouch,
    /// How hard the Blur, Sharpen and Smudge brushes work, `0..=1`. One
    /// control between the three, as with the retouching tools: it means the
    /// same thing in each, and the brush size decides the reach.
    pub brush_filter_strength: f32,
    /// Gradient settings.
    pub gradient: cshop_core::fill::Gradient,
    /// A gradient being dragged out, in document coordinates.
    pub gradient_drag: Option<(Vec2, Vec2)>,
    /// Where the Clone Stamp is sampling from, set by Alt-clicking.
    pub clone_anchor: Option<Vec2>,
    /// Keep the clone offset between strokes rather than restarting from the
    /// anchor each time.
    pub clone_aligned: bool,
    /// The offset in force for the current aligned run.
    clone_offset: Option<(i32, i32)>,

    pub dialog: Dialog,
    /// Transient message for the status bar; `true` means it is an error.
    pub toast: Option<(String, bool)>,
    pub show_panels: bool,

    /// Where the canvas was last drawn, so shortcuts can zoom about its centre.
    pub canvas_viewport: egui::Rect,

    /// Luminance histogram of the composited document, behind the Levels and
    /// Curves editors. Recomputed only when the document changes, because it
    /// costs a full read-back off the GPU.
    histogram: Option<crate::properties::Histogram>,
    histogram_key: Option<(cshop_core::document::DocumentId, usize)>,

    stroke: Option<ActiveStroke>,
    smudge: Option<ActiveSmudge>,
    /// A selection gesture in progress.
    pub drag: Option<SelectionDrag>,
    /// A Free Transform in progress.
    pub transform: Option<ActiveTransform>,
    /// A crop rectangle being dragged.
    pub crop: Option<ActiveCrop>,
    /// The last filter applied, for Repeat Last Filter.
    pub last_filter: Option<cshop_core::filters::Filter>,

    /// Requests for the event loop, drained after each frame.
    pub window_commands: Vec<WindowCommand>,
    /// Whether the window is maximised, so the title bar can show the right
    /// icon. Refreshed by the event loop before each frame.
    pub is_maximized: bool,
    /// The application logo, uploaded once.
    logo: Option<egui::TextureHandle>,
    actions: Vec<Action>,
    untitled_count: u32,
    /// Settings the New Document dialog collected, consumed by its action.
    pub quit: bool,
}

impl CShopApp {
    pub fn new(gpu: GpuContext) -> Self {
        Self::with_settings(gpu, crate::settings::Settings::load())
    }

    /// The same, with settings supplied rather than read — which is what a
    /// test wants, since a test that read the machine's own settings would
    /// pass or fail depending on who ran it.
    pub fn with_settings(gpu: GpuContext, settings: crate::settings::Settings) -> Self {
        let compositor = Compositor::new(&gpu);
        Self {
            gpu,
            compositor,
            docs: Vec::new(),
            active: None,
            tool: settings.tool,
            show_rulers: settings.show_rulers,
            show_guides: settings.show_guides,
            show_grid: settings.show_grid,
            snap: settings.snap,
            grid_spacing: settings.grid_spacing,
            dragging_guide: None,
            foreground: settings.foreground,
            background: settings.background,
            brush: settings.brush,
            text_style: cshop_core::text::TextStyle::default(),
            text_edit: None,
            clipboard: Default::default(),
            shape_kind: cshop_core::shape::ShapeKind::Rectangle { radius: 0.0 },
            shape_style: cshop_core::shape::ShapeStyle::default(),
            shape_synced: None,
            drag_start: None,
            pen: None,
            path_edit: PathEdit::default(),
            edit_run: 0,
            segment_before: None,
            segment_job: None,
            lens_job: None,
            denoise_job: None,
            upscale_job: None,
            separate_job: None,
            separate_map: None,
            inpaint_job: None,
            depth_job: None,
            depth_for_mask: None,
            now: 0.0,
            selection_mode: SelectionMode::Replace,
            selection_feather: 0.0,
            selection_antialias: true,
            wand: WandOptions::default(),
            sample_all_layers: false,
            quick_mask: false,
            bucket: Default::default(),
            retouch: settings.retouch,
            brush_filter_strength: settings.brush_filter_strength,
            gradient: Default::default(),
            gradient_drag: None,
            clone_anchor: None,
            clone_aligned: true,
            clone_offset: None,
            dialog: Dialog::None,
            toast: None,
            show_panels: settings.show_panels,
            settings,
            canvas_viewport: egui::Rect::NOTHING,
            histogram: None,
            histogram_key: None,
            stroke: None,
            smudge: None,
            drag: None,
            transform: None,
            crop: None,
            last_filter: None,
            window_commands: Vec::new(),
            is_maximized: false,
            logo: None,
            actions: Vec::new(),
            untitled_count: 0,
            quit: false,
        }
    }

    pub fn doc(&self) -> Option<&DocView> {
        self.active.and_then(|i| self.docs.get(i))
    }

    pub fn doc_mut(&mut self) -> Option<&mut DocView> {
        match self.active {
            Some(i) => self.docs.get_mut(i),
            None => None,
        }
    }

    pub fn push(&mut self, action: Action) {
        self.actions.push(action);
    }

    pub fn window(&mut self, command: WindowCommand) {
        self.window_commands.push(command);
    }

    /// The logo, decoded and uploaded on first use.
    ///
    /// Embedded in the binary rather than loaded from disk, so a built
    /// executable has no assets to lose.
    pub fn logo(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        if let Some(handle) = &self.logo {
            return handle.clone();
        }
        const LOGO: &[u8] = include_bytes!("../../../assets/logo.png");
        let image = cshop_io::decode(LOGO, None).unwrap_or_else(|e| {
            log::error!("the embedded logo failed to decode: {e}");
            cshop_core::pixels::PixelBuffer::new(1, 1)
        });
        let pixels: Vec<egui::Color32> = image
            .pixels()
            .iter()
            .map(|p| egui::Color32::from_rgba_unmultiplied(p.r, p.g, p.b, p.a))
            .collect();
        let handle = ctx.load_texture(
            "logo",
            egui::ColorImage {
                size: [image.width() as usize, image.height() as usize],
                source_size: egui::vec2(image.width() as f32, image.height() as f32),
                pixels,
            },
            egui::TextureOptions::LINEAR,
        );
        self.logo = Some(handle.clone());
        handle
    }

    /// Histogram of the current document, computed on demand.
    ///
    /// Keyed on the history position, so it refreshes when the image changes
    /// but not while a slider is merely being dragged — which would otherwise
    /// stall the frame on a GPU read-back every mouse move.
    pub fn histogram(&mut self) -> Option<&crate::properties::Histogram> {
        let index = self.active?;
        let key = (self.docs[index].doc.id, self.docs[index].history.cursor());
        if self.histogram_key != Some(key) {
            let gpu = self.gpu.clone();
            let view = &mut self.docs[index];
            view.sync_composite_only(&gpu, &mut self.compositor);
            let pixels = view.read_composite(&gpu);
            self.histogram = Some(crate::properties::Histogram::of(&pixels));
            self.histogram_key = Some(key);
        }
        self.histogram.as_ref()
    }

    fn notify(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), false));
    }

    /// True when the active layer holds sixteen bits a channel.
    fn active_is_deep(&self) -> bool {
        self.doc()
            .and_then(|v| Some(v.doc.tree.get(v.doc.active?)?.surface()?.depth() == 16))
            .unwrap_or(false)
    }

    /// The complaint that fits.
    ///
    /// Every tool works in eight bits, so a sixteen-bit layer turns them all
    /// away — but it is a raster layer, and telling someone it is not one
    /// sends them looking for a problem they do not have. This substitutes
    /// the real objection, and the one menu item that answers it.
    fn why_not(&self, otherwise: &str) -> String {
        if self.active_is_deep() {
            "That layer holds sixteen bits a channel, and the tools work in eight. \
             Image ▸ Mode ▸ 8 Bits/Channel converts it."
                .into()
        } else {
            otherwise.into()
        }
    }

    fn fail(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        log::error!("{msg}");
        self.toast = Some((msg, true));
    }

    // -----------------------------------------------------------------------
    // Frame
    // -----------------------------------------------------------------------

    /// Draw one frame. `ui` is the root [`egui::Ui`] from
    /// [`egui::Context::run_ui`]; egui 0.36 nests panels inside a `Ui` rather
    /// than attaching them to the context.
    pub fn update(&mut self, ui: &mut egui::Ui, renderer: &mut egui_wgpu::Renderer) {
        let ctx = ui.ctx().clone();
        self.now = ctx.input(|i| i.time);
        self.poll_segment(&ctx);
        self.poll_lens(&ctx);
        self.poll_denoise(&ctx);
        self.poll_upscale(&ctx);
        self.poll_separate(&ctx);
        self.poll_inpaint(&ctx);
        self.poll_depth(&ctx);
        self.handle_shortcuts(&ctx);

        egui::Panel::top("titlebar")
            .frame(egui::Frame::NONE.fill(Palette::DARK.titlebar))
            .exact_size(30.0)
            .show(ui, |ui| chrome::title_bar(self, ui));

        egui::Panel::top("optionsbar")
            .frame(crate::theme::bar_frame())
            .exact_size(30.0)
            .show(ui, |ui| chrome::options_bar(self, ui));

        egui::Panel::bottom("statusbar")
            .frame(crate::theme::bar_frame())
            .exact_size(24.0)
            .show(ui, |ui| chrome::status_bar(self, ui));

        egui::Panel::left("toolbox")
            .frame(egui::Frame::NONE.fill(Palette::DARK.chrome).inner_margin(3))
            .exact_size(38.0)
            .resizable(false)
            .show(ui, |ui| chrome::toolbox(self, ui));

        if self.show_panels {
            egui::Panel::right("docks")
                .frame(egui::Frame::NONE.fill(Palette::DARK.panel))
                .default_size(280.0)
                .size_range(220.0..=460.0)
                .show(ui, |ui| panels::dock(self, ui));
        }

        egui::CentralPanel::no_frame()
            .frame(egui::Frame::NONE.fill(Palette::DARK.canvas_backdrop))
            .show(ui, |ui| {
                chrome::document_tabs(self, ui);
                canvas::show(self, ui);
            });

        // Drawn last so the edges sit above every panel.
        chrome::resize_borders(self, ui);

        self.show_dialog(&ctx);
        self.run_actions();
        self.sync_documents(renderer);
    }

    fn show_dialog(&mut self, ctx: &egui::Context) {
        if !self.dialog.is_open() {
            return;
        }
        // Take the dialog out so its `ui` can borrow `self` for the action
        // queue, then put it back unless it asked to close.
        let mut dialog = std::mem::replace(&mut self.dialog, Dialog::None);
        let mut actions = Vec::new();
        let mut close = false;

        let title_owned;
        let title: &str = match &dialog {
            Dialog::NewDocument(_) => "New Document",
            Dialog::FileBrowser(b) if b.mode == BrowserMode::Open => "Open",
            Dialog::FileBrowser(_) => "Save As",
            Dialog::Modify(d) => d.title(),
            Dialog::ImageSize(d) => d.title(),
            Dialog::Rename(_) => "Rename Layer",
            Dialog::Segment(d) => d.title(),
            Dialog::ColorRange(_) => "Colour Range",
            Dialog::RefineEdge(_) => "Refine Edge",
            Dialog::Fill(_) => "Fill",
            Dialog::ColorPicker(d) => d.title(),
            Dialog::ColorProfile(d) => d.title(),
            Dialog::Lens(d) => d.title(),
            Dialog::Denoise(d) => d.title(),
            Dialog::Upscale(d) => d.title(),
            Dialog::Separate(d) => d.title(),
            Dialog::Relight(d) => d.title(),
            Dialog::Adjustment(d) => {
                title_owned = d.title();
                &title_owned
            }
            Dialog::LayerStyle(d) => {
                title_owned = d.title();
                &title_owned
            }
            Dialog::Filter(d) => {
                // Titles are owned here, so borrow-safe: leak-free by cloning.
                title_owned = d.title();
                &title_owned
            }
            Dialog::About => "About C-Shop",
            Dialog::None => "",
        };

        // An explicitly opaque frame: the default popup frame lets the canvas
        // show through, which on a dark theme makes dialog text hard to read.
        let frame = egui::Frame::NONE
            .fill(Palette::DARK.panel)
            .stroke(egui::Stroke::new(1.0, Palette::DARK.separator))
            .inner_margin(egui::Margin::symmetric(14, 12))
            .corner_radius(3);

        // The filter dialog puts a preview beside its controls and needs the
        // room; everything else is a column of fields.
        let max_width = if matches!(dialog, Dialog::Filter(_)) { 780.0 } else { 620.0 };

        // Anything whose answer shows up on the canvas gets a window that can
        // be pushed aside, and its own name to remember where it was put.
        // Sharing one name — which they used to — means moving one of them
        // moves where all the others will next appear.
        let movable = window_key(&dialog);

        let gpu = self.gpu.clone();
        let mut body = |ui: &mut egui::Ui| {
            ui.set_max_width(max_width);
            match &mut dialog {
                Dialog::NewDocument(d) => close = d.ui(ui, &mut actions),
                Dialog::FileBrowser(b) => close = b.ui(ui, &mut actions),
                Dialog::Modify(d) => close = d.ui(ui, &mut actions),
                Dialog::ImageSize(d) => close = d.ui(ui, &mut actions),
                Dialog::Rename(d) => close = d.ui(ui, &mut actions),
                Dialog::Fill(d) => close = d.ui(ui, &mut actions),
                Dialog::ColorPicker(d) => close = d.ui(ui, &mut actions),
                Dialog::ColorProfile(d) => close = d.ui(ui, &mut actions),
                Dialog::Lens(d) => close = d.ui(ui, &mut actions),
                Dialog::Denoise(d) => close = d.ui(ui, &mut actions),
                Dialog::Upscale(d) => close = d.ui(ui, &mut actions),
                Dialog::Separate(d) => close = d.ui(ui, &mut actions),
                Dialog::Relight(d) => close = d.ui(ui, &mut actions),
                Dialog::Adjustment(d) => close = d.ui(ui, &mut actions),
                Dialog::LayerStyle(d) => close = d.ui(ui, &mut actions),
            Dialog::Segment(d) => close = d.ui(ui, &mut actions),
                Dialog::Filter(d) => close = d.ui(ui, &mut actions),
                Dialog::ColorRange(d) => close = d.ui(ui, &mut actions),
                Dialog::RefineEdge(d) => close = d.ui(ui, &mut actions),
                Dialog::About => {
                    ui.label("C-Shop — a native, GPU-accelerated layered image editor.");
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "Renderer: {} ({:?})",
                            gpu.adapter_name(),
                            gpu.adapter.get_info().backend
                        ))
                        .color(Palette::DARK.text_dim)
                        .small(),
                    );
                    ui.add_space(10.0);
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                }
                Dialog::None => {}
            }
        };

        if let Some(key) = movable {
            egui::Window::new(title)
                // Its own id, so dragging it somewhere keeps it there across
                // frames and across openings — and does not drag the others
                // with it.
                .id(egui::Id::new(key))
                .frame(frame)
                .collapsible(false)
                .resizable(false)
                .constrain(true)
                // Out of the middle of the canvas to begin with, since the
                // point is to watch what it does.
                .default_pos(egui::pos2(60.0, 90.0))
                .show(ctx, |ui| body(ui));
        } else {
            egui::Modal::new(egui::Id::new("modal"))
                .frame(frame)
                .backdrop_color(egui::Color32::from_black_alpha(150))
                .show(ctx, |ui| {
                    ui.heading(title);
                    ui.add_space(8.0);
                    body(ui);
                });
        }

        self.finish_dialog_frame(dialog, close, actions);
    }

    /// The settings as they stand, ready to be written.
    ///
    /// Gathered when they are wanted rather than mirrored on every change:
    /// there is one place that has to be right, instead of a dozen that each
    /// have to remember.
    pub fn current_settings(&self) -> crate::settings::Settings {
        crate::settings::Settings {
            window: self.settings.window,
            tool: self.tool,
            brush: self.brush,
            foreground: self.foreground,
            background: self.background,
            show_rulers: self.show_rulers,
            show_guides: self.show_guides,
            show_grid: self.show_grid,
            snap: self.snap,
            grid_spacing: self.grid_spacing,
            show_panels: self.show_panels,
            retouch: self.retouch,
            brush_filter_strength: self.brush_filter_strength,
            recent: self.settings.recent.clone(),
        }
    }

    /// Write them down. Called on the way out, and cheap enough to call again.
    pub fn save_settings(&mut self, window: Option<(u32, u32)>) {
        if window.is_some() {
            self.settings.window = window;
        }
        self.current_settings().save();
    }

    /// What to do with a window once it has drawn itself and said what it
    /// wants.
    ///
    /// The window is put back either way, and closed *after* whatever it asked
    /// for has run. A button that both queues an action and closes the window
    /// is the ordinary shape of these, and dropping the window first meant any
    /// handler that read it back — for which layer, which classes, what to put
    /// back — found nothing and returned in silence.
    ///
    /// Public because it is where that goes wrong, and a test that pushes an
    /// action by hand leaves the window open and so proves nothing.
    pub fn finish_dialog_frame(&mut self, dialog: Dialog, close: bool, actions: Vec<Action>) {
        self.dialog = dialog;
        self.actions.extend(actions);
        if close {
            self.actions.push(Action::CloseDialog);
        }
    }

    fn sync_documents(&mut self, renderer: &mut egui_wgpu::Renderer) {
        // Only the visible document is worth compositing every frame.
        if let Some(i) = self.active {
            let gpu = self.gpu.clone();
            if let Some(doc) = self.docs.get_mut(i) {
                doc.sync(&gpu, &mut self.compositor, renderer);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Keyboard
    // -----------------------------------------------------------------------

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.dialog.is_open() {
            return;
        }
        // While type is being edited the keyboard belongs to it, or pressing
        // "b" would swap to the Brush instead of typing a letter.
        if self.text_edit.is_some() {
            self.handle_text_keys(ctx);
            return;
        }
        // Typing in a text field must not fire tool shortcuts.
        if ctx.egui_wants_keyboard_input() {
            return;
        }

        let mut queued = Vec::new();
        ctx.input(|i| {
            let shift = i.modifiers.shift;
            let plain = !i.modifiers.command && !i.modifiers.alt;

            // Everything that maps straight to one command.
            for binding in crate::shortcuts::bindings() {
                if binding.chord.pressed(i) {
                    queued.push((binding.make)());
                }
            }

            if plain {
                // Enter and Escape mean "apply" and "abandon" for whatever
                // gesture is live, in priority order.
                if i.key_pressed(egui::Key::Escape) {
                    if self.transform.is_some() {
                        queued.push(Action::CancelTransform);
                    } else if self.crop.is_some() {
                        queued.push(Action::CancelCrop);
                    } else if self.pen.is_some() {
                        queued.push(Action::CancelPath);
                    } else if !self.path_edit.selected.is_empty() {
                        self.path_edit.selected.clear();
                    } else {
                        queued.push(Action::CancelDrag);
                    }
                }
                // Delete removes points while the Direct Selection tool has
                // some, rather than whatever it would otherwise mean.
                if (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
                    && self.tool == Tool::DirectSelect
                    && !self.path_edit.selected.is_empty()
                {
                    queued.push(Action::DeletePathAnchors);
                }
                if i.key_pressed(egui::Key::Enter) {
                    if self.pen.is_some() {
                        // Enter ends the path where it is, leaving it open.
                        queued.push(Action::FinishPath { closed: false });
                    } else if self.transform.is_some() {
                        queued.push(Action::CommitTransform);
                    } else if self.crop.is_some() {
                        queued.push(Action::CommitCrop);
                    } else {
                        queued.push(Action::CloseDrag);
                    }
                }
                if !shift
                    && (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
                {
                    queued.push(Action::ClearLayer);
                }

                // Nudging with the arrow keys, 10px with Shift.
                let step = if shift { 10 } else { 1 };
                for (key, dx, dy) in [
                    (egui::Key::ArrowLeft, -step, 0),
                    (egui::Key::ArrowRight, step, 0),
                    (egui::Key::ArrowUp, 0, -step),
                    (egui::Key::ArrowDown, 0, step),
                ] {
                    if i.key_pressed(key) {
                        queued.push(Action::NudgeLayer(dx, dy));
                    }
                }

                // The brackets size the brush, and with Shift set its
                // hardness — both in the conventional steps.
                if !shift {
                    if i.key_pressed(egui::Key::OpenBracket) {
                        queued.push(Action::StepBrushSize(-1));
                    }
                    if i.key_pressed(egui::Key::CloseBracket) {
                        queued.push(Action::StepBrushSize(1));
                    }
                } else {
                    if i.key_pressed(egui::Key::OpenBracket) {
                        queued.push(Action::StepBrushHardness(-1));
                    }
                    if i.key_pressed(egui::Key::CloseBracket) {
                        queued.push(Action::StepBrushHardness(1));
                    }
                }

                // A digit sets the painting opacity: 1 is 10%, 0 is 100%.
                if !shift && self.tool.uses_brush() {
                    for (n, key) in [
                        (1, egui::Key::Num1),
                        (2, egui::Key::Num2),
                        (3, egui::Key::Num3),
                        (4, egui::Key::Num4),
                        (5, egui::Key::Num5),
                        (6, egui::Key::Num6),
                        (7, egui::Key::Num7),
                        (8, egui::Key::Num8),
                        (9, egui::Key::Num9),
                        (10, egui::Key::Num0),
                    ] {
                        if i.key_pressed(key) {
                            queued.push(Action::SetBrushOpacity(n as f32 / 10.0));
                        }
                    }
                }

                // Tool shortcuts, which also cycle within a group.
                for group in crate::tools::TOOL_GROUPS {
                    if i.key_pressed(group.key) {
                        queued.push(Action::SelectTool(crate::tools::cycle(group, self.tool)));
                    }
                }
            }
        });

        self.actions.extend(queued);
    }

    /// The topmost type layer under a document point.
    pub fn text_layer_at(&self, at: Vec2) -> Option<LayerId> {
        let view = self.doc()?;
        // Rows run top-first, which is the order a click should resolve in.
        view.doc
            .tree
            .visible_rows()
            .iter()
            .map(|(id, _)| *id)
            .find(|id| {
                view.doc.tree.get(*id).is_some_and(|l| {
                    l.text().is_some()
                        && l.visible
                        && l.bounds().contains(at.x.round() as i32, at.y.round() as i32)
                })
            })
    }

    /// Whether a point falls inside the type currently being edited.
    pub fn editing_text_contains(&self, at: Vec2) -> bool {
        let Some(edit) = self.text_edit.as_ref() else { return false };
        let Some(view) = self.doc() else { return false };
        view.doc
            .tree
            .get(edit.layer)
            .is_some_and(|l| l.bounds().contains(at.x.round() as i32, at.y.round() as i32))
    }

    /// The caret's rectangle in document space, for drawing it.
    pub fn text_caret_rect(&self) -> Option<(Vec2, Vec2)> {
        let edit = self.text_edit.as_ref()?;
        let view = self.doc()?;
        let layer = view.doc.tree.get(edit.layer)?;
        let text = layer.text()?;
        let content = text.content();
        let font = cshop_core::font::FontDb::global().load(
            &content.style.family,
            content.style.bold,
            content.style.italic,
        )?;
        let caret = cshop_core::text::caret_at(content, &font, edit.caret);
        let origin = (
            (layer.offset.0 + text.layout_origin().0) as f32,
            (layer.offset.1 + text.layout_origin().1) as f32,
        );
        Some((
            Vec2::new(origin.0 + caret.x, origin.1 + caret.top),
            Vec2::new(origin.0 + caret.x, origin.1 + caret.top + caret.height),
        ))
    }

    /// Keyboard while type is being edited.
    fn handle_text_keys(&mut self, ctx: &egui::Context) {
        use crate::text_tool::TextInput as T;
        let mut queued: Vec<T> = Vec::new();
        ctx.input(|i| {
            let cmd = i.modifiers.command;
            for event in &i.events {
                match event {
                    // Typed characters arrive already composed, so this also
                    // covers dead keys and anything an input method produces.
                    egui::Event::Text(text) if !cmd => queued.push(T::Insert(text.clone())),
                    egui::Event::Key { key, pressed: true, modifiers, .. } => {
                        let step = match key {
                            egui::Key::Backspace => Some(T::Backspace),
                            egui::Key::Delete => Some(T::Delete),
                            egui::Key::ArrowLeft => Some(T::Left),
                            egui::Key::ArrowRight => Some(T::Right),
                            egui::Key::ArrowUp => Some(T::Up),
                            egui::Key::ArrowDown => Some(T::Down),
                            egui::Key::Home => Some(T::Home),
                            egui::Key::End => Some(T::End),
                            egui::Key::Escape => Some(T::Cancel),
                            // Ctrl+Enter commits, a bare Enter breaks the
                            // line — the conventional split.
                            egui::Key::Enter if modifiers.command => Some(T::Commit),
                            egui::Key::Enter => Some(T::Newline),
                            _ => None,
                        };
                        if let Some(step) = step {
                            queued.push(step);
                        }
                    }
                    _ => {}
                }
            }
        });
        if !queued.is_empty() {
            // Typing restarts the blink, so the caret stays solid while
            // someone is actually writing.
            if let Some(edit) = self.text_edit.as_mut() {
                edit.blink_epoch = self.now;
            }
        }
        for step in queued {
            self.actions.push(Action::TextInput(step));
        }
    }

    // -----------------------------------------------------------------------
    // Action dispatch
    // -----------------------------------------------------------------------

    fn run_actions(&mut self) {
        // An action can queue another (Save falling through to Save As), so
        // drain until the queue settles rather than only once.
        for _ in 0..8 {
            if self.actions.is_empty() {
                return;
            }
            for action in std::mem::take(&mut self.actions) {
                self.run(action);
            }
        }
        if !self.actions.is_empty() {
            log::warn!("action queue did not settle; dropping {}", self.actions.len());
            self.actions.clear();
        }
    }

    /// What the New Document window is asking for, read from the window
    /// itself rather than from a copy taken as it closed.
    fn new_document_settings(
        &self,
    ) -> Option<(String, u32, u32, cshop_core::document::Background)> {
        match &self.dialog {
            Dialog::NewDocument(d) => Some((d.name.clone(), d.width, d.height, d.background())),
            _ => None,
        }
    }

    fn run(&mut self, action: Action) {
        match action {
            Action::NewDocument => match self.new_document_settings() {
                Some((name, w, h, bg)) => {
                    self.untitled_count += 1;
                    let name = if name.trim().is_empty() {
                        format!("Untitled-{}", self.untitled_count)
                    } else {
                        name
                    };
                    let doc = Document::new(name, w, h, bg);
                    self.add_document(doc, "New");
                }
                None => self.dialog = Dialog::NewDocument(NewDocument::default()),
            },

            Action::ShowOpenDialog => {
                let start = self.doc().and_then(|d| d.doc.path.clone());
                self.dialog =
                    Dialog::FileBrowser(FileBrowser::new(BrowserMode::Open, start));
            }

            Action::OpenPath(path) => self.open_path(path),

            Action::Save => {
                match self.doc().and_then(|d| d.doc.path.clone()) {
                    // Plain Save writes what the document already is. Depth is
                    // a choice made in the Save As window, not carried along.
                    Some(path) => self.save_to(path, false),
                    // Never saved before, so Save behaves as Save As.
                    None => self.push(Action::ShowSaveAsDialog),
                }
            }

            Action::ShowSaveAsDialog => {
                if self.doc().is_none() {
                    return;
                }
                let start = self.doc().and_then(|d| d.doc.path.clone());
                let mut browser = FileBrowser::new(BrowserMode::Save, start);
                if let Some(d) = self.doc() {
                    browser.filename = d.doc.name.clone();
                }
                self.dialog = Dialog::FileBrowser(browser);
            }

            Action::SavePath { path, deep } => self.save_to(path, deep),

            Action::CloseDocument(i) => {
                let current = self.active.unwrap_or(0);
                let i = if i == usize::MAX { current } else { i };
                if i >= self.docs.len() {
                    return;
                }
                self.docs.remove(i);
                self.active = if self.docs.is_empty() {
                    None
                } else if current > i {
                    // Closing a tab to the left must keep the same document
                    // focused, not jump to the one that took its index.
                    Some(current - 1)
                } else {
                    Some(current.min(self.docs.len() - 1))
                };
                // The newly focused tab was not composited while hidden.
                if let Some(view) = self.doc_mut() {
                    view.invalidate();
                }
            }

            Action::SelectDocument(i) => {
                if i < self.docs.len() {
                    self.active = Some(i);
                    // The tab was not composited while hidden.
                    self.docs[i].invalidate();
                }
            }

            Action::Undo => {
                let gpu = self.gpu.clone();
                if let Some(view) = self.doc_mut() {
                    if let Some(dirty) = view.history.undo(&mut view.doc) {
                        view.mark_dirty(dirty);
                        // Undoing can change the document's size — an image
                        // resize, a crop, an enlargement — and the targets it
                        // composites into have to follow. Without this the old
                        // picture stays on screen around the restored one,
                        // which reads as the two superimposed. Cheap: it
                        // returns immediately when the size has not moved.
                        view.resize_targets(&gpu);
                        view.invalidate();
                    }
                }
            }
            Action::Redo => {
                let gpu = self.gpu.clone();
                if let Some(view) = self.doc_mut() {
                    if let Some(dirty) = view.history.redo(&mut view.doc) {
                        view.mark_dirty(dirty);
                        view.resize_targets(&gpu);
                        view.invalidate();
                    }
                }
            }
            Action::HistoryJump(target) => {
                let gpu = self.gpu.clone();
                if let Some(view) = self.doc_mut() {
                    let dirty = view.history.jump_to(&mut view.doc, target);
                    view.mark_dirty(dirty);
                    view.resize_targets(&gpu);
                    view.invalidate();
                }
            }

            Action::SetHistorySource(target) => {
                let gpu = self.gpu.clone();
                let Some(view) = self.doc_mut() else { return };
                let Some(id) = view.doc.active else {
                    self.fail("Select a layer to paint back from");
                    return;
                };
                // Walk to that state, take a copy of the layer, and walk back.
                // Undo is exact, so this leaves the document precisely where
                // it was; doing it now rather than per stroke keeps the walk
                // off the pointer's path.
                let was = view.history.cursor();
                let there = view.history.jump_to(&mut view.doc, target);
                let taken = view.doc.tree.get(id).and_then(|l| l.pixels()).cloned();
                let back = view.history.jump_to(&mut view.doc, was);
                view.mark_dirty(there);
                view.mark_dirty(back);
                view.resize_targets(&gpu);
                view.invalidate();

                match taken {
                    Some(px) => {
                        let name = view.history.label_at(target);
                        view.history_source = Some((target, id, px));
                        self.notify(format!("The History Brush now paints back to {name}"));
                    }
                    None => self.fail(
                        "That layer is not a raster layer at that point in the history",
                    ),
                }
            }

            Action::SelectTool(t) => {
                // Leaving the Type tool commits, as clicking another tool does
                // elsewhere.
                if t != Tool::Text {
                    self.commit_text();
                }
                self.tool = t;
                // Otherwise picking, say, the Paint Bucket and clicking looks
                // like the canvas has stopped responding. The options bar says
                // so too, but that is easy to miss.
                if !t.is_implemented() {
                    self.notify(format!("The {} tool is not implemented yet", t.name()));
                }
            }
            Action::SwapColors => std::mem::swap(&mut self.foreground, &mut self.background),
            Action::ResetColors => {
                self.foreground = Rgba8::BLACK;
                self.background = Rgba8::WHITE;
            }

            Action::ZoomIn | Action::ZoomOut => {
                let up = matches!(action, Action::ZoomIn);
                let viewport = self.canvas_viewport;
                if let Some(view) = self.doc_mut() {
                    let z = view.stepped_zoom(up);
                    view.zoom_to(viewport, z, viewport.center());
                }
            }
            Action::ZoomFit => {
                let viewport = self.canvas_viewport;
                if let Some(view) = self.doc_mut() {
                    view.fit_to(viewport);
                }
            }
            Action::ZoomActual => {
                let viewport = self.canvas_viewport;
                if let Some(view) = self.doc_mut() {
                    view.zoom_to(viewport, 1.0, viewport.center());
                }
            }
            Action::TogglePanels => self.show_panels = !self.show_panels,

            Action::NewLayer => self.add_layer(false),
            Action::NewGroup => self.add_layer(true),

            Action::DeleteLayer => {
                if let Some(view) = self.doc_mut() {
                    if let Some(id) = view.doc.active {
                        if view.doc.tree.len() > 1 {
                            let dirty =
                                view.history.apply(&mut view.doc, Box::new(DeleteLayer::new(id)));
                            view.mark_dirty(dirty);
                            view.invalidate();
                        }
                    }
                }
            }

            Action::DuplicateLayer => self.duplicate_layer(),
            Action::LayerViaCopy => self.layer_via_copy(),
            Action::MergeDown => self.merge_down(),
            Action::FlattenImage => self.flatten(),

            Action::SelectLayer(id) => {
                if let Some(view) = self.doc_mut() {
                    view.doc.select(Some(id));
                }
            }

            Action::SetLayerProperty(id, prop) => {
                if let Some(view) = self.doc_mut() {
                    let dirty = view
                        .history
                        .apply(&mut view.doc, Box::new(SetLayerProperty::new(id, prop)));
                    view.mark_dirty(dirty);
                }
            }

            Action::ToggleClippingMask => {
                let Some(view) = self.doc_mut() else { return };
                let Some(id) = view.doc.active else { return };
                let Some(layer) = view.doc.tree.get(id) else { return };
                let now = !layer.clipping;
                self.push(Action::SetLayerProperty(id, cshop_core::history::LayerProperty::Clipping(now)));
            }

            Action::ReorderActiveLayer(by) => {
                let Some(view) = self.doc() else { return };
                let Some(id) = view.doc.active else { return };
                let Some(pos) = view.doc.tree.position(id) else { return };
                let count = view.doc.tree.children(pos.parent).len();
                // Index 0 is the bottom of the stack, so "forward" is up.
                let target = match by {
                    i32::MAX => count.saturating_sub(1),
                    i32::MIN => 0,
                    d => (pos.index as i64 + d as i64).clamp(0, count as i64 - 1) as usize,
                };
                if target != pos.index {
                    // `move_to` reads the index as a position *before* the
                    // layer is lifted out, so moving up has to aim one past
                    // where it should land.
                    let insert_at = if target > pos.index { target + 1 } else { target };
                    self.push(Action::MoveLayer(
                        id,
                        LayerPos { parent: pos.parent, index: insert_at },
                    ));
                }
            }

            Action::StepActiveLayer(by) => {
                let Some(view) = self.doc() else { return };
                let Some(id) = view.doc.active else { return };
                // Walk the panel's own row order, so Alt+] always lands on the
                // row above the one highlighted, group nesting included.
                let rows: Vec<_> = view.doc.tree.visible_rows().iter().map(|(id, _)| *id).collect();
                let Some(at) = rows.iter().position(|r| *r == id) else { return };
                // Rows run top-first, the opposite of the stack, so a step
                // "up" the panel is a step back through this list.
                let next = (at as i64 - by as i64).clamp(0, rows.len() as i64 - 1) as usize;
                if next != at {
                    self.push(Action::SelectLayer(rows[next]));
                }
            }

            Action::ShowModifyDialog(kind) => {
                self.dialog =
                    Dialog::Modify(crate::dialogs::ModifyDialog::new(kind));
            }

            Action::MoveLayer(id, pos) => {
                if let Some(view) = self.doc_mut() {
                    let dirty =
                        view.history.apply(&mut view.doc, Box::new(MoveLayer::new(id, pos)));
                    view.mark_dirty(dirty);
                    view.invalidate();
                }
            }

            Action::NudgeLayer(dx, dy) => {
                if let Some(view) = self.doc_mut() {
                    let Some(id) = view.doc.active else { return };
                    let movable =
                        view.doc.tree.get(id).map(|l| !l.locks.blocks_move()).unwrap_or(false);
                    if movable {
                        let dirty = view
                            .history
                            .apply(&mut view.doc, Box::new(OffsetLayer::new(id, (dx, dy))));
                        view.mark_dirty(dirty);
                        view.invalidate();
                    }
                }
            }

            Action::FillSwatch { background, preserve_transparency } => {
                let color = if background { self.background } else { self.foreground };
                self.fill_active(color, cshop_core::blend::BlendMode::Normal, 1.0, preserve_transparency)
            }
            // A brush ladder: fine steps while the brush is small,
            // coarser ones once it is large, so one key press is always a
            // visible change without taking forever to cross the range.
            Action::StepBrushSize(dir) => {
                // Step along a ladder rather than by a fixed amount: fine at
                // the small end where a pixel matters, coarse at the large end
                // where it does not. Stepping down always undoes stepping up.
                let at = BRUSH_SIZES
                    .iter()
                    .position(|s| *s >= self.brush.size - 0.01)
                    .unwrap_or(BRUSH_SIZES.len() - 1);
                let next = (at as i32 + dir).clamp(0, BRUSH_SIZES.len() as i32 - 1) as usize;
                self.brush.size = BRUSH_SIZES[next];
            }
            Action::StepBrushHardness(dir) => {
                self.brush.hardness = (self.brush.hardness + dir as f32 * 0.25).clamp(0.0, 1.0);
            }
            Action::SetBrushOpacity(v) => self.brush.opacity = v.clamp(0.0, 1.0),

            Action::BeginText { at, wrap } => self.begin_text(at, wrap),
            Action::EditTextLayer(id) => self.edit_text_layer(id),
            Action::TextInput(input) => self.apply_text_input(input),
            Action::TextCaretAt(at) => self.text_caret_at(at),
            Action::CommitText => self.commit_text(),
            Action::CancelText => self.cancel_text(),
            Action::RasterizeLayer => self.rasterize_layer(),
            Action::ConvertToSmartObject => self.convert_to_smart_object(),
            Action::ShowLayerStyle => {
                let Some(view) = self.doc() else { return };
                let Some(id) = view.doc.active else { return };
                let Some(layer) = view.doc.tree.get(id) else { return };
                if layer.pixels().is_none() {
                    self.notify("Effects need a layer with pixels");
                    return;
                }
                let mut fx = layer.effects;
                // Opening the dialog on a layer with nothing set should still
                // give something to switch on.
                if fx.global_light_angle == 0.0 && fx.global_light_altitude == 0.0 {
                    fx = cshop_core::effects::LayerEffects {
                        enabled: true,
                        ..cshop_core::effects::LayerEffects::new()
                    };
                }
                fx.enabled = true;
                self.dialog = Dialog::LayerStyle(Box::new(
                    crate::layer_style::LayerStyleDialog::new(id, fx, layer.name.clone()),
                ));
            }
            Action::SetLayerEffects(id, fx) => {
                let Some(view) = self.doc_mut() else { return };
                let dirty = view.history.apply(
                    &mut view.doc,
                    Box::new(cshop_core::history::SetLayerEffects::new(id, *fx)),
                );
                view.mark_dirty(dirty);
                view.invalidate();
            }
            Action::ClearLayerEffects(id) => {
                let Some(view) = self.doc_mut() else { return };
                let dirty = view.history.apply(
                    &mut view.doc,
                    Box::new(cshop_core::history::SetLayerEffects::new(
                        id,
                        cshop_core::effects::LayerEffects::default(),
                    )),
                );
                view.mark_dirty(dirty);
                view.invalidate();
            }
            Action::DrawShape { from, to, from_centre, constrain } => {
                self.draw_shape(from, to, from_centre, constrain)
            }
            Action::FinishPath { closed } => self.finish_path(closed),
            Action::CancelPath => {
                self.pen = None;
            }
            Action::DeletePathAnchors => self.delete_path_anchors(),
            Action::ShowSegment => {
                self.segment_before = self.doc().and_then(|v| v.doc.selection.clone());
                self.dialog = Dialog::Segment(Box::new(crate::segment_ui::SegmentDialog::new()));
            }
            Action::SegmentDetect => self.segment_detect(),
            Action::SegmentPreview => self.segment_preview(),
            Action::SegmentCancel => {
                let previous = self.segment_before.clone();
                if let Some(view) = self.doc_mut() {
                    view.doc.selection = previous;
                    view.invalidate();
                }
            }
            Action::CombineShapes(op) => self.combine_shapes(op),

            Action::Copy => self.copy(false, false),
            Action::CopyMerged => self.copy(true, false),
            Action::Cut => self.copy(false, true),
            Action::Paste => self.paste(false),
            Action::PasteInPlace => self.paste(true),

            Action::ShowFillDialog => {
                self.dialog = Dialog::Fill(crate::dialogs::FillDialog::new(
                    self.foreground,
                    self.background,
                ));
            }
            Action::FillWith { color, mode, opacity, preserve_transparency } => {
                self.fill_active(color, mode, opacity, preserve_transparency)
            }
            Action::ShowColorPicker(target) => {
                let current = match target {
                    crate::dialogs::PickerTarget::Foreground => self.foreground,
                    crate::dialogs::PickerTarget::Background => self.background,
                };
                self.dialog = Dialog::ColorPicker(crate::dialogs::ColorPickerDialog::new(
                    target, current,
                ));
            }
            Action::SetColor { target, color } => match target {
                crate::dialogs::PickerTarget::Foreground => self.foreground = color,
                crate::dialogs::PickerTarget::Background => self.background = color,
            },
            Action::ClearLayer => self.fill_active(
                Rgba8::TRANSPARENT,
                cshop_core::blend::BlendMode::Normal,
                1.0,
                false,
            ),

            // --- selections ---
            Action::SelectAll => {
                let Some(view) = self.doc_mut() else { return };
                let all = Selection::all(view.doc.width, view.doc.height);
                let dirty = view
                    .history
                    .apply(&mut view.doc, Box::new(SetSelection::new(Some(&all), "Select All")));
                view.mark_dirty(dirty);
            }

            Action::Deselect => {
                let Some(view) = self.doc_mut() else { return };
                if view.doc.has_selection() {
                    let dirty =
                        view.history.apply(&mut view.doc, Box::new(SetSelection::deselect()));
                    view.mark_dirty(dirty);
                }
            }

            Action::Reselect => {
                let Some(view) = self.doc_mut() else { return };
                let Some(previous) = view.doc.last_selection.clone() else {
                    self.notify("There is no selection to restore");
                    return;
                };
                let dirty = view.history.apply(
                    &mut view.doc,
                    Box::new(SetSelection::new(Some(&previous), "Reselect")),
                );
                view.mark_dirty(dirty);
            }

            Action::InverseSelection => {
                let Some(view) = self.doc_mut() else { return };
                let (w, h) = (view.doc.width, view.doc.height);
                // Starting from an empty selection so that inverting with
                // nothing selected yields everything, which is more useful
                // than a silent no-op.
                let mut next = match &view.doc.selection {
                    Some(s) => s.clone(),
                    None => Selection::empty(w, h),
                };
                next.invert();
                let value = if next.is_empty() { None } else { Some(&next) };
                let dirty = view
                    .history
                    .apply(&mut view.doc, Box::new(SetSelection::new(value, "Inverse")));
                view.mark_dirty(dirty);
            }

            Action::SetSelection(selection, label) => {
                self.commit_selection(*selection, self.selection_mode, label)
            }

            Action::ModifySelection(op) => self.modify_selection(op),
            Action::GrowSelection => self.grow_or_similar(false),
            Action::SimilarSelection => self.grow_or_similar(true),

            Action::ToggleQuickMask => {
                self.quick_mask = !self.quick_mask;
                if self.quick_mask {
                    // Entering Quick Mask with nothing selected starts from a
                    // fully selected canvas, so painting removes from it.
                    if let Some(view) = self.doc_mut() {
                        if !view.doc.has_selection() {
                            let (w, h) = (view.doc.width, view.doc.height);
                            view.doc.selection = Some(Selection::all(w, h));
                        }
                    }
                }
                let state = if self.quick_mask { "on" } else { "off" };
                self.notify(format!("Quick Mask {state}"));
            }

            Action::SaveSelectionAsChannel => {
                let Some(view) = self.doc_mut() else { return };
                let Some(selection) = &view.doc.selection else {
                    self.notify("Nothing is selected");
                    return;
                };
                let data = selection.to_mask();
                let i = view.doc.add_channel(data);
                let name = view.doc.channels[i].name.clone();
                self.notify(format!("Saved selection as {name}"));
            }

            Action::LoadChannelAsSelection(i) => {
                let Some(view) = self.doc_mut() else { return };
                let Some(channel) = view.doc.channels.get(i) else { return };
                let loaded = Selection::from_mask(channel.data.clone());
                let name = channel.name.clone();
                let value = if loaded.is_empty() { None } else { Some(&loaded) };
                let dirty = view
                    .history
                    .apply(&mut view.doc, Box::new(SetSelection::new(value, "Load Selection")));
                view.mark_dirty(dirty);
                self.notify(format!("Loaded {name}"));
            }

            Action::DeleteChannel(i) => {
                if let Some(view) = self.doc_mut() {
                    if i < view.doc.channels.len() {
                        view.doc.channels.remove(i);
                    }
                }
            }

            Action::ToggleChannelVisible(i) => {
                if let Some(view) = self.doc_mut() {
                    if let Some(c) = view.doc.channels.get_mut(i) {
                        c.visible = !c.visible;
                    }
                }
            }

            // --- adjustments ---
            Action::AddAdjustmentLayer(adjustment) => self.add_adjustment_layer(*adjustment),
            Action::ShowAdjustmentDialog(adjustment) => {
                self.show_adjustment_dialog(*adjustment)
            }
            Action::ApplyAdjustment(adjustment) => self.apply_adjustment(*adjustment),
            Action::SetAdjustment(adjustment) => {
                let Some(view) = self.doc_mut() else { return };
                let Some(id) = view.doc.active else { return };
                let dirty = view
                    .history
                    .apply(&mut view.doc, Box::new(SetAdjustment::new(id, *adjustment)));
                view.mark_dirty(dirty);
            }

            // --- transform ---
            Action::BeginTransform => self.begin_transform(),
            Action::CommitTransform => self.commit_transform(),
            Action::CancelTransform => {
                if let Some(t) = self.transform.take() {
                    // The layer was hidden while its preview stood in for it.
                    if let Some(view) = self.doc_mut() {
                        if let Some(layer) = view.doc.tree.get_mut(t.layer) {
                            layer.visible = true;
                        }
                        view.invalidate();
                    }
                }
            }
            Action::TransformPreset(preset) => self.transform_preset(preset),

            Action::CommitCrop => self.commit_crop(),
            Action::CancelCrop => {
                self.crop = None;
            }
            Action::CropToSelection => {
                let rect = self
                    .doc()
                    .and_then(|d| d.doc.selection.as_ref().map(|s| s.bounds()));
                match rect {
                    Some(rect) if !rect.is_empty() => {
                        self.crop = Some(ActiveCrop::new(rect));
                        self.push(Action::CommitCrop);
                    }
                    _ => self.notify("Nothing is selected"),
                }
            }

            Action::ShowImageSize => {
                if let Some(view) = self.doc() {
                    self.dialog = Dialog::ImageSize(crate::dialogs::SizeDialog::image(
                        view.doc.width,
                        view.doc.height,
                    ));
                }
            }
            Action::ShowCanvasSize => {
                if let Some(view) = self.doc() {
                    self.dialog = Dialog::ImageSize(crate::dialogs::SizeDialog::canvas(
                        view.doc.width,
                        view.doc.height,
                    ));
                }
            }
            Action::FillInSelection => self.start_inpaint(),

            Action::ShowRelight => {
                let picked = self.doc().and_then(|v| {
                    let id = v.doc.active?;
                    Some((id, v.doc.tree.get(id)?.pixels()?.clone()))
                });
                match picked {
                    Some((id, pixels)) => {
                        self.dialog = Dialog::Relight(Box::new(
                            crate::relight_ui::RelightDialog::new(id, pixels.clone()),
                        ));
                        if crate::vision::is_available() {
                            self.start_depth(pixels);
                        }
                    }
                    None => self.fail("Relighting needs a layer with pixels in it"),
                }
            }
            Action::RelightPreview => {
                let lit = match &mut self.dialog {
                    Dialog::Relight(d) => {
                        let layer = d.layer;
                        d.lit().map(|p| (layer, p))
                    }
                    _ => None,
                };
                if let Some((id, pixels)) = lit {
                    self.show_relight(id, &pixels);
                }
            }
            Action::RelightKeep => self.keep_relight(),
            Action::RelightCancel => {
                let restore = match &self.dialog {
                    Dialog::Relight(d) if d.showing => Some((d.layer, d.before.clone())),
                    _ => None,
                };
                if let Some((id, before)) = restore {
                    self.show_relight(id, &before);
                }
                self.dialog = Dialog::None;
            }

            Action::ShowSeparate => {
                let picked = self.doc().and_then(|v| {
                    let id = v.doc.active?;
                    Some((id, v.doc.tree.get(id)?.pixels()?.clone()))
                });
                match picked {
                    Some((id, pixels)) => {
                        self.dialog = Dialog::Separate(Box::new(
                            crate::separate_ui::SeparateDialog::new(id),
                        ));
                        if crate::vision::is_available() {
                            self.start_separate_look(pixels);
                        }
                    }
                    None => self.fail("Separating needs a layer with pixels in it"),
                }
            }
            Action::RunSeparate => self.run_separate(),

            Action::ToggleRulers => self.show_rulers = !self.show_rulers,
            Action::ToggleGuides => self.show_guides = !self.show_guides,
            Action::ToggleGrid => self.show_grid = !self.show_grid,
            Action::ToggleSnap => self.snap = !self.snap,
            Action::AddGuide { vertical, at } => {
                self.show_guides = true;
                if let Some(view) = self.doc_mut() {
                    view.doc.guides.push(cshop_core::guides::Guide { vertical, at });
                }
            }
            Action::ClearGuides => {
                if let Some(view) = self.doc_mut() {
                    let had = view.doc.guides.len();
                    view.doc.guides.clear();
                    if had > 0 {
                        self.notify(format!("Cleared {had} guide{}", if had == 1 { "" } else { "s" }));
                    }
                }
            }

            Action::ClearRecent => {
                self.settings.recent.clear();
                self.current_settings().save();
                self.notify("Cleared the recent files");
            }

            Action::ShowUpscale => {
                let counted = self.doc().map(|v| {
                    let n = v
                        .doc
                        .tree
                        .iter_all()
                        .into_iter()
                        .filter(|id| {
                            v.doc.tree.get(*id).is_some_and(|l| {
                                matches!(l.kind, cshop_core::layer::LayerKind::Raster(_))
                            })
                        })
                        .count();
                    ((v.doc.width, v.doc.height), n)
                });
                match counted {
                    Some((_, 0)) => self.fail("There are no pixels here to enlarge"),
                    Some((size, n)) => {
                        self.dialog = Dialog::Upscale(Box::new(
                            crate::upscale_ui::UpscaleDialog::new(size, n),
                        ));
                    }
                    None => self.fail("Open a picture first"),
                }
            }
            Action::RunUpscale => self.start_upscale(),

            Action::ShowDenoise => {
                let picked = self.doc().and_then(|v| {
                    let id = v.doc.active?;
                    let layer = v.doc.tree.get(id)?;
                    let pixels = layer.pixels()?;
                    let offset = layer.offset;
                    // A selection narrows the work, in the layer's own frame.
                    let region = match v.doc.selection.as_ref().map(|s| s.bounds()) {
                        Some(r) if !r.is_empty() => IRect::new(
                            r.x0 - offset.0,
                            r.y0 - offset.1,
                            r.x1 - offset.0,
                            r.y1 - offset.1,
                        )
                        .intersect(&pixels.bounds()),
                        _ => pixels.bounds(),
                    };
                    (!region.is_empty()).then(|| (id, region, pixels.copy_rect(region)))
                });
                match picked {
                    Some((id, region, before)) => {
                        self.dialog = Dialog::Denoise(Box::new(
                            crate::denoise_ui::DenoiseDialog::new(id, region, before),
                        ));
                    }
                    None => self.fail("Removing noise needs a layer with pixels in it"),
                }
            }
            Action::RunDenoise => self.start_denoise(),
            Action::DenoiseRestrength => {
                let blended = match &self.dialog {
                    Dialog::Denoise(d) => d.blended().map(|b| (d.layer, d.region, b)),
                    _ => None,
                };
                if let Some((id, region, pixels)) = blended {
                    self.show_denoise_patch(id, region, &pixels);
                }
            }
            Action::DenoiseKeep => self.keep_denoise(),
            Action::DenoiseCancel => {
                let restore = match &self.dialog {
                    Dialog::Denoise(d) if d.showing => {
                        Some((d.layer, d.region, d.before.clone()))
                    }
                    _ => None,
                };
                if let Some((id, region, before)) = restore {
                    self.show_denoise_patch(id, region, &before);
                }
                self.dialog = Dialog::None;
            }

            Action::ShowLens => {
                let source = self.doc().and_then(|v| {
                    let id = v.doc.active?;
                    Some((id, v.doc.tree.get(id)?.pixels()?.clone()))
                });
                match source {
                    Some((id, pixels)) => {
                        self.dialog = Dialog::Lens(Box::new(
                            crate::lens_ui::LensDialog::new(id, &pixels),
                        ));
                    }
                    None => self.fail("Lens correction needs a layer with pixels in it"),
                }
            }
            Action::ApplyLens => self.start_lens(),

            Action::ShowColorProfile => {
                if let Some(view) = self.doc() {
                    self.dialog = Dialog::ColorProfile(Box::new(
                        crate::profile_ui::ProfileDialog::new(&view.doc.profile),
                    ));
                }
            }
            Action::SetColorProfile { path, convert } => {
                let to = match path {
                    None => Ok(cshop_core::profile::Profile::srgb()),
                    Some(p) => cshop_core::profile::Profile::load(&p)
                        .map_err(|e| format!("could not read that profile: {e}")),
                };
                match to {
                    Err(why) => self.fail(why),
                    Ok(to) => {
                        let name = to.name().to_string();
                        let edit: Box<dyn cshop_core::history::Command> = if convert {
                            Box::new(cshop_core::history::SetProfile::convert(to))
                        } else {
                            Box::new(cshop_core::history::SetProfile::assign(to))
                        };
                        let Some(view) = self.doc_mut() else { return };
                        let dirty = view.history.apply(&mut view.doc, edit);
                        view.mark_dirty(dirty);
                        view.invalidate();
                        self.notify(if convert {
                            format!("Converted to {name}")
                        } else {
                            format!("Assigned {name}; the pixels are unchanged")
                        });
                    }
                }
            }
            Action::SetDepth(bits) => {
                let Some(view) = self.doc_mut() else { return };
                if view.doc.depth() == bits {
                    let already = format!("Already at {bits} bits a channel");
                    self.notify(already);
                    return;
                }
                let edit = Box::new(cshop_core::history::SetDepth::new(bits));
                let dirty = view.history.apply(&mut view.doc, edit);
                view.mark_dirty(dirty);
                view.invalidate();
                self.notify(if bits == 16 {
                    "Now 16 bits a channel; nothing was lost widening".into()
                } else {
                    "Now 8 bits a channel".to_string()
                });
            }
            Action::ResizeImage { width, height, filter } => {
                let gpu = self.gpu.clone();
                let Some(view) = self.doc_mut() else { return };
                let dirty = view
                    .history
                    .apply(&mut view.doc, Box::new(ResizeImage::new(width, height, filter)));
                view.mark_dirty(dirty);
                view.resize_targets(&gpu);
                view.invalidate();
                view.zoom_initialised = false;
            }
            Action::ResizeCanvas { width, height, anchor } => {
                let gpu = self.gpu.clone();
                let Some(view) = self.doc_mut() else { return };
                let shift = anchor.shift((view.doc.width, view.doc.height), (width, height));
                let dirty =
                    view.history.apply(&mut view.doc, Box::new(ResizeCanvas::new(width, height, shift)));
                view.mark_dirty(dirty);
                view.resize_targets(&gpu);
                view.invalidate();
                view.zoom_initialised = false;
            }

            Action::RenameLayer(id) => {
                let name = self
                    .doc()
                    .and_then(|d| d.doc.tree.get(id).map(|l| l.name.clone()));
                if let Some(name) = name {
                    self.dialog = Dialog::Rename(crate::dialogs::RenameDialog::new(id, name));
                }
            }

            // --- filters ---
            Action::ShowFilterDialog(filter) => self.show_filter_dialog(*filter),
            Action::ApplyFilter(filter) => self.apply_filter(*filter),
            Action::AttachFilter(filter) => self.edit_filters("Smart Filter", |f| {
                f.enabled = true;
                f.slots.push(cshop_core::smart_filters::FilterSlot::new(*filter));
            }),
            Action::RemoveAttachedFilter(i) => {
                self.edit_filters("Remove Smart Filter", |f| {
                    if i < f.slots.len() {
                        f.slots.remove(i);
                    }
                })
            }
            Action::ToggleAttachedFilter(i) => self.edit_filters("Smart Filter", |f| {
                if let Some(slot) = f.slots.get_mut(i) {
                    slot.enabled = !slot.enabled;
                }
            }),
            Action::ToggleAttachedFilters => {
                self.edit_filters("Smart Filters", |f| f.enabled = !f.enabled)
            }
            Action::EditAttachedFilter(i) => self.edit_attached_filter(i),
            Action::SetAttachedFilterOpacity(i, k) => {
                self.edit_filters("Smart Filter Opacity", |f| {
                    if let Some(slot) = f.slots.get_mut(i) {
                        slot.opacity = k.clamp(0.0, 1.0);
                    }
                })
            }
            Action::ReplaceAttachedFilter(i, filter) => {
                self.edit_filters("Smart Filter", |f| {
                    if let Some(slot) = f.slots.get_mut(i) {
                        slot.filter = *filter;
                    }
                })
            }
            Action::ApplyAttachedFilters => self.apply_attached_filters(),
            Action::RepeatLastFilter => match self.last_filter.clone() {
                Some(filter) => self.apply_filter(filter),
                None => self.notify("No filter has been used yet"),
            },

            // --- masks ---
            Action::AddLayerMask { hide_all } => self.add_layer_mask(false, false, hide_all),
            Action::AddLayerMaskFromSelection { invert } => {
                self.add_layer_mask(true, invert, false)
            }
            Action::AddLayerMaskFromDepth { invert } => self.start_depth_mask(invert),
            Action::AddVectorMask { invert } => self.add_vector_mask(invert),

            Action::ShowColorRange => self.show_color_range(),
            Action::ApplyColorRange(range) => self.apply_color_range(*range),
            Action::ShowRefineEdge => self.show_refine_edge(),
            Action::ApplyRefineEdge(settings) => self.apply_refine_edge(*settings),
            Action::SelectionFromPath => self.selection_from_path(),
            Action::PathFromSelection => self.path_from_selection(),

            Action::SaveLayerState(name) => self.edit_states("Save Layer State", |doc, s| {
                let name = if name.trim().is_empty() {
                    format!("State {}", s.len() + 1)
                } else {
                    name
                };
                s.push(cshop_core::states::LayerState::capture(&doc.tree, name));
                None
            }),
            Action::ApplyLayerState(i) => {
                self.edit_states("Layer State", |_, s| (i < s.len()).then_some(i))
            }
            Action::UpdateLayerState(i) => self.edit_states("Update Layer State", |doc, s| {
                if let Some(slot) = s.get_mut(i) {
                    let name = slot.name.clone();
                    *slot = cshop_core::states::LayerState::capture(&doc.tree, name);
                }
                None
            }),
            Action::DeleteLayerState(i) => self.edit_states("Delete Layer State", |_, s| {
                if i < s.len() {
                    s.remove(i);
                }
                None
            }),
            Action::LayerToMask => self.layer_to_mask(),
            Action::SelectionFromMask => self.selection_from_mask(),
            Action::DeleteLayerMask => self.remove_layer_mask(false),
            Action::ApplyLayerMask => self.remove_layer_mask(true),

            Action::ToggleMaskEnabled => {
                let Some(view) = self.doc_mut() else { return };
                let Some(id) = view.doc.active else { return };
                if let Some(mask) = view.doc.tree.get_mut(id).and_then(|l| l.mask.as_mut()) {
                    mask.enabled = !mask.enabled;
                    view.mark_dirty(Dirty::pixels(id, IRect::from_size(view.doc.width, view.doc.height)));
                    view.invalidate();
                }
            }

            Action::SetEditTarget(target) => {
                if let Some(view) = self.doc_mut() {
                    view.doc.edit_target = target;
                }
            }

            Action::CancelDrag => {
                if self.drag.take().is_some() {
                    self.notify("Selection cancelled");
                }
            }
            Action::CloseDrag => self.finish_selection_drag(),

            Action::CloseDialog => self.dialog = Dialog::None,
            Action::Quit => self.quit = true,
        }
    }

    // -----------------------------------------------------------------------
    // Operations
    // -----------------------------------------------------------------------

    fn add_document(&mut self, doc: Document, origin: &str) {
        let view = DocView::new(&self.gpu, doc, origin);
        self.docs.push(view);
        self.active = Some(self.docs.len() - 1);
    }

    /// Open a document that was built elsewhere, and make it active.
    ///
    /// Used by tests and by the demo screenshot, which need to assemble a layer
    /// stack without going through a file.
    pub fn open_document(&mut self, doc: Document) {
        self.add_document(doc, "New");
    }

    /// Run one action immediately rather than queueing it, for tests and
    /// scripted setup where there is no frame to drain the queue.
    ///
    /// Follow-up actions are drained too — Save falls through to Save As, Crop
    /// to Selection to Commit Crop — so this behaves as a frame would.
    /// Composite document `index` and read it back.
    ///
    /// For callers with no window: the same path saving uses, exposed so a
    /// script can render without an interface around it.
    pub fn render_composite(
        &mut self,
        gpu: &cshop_gpu::context::GpuContext,
        index: usize,
    ) -> cshop_core::pixels::PixelBuffer {
        let view = &mut self.docs[index];
        view.sync_composite_only(gpu, &mut self.compositor);
        view.read_composite(gpu)
    }

    /// The same, at sixteen bits a channel — see
    /// [`crate::doc_view::DocView::read_composite_deep`].
    pub fn render_composite_deep(
        &mut self,
        gpu: &cshop_gpu::context::GpuContext,
        index: usize,
    ) -> cshop_core::pixels::DeepBuffer {
        // Skip the sync as well as the composite when there is nothing to
        // composite; `read_composite_deep` explains why.
        if self.docs[index].doc.single_deep_layer().is_some() {
            return self.docs[index].read_composite_deep(gpu);
        }
        let view = &mut self.docs[index];
        view.sync_composite_only(gpu, &mut self.compositor);
        view.read_composite_deep(gpu)
    }

    pub fn dispatch(&mut self, action: Action) {
        self.run(action);
        self.run_actions();
    }

    fn open_path(&mut self, path: PathBuf) {
        // A project or a PSD arrives with its layers; anything else becomes a
        // single background layer, which `load_document` also handles.
        match cshop_io::load_document(&path) {
            Ok(doc) => {
                let (name, w, h) = (doc.name.clone(), doc.width, doc.height);
                let layers = doc.tree.len();
                self.settings.remember(&path);
                self.add_document(doc, "Open");
                if layers > 1 {
                    self.notify(format!("Opened {name} ({w} x {h}, {layers} layers)"));
                } else {
                    self.notify(format!("Opened {name} ({w} x {h})"));
                }
            }
            Err(e) => self.fail(format!("Could not open {}: {e}", path.display())),
        }
    }

    fn save_to(&mut self, path: PathBuf, deep: bool) {
        let Some(i) = self.active else { return };

        // Saving a flat format means exporting the composited result, so read
        // it back off the GPU rather than re-doing the work on the CPU.
        let gpu = self.gpu.clone();
        let pixels = {
            let view = &mut self.docs[i];
            view.sync_composite_only(&gpu, &mut self.compositor);
            view.read_composite(&gpu)
        };

        // A layered format saves the document itself; a flat one gets the
        // composite. `save_document` decides from the extension.
        let result = if deep {
            let format = cshop_io::ImageFormat::from_path(&path);
            match format {
                Some(f) => {
                    let view = &mut self.docs[i];
                    let profile = view.doc.profile.clone();
                    let deep_pixels = view.read_composite_deep(&gpu);
                    cshop_io::encode_deep(&deep_pixels, f, &profile, &profile)
                        .and_then(|bytes| Ok(std::fs::write(&path, bytes)?))
                }
                None => Err(cshop_io::IoError::Unsupported(path.display().to_string())),
            }
        } else {
            let view = &self.docs[i];
            cshop_io::save_document(&path, &view.doc, &pixels)
        };
        match result {
            Ok(()) => {
                let layered = cshop_io::ImageFormat::from_path(&path)
                    .is_some_and(|f| f.is_layered());
                let view = &mut self.docs[i];
                let layers = view.doc.tree.len();
                view.doc.modified = false;
                view.doc.path = Some(path.clone());
                if let Some(name) = path.file_name() {
                    view.doc.name = name.to_string_lossy().to_string();
                }
                let shown = view.doc.name.clone();
                // A flat format keeps the composite and nothing else. Saying
                // so is the difference between an export and quietly losing
                // the stack the next time this file is opened.
                if !layered && layers > 1 {
                    self.notify(format!(
                        "Saved {shown} — flattened. Save as .cshop or .psd to keep the {layers} layers."
                    ));
                } else {
                    self.notify(format!("Saved {shown}"));
                }
            }
            Err(e) => self.fail(format!("Could not save {}: {e}", path.display())),
        }
    }

    fn add_layer(&mut self, group: bool) {
        let Some(view) = self.doc_mut() else { return };
        let id = view.doc.tree.alloc_id();
        let (w, h) = (view.doc.width, view.doc.height);

        let layer = if group {
            Layer::group(id, format!("Group {}", view.doc.tree.len()))
        } else {
            Layer::raster(id, view.doc.next_layer_name(), PixelBuffer::new(w, h))
        };

        // New layers land directly above the active one, inside the same group.
        let pos = view
            .doc
            .active
            .and_then(|a| view.doc.tree.position(a))
            .map(|p| LayerPos { parent: p.parent, index: p.index + 1 })
            .unwrap_or(LayerPos { parent: None, index: view.doc.tree.root().len() });

        let label = if group { "New Group" } else { "New Layer" };
        let dirty = view.history.apply(&mut view.doc, Box::new(AddLayer::new(layer, pos, label)));
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// Layer via Copy.
    ///
    /// With a selection this lifts only the selected pixels onto a new layer,
    /// cropped to the selection's bounds and faded by its coverage so a
    /// feathered edge carries over. With no selection it is a plain duplicate,
    /// which is what Ctrl+J does there too.
    fn layer_via_copy(&mut self) {
        let Some(view) = self.doc_mut() else { return };
        let Some(active) = view.doc.active else { return };
        let Some(selection) = view.doc.selection.clone() else {
            self.duplicate_layer();
            return;
        };
        let Some(layer) = view.doc.tree.get(active) else { return };
        if layer.kind.is_group() {
            self.notify("Select a raster layer to copy from");
            return;
        }
        let Some(pixels) = layer.pixels() else {
            let why = self.why_not("Only raster layers can be copied from");
            self.notify(why);
            return;
        };
        let offset = layer.offset;

        // Work in document space, then crop to where the selection and the
        // layer actually overlap.
        let rect = selection.bounds().intersect(&pixels.bounds().translate(offset.0, offset.1));
        if rect.is_empty() {
            self.notify("The selection does not overlap this layer");
            return;
        }

        let mut lifted = PixelBuffer::new(rect.width(), rect.height());
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let cov = selection.coverage(x, y);
                if cov == 0 {
                    continue;
                }
                let mut px = pixels.get(x - offset.0, y - offset.1);
                // Scale alpha by coverage, so a feathered selection produces a
                // feathered layer rather than a hard cut-out.
                px.a = ((px.a as u32 * cov as u32) / 255) as u8;
                lifted.set(x - rect.x0, y - rect.y0, px);
            }
        }

        let id = view.doc.tree.alloc_id();
        let mut copy = Layer::raster(id, view.doc.next_layer_name(), lifted);
        copy.offset = (rect.x0, rect.y0);
        let pos = view
            .doc
            .tree
            .position(active)
            .map(|p| LayerPos { parent: p.parent, index: p.index + 1 })
            .unwrap_or(LayerPos { parent: None, index: 0 });

        // Adding the layer and dropping the selection are one gesture, so
        // they undo together — the selection is dropped once the pixels
        // are on their own layer.
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::Compound::new(
                "Layer via Copy",
                vec![
                    Box::new(AddLayer::new(copy, pos, "Layer via Copy")),
                    Box::new(cshop_core::history::SetSelection::deselect()),
                ],
            )),
        );
        view.mark_dirty(dirty);
        view.invalidate();
    }

    fn duplicate_layer(&mut self) {
        let Some(view) = self.doc_mut() else { return };
        let Some(active) = view.doc.active else { return };
        // Clone first so the tree is free to hand out a new id.
        let Some(mut copy) = view.doc.tree.get(active).cloned() else { return };

        // Groups need a recursive copy, which the tree does not offer yet.
        if copy.kind.is_group() {
            self.notify("Duplicating groups arrives with the layer-effects phase");
            return;
        }

        let id = view.doc.tree.alloc_id();
        copy.name = format!("{} copy", copy.name);
        copy.id = id;
        copy.is_background = false;
        copy.locks = Default::default();

        let pos = view
            .doc
            .tree
            .position(active)
            .map(|p| LayerPos { parent: p.parent, index: p.index + 1 })
            .unwrap_or(LayerPos { parent: None, index: 0 });

        let dirty =
            view.history.apply(&mut view.doc, Box::new(AddLayer::new(copy, pos, "Duplicate Layer")));
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// Merge the active layer into the one directly below it.
    fn merge_down(&mut self) {
        let gpu = self.gpu.clone();
        let Some(i) = self.active else { return };

        let (active, below) = {
            let view = &self.docs[i];
            let Some(active) = view.doc.active else { return };
            let Some(pos) = view.doc.tree.position(active) else { return };
            if pos.index == 0 {
                return;
            }
            (active, view.doc.tree.children(pos.parent)[pos.index - 1])
        };

        // Compose just these two layers by hiding everything else, reading the
        // result back, and writing it into the lower layer. Slower than a
        // dedicated path, but it reuses the compositor and so honours blend
        // modes, masks and opacity exactly.
        let merged = {
            let view = &mut self.docs[i];
            let mut scratch = view.doc.clone();
            for id in scratch.tree.iter_all() {
                let keep = id == active || id == below;
                if let Some(l) = scratch.tree.get_mut(id) {
                    if !keep {
                        l.visible = false;
                    }
                }
            }
            crate::render_document(&gpu, &mut self.compositor, &scratch)
        };

        let view = &mut self.docs[i];
        let rect = IRect::from_size(view.doc.width, view.doc.height);

        // Replace the lower layer's pixels, then delete the upper one. Two
        // history entries would be wrong, so the delete is applied first and
        // both share a single label via the pixel command.
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(ReplacePixels::new(below, rect, merged, "Merge Down")),
        );
        view.mark_dirty(dirty);
        let dirty = view.history.apply(&mut view.doc, Box::new(DeleteLayer::new(active)));
        view.mark_dirty(dirty);
        view.doc.select(Some(below));
        view.invalidate();
    }

    fn flatten(&mut self) {
        let gpu = self.gpu.clone();
        let Some(i) = self.active else { return };

        let flat = {
            let view = &self.docs[i];
            crate::render_document(&gpu, &mut self.compositor, &view.doc)
        };

        let view = &mut self.docs[i];
        let ids = view.doc.tree.iter_all();
        // Everything except the bottom layer goes, then the bottom layer takes
        // the composited pixels.
        let Some(&bottom) = view.doc.tree.root().first() else { return };
        for id in ids.into_iter().rev() {
            if id == bottom {
                continue;
            }
            if view.doc.tree.get(id).is_none() {
                continue;
            }
            let dirty = view.history.apply(&mut view.doc, Box::new(DeleteLayer::new(id)));
            view.mark_dirty(dirty);
        }
        let rect = IRect::from_size(view.doc.width, view.doc.height);
        let dirty = view
            .history
            .apply(&mut view.doc, Box::new(ReplacePixels::new(bottom, rect, flat, "Flatten Image")));
        view.mark_dirty(dirty);
        if let Some(l) = view.doc.tree.get_mut(bottom) {
            l.name = "Background".into();
            l.offset = (0, 0);
        }
        view.doc.select(Some(bottom));
        view.invalidate();
    }

    // -----------------------------------------------------------------------
    // Clipboard
    // -----------------------------------------------------------------------

    /// The region a copy would take: the selection where there is one, and
    /// otherwise everything the source covers.
    fn copy_region(&self, source: IRect) -> IRect {
        match self.doc().and_then(|v| v.doc.selection.as_ref()) {
            Some(s) => source.intersect(&s.bounds()),
            None => source,
        }
    }

    /// Copy to the clipboard, optionally from the whole composite, and
    /// optionally clearing what was taken.
    fn copy(&mut self, merged: bool, cut: bool) {
        let Some(i) = self.active else { return };

        let (pixels, origin, rect) = if merged {
            // The composite lives on the GPU; read it back rather than
            // recomputing the stack here.
            let gpu = self.gpu.clone();
            let view = &mut self.docs[i];
            view.sync_composite_only(&gpu, &mut self.compositor);
            let flat = view.read_composite(&gpu);
            let rect = self.copy_region(flat.bounds());
            (flat, (0, 0), rect)
        } else {
            let Some(view) = self.doc() else { return };
            let Some(id) = view.doc.active else { return };
            let Some(layer) = view.doc.tree.get(id) else { return };
            let Some(px) = layer.pixels() else {
                self.notify("That layer has no pixels to copy");
                return;
            };
            let rect = self.copy_region(layer.bounds());
            (px.clone(), layer.offset, rect)
        };

        if rect.is_empty() {
            self.notify("The selection does not overlap anything to copy");
            return;
        }

        let doc_ref = &self.docs[i].doc;
        let taken = crate::clipboard::extract(&pixels, origin, rect, |x, y| {
            doc_ref.selection_coverage(x, y)
        });
        self.clipboard.set(taken, (rect.x0, rect.y0));

        if cut {
            // Clearing is exactly what Delete does, selection coverage and
            // locks included, so it goes through the same path.
            self.fill_active(Rgba8::TRANSPARENT, cshop_core::blend::BlendMode::Normal, 1.0, false);
            self.notify("Cut");
        } else {
            self.notify(if merged { "Copied merged" } else { "Copied" });
        }
    }

    /// Paste onto a new layer above the active one.
    fn paste(&mut self, in_place: bool) {
        let Some(clip) = self.clipboard.get() else {
            self.notify("There is nothing to paste");
            return;
        };
        if self.doc().is_none() {
            // Nothing open: the pasted image becomes a document of its own.
            let (w, h) = (clip.pixels.width(), clip.pixels.height());
            let mut doc = Document::new("Untitled", w, h, cshop_core::document::Background::Transparent);
            doc.tree = Default::default();
            let id = doc.tree.alloc_id();
            doc.tree.push(Layer::raster(id, "Layer 1", clip.pixels), None);
            doc.active = doc.tree.root().last().copied();
            self.add_document(doc, "Paste");
            return;
        }

        let Some(view) = self.doc_mut() else { return };
        let (pw, ph) = (clip.pixels.width() as i32, clip.pixels.height() as i32);
        let offset = if in_place {
            clip.origin
        } else {
            // Centred on the canvas, which is where the eye is looking when
            // nothing says otherwise.
            (
                (view.doc.width as i32 - pw) / 2,
                (view.doc.height as i32 - ph) / 2,
            )
        };

        let id = view.doc.tree.alloc_id();
        let name = view.doc.next_layer_name();
        let mut layer = Layer::raster(id, name, clip.pixels);
        layer.offset = offset;
        let pos = new_layer_pos(view);
        let dirty = view.history.apply(&mut view.doc, Box::new(AddLayer::new(layer, pos, "Paste")));
        view.mark_dirty(dirty);
        view.invalidate();
        self.notify(if in_place { "Pasted in place" } else { "Pasted" });
    }

    /// Fill or clear, restricted to the selection when there is one.
    fn fill_active(
        &mut self,
        color: Rgba8,
        mode: cshop_core::blend::BlendMode,
        opacity: f32,
        preserve_transparency: bool,
    ) {
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        if layer.locks.blocks_pixels() {
            self.fail("The layer is locked");
            return;
        }
        let offset = layer.offset;
        let Some(px) = layer.pixels() else { return };

        // Only the part of the layer inside the selection needs rewriting.
        let layer_rect = IRect::at(offset.0, offset.1, px.width(), px.height());
        let rect = match &view.doc.selection {
            Some(s) => layer_rect.intersect(&s.bounds()),
            None => layer_rect,
        };
        if rect.is_empty() {
            self.notify("The selection does not overlap this layer");
            return;
        }

        let mut patch = px.copy_rect(rect.translate(-offset.0, -offset.1));
        let clear = color.a == 0;
        // A locked-transparency layer behaves as though the fill asked to
        // preserve it, whatever the dialog said.
        let preserve = preserve_transparency
            || view.doc.tree.get(id).is_some_and(|l| l.locks.transparency);

        for y in 0..patch.height() as i32 {
            for x in 0..patch.width() as i32 {
                let mut amount =
                    view.doc.selection_coverage(rect.x0 + x, rect.y0 + y) * opacity;
                if amount <= 0.0 {
                    continue;
                }
                let existing = patch.get(x, y);
                if preserve {
                    if existing.a == 0 {
                        continue;
                    }
                    amount *= existing.a as f32 / 255.0;
                }
                let out = if clear {
                    // Clearing removes alpha in proportion to coverage rather
                    // than punching a hard hole through a feathered edge.
                    let mut c = existing;
                    c.a = (c.a as f32 * (1.0 - amount)) as u8;
                    c
                } else {
                    let blended = cshop_core::blend::composite(
                        mode,
                        existing.to_f32(),
                        color.to_f32(),
                        amount,
                    );
                    if preserve {
                        cshop_core::color::Rgba {
                            a: existing.a as f32 / 255.0,
                            ..blended
                        }
                        .to_u8()
                    } else {
                        blended.to_u8()
                    }
                };
                patch.set(x, y, out);
            }
        }

        let label = if clear { "Clear" } else { "Fill" };
        let dirty =
            view.history.apply(&mut view.doc, Box::new(ReplacePixels::new(id, rect, patch, label)));
        view.mark_dirty(dirty);
    }

    // -----------------------------------------------------------------------
    // Painting
    // -----------------------------------------------------------------------

    /// True while a stroke is in progress.
    pub fn is_painting(&self) -> bool {
        self.stroke.is_some() || self.smudge.is_some()
    }

    /// Begin a smudge at a document-space position.
    ///
    /// `strength` is how far each dab drags what it picked up: at zero the
    /// brush puts back exactly what it took and nothing moves.
    pub fn begin_smudge(&mut self, at: Vec2, strength: f32) {
        let brush = self.brush;
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        if layer.locks.blocks_pixels() {
            self.fail("The layer is locked");
            return;
        }
        if view.doc.effective_edit_target() != EditTarget::Pixels {
            self.fail("The Smudge tool drags colour, and a mask has none");
            return;
        }
        let Some(px) = layer.pixels() else {
            let why = self.why_not("The Smudge tool works on raster layers");
            self.fail(why);
            return;
        };
        let snapshot = Snapshot::new(px.width(), px.height(), Rgba8::TRANSPARENT);
        let offset = layer.offset;
        let local = layer_local(at, offset);
        self.smudge = Some(ActiveSmudge {
            layer: id,
            smudge: cshop_core::paint::Smudge::new(brush, strength),
            snapshot,
            last: local,
        });
        self.continue_smudge(at);
    }

    /// Extend the smudge. Unlike a stroke this writes as it goes and is never
    /// recomputed, so the original has to be taken before each segment.
    fn continue_smudge(&mut self, at: Vec2) {
        let Some(active) = self.smudge.as_mut() else { return };
        let Some(index) = self.active else { return };
        let view = &mut self.docs[index];
        let Some(layer) = view.doc.tree.get(active.layer) else { return };
        let offset = layer.offset;
        let to = layer_local(at, offset);

        let reach = active.smudge.reach(active.last, to);
        let Document { tree, selection, .. } = &mut view.doc;
        let Some(pixels) = tree.get_mut(active.layer).and_then(|l| l.pixels_mut()) else { return };
        active.snapshot.capture(&*pixels, reach);
        let clip = selection.as_ref().map(|s| Clip { selection: s, offset });
        let touched = active.smudge.advance(pixels, to, clip.as_ref());
        active.last = to;
        if !touched.is_empty() {
            let layer = active.layer;
            view.mark_dirty(Dirty::pixels(layer, touched.translate(offset.0, offset.1)));
        }
    }

    /// Finish the smudge and record it as one undo step.
    fn end_smudge(&mut self) {
        let Some(active) = self.smudge.take() else { return };
        let Some(index) = self.active else { return };
        let view = &mut self.docs[index];
        if active.smudge.is_empty() {
            return;
        }
        let Some(layer) = view.doc.tree.get(active.layer) else { return };
        let offset = layer.offset;
        let Some(pixels) = layer.pixels() else { return };
        let rect = active.smudge.bounds().intersect(&pixels.bounds());
        if rect.is_empty() {
            return;
        }
        // The layer already shows the finished smudge. Take it, put the
        // original back, and let the command lay it down again so the entry's
        // own "before" is the truth.
        let after = pixels.copy_rect(rect);
        let before = active.snapshot.copy_rect(rect);
        if let Some(px) = view.doc.tree.get_mut(active.layer).and_then(|l| l.pixels_mut()) {
            px.paste(&before, rect.x0, rect.y0);
        }
        if after.pixels() == before.pixels() {
            return; // A smudge that moved nothing is not an undo step.
        }
        let doc_rect = rect.translate(offset.0, offset.1);
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(ReplacePixels::new(active.layer, doc_rect, after, "Smudge Tool")),
        );
        view.mark_dirty(dirty);
    }

    /// What the Blur or Sharpen brush should do, given the current brush.
    ///
    /// The reach comes from the brush size rather than a separate radius, so
    /// there is one thing to adjust and it is the one already under the
    /// pointer: a big brush blurs broadly, a small one softens a detail.
    pub fn brush_filter(&self) -> Option<cshop_core::paint::BrushFilter> {
        use cshop_core::paint::BrushFilter;
        let strength = self.brush_filter_strength.clamp(0.0, 1.0);
        match self.tool {
            Tool::Blur => {
                Some(BrushFilter::Blur { radius: (self.brush.size * 0.25 * strength).clamp(1.0, 50.0) })
            }
            Tool::Sharpen => Some(BrushFilter::Sharpen {
                radius: (self.brush.size * 0.10).clamp(1.0, 8.0),
                amount: strength * 2.0,
            }),
            _ => None,
        }
    }

    /// Where a stroke about to begin takes its colour from.
    ///
    /// The Clone Stamp and the Blur and Sharpen brushes are the ordinary brush
    /// with this one thing changed; everything else about them — size,
    /// hardness, opacity, flow, spacing, clipping, undo — is shared.
    pub fn begin_stroke_from(&mut self, at: Vec2, mode: PaintMode, from: StrokeFrom) {
        self.begin_stroke_inner(at, mode, from)
    }

    /// Begin a stroke at a document-space position.
    ///
    /// Where the paint lands depends on the mode: normally the active layer's
    /// pixels, its mask when the mask is the edit target, or the selection
    /// itself while Quick Mask is on.
    pub fn begin_stroke(&mut self, at: Vec2, mode: PaintMode) {
        self.begin_stroke_with(at, mode, false)
    }

    /// Begin a stroke, optionally as a Clone Stamp.
    pub fn begin_stroke_with(&mut self, at: Vec2, mode: PaintMode, clone: bool) {
        self.begin_stroke_inner(
            at,
            mode,
            if clone { StrokeFrom::Clone } else { StrokeFrom::Colour },
        )
    }

    fn begin_stroke_inner(&mut self, at: Vec2, mode: PaintMode, from: StrokeFrom) {
        let brush = self.brush;
        let sample_all = self.sample_all_layers;
        // On a mask, black conceals and white reveals, so the eraser is simply
        // the brush loaded with the background colour rather than a separate
        // operation.
        let quick_mask = self.quick_mask;
        let (foreground, background) = (self.foreground, self.background);

        // The Clone Stamp and the Healing Brush both copy from somewhere the
        // pointer is not, and both settle where from before the layer is
        // touched — partly because they share the rule, and partly because
        // reading it afterwards would mean reading the app while the layer it
        // owns is borrowed.
        let anchored = match from {
            StrokeFrom::Clone | StrokeFrom::Heal => {
                let Some(anchor) = self.clone_anchor else {
                    self.notify(match from {
                        StrokeFrom::Heal => "Alt-click to set the source to heal from first",
                        _ => "Alt-click to set the clone source first",
                    });
                    return;
                };
                // Non-aligned strokes each restart from the anchor; aligned
                // ones keep the offset the first stroke set.
                Some(match (self.clone_aligned, self.clone_offset) {
                    (true, Some(existing)) => existing,
                    _ => {
                        let o = (
                            (anchor.x - at.x).round() as i32,
                            (anchor.y - at.y).round() as i32,
                        );
                        self.clone_offset = Some(o);
                        o
                    }
                })
            }
            _ => None,
        };
        // Likewise the History Brush's marked state, read before the layer it
        // will paint over is borrowed.
        let from_history = match from {
            StrokeFrom::History => match self.doc().and_then(|v| v.history_source.clone()) {
                Some(found) => Some(found),
                None => {
                    self.notify("Mark a state in the History panel to paint back to first");
                    return;
                }
            },
            _ => None,
        };

        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let (doc_w, doc_h) = (view.doc.width, view.doc.height);

        // --- Quick Mask: the stroke edits the selection ---------------------
        if quick_mask {
            // Nothing is copied yet; the original is taken tile by tile as
            // the stroke reaches it.
            let snapshot = Snapshot::new(doc_w, doc_h, 0);
            // Painting black in Quick Mask protects (deselects); white selects.
            let colour = if mode == PaintMode::Erase { background } else { foreground };
            let mut stroke = Stroke::new(doc_w, doc_h, brush, PaintMode::Paint, colour);
            stroke.add_point(at);
            self.stroke =
                Some(ActiveStroke { layer: id, stroke, target: StrokeTarget::QuickMask(snapshot), tool: self.tool });
            self.continue_stroke(at);
            return;
        }

        let Some(layer) = view.doc.tree.get(id) else { return };
        if layer.locks.blocks_pixels() {
            self.fail("The layer is locked");
            return;
        }

        match view.doc.effective_edit_target() {
            // --- painting the layer's mask ---------------------------------
            EditTarget::Mask => {
                if matches!(mode, PaintMode::Retouch(_)) {
                    self.fail(
                        "Dodge, Burn and Sponge shape colour, and a mask has none. \
                         Paint the mask with the brush, or turn the mask off to \
                         retouch the layer.",
                    );
                    return;
                }
                let mask = layer.mask.as_ref().expect("effective_edit_target checked this");
                let (mw, mh) = (mask.data.width(), mask.data.height());
                let snapshot = Snapshot::new(mw, mh, 0);
                let offset = mask.offset;
                let colour = if mode == PaintMode::Erase { background } else { foreground };
                let mut stroke = Stroke::new(mw, mh, brush, PaintMode::Paint, colour);
                stroke.add_point(layer_local(at, offset));
                self.stroke =
                    Some(ActiveStroke { layer: id, stroke, target: StrokeTarget::Mask(snapshot), tool: self.tool });
            }
            // --- painting the layer's pixels -------------------------------
            EditTarget::Pixels => {
                // Type, shape and smart layers all *show* a raster, and
                // `pixels()` hands it over, but what it hands over is a
                // rendering of something else: painting on it would be thrown
                // away the next time that something was rendered. Without this
                // the stroke was accepted and then quietly did nothing, which
                // looks exactly like a broken brush.
                if layer.is_rendered() {
                    let what = match &layer.kind {
                        cshop_core::layer::LayerKind::Text(_) => "A type layer",
                        cshop_core::layer::LayerKind::Smart(_) => "A smart object",
                        _ => "A shape layer",
                    };
                    self.fail(format!(
                        "{what} shows a rendering of something else, so it cannot be \
                         painted on. Rasterise it first, from the Layer menu."
                    ));
                    return;
                }
                let Some(px) = layer.pixels() else {
                    let why = self.why_not("Only raster layers can be painted on");
                    self.fail(why);
                    return;
                };
                // Deliberately not `px.clone()`. On a large canvas that copied
                // the whole layer — 400 MB and about 150 ms on a 10000x10000
                // document — every time the mouse went down, for a stroke that
                // might cover a hundred pixels. See `cshop_core::snapshot`.
                let snapshot = Snapshot::new(px.width(), px.height(), Rgba8::TRANSPARENT);
                let layer_offset = layer.offset;

                // The Clone Stamp is the brush with its colour coming from
                // somewhere else; every other control behaves identically.
                let source = match from {
                    StrokeFrom::Colour => StrokeSource::Solid(foreground),
                    // Blur and sharpen read the layer as it was when the
                    // stroke began, so a stroke cannot chase its own output.
                    StrokeFrom::Filter(filter) => {
                        StrokeSource::Filtered { pixels: px.clone(), filter }
                    }
                    // Spot healing needs no source: it looks for one near
                    // where the stroke starts.
                    StrokeFrom::HealSpot => StrokeSource::Heal(Box::new(
                        cshop_core::heal::Heal::spot(
                            px.clone(),
                            {
                                let l = layer_local(at, layer_offset);
                                (l.x.round() as i32, l.y.round() as i32)
                            },
                            brush.radius(),
                        ),
                    )),
                    // The History Brush is the Clone Stamp copying from the
                    // same place in a different time, so it is the same
                    // source with no offset at all.
                    StrokeFrom::History => {
                        let (_, from_layer, pixels) =
                            from_history.expect("settled above for a history stroke");
                        if from_layer != id {
                            self.fail("That history state was taken from another layer");
                            return;
                        }
                        StrokeSource::Clone { pixels, offset: (0, 0) }
                    }
                    StrokeFrom::Heal => StrokeSource::Heal(Box::new(
                        cshop_core::heal::Heal::within(
                            px.clone(),
                            anchored.expect("settled above for a healing stroke"),
                        ),
                    )),
                    StrokeFrom::Clone => {
                        let offset = anchored.expect("settled above for a clone stroke");
                        let Some(pixels) = self.sample_source(sample_all) else { return };
                        // The source is in document space; the stroke is layer
                        // space, so fold the layer's own offset in.
                        StrokeSource::Clone {
                            pixels,
                            offset: (offset.0 + layer_offset.0, offset.1 + layer_offset.1),
                        }
                    }
                };

                let Some(view) = self.doc_mut() else { return };
                let Some(px) = view.doc.tree.get(id).and_then(|l| l.pixels()) else { return };
                let mut stroke =
                    Stroke::with_source(px.width(), px.height(), brush, mode, source);
                stroke.add_point(layer_local(at, layer_offset));
                self.stroke = Some(ActiveStroke {
                    tool: self.tool,
                    layer: id,
                    stroke,
                    target: StrokeTarget::Pixels(snapshot),
                });
            }
        }
        self.continue_stroke(at);
    }

    /// Extend the stroke to a new document-space position.
    pub fn continue_stroke(&mut self, at: Vec2) {
        if self.smudge.is_some() {
            self.continue_smudge(at);
            return;
        }
        let Some(active) = self.stroke.as_mut() else { return };
        let Some(index) = self.active else { return };
        let view = &mut self.docs[index];

        // The offset that maps the buffer being painted into document space.
        let offset = match (&active.target, view.doc.tree.get(active.layer)) {
            (StrokeTarget::QuickMask(_), _) => (0, 0),
            (StrokeTarget::Mask(_), Some(l)) => l.mask.as_ref().map_or((0, 0), |m| m.offset),
            (StrokeTarget::Pixels(_), Some(l)) => l.offset,
            _ => return,
        };

        active.stroke.add_point(layer_local(at, offset));
        let recent = active.stroke.take_recent();
        if recent.is_empty() {
            return;
        }

        // Split the document borrow so the selection can clip a write to the
        // layer it lives beside.
        let Document { tree, selection, .. } = &mut view.doc;

        // The original has to be taken before the dab overwrites it, and only
        // for the part the dab reaches.
        match &mut active.target {
            StrokeTarget::Pixels(snapshot) => {
                let Some(pixels) = tree.get_mut(active.layer).and_then(|l| l.pixels_mut()) else {
                    return;
                };
                snapshot.capture(&*pixels, recent);
                let clip = selection.as_ref().map(|s| Clip { selection: s, offset });
                active.stroke.render_region(snapshot, pixels, recent, clip.as_ref());
            }
            StrokeTarget::Mask(snapshot) => {
                let clip = selection.as_ref().map(|s| Clip { selection: s, offset });
                let Some(mask) = tree.get_mut(active.layer).and_then(|l| l.mask.as_mut()) else {
                    return;
                };
                snapshot.capture(&mask.data, recent);
                active.stroke.render_region_into_mask(
                    snapshot,
                    &mut mask.data,
                    recent,
                    clip.as_ref(),
                );
            }
            StrokeTarget::QuickMask(snapshot) => {
                // Editing the selection directly, so nothing clips it.
                let (sw, sh) = (snapshot.width(), snapshot.height());
                let target = selection
                    .get_or_insert_with(|| Selection::from_mask(MaskBuffer::reveal_all(sw, sh)));
                // A Quick Mask stroke can land anywhere, so the selection has
                // to hold the whole document rather than only what is already
                // covered.
                target.widen_to_document();
                snapshot.capture(target.window().0, recent);
                active.stroke.render_region_into_mask(
                    snapshot,
                    target.mask_mut(),
                    recent,
                    None,
                );
                target.invalidate();
            }
        }

        // A mask or selection change alters what the composite shows, so both
        // report as pixel edits on the layer.
        view.mark_dirty(Dirty::pixels(active.layer, recent.translate(offset.0, offset.1)));
    }

    // -----------------------------------------------------------------------
    // Type
    // -----------------------------------------------------------------------


    /// The style a new layer starts with, with a family filled in if the tool
    /// has not been used yet.
    fn resolved_text_style(&mut self) -> cshop_core::text::TextStyle {
        if self.text_style.family.is_empty() {
            self.text_style.family = cshop_core::font::FontDb::global().default_family();
        }
        let mut style = self.text_style.clone();
        style.color = self.foreground;
        style
    }

    fn begin_text(&mut self, at: Vec2, wrap: Option<f32>) {
        self.commit_text();
        let style = self.resolved_text_style();
        if style.family.is_empty() {
            self.fail("No fonts were found on this system");
            return;
        }
        let content = cshop_core::text::TextContent {
            text: String::new(),
            style,
            wrap_width: wrap.filter(|w| *w > 8.0),
        };

        let Some(view) = self.doc_mut() else { return };
        let id = view.doc.tree.alloc_id();
        let Some(mut layer) = Layer::text_layer(id, content) else {
            self.fail("That font could not be loaded");
            return;
        };
        // Put the anchor where the click was.
        let anchor = layer.text().expect("just built as type").anchor();
        layer.offset = (at.x.round() as i32 - anchor.0, at.y.round() as i32 - anchor.1);

        // Inserted straight into the tree rather than through the history: an
        // empty type layer is not an undo step, and the whole session becomes
        // one when it is committed.
        let pos = new_layer_pos(view);
        view.doc.tree.insert(layer, pos.parent, pos.index);
        view.doc.select(Some(id));
        view.mark_dirty(cshop_core::document::Dirty::structural(view.doc.bounds()));
        view.invalidate();

        self.text_edit =
            Some(crate::text_tool::TextEdit {
                layer: id,
                caret: 0,
                before: None,
                anchor: at,
                blink_epoch: self.now,
            });
    }

    fn edit_text_layer(&mut self, id: LayerId) {
        self.commit_text();
        let now = self.now;
        let Some(view) = self.doc_mut() else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        let Some(text) = layer.text() else {
            self.notify("That is not a type layer");
            return;
        };
        let before = (text.content().clone(), layer.offset);
        let anchor = Vec2::new(
            (layer.offset.0 + text.anchor().0) as f32,
            (layer.offset.1 + text.anchor().1) as f32,
        );
        let caret = before.0.text.len();
        view.doc.select(Some(id));
        self.text_edit =
            Some(crate::text_tool::TextEdit {
            layer: id,
            caret,
            before: Some(before),
            anchor,
            blink_epoch: now,
        });
        self.text_style = self.text_edit.as_ref().unwrap().before.as_ref().unwrap().0.style.clone();
    }

    /// The content of the layer being edited.
    fn editing_content(&self) -> Option<cshop_core::text::TextContent> {
        let edit = self.text_edit.as_ref()?;
        let view = self.doc()?;
        Some(view.doc.tree.get(edit.layer)?.text()?.content().clone())
    }

    /// Write new content straight onto the layer, outside the undo stack.
    ///
    /// The whole session becomes one history step when it is committed, so a
    /// keystroke must not push one of its own.
    fn live_set_text(&mut self, content: cshop_core::text::TextContent) {
        let Some(edit) = self.text_edit.as_ref() else { return };
        let (layer_id, anchor) = (edit.layer, edit.anchor);
        let Some(view) = self.doc_mut() else { return };
        let Some(layer) = view.doc.tree.get_mut(layer_id) else { return };
        let before = layer.bounds();
        let Some(text) = layer.text_mut() else { return };
        text.set_content(content);
        // Re-rendering can move the raster's corner — right-aligned text grows
        // leftwards — so re-derive the offset from the anchor that has not
        // moved.
        let new_anchor = text.anchor();
        layer.offset =
            (anchor.x.round() as i32 - new_anchor.0, anchor.y.round() as i32 - new_anchor.1);
        let dirty = cshop_core::document::Dirty::pixels(layer_id, before.union(&layer.bounds()));
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// Apply the current tool style to the type being edited.
    pub fn refresh_text_style(&mut self) {
        let Some(mut content) = self.editing_content() else { return };
        let style = self.resolved_text_style();
        if content.style == style {
            return;
        }
        content.style = style;
        self.live_set_text(content);
    }

    fn apply_text_input(&mut self, input: crate::text_tool::TextInput) {
        use crate::text_tool::{next_char, prev_char, TextInput as T};
        let Some(mut content) = self.editing_content() else { return };
        let Some(edit) = self.text_edit.as_mut() else { return };
        let caret = edit.caret.min(content.text.len());

        let font = cshop_core::font::FontDb::global().load(
            &content.style.family,
            content.style.bold,
            content.style.italic,
        );

        match input {
            T::Insert(s) => {
                content.text.insert_str(caret, &s);
                edit.caret = caret + s.len();
            }
            T::Newline => {
                content.text.insert(caret, '\n');
                edit.caret = caret + 1;
            }
            T::Backspace => {
                if caret == 0 {
                    return;
                }
                let from = prev_char(&content.text, caret);
                content.text.replace_range(from..caret, "");
                edit.caret = from;
            }
            T::Delete => {
                if caret >= content.text.len() {
                    return;
                }
                let to = next_char(&content.text, caret);
                content.text.replace_range(caret..to, "");
            }
            T::Left => {
                edit.caret = prev_char(&content.text, caret);
                return;
            }
            T::Right => {
                edit.caret = next_char(&content.text, caret);
                return;
            }
            T::Up | T::Down => {
                if let Some(font) = &font {
                    edit.caret = cshop_core::text::caret_line_step(
                        &content,
                        font,
                        caret,
                        input == T::Down,
                    );
                }
                return;
            }
            T::Home | T::End => {
                if let Some(font) = &font {
                    let (a, b) = cshop_core::text::line_bounds(&content, font, caret);
                    edit.caret = if input == T::Home { a } else { b };
                }
                return;
            }
            T::Commit => {
                self.commit_text();
                return;
            }
            T::Cancel => {
                self.cancel_text();
                return;
            }
        }
        self.live_set_text(content);
    }

    fn text_caret_at(&mut self, at: Vec2) {
        let Some(content) = self.editing_content() else { return };
        let Some(edit) = self.text_edit.as_ref() else { return };
        let Some(view) = self.doc() else { return };
        let Some(layer) = view.doc.tree.get(edit.layer) else { return };
        let Some(text) = layer.text() else { return };
        let Some(font) = cshop_core::font::FontDb::global().load(
            &content.style.family,
            content.style.bold,
            content.style.italic,
        ) else {
            return;
        };
        // The click is in document space; the layout's origin is the layout
        // box's top-left, which sits at the raster's anchor for a box and one
        // ascent above it for point text.
        let origin = (
            (layer.offset.0 + text.layout_origin().0) as f32,
            (layer.offset.1 + text.layout_origin().1) as f32,
        );
        let caret = cshop_core::text::byte_at(&content, &font, at.x - origin.0, at.y - origin.1);
        if let Some(edit) = self.text_edit.as_mut() {
            edit.caret = caret;
        }
    }

    /// Finish editing, recording the whole session as one undo step.
    pub fn commit_text(&mut self) {
        let Some(edit) = self.text_edit.take() else { return };
        let Some(view) = self.doc_mut() else { return };
        let Some(layer) = view.doc.tree.get(edit.layer) else { return };
        let Some(text) = layer.text() else { return };
        let content = text.content().clone();
        let offset = layer.offset;

        // Empty type is not worth a layer.
        if content.text.trim().is_empty() {
            if edit.is_new() {
                view.doc.tree.remove(edit.layer);
                view.doc.prune_selection();
                view.mark_dirty(cshop_core::document::Dirty::structural(view.doc.bounds()));
            }
            view.invalidate();
            return;
        }

        match edit.before {
            // The layer is in the tree but not the history, so take it back
            // out and add it properly as one step.
            None => {
                let id = edit.layer;
                let pos = view.doc.tree.position(id);
                view.doc.tree.remove(id);
                let Some(mut layer) = Layer::text_layer(id, content) else { return };
                layer.offset = offset;
                let pos = pos.unwrap_or_else(|| new_layer_pos(view));
                let dirty = view
                    .history
                    .apply(&mut view.doc, Box::new(AddLayer::new(layer, pos, "Type Layer")));
                view.mark_dirty(dirty);
            }
            // An existing layer is already showing the edit, so put the old
            // content back and let the command re-apply it.
            Some((old, old_offset)) => {
                if old == content && old_offset == offset {
                    return;
                }
                if let Some(l) = view.doc.tree.get_mut(edit.layer) {
                    if let Some(t) = l.text_mut() {
                        t.set_content(old);
                    }
                    l.offset = old_offset;
                }
                let dirty = view.history.apply(
                    &mut view.doc,
                    Box::new(cshop_core::history::SetTextContent::new(
                        edit.layer,
                        content,
                        offset,
                        "Edit Type",
                    )),
                );
                view.mark_dirty(dirty);
            }
        }
        view.invalidate();
    }

    /// Abandon the edit, putting back whatever was there before.
    pub fn cancel_text(&mut self) {
        let Some(edit) = self.text_edit.take() else { return };
        let Some(view) = self.doc_mut() else { return };
        match edit.before {
            // Never entered the history, so it just goes away.
            None => {
                view.doc.tree.remove(edit.layer);
                view.doc.prune_selection();
                view.mark_dirty(cshop_core::document::Dirty::structural(view.doc.bounds()));
            }
            Some((old, old_offset)) => {
                if let Some(l) = view.doc.tree.get_mut(edit.layer) {
                    let before = l.bounds();
                    if let Some(t) = l.text_mut() {
                        t.set_content(old);
                    }
                    l.offset = old_offset;
                    let dirty =
                        cshop_core::document::Dirty::pixels(edit.layer, before.union(&l.bounds()));
                    view.mark_dirty(dirty);
                }
            }
        }
        view.invalidate();
    }

    // -----------------------------------------------------------------------
    // Shapes
    // -----------------------------------------------------------------------

    /// The rectangle a drag describes, with Shift and Alt applied.
    pub fn shape_rect(
        from: Vec2,
        to: Vec2,
        from_centre: bool,
        constrain: bool,
    ) -> (Vec2, (f32, f32)) {
        let (mut dx, mut dy) = (to.x - from.x, to.y - from.y);
        if constrain {
            // Shift makes it square, keeping the direction of the drag.
            let s = dx.abs().max(dy.abs());
            dx = s * if dx < 0.0 { -1.0 } else { 1.0 };
            dy = s * if dy < 0.0 { -1.0 } else { 1.0 };
        }
        if from_centre {
            // Alt draws out from the point the drag started.
            (Vec2::new(from.x - dx, from.y - dy), (dx.abs() * 2.0, dy.abs() * 2.0))
        } else {
            (Vec2::new(from.x.min(from.x + dx), from.y.min(from.y + dy)), (dx.abs(), dy.abs()))
        }
    }

    fn draw_shape(&mut self, from: Vec2, to: Vec2, from_centre: bool, constrain: bool) {
        let (origin, size) = Self::shape_rect(from, to, from_centre, constrain);
        if size.0 < 1.0 && size.1 < 1.0 {
            return;
        }
        // A line runs corner to corner of the drag, so its direction survives.
        let kind = match self.shape_kind.clone() {
            cshop_core::shape::ShapeKind::Line { thickness, .. } => {
                let unit = |a: f32, b: f32, span: f32| {
                    if span <= 0.0 {
                        0.0
                    } else {
                        ((b - a) / span).clamp(0.0, 1.0)
                    }
                };
                let (start, end) = if from_centre {
                    (Vec2::new(from.x - (to.x - from.x), from.y - (to.y - from.y)), to)
                } else {
                    (from, to)
                };
                cshop_core::shape::ShapeKind::Line {
                    thickness,
                    from: (
                        unit(origin.x, start.x, size.0),
                        unit(origin.y, start.y, size.1),
                    ),
                    to: (unit(origin.x, end.x, size.0), unit(origin.y, end.y, size.1)),
                }
            }
            other => other,
        };

        let content = cshop_core::shape::ShapeContent::new(
            kind,
            (size.0.max(1.0), size.1.max(1.0)),
            self.shape_style,
        );
        let Some(view) = self.doc_mut() else { return };
        let id = view.doc.tree.alloc_id();
        let Some(mut layer) = Layer::shape_layer(id, content) else {
            self.fail("That shape has neither a fill nor a stroke");
            return;
        };
        let anchor = layer.shape().expect("just built as a shape").anchor();
        layer.offset = (origin.x.round() as i32 - anchor.0, origin.y.round() as i32 - anchor.1);

        let pos = new_layer_pos(view);
        let dirty =
            view.history.apply(&mut view.doc, Box::new(AddLayer::new(layer, pos, "Shape Layer")));
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// Turn the Pen tool's draft into a shape layer.
    ///
    /// The path's own coordinates are moved into the layer's box so that the
    /// layer can be dragged afterwards like any other, which is what keeps the
    /// pen from being a special case everywhere downstream.
    fn finish_path(&mut self, closed: bool) {
        let Some(draft) = self.pen.take() else { return };
        if draft.anchors.len() < 2 {
            return;
        }
        // An open path has nothing to fill, so it is drawn from its stroke —
        // and a stroke it has no colour for would be invisible.
        let mut style = self.shape_style;
        if !closed {
            style.stroke = style.stroke.or(style.fill).or(Some(self.foreground));
            style.fill = None;
        }
        let label = if closed { "Path" } else { "Open Path" };
        self.add_path_layer(draft.to_path(closed), style, label);
    }

    /// Put a path into the document as its own shape layer.
    ///
    /// The path's own coordinates move into the layer's box so that the layer
    /// can be dragged afterwards like any other, which is what keeps a path
    /// from being a special case everywhere downstream. Shared by the Pen tool
    /// and by the script, so both land in exactly the same place.
    pub fn add_path_layer(
        &mut self,
        path: cshop_core::path::PathShape,
        style: cshop_core::shape::ShapeStyle,
        label: &str,
    ) {
        let Some(bounds) = path.bounds() else { return };
        // Whole pixels, so the path's own coordinates and the layer's offset
        // agree exactly; see `write_path`.
        let origin = Vec2::new(bounds.x0.floor(), bounds.y0.floor());
        let local = path.translate(Vec2::new(-origin.x, -origin.y));
        let size = ((bounds.x1 - origin.x).max(1.0), (bounds.y1 - origin.y).max(1.0));

        let content = cshop_core::shape::ShapeContent::new(
            cshop_core::shape::ShapeKind::Path(local),
            size,
            style,
        );
        let Some(view) = self.doc_mut() else { return };
        let id = view.doc.tree.alloc_id();
        let Some(mut layer) = Layer::shape_layer(id, content) else {
            self.fail("That path has neither a fill nor a stroke");
            return;
        };
        let anchor = layer.shape().expect("just built as a shape").anchor();
        layer.offset = (origin.x.round() as i32 - anchor.0, origin.y.round() as i32 - anchor.1);

        let pos = new_layer_pos(view);
        let dirty = view.history.apply(&mut view.doc, Box::new(AddLayer::new(layer, pos, label)));
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// Combine the selected shape layers into one path.
    ///
    /// The operands keep their own geometry inside the result, so the
    /// operation stays editable — changing the mode afterwards re-combines the
    /// same shapes rather than starting again from a flattened outline.
    fn combine_shapes(&mut self, op: cshop_core::path::BoolOp) {
        use cshop_core::path::{PathPart, PathShape};

        let ids: Vec<LayerId> = match self.doc() {
            Some(view) if !view.doc.selected_layers.is_empty() => {
                view.doc.selected_layers.clone()
            }
            _ => Vec::new(),
        };
        if ids.len() < 2 {
            self.fail("Select two or more shape layers to combine");
            return;
        }

        // Bottom-up, so the result reads the way the layers are stacked: the
        // lowest is the shape being cut into.
        let Some(view) = self.doc_mut() else { return };
        let order = view.doc.tree.iter_all();
        let mut chosen: Vec<LayerId> =
            order.into_iter().filter(|id| ids.contains(id)).collect();
        if chosen.len() < 2 {
            return;
        }

        let mut parts: Vec<PathPart> = Vec::new();
        let mut style = None;
        for (i, id) in chosen.iter().enumerate() {
            let Some(layer) = view.doc.tree.get(*id) else { return };
            let Some(shape) = layer.shape() else {
                self.fail("Only shape layers can be combined");
                return;
            };
            let content = shape.content();
            if style.is_none() {
                style = Some(content.style);
            }
            // Everything is brought into document space, since the operands
            // came from layers that sat in different places.
            let anchor = shape.anchor();
            let at = Vec2::new(
                (layer.offset.0 + anchor.0) as f32,
                (layer.offset.1 + anchor.1) as f32,
            );
            let mut path = match &content.kind {
                cshop_core::shape::ShapeKind::Path(p) => p.clone(),
                other => PathShape::new(cshop_core::shape::outline(other, content.size)),
            }
            .translate(at);
            for part in &mut path.parts {
                // Only the first operand of each shape keeps its own mode; a
                // compound being folded in combines as a whole.
                part.op = op;
            }
            if i == 0 {
                if let Some(first) = path.parts.first_mut() {
                    first.op = cshop_core::path::BoolOp::Union;
                }
            }
            parts.extend(path.parts);
        }

        let combined = PathShape { parts };
        let Some(bounds) = combined.bounds() else { return };
        let origin = Vec2::new(bounds.x0, bounds.y0);
        let local = combined.translate(Vec2::new(-origin.x, -origin.y));
        let size = ((bounds.x1 - bounds.x0).max(1.0), (bounds.y1 - bounds.y0).max(1.0));

        let content = cshop_core::shape::ShapeContent::new(
            cshop_core::shape::ShapeKind::Path(local),
            size,
            style.unwrap_or_default(),
        );
        let id = view.doc.tree.alloc_id();
        let Some(mut layer) = Layer::shape_layer(id, content) else {
            self.fail("The combined path has neither a fill nor a stroke");
            return;
        };
        let anchor = layer.shape().expect("just built as a shape").anchor();
        layer.offset = (origin.x.round() as i32 - anchor.0, origin.y.round() as i32 - anchor.1);

        // One history step: the operands go, the result arrives.
        let pos = new_layer_pos(view);
        let mut steps: Vec<Box<dyn cshop_core::history::Command>> =
            vec![Box::new(AddLayer::new(layer, pos, op.name()))];
        chosen.reverse();
        for id in chosen {
            steps.push(Box::new(cshop_core::history::DeleteLayer::new(id)));
        }
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::Compound::new(op.name(), steps)),
        );
        view.mark_dirty(dirty);
        view.invalidate();
    }

    // -----------------------------------------------------------------------
    // Segmenting
    // -----------------------------------------------------------------------

    /// Write the composite somewhere the models can read it.
    ///
    /// They take a file, and the document is a stack of layers that exists
    /// only in memory, so there has to be a flattened copy. Kept beside the
    /// mask under one directory so both go together.
    fn segment_source(&mut self) -> Result<(std::path::PathBuf, u32, u32), String> {
        let gpu = self.gpu.clone();
        let index = self.active.ok_or("no document")?;
        let view = &mut self.docs[index];
        view.sync_composite_only(&gpu, &mut self.compositor);
        let pixels = view.read_composite(&gpu);
        let (w, h) = (pixels.width(), pixels.height());
        let dir = crate::vision::scratch();
        crate::vision::make_scratch(&dir).map_err(|e| format!("{e}"))?;
        let path = dir.join("source.png");
        cshop_io::save(&path, &pixels, 100).map_err(|e| format!("{e}"))?;
        Ok((path, w, h))
    }

    fn segment_detect(&mut self) {
        let source = match self.segment_source() {
            Ok(s) => s,
            Err(e) => return self.fail(format!("Could not prepare the image: {e}")),
        };
        if let Dialog::Segment(d) = &mut self.dialog {
            d.busy = true;
            d.status = "Looking…".into();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.segment_job = Some(rx);
        std::thread::spawn(move || {
            let found = crate::vision::detect(&source.0, 0.25, "");
            if let Some(dir) = source.0.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
            let _ = tx.send(match found {
                Ok(objects) => SegmentOutcome::Found(objects),
                Err(e) => SegmentOutcome::Failed(e),
            });
        });
    }

    /// Collect a finished segmentation, if one has come back.
    ///
    /// Called once a frame. While a job is outstanding the frame is asked to
    /// repaint, which is what keeps the spinner turning.
    /// Ask the depth model how far away everything is, on a thread.
    fn start_depth(&mut self, pixels: PixelBuffer) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dir = crate::vision::scratch();
            let result = (|| {
                crate::vision::make_scratch(&dir)
                    .map_err(|e| format!("could not use a scratch directory: {e}"))?;
                let input = dir.join("source.png");
                let out = dir.join("depth.png");
                cshop_io::save(&input, &pixels, 100)
                    .map_err(|e| format!("could not write the image for the model: {e}"))?;
                let path = crate::vision::depth(&input, &out)?;
                crate::vision::depth_map(&path)
            })();
            let _ = std::fs::remove_dir_all(&dir);
            let _ = tx.send(result);
        });
        self.depth_job = Some(rx);
    }

    pub fn poll_depth(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.depth_job else { return };
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            Err(_) => {
                self.depth_job = None;
                let for_mask = self.depth_for_mask.take().is_some();
                if let Dialog::Relight(d) = &mut self.dialog {
                    d.busy = false;
                    d.status = "The depth model stopped without answering.".into();
                }
                if for_mask {
                    self.fail("The depth model stopped without answering");
                }
            }
            Ok(Err(why)) => {
                self.depth_job = None;
                let for_mask = self.depth_for_mask.take().is_some();
                if let Dialog::Relight(d) = &mut self.dialog {
                    d.busy = false;
                    d.status = why.clone();
                }
                if for_mask {
                    self.fail(why);
                }
            }
            Ok(Ok(map)) => {
                self.depth_job = None;
                // The same model answers two questions. Which one asked is
                // recorded when the job starts, so the answer knows where to go.
                if let Some((id, invert)) = self.depth_for_mask.take() {
                    self.finish_depth_mask(id, invert, map);
                    return;
                }
                let lit = match &mut self.dialog {
                    Dialog::Relight(d) => {
                        d.depth = Some(std::sync::Arc::new(map));
                        d.busy = false;
                        d.status = "Move the lamp; the shape is worked out.".into();
                        let layer = d.layer;
                        d.lit().map(|p| (layer, p))
                    }
                    _ => None,
                };
                if let Some((id, pixels)) = lit {
                    self.show_relight(id, &pixels);
                }
            }
        }
    }

    /// Show a lighting straight on the canvas, without a history entry.
    fn show_relight(&mut self, id: LayerId, pixels: &PixelBuffer) {
        let Some(view) = self.doc_mut() else { return };
        let Some(layer) = view.doc.tree.get_mut(id) else { return };
        let bounds = layer.bounds();
        let Some(px) = layer.pixels_mut() else { return };
        *px = pixels.clone();
        let dirty = cshop_core::document::Dirty::pixels(id, bounds);
        view.mark_dirty(dirty);
        view.invalidate();
        if let Dialog::Relight(d) = &mut self.dialog {
            d.showing = true;
        }
    }

    /// Commit the lighting that is showing, as one entry.
    fn keep_relight(&mut self) {
        let taken = match &mut self.dialog {
            Dialog::Relight(d) => {
                let (layer, before) = (d.layer, d.before.clone());
                d.lit().map(|after| (layer, before, after))
            }
            _ => None,
        };
        let Some((id, before, after)) = taken else {
            self.fail("There is no lighting to keep");
            return;
        };
        let Some(view) = self.doc_mut() else { return };
        let Some(layer) = view.doc.tree.get(id) else {
            self.fail("That layer has gone");
            return;
        };
        let (offset, mask) = (layer.offset, layer.mask.clone());
        // The preview is showing, so the layer is put back first and the entry
        // built from what was really there.
        if let Some(px) = view.doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
            *px = before;
        }
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::ReplaceLayerPixels::new(
                id, after, offset, mask, "Relight",
            )),
        );
        view.mark_dirty(dirty);
        view.invalidate();
        self.notify("Relit");
    }

    /// Hand the selection to the model as a hole to fill in.
    ///
    /// On a thread, because it is a couple of seconds and a window that stops
    /// answering for a couple of seconds is a window that looks broken.
    fn start_inpaint(&mut self) {
        if self.inpaint_job.is_some() {
            return;
        }
        let picked = self.doc().and_then(|v| {
            let id = v.doc.active?;
            let layer = v.doc.tree.get(id)?;
            let pixels = layer.pixels()?.clone();
            let selection = v.doc.selection.clone()?;
            Some((id, layer.offset, pixels, selection))
        });
        let Some((id, offset, source, selection)) = picked else {
            self.fail("Filling in needs a selection on a layer with pixels in it");
            return;
        };

        let (w, h) = (source.width(), source.height());
        let mut mask = PixelBuffer::filled(w, h, Rgba8::BLACK);
        let mut any = false;
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                if selection.coverage(x + offset.0, y + offset.1) > 127 {
                    mask.set(x, y, Rgba8::WHITE);
                    any = true;
                }
            }
        }
        if !any {
            self.fail("The selection does not overlap this layer");
            return;
        }

        self.notify("Filling in…");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dir = crate::vision::scratch();
            let result = (|| {
                crate::vision::make_scratch(&dir)
                    .map_err(|e| format!("could not use a scratch directory: {e}"))?;
                let input = dir.join("source.png");
                let mask_path = dir.join("mask.png");
                let output = dir.join("filled.png");
                cshop_io::save(&input, &source, 100)
                    .map_err(|e| format!("could not write the image for the model: {e}"))?;
                cshop_io::save(&mask_path, &mask, 100)
                    .map_err(|e| format!("could not write the mask: {e}"))?;
                let path = crate::vision::inpaint(&input, &mask_path, &output)?;
                let filled = cshop_io::load(&path)
                    .map_err(|e| format!("could not read the filled image back: {e}"))?;

                // Alpha is the layer's own; the model works in RGB and has no
                // opinion about coverage.
                let mut pixels = source.clone();
                for y in 0..h as i32 {
                    for x in 0..w as i32 {
                        if mask.get(x, y).r > 127 {
                            let c = filled.get(x, y);
                            let a = pixels.get(x, y).a;
                            pixels.set(x, y, Rgba8::new(c.r, c.g, c.b, a));
                        }
                    }
                }
                Ok((id, pixels))
            })();
            let _ = std::fs::remove_dir_all(&dir);
            let _ = tx.send(result);
        });
        self.inpaint_job = Some(rx);
    }

    pub fn poll_inpaint(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.inpaint_job else { return };
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(_) => {
                self.inpaint_job = None;
                self.fail("The model stopped without answering");
            }
            Ok(Err(why)) => {
                self.inpaint_job = None;
                self.fail(why);
            }
            Ok(Ok((id, pixels))) => {
                self.inpaint_job = None;
                let Some(view) = self.doc_mut() else { return };
                let Some(layer) = view.doc.tree.get(id) else { return };
                let (offset, mask) = (layer.offset, layer.mask.clone());
                let dirty = view.history.apply(
                    &mut view.doc,
                    Box::new(cshop_core::history::ReplaceLayerPixels::new(
                        id, pixels, offset, mask, "Fill In",
                    )),
                );
                view.mark_dirty(dirty);
                view.invalidate();
                self.notify("Filled in");
            }
        }
    }

    /// Ask the labeller what is in the picture, on a thread.
    ///
    /// Half a second, but on the frame thread half a second is a visible
    /// stutter, and the window has a spinner to earn.
    fn start_separate_look(&mut self, pixels: PixelBuffer) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dir = crate::vision::scratch();
            let result = (|| {
                crate::vision::make_scratch(&dir)
                    .map_err(|e| format!("could not use a scratch directory: {e}"))?;
                let input = dir.join("source.png");
                let map = dir.join("labels.png");
                cshop_io::save(&input, &pixels, 100)
                    .map_err(|e| format!("could not write the image for the model: {e}"))?;
                let answer = crate::vision::classify(&input, &map)?;
                let labels = cshop_io::load(&answer.map)
                    .map_err(|e| format!("could not read the map back: {e}"))?;
                Ok((answer.regions, labels))
            })();
            let _ = std::fs::remove_dir_all(&dir);
            let _ = tx.send(result);
        });
        self.separate_job = Some(rx);
    }

    pub fn poll_separate(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.separate_job else { return };
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            Err(_) => {
                self.separate_job = None;
                if let Dialog::Separate(d) = &mut self.dialog {
                    d.busy = false;
                    d.status = "The labeller stopped without answering.".into();
                }
            }
            Ok(Err(why)) => {
                self.separate_job = None;
                if let Dialog::Separate(d) = &mut self.dialog {
                    d.busy = false;
                    d.status = why;
                }
            }
            Ok(Ok((regions, labels))) => {
                self.separate_job = None;
                self.separate_map = Some(labels);
                if let Dialog::Separate(d) = &mut self.dialog {
                    d.show(regions);
                }
            }
        }
    }

    /// One layer for each kind of thing that was ticked.
    ///
    /// Every way this can fail says so. Separating is one click and then
    /// nothing visibly happens — the new layers reconstruct the picture
    /// exactly, so the canvas is unchanged — which makes a silent failure
    /// indistinguishable from success.
    fn run_separate(&mut self) {
        let (id, picked, feather) = match &self.dialog {
            Dialog::Separate(d) => (d.layer, d.picked(), d.feather),
            _ => {
                self.fail("The Separate window is not open any more");
                return;
            }
        };
        if picked.is_empty() {
            self.fail("Nothing was ticked to separate");
            return;
        }
        let Some(map) = self.separate_map.clone() else {
            self.fail("The labeller's answer has gone; open the window again");
            return;
        };
        let Some(source) = self
            .doc()
            .and_then(|v| v.doc.tree.get(id)?.pixels().cloned())
        else {
            self.fail("That layer has no pixels any more");
            return;
        };

        let mut made = 0usize;
        for region in &picked {
            let Some(pixels) = crate::separate_ui::separated_layer(&source, &map, region.id, feather) else {
                continue;
            };
            let Some(view) = self.doc_mut() else { return };
            let new_id = view.doc.tree.alloc_id();
            let mut fresh =
                cshop_core::layer::Layer::raster(new_id, region.class.clone(), pixels);
            fresh.offset = view.doc.tree.get(id).map(|l| l.offset).unwrap_or((0, 0));
            let pos = view
                .doc
                .tree
                .position(id)
                .map(|p| cshop_core::LayerPos { parent: p.parent, index: p.index + 1 })
                .unwrap_or(cshop_core::LayerPos {
                    parent: None,
                    index: view.doc.tree.root().len(),
                });
            let dirty = view
                .history
                .apply(&mut view.doc, Box::new(AddLayer::new(fresh, pos, "Separate")));
            view.mark_dirty(dirty);
            made += 1;
        }
        if let Some(view) = self.doc_mut() {
            view.invalidate();
        }
        self.separate_map = None;
        if made == 0 {
            // Every chosen class turned out to hold nothing once the map was
            // read back, which is a real answer and not a reason for silence.
            self.fail("None of those turned out to cover any pixels");
        } else {
            self.notify(format!(
                "Separated into {made} layer{}",
                if made == 1 { "" } else { "s" }
            ));
        }
    }

    /// Hand every raster layer to the enlarger, on a thread.
    fn start_upscale(&mut self) {
        let Dialog::Upscale(d) = &self.dialog else { return };
        if d.progress.is_some() {
            return;
        }
        let scale = d.scale;
        let rasters: Vec<(LayerId, PixelBuffer)> = match self.doc() {
            Some(v) => v
                .doc
                .tree
                .iter_all()
                .into_iter()
                .filter_map(|id| {
                    let layer = v.doc.tree.get(id)?;
                    if !matches!(layer.kind, cshop_core::layer::LayerKind::Raster(_)) {
                        return None;
                    }
                    Some((id, layer.pixels()?.clone()))
                })
                .collect(),
            None => return,
        };
        if rasters.is_empty() {
            self.fail("There are no pixels here to enlarge");
            return;
        }
        let size = self.doc().map(|v| (v.doc.width, v.doc.height)).unwrap_or((1, 1));

        let progress = std::sync::Arc::new(crate::vision::DenoiseProgress::default());
        if let Dialog::Upscale(d) = &mut self.dialog {
            d.progress = Some(progress.clone());
            d.status.clear();
        }

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dir = crate::vision::scratch();
            let result = (|| {
                crate::vision::make_scratch(&dir)
                    .map_err(|e| format!("could not use a scratch directory: {e}"))?;
                let mut out = Vec::new();
                for (i, (id, pixels)) in rasters.iter().enumerate() {
                    let input = dir.join(format!("in{i}.png"));
                    let output = dir.join(format!("out{i}.png"));
                    cshop_io::save(&input, pixels, 100)
                        .map_err(|e| format!("could not write the image for the model: {e}"))?;
                    let answer = crate::vision::upscale(&input, &output, scale, &progress)?;
                    let big = cshop_io::load(&answer.path)
                        .map_err(|e| format!("could not read the enlargement back: {e}"))?;
                    out.push((*id, big));
                }
                Ok(out)
            })();
            let _ = std::fs::remove_dir_all(&dir);
            let _ = tx.send(result.map(|layers| (size, scale, layers)));
        });
        self.upscale_job = Some(rx);
    }

    /// Collect the enlargement and put the document at its new size.
    pub fn poll_upscale(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.upscale_job else { return };
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(_) => {
                self.upscale_job = None;
                if let Dialog::Upscale(d) = &mut self.dialog {
                    d.progress = None;
                    d.status = "The model stopped without answering.".into();
                }
            }
            Ok(Err(why)) => {
                self.upscale_job = None;
                if let Dialog::Upscale(d) = &mut self.dialog {
                    d.progress = None;
                    d.status = why;
                }
            }
            Ok(Ok(((w, h), scale, layers))) => {
                self.upscale_job = None;
                self.finish_upscale(w, h, scale, layers);
                self.dialog = Dialog::None;
            }
        }
    }

    /// The resize first, then the model's pixels into the room it made.
    fn finish_upscale(
        &mut self,
        w: u32,
        h: u32,
        scale: f32,
        layers: Vec<(LayerId, PixelBuffer)>,
    ) {
        let (nw, nh) = (
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
        );
        let mut steps: Vec<Box<dyn cshop_core::history::Command>> =
            vec![Box::new(cshop_core::history::ResizeImage::new(
                nw,
                nh,
                cshop_core::resample::Resampling::Lanczos3,
            ))];
        for (id, big) in layers {
            steps.push(Box::new(cshop_core::history::UpscaleLayer::new(id, big)));
        }

        let gpu = self.gpu.clone();
        let Some(view) = self.doc_mut() else { return };
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::Compound::new("Upscale", steps)),
        );
        view.mark_dirty(dirty);
        view.resize_targets(&gpu);
        view.zoom_initialised = false;
        view.invalidate();
        self.notify(format!("Enlarged to {nw}×{nh}"));
    }

    /// Put a patch straight into the layer, without going through the history.
    ///
    /// The same trick the Segment window uses: this is a preview that happens
    /// to be made of pixels, and giving it an undo entry per slider movement
    /// would fill the panel with steps nobody wants to go back to. What lands
    /// in the history is whatever is showing when Keep is pressed.
    fn show_denoise_patch(&mut self, id: LayerId, region: IRect, patch: &PixelBuffer) {
        let Some(view) = self.doc_mut() else { return };
        let Some(layer) = view.doc.tree.get_mut(id) else { return };
        let offset = layer.offset;
        let Some(pixels) = layer.pixels_mut() else { return };
        pixels.paste(patch, region.x0, region.y0);
        let dirty = cshop_core::document::Dirty::pixels(
            id,
            IRect::new(
                region.x0 + offset.0,
                region.y0 + offset.1,
                region.x1 + offset.0,
                region.y1 + offset.1,
            ),
        );
        view.mark_dirty(dirty);
        view.invalidate();
        if let Dialog::Denoise(d) = &mut self.dialog {
            d.showing = true;
        }
    }

    /// Hand the region to the model, on a thread, and watch the tiles go by.
    fn start_denoise(&mut self) {
        let Dialog::Denoise(d) = &self.dialog else { return };
        if d.progress.is_some() || d.cleaned.is_some() {
            return;
        }
        let (before, strength) = (d.before.clone(), 1.0f32);
        let progress = std::sync::Arc::new(crate::vision::DenoiseProgress::default());
        if let Dialog::Denoise(d) = &mut self.dialog {
            d.progress = Some(progress.clone());
            d.status.clear();
        }

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dir = crate::vision::scratch();
            let result = (|| {
                std::fs::create_dir_all(&dir).map_err(|e| format!("could not use a scratch directory: {e}"))?;
                let input = dir.join("noisy.png");
                let output = dir.join("clean.png");
                cshop_io::save(&input, &before, 100)
                    .map_err(|e| format!("could not write the image for the model: {e}"))?;
                let answer = crate::vision::denoise(&input, &output, strength, &progress)?;
                cshop_io::load(&answer.path)
                    .map_err(|e| format!("could not read the cleaned image back: {e}"))
            })();
            let _ = std::fs::remove_dir_all(&dir);
            let _ = tx.send(result);
        });
        self.denoise_job = Some(rx);
    }

    /// Collect the model's answer, and keep the bar moving until it arrives.
    pub fn poll_denoise(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.denoise_job else { return };
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(_) => {
                self.denoise_job = None;
                if let Dialog::Denoise(d) = &mut self.dialog {
                    d.progress = None;
                    d.status = "The model stopped without answering.".into();
                }
            }
            Ok(Err(why)) => {
                self.denoise_job = None;
                if let Dialog::Denoise(d) = &mut self.dialog {
                    d.progress = None;
                    d.status = why;
                }
            }
            Ok(Ok(cleaned)) => {
                self.denoise_job = None;
                let shown = match &mut self.dialog {
                    Dialog::Denoise(d) => {
                        d.progress = None;
                        d.cleaned = Some(cleaned);
                        d.status = "Done. Move the strength to take less of it.".into();
                        d.blended().map(|b| (d.layer, d.region, b))
                    }
                    // The window was closed while the model worked, so there
                    // is nothing to show it to.
                    _ => None,
                };
                if let Some((id, region, pixels)) = shown {
                    self.show_denoise_patch(id, region, &pixels);
                }
            }
        }
    }

    /// Commit what is on the canvas: one entry, the whole layer.
    fn keep_denoise(&mut self) {
        let taken = match &self.dialog {
            Dialog::Denoise(d) => d.blended().map(|b| (d.layer, d.region, d.before.clone(), b)),
            _ => None,
        };
        let Some((id, region, before, after)) = taken else {
            self.fail("There is no denoised result to keep");
            return;
        };
        let Some(view) = self.doc_mut() else { return };
        let Some(layer) = view.doc.tree.get(id) else {
            self.fail("That layer has gone");
            return;
        };
        let (offset, mask) = (layer.offset, layer.mask.clone());
        let Some(current) = layer.pixels() else { return };

        // The history holds whole layers, so the entry is built from the
        // layer as it was before the preview was pasted over it.
        let mut original = current.clone();
        original.paste(&before, region.x0, region.y0);
        let mut corrected = original.clone();
        corrected.paste(&after, region.x0, region.y0);

        // Put the original back first, so the entry's "before" is the truth
        // rather than the preview that is currently showing.
        if let Some(px) = view.doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
            *px = original;
        }
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::ReplaceLayerPixels::new(
                id,
                corrected,
                offset,
                mask,
                "Remove Noise",
            )),
        );
        view.mark_dirty(dirty);
        view.invalidate();
        // Closed here rather than by the button that pushed this, so the
        // action is the whole of the operation however it was reached.
        self.dialog = Dialog::None;
        self.notify("Noise removed");
    }

    /// Start the full-resolution pass on a worker thread.
    ///
    /// The window stays live while it runs, which is the point of doing it
    /// this way: a 60 megapixel correction takes seconds, and a frozen window
    /// with no progress bar is indistinguishable from a crash.
    fn start_lens(&mut self) {
        let Dialog::Lens(d) = &self.dialog else { return };
        if d.applying.is_some() {
            return;
        }
        let (lens, autocrop, id) = (d.lens, d.autocrop, d.layer);
        let Some(source) = self.doc().and_then(|v| v.doc.tree.get(id)?.pixels().cloned()) else {
            self.fail("That layer has no pixels any more");
            return;
        };

        let total = cshop_core::lens::total_rows(source.height());
        let progress = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        if let Dialog::Lens(d) = &mut self.dialog {
            d.applying = Some((progress.clone(), total));
        }

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let plane = cshop_core::filters::plane::Plane::from_pixels(&source);
            let out = cshop_core::lens::apply(&plane, lens, Some(&progress));
            let crop = autocrop
                .then(|| cshop_core::lens::largest_opaque_rect(&out))
                .filter(|r| !r.is_empty());
            let _ = tx.send(LensOutcome::Done { pixels: Box::new(out.to_pixels()), crop });
        });
        self.lens_job = Some(rx);
    }

    /// Collect the pass when it finishes, and keep the progress bar moving
    /// while it does not.
    pub fn poll_lens(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.lens_job else { return };
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            Err(_) => {
                self.lens_job = None;
                if let Dialog::Lens(d) = &mut self.dialog {
                    d.applying = None;
                }
                self.fail("The correction stopped without finishing");
            }
            Ok(LensOutcome::Failed(why)) => {
                self.lens_job = None;
                if let Dialog::Lens(d) = &mut self.dialog {
                    d.applying = None;
                }
                self.fail(why);
            }
            Ok(LensOutcome::Done { pixels, crop }) => {
                self.lens_job = None;
                let Dialog::Lens(d) = &self.dialog else { return };
                let (id, autocrop) = (d.layer, d.autocrop);
                let _ = autocrop;
                self.finish_lens(id, *pixels, crop);
                self.dialog = Dialog::None;
            }
        }
    }

    /// Put the corrected pixels in, and crop if that was asked for.
    ///
    /// One history entry for both, because they are one action as far as
    /// anyone undoing them is concerned.
    fn finish_lens(&mut self, id: LayerId, pixels: PixelBuffer, crop: Option<IRect>) {
        let gpu = self.gpu.clone();
        let Some(view) = self.doc_mut() else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        let (offset, mask) = (layer.offset, layer.mask.clone());

        let mut steps: Vec<Box<dyn cshop_core::history::Command>> =
            vec![Box::new(cshop_core::history::ReplaceLayerPixels::new(
                id,
                pixels,
                offset,
                mask,
                "Lens Correction",
            ))];

        // The crop is measured in the layer's own frame; the canvas is in the
        // document's, so it moves by the layer's offset before being used.
        let mut cropped = None;
        if let Some(r) = crop {
            let doc_rect = IRect::new(
                r.x0 + offset.0,
                r.y0 + offset.1,
                r.x1 + offset.0,
                r.y1 + offset.1,
            )
            .intersect(&view.doc.bounds());
            if !doc_rect.is_empty()
                && (doc_rect.width() != view.doc.width || doc_rect.height() != view.doc.height)
            {
                steps.push(Box::new(cshop_core::history::ResizeCanvas::new(
                    doc_rect.width(),
                    doc_rect.height(),
                    (-doc_rect.x0, -doc_rect.y0),
                )));
                cropped = Some((doc_rect.width(), doc_rect.height()));
            }
        }

        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::Compound::new("Lens Correction", steps)),
        );
        view.mark_dirty(dirty);
        if cropped.is_some() {
            view.resize_targets(&gpu);
            view.zoom_initialised = false;
        }
        view.invalidate();
        match cropped {
            Some((w, h)) => self.notify(format!("Lens correction applied, cropped to {w}x{h}")),
            None => self.notify("Lens correction applied"),
        }
    }

    pub fn poll_segment(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.segment_job else { return };
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            Err(_) => {
                self.segment_job = None;
                if let Dialog::Segment(d) = &mut self.dialog {
                    d.busy = false;
                    d.status = "The segmenter stopped without answering.".into();
                }
            }
            Ok(outcome) => {
                self.segment_job = None;
                match outcome {
                    SegmentOutcome::Found(objects) => {
                        if let Dialog::Segment(d) = &mut self.dialog {
                            d.busy = false;
                            d.status = if objects.is_empty() {
                                "Nothing recognised here — click the object instead.".into()
                            } else {
                                format!("Found {}. Pick one, or click the canvas.", objects.len())
                            };
                            d.found = objects;
                        }
                    }
                    SegmentOutcome::Ready { selection, coverage } => {
                        if let Some(view) = self.doc_mut() {
                            // Shown directly rather than through the history,
                            // so refining does not leave a step per click. It
                            // is recorded when the window is kept.
                            view.doc.selection = Some(*selection);
                            view.invalidate();
                        }
                        if let Dialog::Segment(d) = &mut self.dialog {
                            d.busy = false;
                            d.applied = true;
                            d.coverage = Some(coverage);
                            d.status = "Click again to refine, or Alt-click to exclude.".into();
                        }
                    }
                    SegmentOutcome::Failed(e) => {
                        if let Dialog::Segment(d) = &mut self.dialog {
                            d.busy = false;
                            d.status = e;
                        }
                    }
                }
            }
        }
    }

    /// Run the segmenter for whatever the window has been told, and show the
    /// result as the selection so the canvas is the preview.
    /// Run the segmenter for whatever the window has been told, on another
    /// thread, and show the result as the selection when it arrives.
    fn segment_preview(&mut self) {
        let (prompt, feather, expand) = {
            let Dialog::Segment(d) = &self.dialog else { return };
            (d.prompt(), d.feather, d.expand)
        };
        let Some(prompt) = prompt else {
            // Nothing to go on: put the selection back as it was.
            let previous = self.segment_before.clone();
            if let Some(view) = self.doc_mut() {
                view.doc.selection = previous;
                view.invalidate();
            }
            if let Dialog::Segment(d) = &mut self.dialog {
                d.applied = false;
                d.coverage = None;
            }
            return;
        };

        let (image, w, h) = match self.segment_source() {
            Ok(s) => s,
            Err(e) => return self.fail(format!("Could not prepare the image: {e}")),
        };
        if let Dialog::Segment(d) = &mut self.dialog {
            d.busy = true;
            d.status = "Segmenting…".into();
        }

        let (tx, rx) = std::sync::mpsc::channel();
        self.segment_job = Some(rx);
        std::thread::spawn(move || {
            let mask_path = image.with_file_name("mask.png");
            let outcome = crate::vision::segment(&image, &prompt, &mask_path, 0.25)
                .and_then(|r| {
                    let mask = cshop_io::load(&r.mask).map_err(|e| format!("{e}"))?;
                    if mask.width() != w || mask.height() != h {
                        return Err(format!(
                            "the mask came back {}x{}",
                            mask.width(),
                            mask.height()
                        ));
                    }
                    let mut coverage = cshop_core::mask::MaskBuffer::hide_all(w, h);
                    for y in 0..h as i32 {
                        for x in 0..w as i32 {
                            coverage.set(x, y, mask.get(x, y).r);
                        }
                    }
                    let mut selection = cshop_core::selection::Selection::from_mask(coverage);
                    // Grown first, softened second — see the window's own note.
                    if expand > 0 {
                        selection.expand(expand);
                    }
                    if feather > 0.0 {
                        selection.feather(feather);
                    }
                    if selection.is_empty() {
                        return Err("nothing there to separate — try another point".into());
                    }
                    Ok((selection, r.coverage))
                });
            if let Some(dir) = image.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
            let _ = tx.send(match outcome {
                Ok((selection, coverage)) => {
                    SegmentOutcome::Ready { selection: Box::new(selection), coverage }
                }
                Err(e) => SegmentOutcome::Failed(e),
            });
        });
    }

    // -----------------------------------------------------------------------
    // Editing a path
    // -----------------------------------------------------------------------

    /// Replace a path layer's geometry outright. For tests that need a shape
    /// the Pen tool cannot draw in a click or two.
    pub fn set_path_for_test(&mut self, id: LayerId, path: cshop_core::path::PathShape) {
        self.write_path(id, path, "Edit Path", None);
    }

    /// A fresh identifier for a run of edits that should undo as one step.
    pub fn next_edit_run(&mut self) -> u64 {
        self.edit_run += 1;
        self.edit_run
    }

    /// The path layer the Direct Selection tool is working on.
    ///
    /// Whatever is active, so selecting a layer in the panel is how you choose
    /// which path to edit — there is no separate notion of a "target path".
    pub fn editable_path(&self) -> Option<(LayerId, cshop_core::path::PathShape, Vec2)> {
        let view = self.doc()?;
        let id = view.doc.active?;
        let layer = view.doc.tree.get(id)?;
        let shape = layer.shape()?;
        let cshop_core::shape::ShapeKind::Path(path) = &shape.content().kind else {
            return None;
        };
        // The box's top-left in document space. Path points are box-local, so
        // this is what maps between the two.
        let origin = Vec2::new(
            (layer.offset.0 + shape.anchor().0) as f32,
            (layer.offset.1 + shape.anchor().1) as f32,
        );
        Some((id, path.clone(), origin))
    }

    /// The anchor or handle nearest `at`, within grabbing distance.
    ///
    /// Handles are only offered for anchors already selected, because those
    /// are the only ones drawn — a handle you cannot see is not one you meant
    /// to grab.
    pub fn path_hit(&self, at: Vec2, zoom: f32) -> Option<(AnchorRef, HandleKind)> {
        let (_, path, origin) = self.editable_path()?;
        let radius = PathEdit::GRAB_RADIUS / zoom.max(0.05);
        let mut best: Option<(f32, AnchorRef, HandleKind)> = None;
        let mut consider = |d: f32, r: AnchorRef, k: HandleKind| {
            if d <= radius && best.as_ref().is_none_or(|(bd, _, _)| d < *bd) {
                best = Some((d, r, k));
            }
        };

        for (pi, part) in path.parts.iter().enumerate() {
            for (si, sub) in part.subpaths.iter().enumerate() {
                for (ai, a) in sub.anchors.iter().enumerate() {
                    let r = (pi, si, ai);
                    let doc = |p: Vec2| Vec2::new(origin.x + p.x, origin.y + p.y);
                    // Handles first, so an anchor sitting under its own handle
                    // does not swallow the click.
                    if self.path_edit.is_selected(r) && a.at.distance(a.out_handle) > 0.5 {
                        consider(at.distance(doc(a.in_handle)), r, HandleKind::In);
                        consider(at.distance(doc(a.out_handle)), r, HandleKind::Out);
                    }
                    consider(at.distance(doc(a.at)), r, HandleKind::Anchor);
                }
            }
        }
        best.map(|(_, r, k)| (r, k))
    }

    /// Move whatever the drag has hold of by `delta`, in document pixels.
    ///
    /// `break_handle` is Alt: it lets one side of a smooth anchor move without
    /// the other, which is how a smooth point becomes a corner.
    pub fn drag_path(&mut self, delta: Vec2, break_handle: bool) {
        let Some((target, kind)) = self.path_edit.drag else { return };
        let Some((id, mut path, _)) = self.editable_path() else { return };
        if delta.x == 0.0 && delta.y == 0.0 {
            return;
        }

        // Moving an anchor moves every selected anchor, which is what makes a
        // multiple selection worth having.
        let moving: Vec<AnchorRef> = match kind {
            HandleKind::Anchor if self.path_edit.is_selected(target) => {
                self.path_edit.selected.clone()
            }
            HandleKind::Anchor => vec![target],
            _ => vec![target],
        };

        for r in moving {
            let Some(a) = path
                .parts
                .get_mut(r.0)
                .and_then(|p| p.subpaths.get_mut(r.1))
                .and_then(|s| s.anchors.get_mut(r.2))
            else {
                continue;
            };
            match kind {
                HandleKind::Anchor => {
                    a.at = a.at + delta;
                    a.in_handle = a.in_handle + delta;
                    a.out_handle = a.out_handle + delta;
                }
                HandleKind::Out => {
                    let smooth = a.is_smooth();
                    a.out_handle = a.out_handle + delta;
                    if smooth && !break_handle {
                        a.in_handle = a.at + (a.at - a.out_handle);
                    }
                }
                HandleKind::In => {
                    let smooth = a.is_smooth();
                    a.in_handle = a.in_handle + delta;
                    if smooth && !break_handle {
                        a.out_handle = a.at + (a.at - a.in_handle);
                    }
                }
            }
        }

        let run = self.path_edit.run;
        self.write_path(id, path, "Edit Path", run);
    }

    /// Delete the selected anchors.
    ///
    /// A contour left with fewer than two anchors is dropped, and a path left
    /// with nothing is deleted along with its layer — a shape layer holding no
    /// shape would render as nothing and could not be selected again.
    pub fn delete_path_anchors(&mut self) {
        if self.path_edit.selected.is_empty() {
            return;
        }
        let Some((id, mut path, _)) = self.editable_path() else { return };
        let doomed = self.path_edit.selected.clone();

        for (pi, part) in path.parts.iter_mut().enumerate() {
            for (si, sub) in part.subpaths.iter_mut().enumerate() {
                // Back to front, so the earlier indices stay valid.
                let mut keep: Vec<usize> = (0..sub.anchors.len())
                    .filter(|ai| !doomed.contains(&(pi, si, *ai)))
                    .collect();
                keep.sort_unstable();
                let kept: Vec<_> = keep.iter().map(|i| sub.anchors[*i]).collect();
                sub.anchors = kept;
            }
            part.subpaths.retain(|s| s.anchors.len() >= 2);
        }
        path.parts.retain(|p| !p.subpaths.is_empty());
        self.path_edit.selected.clear();

        if path.parts.is_empty() {
            let Some(view) = self.doc_mut() else { return };
            let dirty = view
                .history
                .apply(&mut view.doc, Box::new(cshop_core::history::DeleteLayer::new(id)));
            view.mark_dirty(dirty);
            view.invalidate();
            self.path_edit.clear();
            return;
        }
        self.write_path(id, path, "Delete Anchors", None);
    }

    /// Put an edited path back on its layer, keeping it where it looked.
    ///
    /// The path is renormalised so its bounds start at the box's corner, since
    /// dragging a point can carry it outside the box the shape was built with.
    /// The layer's offset then moves by the same amount, so the geometry does
    /// not appear to jump.
    fn write_path(
        &mut self,
        id: LayerId,
        path: cshop_core::path::PathShape,
        label: &str,
        run: Option<u64>,
    ) {
        use cshop_core::history::SetShapeContent;
        let Some(bounds) = path.bounds() else { return };
        // Whole pixels, and the same number used to move the path and to move
        // the layer. Translating the path by a fraction while rounding the
        // layer's offset loses the remainder, and on a drag — which rewrites
        // the path every frame — the loss accumulates until the geometry has
        // visibly crept.
        let shift = Vec2::new(bounds.x0.floor(), bounds.y0.floor());
        let local = path.translate(Vec2::new(-shift.x, -shift.y));
        let size = ((bounds.x1 - shift.x).max(1.0), (bounds.y1 - shift.y).max(1.0));

        let Some(view) = self.doc_mut() else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        let Some(shape) = layer.shape() else { return };
        let old_origin = (layer.offset.0 + shape.anchor().0, layer.offset.1 + shape.anchor().1);

        let next = cshop_core::shape::ShapeContent {
            kind: cshop_core::shape::ShapeKind::Path(local),
            size,
            style: shape.content().style,
        };
        let Some(rendered) = cshop_core::shape::rasterize(&next) else { return };
        // Where the box's corner has moved to, in document space.
        let new_origin = (old_origin.0 + shift.x as i32, old_origin.1 + shift.y as i32);
        let offset = (new_origin.0 - rendered.anchor.0, new_origin.1 - rendered.anchor.1);

        let mut cmd = SetShapeContent::new(id, next, offset, label);
        if let Some(run) = run {
            cmd = cmd.in_run(run);
        }
        let dirty = view.history.apply(&mut view.doc, Box::new(cmd));
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// The shape layer the options bar is editing, if any.
    fn editing_shape(&self) -> Option<LayerId> {
        if self.tool != Tool::Shape {
            return None;
        }
        let view = self.doc()?;
        let id = view.doc.active?;
        view.doc.tree.get(id)?.shape().map(|_| id)
    }

    /// Load the selected shape layer's settings into the tool.
    ///
    /// Without this the first option touched after selecting a shape would
    /// stamp the tool's fill and stroke over whatever the layer had.
    pub fn sync_shape_options(&mut self) {
        let selected = self.editing_shape();
        if selected == self.shape_synced {
            return;
        }
        self.shape_synced = selected;
        let Some(id) = selected else { return };
        let Some(view) = self.doc() else { return };
        let Some(content) = view.doc.tree.get(id).and_then(|l| l.shape()).map(|s| s.content().clone())
        else {
            return;
        };
        self.shape_kind = content.kind;
        self.shape_style = content.style;
    }

    /// Apply the tool's current geometry and style to the selected shape.
    ///
    /// Recorded through the history so it is undoable, and merged into one
    /// step per drag by the command itself.
    pub fn refresh_shape_style(&mut self) {
        let Some(id) = self.editing_shape() else { return };
        let (kind, style) = (self.shape_kind.clone(), self.shape_style);
        let Some(view) = self.doc_mut() else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        let Some(shape) = layer.shape() else { return };
        let current = shape.content().clone();

        // Keep the shape's own size and, for a line, its direction; only the
        // options the bar actually shows are pushed onto it.
        let kind = match (kind, &current.kind) {
            (
                cshop_core::shape::ShapeKind::Line { thickness, .. },
                cshop_core::shape::ShapeKind::Line { from, to, .. },
            ) => cshop_core::shape::ShapeKind::Line { thickness, from: *from, to: *to },
            (k, _) => k,
        };
        let next = cshop_core::shape::ShapeContent { kind, size: current.size, style };
        if next == current {
            return;
        }
        // Re-render first so the new anchor is known, then keep the shape's
        // corner where it was.
        let anchor = (layer.offset.0 + shape.anchor().0, layer.offset.1 + shape.anchor().1);
        let Some(rendered) = cshop_core::shape::rasterize(&next) else { return };
        let offset = (anchor.0 - rendered.anchor.0, anchor.1 - rendered.anchor.1);
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::SetShapeContent::new(id, next, offset, "Edit Shape")),
        );
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// Turn the active type or shape layer into pixels.
    fn rasterize_layer(&mut self) {
        self.commit_text();
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        if !layer.is_rendered() {
            self.notify("Only type, shape and smart layers can be rasterised");
            return;
        }
        let label = match &layer.kind {
            cshop_core::layer::LayerKind::Text(_) => "Rasterize Type",
            cshop_core::layer::LayerKind::Smart(_) => "Rasterize Smart Object",
            _ => "Rasterize Shape",
        };
        let Some(pixels) = layer.pixels().cloned() else { return };
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::RasterizeLayer::new(id, pixels, label)),
        );
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// Change the active layer's filter stack as one undoable step.
    fn edit_filters(
        &mut self,
        label: &str,
        change: impl FnOnce(&mut cshop_core::smart_filters::SmartFilters),
    ) {
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        if layer.pixels().is_none() {
            self.fail("Filters need a layer with pixels");
            return;
        }
        let mut next = layer.filters.clone();
        change(&mut next);
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::SetSmartFilters::new(id, next, label)),
        );
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// Reopen the filter window on one that is already attached, so a setting
    /// can be changed rather than the filter redone.
    fn edit_attached_filter(&mut self, index: usize) {
        let Some(view) = self.doc() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        let Some(slot) = layer.filters.slots.get(index) else { return };
        let filter = slot.filter.clone();
        // The window edits the layer's own pixels, so it previews the filter
        // on what the stack below it produced rather than on the finished
        // layer: that is the picture this slot actually sees.
        let below = cshop_core::smart_filters::SmartFilters {
            enabled: true,
            slots: layer.filters.slots[..index].to_vec(),
            mask: None,
        };
        let source = match below.render(
            layer.pixels().expect("checked when it was attached"),
            &self.filter_context(),
        ) {
            Some(px) => px,
            None => layer.pixels().expect("checked when it was attached").clone(),
        };
        let context = self.filter_context();
        self.dialog = Dialog::Filter(Box::new(crate::filter_ui::FilterDialog::editing(
            filter, source, context, index,
        )));
    }

    /// Run the stack into the pixels, so it stops being editable.
    fn apply_attached_filters(&mut self) {
        let ctx = self.filter_context();
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        let Some(px) = layer.filtered_pixels(&ctx) else {
            self.notify("There are no smart filters on this layer");
            return;
        };
        let offset = layer.offset;
        let rect = cshop_core::geom::IRect::at(offset.0, offset.1, px.width(), px.height());
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(ReplacePixels::new(id, rect, px, "Apply Smart Filters")),
        );
        view.mark_dirty(dirty);
        // The stack has been run in; leaving it attached would apply it twice.
        self.edit_filters("Apply Smart Filters", |f| {
            f.slots.clear();
            f.enabled = false;
        });
    }

    /// Wrap the active raster layer so that placing it stops costing anything.
    ///
    /// The pixels it has now become the source, so nothing changes on screen.
    /// What changes is everything afterwards: scaling, rotating and skewing
    /// are re-rendered from this picture rather than from the last rendering,
    /// so they no longer accumulate.
    fn convert_to_smart_object(&mut self) {
        self.commit_text();
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        if layer.smart().is_some() {
            self.notify("That layer is already a smart object");
            return;
        }
        let Some(pixels) = layer.pixels().cloned() else {
            let why = self.why_not("Only layers with pixels can become smart objects");
            self.fail(why);
            return;
        };
        let smart = cshop_core::smart::SmartObject::new(pixels);
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::ReplaceLayerKind::new(
                id,
                cshop_core::layer::LayerKind::Smart(Box::new(smart)),
                "Convert to Smart Object",
            )),
        );
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// Finish the stroke and record it as one undo step.
    pub fn end_stroke(&mut self) {
        if self.smudge.is_some() {
            self.end_smudge();
            return;
        }
        let Some(active) = self.stroke.take() else { return };
        let Some(index) = self.active else { return };
        let view = &mut self.docs[index];
        if active.stroke.is_empty() {
            return;
        }

        // Name the step after the tool rather than after
        // the blend mode — a clone stroke is not a brush stroke.
        // Named for what the stroke did rather than for which button was down:
        // Alt swaps dodge for burn part-way through, and the step should say
        // which one it turned out to be.
        let label = match active.stroke.mode() {
            PaintMode::Retouch(r) => match r.kind {
                cshop_core::retouch::RetouchKind::Dodge => "Dodge Tool",
                cshop_core::retouch::RetouchKind::Burn => "Burn Tool",
                cshop_core::retouch::RetouchKind::Sponge => "Sponge Tool",
            },
            PaintMode::Erase => "Eraser Tool",
            PaintMode::Paint => match active.tool {
                Tool::Pencil => "Pencil Tool",
                Tool::CloneStamp => "Clone Stamp",
                Tool::HealingBrush => "Healing Brush",
                Tool::HistoryBrush => "History Brush",
                Tool::SpotHealing => "Spot Healing",
                Tool::Blur => "Blur Tool",
                Tool::Sharpen => "Sharpen Tool",
                _ => "Brush Tool",
            },
        };

        match active.target {
            StrokeTarget::Pixels(snapshot) => {
                let Some(layer) = view.doc.tree.get(active.layer) else { return };
                let offset = layer.offset;
                let Some(pixels) = layer.pixels() else { return };
                let rect = active.stroke.bounds().intersect(&pixels.bounds());
                if rect.is_empty() {
                    return;
                }
                // The layer already shows the finished stroke. Capture it, roll
                // back to the snapshot, then let the command re-apply it, so
                // the command's own "before" capture sees the true original.
                let after = pixels.copy_rect(rect);
                let before = snapshot.copy_rect(rect);
                if let Some(px) = view.doc.tree.get_mut(active.layer).and_then(|l| l.pixels_mut()) {
                    px.paste(&before, rect.x0, rect.y0);
                }
                // A stroke that changed nothing must not become an undo step.
                // The Clone Stamp reaches this whenever its source has left
                // the canvas, and an "undo Clone Stamp" that undoes nothing is
                // worse than no entry at all.
                if after.pixels() == before.pixels() {
                    if active.tool == Tool::CloneStamp {
                        self.notify(
                            "Nothing to clone — the source is outside the image. Alt-click to set a new one.",
                        );
                    }
                    return;
                }
                let doc_rect = rect.translate(offset.0, offset.1);
                let dirty = view.history.apply(
                    &mut view.doc,
                    Box::new(ReplacePixels::new(active.layer, doc_rect, after, label)),
                );
                view.mark_dirty(dirty);
            }

            StrokeTarget::Mask(snapshot) => {
                let Some(layer) = view.doc.tree.get(active.layer) else { return };
                let Some(mask) = &layer.mask else { return };
                let offset = mask.offset;
                let rect = active.stroke.bounds().intersect(&mask.data.bounds());
                if rect.is_empty() {
                    return;
                }
                let after = mask.data.copy_rect(rect);
                let before = snapshot.copy_rect(rect);
                if let Some(m) = view.doc.tree.get_mut(active.layer).and_then(|l| l.mask.as_mut()) {
                    for y in 0..before.height() as i32 {
                        for x in 0..before.width() as i32 {
                            m.data.set(rect.x0 + x, rect.y0 + y, before.get(x, y));
                        }
                    }
                }
                let doc_rect = rect.translate(offset.0, offset.1);
                let dirty = view.history.apply(
                    &mut view.doc,
                    Box::new(ReplaceMaskPixels::new(active.layer, doc_rect, after, label)),
                );
                view.mark_dirty(dirty);
            }

            StrokeTarget::QuickMask(snapshot) => {
                // Roll back and let the command re-apply, so undo returns to
                // the selection as it stood before the stroke. Only the painted
                // area is put back, because only that was ever captured.
                let after = view.doc.selection.clone();
                let rect = active.stroke.bounds();
                if let Some(sel) = view.doc.selection.as_mut() {
                    sel.widen_to_document();
                    snapshot.restore(sel.mask_mut(), rect);
                    sel.invalidate();
                }
                let dirty = view.history.apply(
                    &mut view.doc,
                    Box::new(SetSelection::new(after.as_ref(), "Quick Mask")),
                );
                view.mark_dirty(dirty);
                view.invalidate();
            }
        }
    }

    pub fn cancel_stroke(&mut self) {
        let Some(active) = self.stroke.take() else { return };
        let Some(index) = self.active else { return };
        let view = &mut self.docs[index];
        // Only what the stroke painted is put back; nothing else was captured,
        // and nothing else changed.
        let rect = active.stroke.bounds();
        match active.target {
            StrokeTarget::Pixels(snapshot) => {
                if let Some(px) = view.doc.tree.get_mut(active.layer).and_then(|l| l.pixels_mut()) {
                    snapshot.restore(px, rect);
                }
            }
            StrokeTarget::Mask(snapshot) => {
                if let Some(m) = view.doc.tree.get_mut(active.layer).and_then(|l| l.mask.as_mut()) {
                    snapshot.restore(&mut m.data, rect);
                }
            }
            StrokeTarget::QuickMask(snapshot) => {
                if let Some(sel) = view.doc.selection.as_mut() {
                    sel.widen_to_document();
                    snapshot.restore(sel.mask_mut(), rect);
                    sel.invalidate();
                }
            }
        }
        view.invalidate();
    }

    /// Sample the composited colour under a document-space point.
    pub fn pick_color(&mut self, at: Vec2) {
        let gpu = self.gpu.clone();
        let Some(i) = self.active else { return };
        let view = &mut self.docs[i];
        if let Some(c) = view.sample_composite(&gpu, at.x as i32, at.y as i32) {
            self.foreground = c;
        }
    }
}

fn layer_local(doc_point: cshop_core::geom::Vec2, offset: (i32, i32)) -> cshop_core::geom::Vec2 {
    cshop_core::geom::Vec2::new(doc_point.x - offset.0 as f32, doc_point.y - offset.1 as f32)
}

/// Layer kinds that can be painted on.
pub fn is_paintable(kind: &LayerKind) -> bool {
    matches!(kind, LayerKind::Raster(_))
}

/// A selection gesture the user is part-way through.
pub enum SelectionDrag {
    /// Rectangular or elliptical marquee being dragged out.
    Marquee { start: Vec2, current: Vec2, ellipse: bool, mode: SelectionMode },
    /// Freehand lasso being traced.
    Lasso { points: Vec<Vec2>, mode: SelectionMode },
    /// Polygonal lasso: clicks add vertices, so this survives between drags
    /// until the shape is closed.
    Polygon { points: Vec<Vec2>, cursor: Vec2, mode: SelectionMode },
}

impl SelectionDrag {
    /// The outline to draw while the gesture is live, in document coordinates.
    pub fn preview(&self) -> Vec<Vec2> {
        match self {
            SelectionDrag::Marquee { start, current, ellipse, .. } => {
                let r = cshop_core::selection::Rectf::from_points(*start, *current);
                if *ellipse {
                    // Enough segments that the preview reads as a smooth curve.
                    let (cx, cy) = ((r.x0 + r.x1) * 0.5, (r.y0 + r.y1) * 0.5);
                    let (rx, ry) = (r.width() * 0.5, r.height() * 0.5);
                    (0..48)
                        .map(|i| {
                            let t = i as f32 / 48.0 * std::f32::consts::TAU;
                            Vec2::new(cx + rx * t.cos(), cy + ry * t.sin())
                        })
                        .collect()
                } else {
                    vec![
                        Vec2::new(r.x0, r.y0),
                        Vec2::new(r.x1, r.y0),
                        Vec2::new(r.x1, r.y1),
                        Vec2::new(r.x0, r.y1),
                    ]
                }
            }
            SelectionDrag::Lasso { points, .. } => points.clone(),
            SelectionDrag::Polygon { points, cursor, .. } => {
                // Show the segment that would be added by the next click.
                let mut p = points.clone();
                p.push(*cursor);
                p
            }
        }
    }

    pub fn mode(&self) -> SelectionMode {
        match self {
            SelectionDrag::Marquee { mode, .. }
            | SelectionDrag::Lasso { mode, .. }
            | SelectionDrag::Polygon { mode, .. } => *mode,
        }
    }

    /// Whether the shape should be closed when drawing the preview.
    pub fn is_closed(&self) -> bool {
        !matches!(self, SelectionDrag::Polygon { .. })
    }
}

impl CShopApp {
    // -----------------------------------------------------------------------
    // Selections
    // -----------------------------------------------------------------------

    /// Apply a new selection through the history, combining it with whatever
    /// is already selected according to the active boolean mode.
    fn commit_selection(&mut self, new: Selection, mode: SelectionMode, label: &'static str) {
        let Some(view) = self.doc_mut() else { return };

        let combined = match (&view.doc.selection, mode) {
            // Nothing to combine with, so any mode but Replace still has to
            // start from the new shape.
            (None, SelectionMode::Subtract) => return,
            (None, _) | (Some(_), SelectionMode::Replace) => new,
            (Some(current), _) => {
                let mut base = current.clone();
                base.combine(&new, mode);
                base
            }
        };

        let next = if combined.is_empty() { None } else { Some(&combined) };
        let dirty = view.history.apply(&mut view.doc, Box::new(SetSelection::new(next, label)));
        view.mark_dirty(dirty);
    }

    /// Build a selection from a finished gesture and commit it.
    pub fn finish_selection_drag(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        let Some(view) = self.doc() else { return };
        let (w, h) = (view.doc.width, view.doc.height);
        let antialias = self.selection_antialias;
        let feather = self.selection_feather;
        let mode = drag.mode();

        let (mut selection, label) = match &drag {
            SelectionDrag::Marquee { start, current, ellipse, .. } => {
                let rect = cshop_core::selection::Rectf::from_points(*start, *current);
                if rect.width() < 0.5 || rect.height() < 0.5 {
                    // A click with no drag clears the selection.
                    self.push(Action::Deselect);
                    return;
                }
                if *ellipse {
                    (
                        Selection::from_ellipse(w, h, rect, antialias),
                        "Elliptical Marquee",
                    )
                } else {
                    (Selection::from_rect(w, h, rect, antialias), "Rectangular Marquee")
                }
            }
            SelectionDrag::Lasso { points, .. } => {
                if points.len() < 3 {
                    self.push(Action::Deselect);
                    return;
                }
                (Selection::from_polygon(w, h, points, antialias), "Lasso")
            }
            SelectionDrag::Polygon { points, .. } => {
                if points.len() < 3 {
                    return;
                }
                (Selection::from_polygon(w, h, points, antialias), "Polygonal Lasso")
            }
        };

        if feather > 0.0 {
            selection.feather(feather);
        }
        self.commit_selection(selection, mode, label);
    }

    /// The image the wand and Grow/Similar sample.
    fn wand_source(&mut self) -> Option<PixelBuffer> {
        let gpu = self.gpu.clone();
        let index = self.active?;
        if self.sample_all_layers {
            let view = &mut self.docs[index];
            view.sync_composite_only(&gpu, &mut self.compositor);
            return Some(view.read_composite(&gpu));
        }
        let view = &self.docs[index];
        let id = view.doc.active?;
        let layer = view.doc.tree.get(id)?;
        let pixels = layer.pixels()?;

        // The wand works in document space, so a layer that is offset or
        // smaller than the canvas has to be placed into a full-size buffer
        // first — otherwise every coordinate would be off by the offset.
        if layer.offset == (0, 0)
            && pixels.width() == view.doc.width
            && pixels.height() == view.doc.height
        {
            return Some(pixels.clone());
        }
        let mut full = PixelBuffer::new(view.doc.width, view.doc.height);
        full.paste(pixels, layer.offset.0, layer.offset.1);
        Some(full)
    }

    /// Magic Wand click at a document position.
    pub fn magic_wand_at(&mut self, at: Vec2, mode: SelectionMode) {
        let Some(source) = self.wand_source() else { return };
        let options = self.wand;
        let mut selection = wand::magic_wand(&source, at.x as i32, at.y as i32, options);
        if selection.is_empty() {
            self.notify("Nothing matched at that point");
            return;
        }
        if self.selection_feather > 0.0 {
            selection.feather(self.selection_feather);
        }
        self.commit_selection(selection, mode, "Magic Wand");
    }

    fn modify_selection(&mut self, op: ModifySelection) {
        let label = op.label();
        let Some(view) = self.doc_mut() else { return };
        let Some(current) = &view.doc.selection else {
            self.notify("Nothing is selected");
            return;
        };

        let mut next = current.clone();
        match op {
            ModifySelection::Feather(r) => next.feather(r),
            ModifySelection::Expand(n) => next.expand(n),
            ModifySelection::Contract(n) => next.contract(n),
            ModifySelection::Border(n) => next.border(n),
            ModifySelection::Smooth(n) => next.smooth(n),
        }

        let view = self.doc_mut().expect("checked above");
        let value = if next.is_empty() { None } else { Some(&next) };
        let dirty = view.history.apply(&mut view.doc, Box::new(SetSelection::new(value, label)));
        view.mark_dirty(dirty);
        if next.is_empty() {
            self.notify("The selection became empty");
        }
    }

    fn grow_or_similar(&mut self, similar: bool) {
        let Some(source) = self.wand_source() else { return };
        let options = self.wand;
        let Some(view) = self.doc() else { return };
        let Some(current) = &view.doc.selection else {
            self.notify("Nothing is selected");
            return;
        };

        let next = if similar {
            wand::similar(&source, current, options)
        } else {
            wand::grow(&source, current, options)
        };
        let label = if similar { "Similar" } else { "Grow" };
        self.commit_selection(next, SelectionMode::Replace, label);
    }

    // -----------------------------------------------------------------------
    // Masks
    // -----------------------------------------------------------------------

    /// Mask a layer by how far away everything in it is.
    ///
    /// The same depth the Relight window reads, put to the other obvious use:
    /// near reveals and far hides, so an adjustment clipped to it lands on the
    /// subject and leaves the background alone. Inverted, it builds with
    /// distance instead, which is what haze does.
    fn start_depth_mask(&mut self, invert: bool) {
        if self.depth_job.is_some() {
            self.notify("Already working out the depth");
            return;
        }
        let picked = self.doc().and_then(|v| {
            let id = v.doc.active?;
            let layer = v.doc.tree.get(id)?;
            if layer.mask.is_some() {
                return None;
            }
            Some((id, layer.pixels()?.clone()))
        });
        let Some((id, pixels)) = picked else {
            let has_mask = self.doc().is_some_and(|d| d.doc.active_has_mask());
            if has_mask {
                self.notify("That layer already has a mask");
            } else {
                self.fail("Masking by depth needs a layer with pixels in it");
            }
            return;
        };
        if !crate::vision::is_available() {
            self.fail(crate::vision::NOT_INSTALLED);
            return;
        }
        self.depth_for_mask = Some((id, invert));
        self.notify("Working out the depth…");
        self.start_depth(pixels);
    }

    /// Put a worked-out depth onto a layer as its mask.
    fn finish_depth_mask(&mut self, id: LayerId, invert: bool, map: cshop_core::relight::DepthMap) {
        let Some(view) = self.doc_mut() else { return };
        if view.doc.tree.get(id).is_none_or(|l| l.mask.is_some()) {
            self.notify("That layer already has a mask");
            return;
        }
        let offset = view.doc.tree.get(id).map(|l| l.offset).unwrap_or((0, 0));
        let mask = cshop_core::layer::LayerMask {
            data: cshop_core::relight::to_mask(&map, invert),
            offset,
            enabled: true,
            linked: true, path: None,
        };
        let dirty = view
            .history
            .apply(&mut view.doc, Box::new(AddLayerMask::new(id, mask, "Mask from Depth")));
        view.mark_dirty(dirty);
        view.doc.edit_target = EditTarget::Mask;
        view.invalidate();
        self.notify(if invert { "Masked by distance" } else { "Masked by nearness" });
    }

    /// Turn the active layer into a mask on the one below it.
    ///
    /// The layer is consumed, because that is what the operation means: a
    /// greyscale layer sitting above a picture is a mask that has not been
    /// attached yet. Its tone becomes coverage, weighted by its own alpha, and
    /// both halves are one history entry — a document left with the mask
    /// attached and the layer still there would be showing it twice.
    fn layer_to_mask(&mut self) {
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(pixels) = view.doc.tree.get(id).and_then(|l| l.pixels().cloned()) else {
            self.fail("That layer has no pixels to make a mask from");
            return;
        };
        // The layer below, among its own siblings.
        let below = view.doc.tree.position(id).and_then(|p| {
            let siblings = match p.parent {
                Some(parent) => view.doc.tree.get(parent)?.children().to_vec(),
                None => view.doc.tree.root().to_vec(),
            };
            (p.index > 0).then(|| siblings.get(p.index - 1).copied()).flatten()
        });
        let Some(target) = below else {
            self.fail("There is no layer underneath to put the mask on");
            return;
        };
        if view.doc.tree.get(target).is_none_or(|l| l.mask.is_some()) {
            self.fail("The layer underneath already has a mask");
            return;
        }

        let offset = view.doc.tree.get(id).map(|l| l.offset).unwrap_or((0, 0));
        let mask = cshop_core::layer::LayerMask {
            data: cshop_core::mask::MaskBuffer::from_luminance(&pixels),
            offset,
            enabled: true,
            linked: true, path: None,
        };
        let steps: Vec<Box<dyn cshop_core::history::Command>> = vec![
            Box::new(AddLayerMask::new(target, mask, "Layer to Mask")),
            Box::new(cshop_core::history::DeleteLayer::new(id)),
        ];
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::Compound::new("Layer to Mask", steps)),
        );
        view.mark_dirty(dirty);
        view.doc.active = Some(target);
        view.doc.selected_layers = vec![target];
        view.doc.edit_target = EditTarget::Mask;
        view.invalidate();
        self.notify("Layer became a mask");
    }

    /// Load the active layer's mask as the selection.
    fn selection_from_mask(&mut self) {
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(mask) = view.doc.tree.get(id).and_then(|l| l.mask.as_ref()) else {
            self.fail("That layer has no mask to make a selection from");
            return;
        };
        // The mask sits in the layer's frame and a selection in the
        // document's, so it is placed rather than simply copied.
        let (w, h) = (view.doc.width, view.doc.height);
        let mut placed = cshop_core::mask::MaskBuffer::hide_all(w, h);
        placed.paste(&mask.data, mask.offset.0, mask.offset.1);
        let selection = cshop_core::selection::Selection::from_mask(placed);
        let empty = selection.is_empty();
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::SetSelection::new(
                Some(&selection),
                "Selection from Mask",
            )),
        );
        view.mark_dirty(dirty);
        view.invalidate();
        if empty {
            self.notify("That mask hides everything, so the selection is empty");
        } else {
            self.notify("Selection from mask");
        }
    }

    fn add_layer_mask(&mut self, from_selection: bool, invert: bool, hide_all: bool) {
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        if view.doc.tree.get(id).is_none_or(|l| l.mask.is_some()) {
            self.notify("That layer already has a mask");
            return;
        }
        let (w, h) = (view.doc.width, view.doc.height);

        let data = if from_selection {
            let Some(selection) = &view.doc.selection else {
                self.notify("Nothing is selected");
                return;
            };
            let mut data = selection.to_mask();
            if invert {
                data.invert();
            }
            data
        } else if hide_all {
            MaskBuffer::hide_all(w, h)
        } else {
            MaskBuffer::reveal_all(w, h)
        };

        let mask =
            LayerMask { data, offset: (0, 0), enabled: true, linked: true, path: None };
        let dirty = view
            .history
            .apply(&mut view.doc, Box::new(AddLayerMask::new(id, mask, "Add Layer Mask")));
        view.mark_dirty(dirty);
        // A new mask is what the user wants to edit next.
        view.doc.edit_target = EditTarget::Mask;
        view.invalidate();
    }

    /// The picture a selection should be made from: the composite, since a
    /// selection belongs to the document rather than to one layer.
    fn selection_source(&mut self) -> Option<PixelBuffer> {
        let gpu = self.gpu.clone();
        let i = self.active?;
        Some(self.render_composite(&gpu, i))
    }

    fn show_color_range(&mut self) {
        let Some(source) = self.selection_source() else {
            self.notify("Open a document first");
            return;
        };
        self.dialog =
            Dialog::ColorRange(Box::new(crate::select_ui::ColorRangeDialog::new(source)));
    }

    fn apply_color_range(&mut self, range: cshop_core::color_range::ColorRange) {
        let Some(source) = self.selection_source() else { return };
        let mask = range.matte(&source);
        let Some(view) = self.doc_mut() else { return };
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::SetSelection::new(
                Some(&cshop_core::selection::Selection::from_mask(mask)),
                "Colour Range",
            )),
        );
        view.mark_dirty(dirty);
        view.invalidate();
    }

    fn show_refine_edge(&mut self) {
        let Some(mask) = self.doc().and_then(|v| v.doc.selection.as_ref().map(|s| s.to_mask()))
        else {
            self.notify("Refine Edge works on a selection; make one first");
            return;
        };
        let Some(source) = self.selection_source() else { return };
        self.dialog = Dialog::RefineEdge(Box::new(crate::select_ui::RefineEdgeDialog::new(
            source, &mask,
        )));
    }

    fn apply_refine_edge(&mut self, settings: cshop_core::refine::RefineEdge) {
        let Some(mask) = self.doc().and_then(|v| v.doc.selection.as_ref().map(|s| s.to_mask()))
        else {
            return;
        };
        let Some(source) = self.selection_source() else { return };
        let refined = settings.apply(&mask, &source);
        let Some(view) = self.doc_mut() else { return };
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::SetSelection::new(
                Some(&cshop_core::selection::Selection::from_mask(refined)),
                "Refine Edge",
            )),
        );
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// The path being drawn becomes the selection.
    fn selection_from_path(&mut self) {
        let Some(draft) = self.pen.take() else {
            self.notify("Draw a path with the Pen tool first");
            return;
        };
        if draft.anchors.len() < 3 {
            self.notify("A selection needs a closed path, so at least three points");
            self.pen = Some(draft);
            return;
        }
        let path = draft.to_path(true);
        let Some(view) = self.doc_mut() else { return };
        let (w, h) = (view.doc.width, view.doc.height);
        let mask = cshop_core::layer::mask_from_path(&path, w, h, false);
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::SetSelection::new(
                Some(&cshop_core::selection::Selection::from_mask(mask)),
                "Selection from Path",
            )),
        );
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// The selection becomes a path, as a shape layer that can be edited with
    /// the Pen like any other.
    fn path_from_selection(&mut self) {
        let Some(view) = self.doc_mut() else { return };
        let Some(selection) = view.doc.selection.clone() else {
            self.notify("Nothing is selected");
            return;
        };
        let mut selection = selection;
        // A pixel of tolerance: closer than that mostly preserves the
        // aliasing, which is not something anyone wants handles on.
        let path = cshop_core::trace::path_from_selection(&mut selection, 1.0);
        if path.is_empty() {
            self.notify("That selection has no outline to trace");
            return;
        }
        let mut style = self.shape_style;
        // An outline, not a filled shape: what is wanted is the boundary.
        style.stroke = style.stroke.or(Some(self.foreground));
        style.fill = None;
        self.add_path_layer(path, style, "Path from Selection");
    }

    /// Change the document's saved states as one undoable step.
    ///
    /// The closure is handed the document and the list, and returns the state
    /// to switch to afterwards, if any.
    fn edit_states(
        &mut self,
        label: &str,
        change: impl FnOnce(
            &cshop_core::document::Document,
            &mut Vec<cshop_core::states::LayerState>,
        ) -> Option<usize>,
    ) {
        let Some(view) = self.doc_mut() else { return };
        let mut next = view.doc.states.clone();
        let show = change(&view.doc, &mut next);
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::SetLayerStates::new(next, show, label)),
        );
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// Turn the path being drawn into a mask rather than a shape layer.
    ///
    /// The path is kept alongside the coverage it produces, so resizing the
    /// document draws it again at the new size instead of resampling the last
    /// drawing of it — which is the whole difference between a vector mask and
    /// a painted one, and the only reason to have both.
    fn add_vector_mask(&mut self, invert: bool) {
        let Some(draft) = self.pen.take() else {
            self.notify(
                "Draw a path with the Pen tool first; a vector mask is that path, kept.",
            );
            return;
        };
        if draft.anchors.len() < 3 {
            self.notify("A mask needs a closed path, so at least three points");
            self.pen = Some(draft);
            return;
        }
        let path = draft.to_path(true);
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        if view.doc.tree.get(id).is_none_or(|l| l.mask.is_some()) {
            self.notify("That layer already has a mask");
            return;
        }
        let (w, h) = (view.doc.width, view.doc.height);
        let mask = LayerMask::from_path(path, w, h, invert);
        let dirty = view
            .history
            .apply(&mut view.doc, Box::new(AddLayerMask::new(id, mask, "Add Vector Mask")));
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// Insert an adjustment layer above the active layer.
    ///
    /// When there is a selection it becomes the new layer's mask, which is what
    /// saves an obvious extra step.
    fn add_adjustment_layer(&mut self, adjustment: Adjustment) {
        let Some(view) = self.doc_mut() else { return };
        let id = view.doc.tree.alloc_id();
        let mut layer = Layer::adjustment(id, adjustment);

        if let Some(selection) = &view.doc.selection {
            layer.mask = Some(LayerMask {
                data: selection.to_mask(),
                offset: (0, 0),
                enabled: true,
                linked: true, path: None,
            });
        }

        let pos = view
            .doc
            .active
            .and_then(|a| view.doc.tree.position(a))
            .map(|p| LayerPos { parent: p.parent, index: p.index + 1 })
            .unwrap_or(LayerPos { parent: None, index: view.doc.tree.root().len() });

        let dirty = view
            .history
            .apply(&mut view.doc, Box::new(AddLayer::new(layer, pos, "New Adjustment Layer")));
        view.mark_dirty(dirty);
        view.invalidate();
    }

    /// Open the settings dialog for a destructive adjustment.
    ///
    /// Most adjustments are neutral at their defaults, so applying one straight
    /// from the menu would appear to do nothing at all.
    fn show_adjustment_dialog(&mut self, adjustment: Adjustment) {
        if !adjustment.has_settings() {
            self.apply_adjustment(adjustment);
            return;
        }
        let Some((id, rect, offset)) = self.filter_region() else {
            let why = self.why_not("Adjustments apply to raster layers; use an adjustment layer instead");
            self.fail(why);
            return;
        };
        let Some(view) = self.doc() else { return };
        let Some(px) = view.doc.tree.get(id).and_then(|l| l.pixels()) else { return };

        let source = px.copy_rect(rect.translate(-offset.0, -offset.1));
        self.dialog = Dialog::Adjustment(Box::new(crate::adjust_ui::AdjustmentDialog::new(
            adjustment, &source,
        )));
    }

    /// Apply an adjustment destructively to the active layer's pixels,
    /// restricted to the selection if there is one.
    fn apply_adjustment(&mut self, adjustment: Adjustment) {
        let label = adjustment.name().to_string();
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        if layer.locks.blocks_pixels() {
            self.fail("The layer is locked");
            return;
        }
        let offset = layer.offset;
        let Some(px) = layer.pixels() else {
            let why = self.why_not("Adjustments apply to raster layers; use an adjustment layer instead");
            self.fail(why);
            return;
        };

        let layer_rect = IRect::at(offset.0, offset.1, px.width(), px.height());
        let rect = match &view.doc.selection {
            Some(s) => layer_rect.intersect(&s.bounds()),
            None => layer_rect,
        };
        if rect.is_empty() {
            self.notify("The selection does not overlap this layer");
            return;
        }

        let mut patch = px.copy_rect(rect.translate(-offset.0, -offset.1));
        // Bake the table once for the whole patch, not once per pixel.
        let prepared = adjustment.prepare();
        for y in 0..patch.height() as i32 {
            for x in 0..patch.width() as i32 {
                let coverage = view.doc.selection_coverage(rect.x0 + x, rect.y0 + y);
                if coverage <= 0.0 {
                    continue;
                }
                let before = patch.get(x, y).to_f32();
                let after = prepared.apply(before);
                // A partially selected pixel gets a proportional share of the
                // change, so a feathered edge stays smooth.
                let mix = |a: f32, b: f32| a + (b - a) * coverage;
                patch.set(
                    x,
                    y,
                    cshop_core::color::Rgba::new(
                        mix(before.r, after.r),
                        mix(before.g, after.g),
                        mix(before.b, after.b),
                        before.a,
                    )
                    .to_u8(),
                );
            }
        }

        let dirty = view
            .history
            .apply(&mut view.doc, Box::new(ReplacePixels::new(id, rect, patch, label)));
        view.mark_dirty(dirty);
    }

    fn remove_layer_mask(&mut self, apply: bool) {
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        if view.doc.tree.get(id).is_none_or(|l| l.mask.is_none()) {
            self.notify("That layer has no mask");
            return;
        }
        let dirty = view.history.apply(&mut view.doc, Box::new(RemoveLayerMask::new(id, apply)));
        view.mark_dirty(dirty);
        view.doc.edit_target = EditTarget::Pixels;
        view.invalidate();
    }
}

impl CShopApp {
    // -----------------------------------------------------------------------
    // Transform and crop
    // -----------------------------------------------------------------------

    fn begin_transform(&mut self) {
        if self.transform.is_some() {
            return;
        }
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        if layer.locks.blocks_move() {
            self.fail("The layer is locked");
            return;
        }
        let Some(px) = layer.pixels() else {
            let why = self.why_not("Free Transform works on raster layers");
            self.fail(why);
            return;
        };
        // Transforming resamples pixels, so a vector layer stops being one.
        // A vector-preserving transform would be better; say what happens here
        // rather than letting the layer quietly change kind.
        let vector = layer.is_vector();

        let active = ActiveTransform::begin(id, px.clone(), layer.offset, layer.mask.clone());
        // Hide the real layer while its preview stands in for it, so the
        // untransformed pixels do not show through underneath.
        if let Some(layer) = view.doc.tree.get_mut(id) {
            layer.visible = false;
        }
        view.invalidate();
        self.transform = Some(active);
        if vector {
            self.notify("Free Transform — applying it rasterises this layer");
        } else {
            self.notify("Free Transform — Enter to apply, Esc to cancel");
        }
    }

    fn commit_transform(&mut self) {
        let Some(active) = self.transform.take() else { return };
        let gpu = self.gpu.clone();
        let Some(view) = self.doc_mut() else { return };

        // Whatever happens, the layer becomes visible again.
        if let Some(layer) = view.doc.tree.get_mut(active.layer) {
            layer.visible = true;
        }
        if !active.modified {
            view.invalidate();
            return;
        }

        let canvas = view.doc.bounds();
        let Some((pixels, offset)) = active.render(canvas) else {
            self.fail("That transform collapses the layer");
            if let Some(view) = self.doc_mut() {
                view.invalidate();
            }
            return;
        };

        // A linked mask follows the same transform, so the two stay registered.
        let mask = active.source_mask.as_ref().and_then(|m| {
            if !m.linked {
                return Some(m.clone());
            }
            let matrix = active.matrix()?;
            let as_pixels = mask_to_pixels(&m.data);
            let (moved, moved_offset) = cshop_core::resample::transform(
                &as_pixels,
                m.offset,
                matrix,
                active.filter,
                Some(canvas.inflate(canvas.width().max(canvas.height()) as i32)),
            )?;
            Some(cshop_core::layer::LayerMask {
                data: pixels_to_mask(&moved),
                offset: moved_offset,
                enabled: m.enabled,
                linked: m.linked, path: None,
            })
        });

        // A smart object is not resampled. The transform composes onto its
        // placement and the picture is re-rendered from the source, so a
        // twentieth transform is exactly as good as the first — which is the
        // entire reason the kind exists.
        if let Some((placement, moved)) = self.smart_placement_after(&active) {
            let Some(view) = self.doc_mut() else { return };
            let dirty = view.history.apply(
                &mut view.doc,
                Box::new(cshop_core::history::PlaceSmart::new(
                    active.layer,
                    placement,
                    moved,
                    mask,
                    active.filter,
                    "Free Transform",
                )),
            );
            view.mark_dirty(dirty);
            view.invalidate();
            return;
        }

        let Some(view) = self.doc_mut() else { return };
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(ReplaceLayerPixels::new(
                active.layer,
                pixels,
                offset,
                mask,
                "Free Transform",
            )),
        );
        view.mark_dirty(dirty);
        view.invalidate();
        let _ = gpu;
    }

    /// Where a smart object's placement and offset end up after a transform,
    /// or `None` when the layer is not one.
    ///
    /// The layer's own offset carries translation, so the placement holds only
    /// the linear part: the new one is the transform's linear part applied to
    /// the old. The offset then has to be worked out so the source's centre
    /// lands where the transform sends it, because re-rendering at a new
    /// placement changes the size of the raster and so where its top-left is.
    fn smart_placement_after(
        &self,
        active: &crate::transform_tool::ActiveTransform,
    ) -> Option<(cshop_core::transform::Transform, (i32, i32))> {
        use cshop_core::geom::Vec2;
        use cshop_core::transform::Transform;

        let view = self.doc()?;
        let layer = view.doc.tree.get(active.layer)?;
        let smart = layer.smart()?;
        let m = active.matrix()?;
        let old = smart.placement();

        let mut next = Transform::IDENTITY;
        for i in 0..2 {
            for j in 0..2 {
                next.m[i][j] = (0..2).map(|k| m.m[i][k] * old.m[k][j]).sum();
            }
        }

        // Where the source's centre is now, and where the transform sends it.
        let (sw, sh) = smart.source_size();
        let centre = Vec2::new(sw as f32 / 2.0, sh as f32 / 2.0);
        let raster = layer.pixels()?;
        let here = Vec2::new(
            layer.offset.0 as f32 + raster.width() as f32 / 2.0,
            layer.offset.1 as f32 + raster.height() as f32 / 2.0,
        );
        let there = m.apply(here);

        // The new raster's size, which decides where its top-left goes.
        let mut probe = smart.clone();
        probe.place(next, active.filter);
        let (nw, nh) = (probe.raster().width() as f32, probe.raster().height() as f32);
        let _ = centre;
        Some((
            next,
            ((there.x - nw / 2.0).round() as i32, (there.y - nh / 2.0).round() as i32),
        ))
    }

    /// Rotate or flip the active layer by a fixed amount.
    fn transform_preset(&mut self, preset: TransformPreset) {
        use cshop_core::transform::Transform;
        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        if layer.locks.blocks_move() {
            self.fail("The layer is locked");
            return;
        }
        let offset = layer.offset;
        let Some(px) = layer.pixels() else { return };

        let rect = IRect::at(offset.0, offset.1, px.width(), px.height());
        let centre = Vec2::new(
            rect.x0 as f32 + rect.width() as f32 / 2.0,
            rect.y0 as f32 + rect.height() as f32 / 2.0,
        );
        let quarter = std::f32::consts::FRAC_PI_2;
        let inner = match preset {
            TransformPreset::Rotate90Cw => Transform::rotate(quarter),
            TransformPreset::Rotate90Ccw => Transform::rotate(-quarter),
            TransformPreset::Rotate180 => Transform::rotate(std::f32::consts::PI),
            TransformPreset::FlipHorizontal => Transform::scale(-1.0, 1.0),
            TransformPreset::FlipVertical => Transform::scale(1.0, -1.0),
        };
        let matrix = Transform::about(centre, inner);

        // Right angles and flips are exact, so nearest neighbour keeps them
        // lossless — a smooth filter would blur an image that simply turned.
        let Some((pixels, new_offset)) = cshop_core::resample::transform(
            px,
            offset,
            matrix,
            cshop_core::resample::Resampling::Nearest,
            None,
        ) else {
            return;
        };

        let mask = layer.mask.as_ref().map(|m| {
            let moved = cshop_core::resample::transform(
                &mask_to_pixels(&m.data),
                m.offset,
                matrix,
                cshop_core::resample::Resampling::Nearest,
                None,
            );
            match moved {
                Some((buf, off)) => cshop_core::layer::LayerMask {
                    data: pixels_to_mask(&buf),
                    offset: off,
                    enabled: m.enabled,
                    linked: m.linked, path: None,
                },
                None => m.clone(),
            }
        });

        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(ReplaceLayerPixels::new(id, pixels, new_offset, mask, preset.name())),
        );
        view.mark_dirty(dirty);
        view.invalidate();
    }

    fn commit_crop(&mut self) {
        let Some(crop) = self.crop.take() else { return };
        let rect = crop.rect;
        if rect.is_empty() {
            return;
        }
        let gpu = self.gpu.clone();
        let Some(view) = self.doc_mut() else { return };

        // Cropping is a canvas resize that also moves the origin, so it reuses
        // the same command and stays a single undo step.
        let shift = (-rect.x0, -rect.y0);
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(ResizeCanvas::new(rect.width(), rect.height(), shift)),
        );
        view.mark_dirty(dirty);
        view.resize_targets(&gpu);
        view.invalidate();
        view.zoom_initialised = false;
    }
}

/// A coverage mask viewed as an image, so the resampler can transform it.
fn mask_to_pixels(mask: &cshop_core::mask::MaskBuffer) -> PixelBuffer {
    let mut out = PixelBuffer::new(mask.width(), mask.height());
    for y in 0..mask.height() as i32 {
        for x in 0..mask.width() as i32 {
            let v = mask.get(x, y);
            // Carried in alpha, so the resampler's premultiplied filtering
            // treats hidden areas as absent rather than as black.
            out.set(x, y, Rgba8::new(255, 255, 255, v));
        }
    }
    out
}

fn pixels_to_mask(px: &PixelBuffer) -> cshop_core::mask::MaskBuffer {
    let mut out = cshop_core::mask::MaskBuffer::hide_all(px.width(), px.height());
    for y in 0..px.height() as i32 {
        for x in 0..px.width() as i32 {
            out.set(x, y, px.get(x, y).a);
        }
    }
    out
}

impl CShopApp {
    // -----------------------------------------------------------------------
    // Filters
    // -----------------------------------------------------------------------

    /// The region a filter would act on: the selection's bounds clipped to the
    /// active layer, or the whole layer when nothing is selected.
    fn filter_region(&self) -> Option<(cshop_core::layer::LayerId, IRect, (i32, i32))> {
        let view = self.doc()?;
        let id = view.doc.active?;
        let layer = view.doc.tree.get(id)?;
        let px = layer.pixels()?;
        let bounds = IRect::at(layer.offset.0, layer.offset.1, px.width(), px.height());
        let rect = match &view.doc.selection {
            Some(s) => bounds.intersect(&s.bounds()),
            None => bounds,
        };
        (!rect.is_empty()).then_some((id, rect, layer.offset))
    }

    fn show_filter_dialog(&mut self, filter: cshop_core::filters::Filter) {
        // A filter with nothing to configure goes straight through; showing an
        // empty dialog with only an OK button is pure friction.
        if !filter.has_settings() {
            self.apply_filter(filter);
            return;
        }
        let Some((id, rect, offset)) = self.filter_region() else {
            let why = self.why_not("Filters apply to raster layers");
            self.fail(why);
            return;
        };
        let Some(view) = self.doc() else { return };
        let Some(px) = view.doc.tree.get(id).and_then(|l| l.pixels()) else { return };

        let source = px.copy_rect(rect.translate(-offset.0, -offset.1));
        let context = self.filter_context();
        self.dialog = Dialog::Filter(Box::new(crate::filter_ui::FilterDialog::new(
            filter, source, context,
        )));
    }

    /// The colours a filter that draws with them should use.
    fn filter_context(&self) -> cshop_core::filters::FilterContext {
        cshop_core::filters::FilterContext {
            foreground: self.foreground,
            background: self.background,
        }
    }

    fn apply_filter(&mut self, filter: cshop_core::filters::Filter) {
        let label = filter.name().to_string();
        let context = self.filter_context();
        let Some((id, rect, offset)) = self.filter_region() else {
            let why = self.why_not("Filters apply to raster layers");
            self.fail(why);
            return;
        };
        if self.doc().is_some_and(|v| {
            v.doc.tree.get(id).is_some_and(|l| l.locks.blocks_pixels())
        }) {
            self.fail("The layer is locked");
            return;
        }

        let Some(view) = self.doc_mut() else { return };
        let Some(px) = view.doc.tree.get(id).and_then(|l| l.pixels()) else { return };

        let local = rect.translate(-offset.0, -offset.1);
        let before = px.copy_rect(local);
        let mut after = filter.apply(&before, &context);

        // Blend by selection coverage, so a feathered selection fades the
        // filter in rather than cutting it off at a hard edge.
        if view.doc.has_selection() {
            for y in 0..after.height() as i32 {
                for x in 0..after.width() as i32 {
                    let coverage = view.doc.selection_coverage(rect.x0 + x, rect.y0 + y);
                    if coverage >= 1.0 {
                        continue;
                    }
                    let a = before.get(x, y).to_f32();
                    let b = after.get(x, y).to_f32();
                    let mix = |p: f32, q: f32| p + (q - p) * coverage;
                    after.set(
                        x,
                        y,
                        cshop_core::color::Rgba::new(
                            mix(a.r, b.r),
                            mix(a.g, b.g),
                            mix(a.b, b.b),
                            mix(a.a, b.a),
                        )
                        .to_u8(),
                    );
                }
            }
        }

        let dirty = view
            .history
            .apply(&mut view.doc, Box::new(ReplacePixels::new(id, rect, after, label)));
        view.mark_dirty(dirty);
        self.last_filter = Some(filter);
    }
}

impl CShopApp {
    // -----------------------------------------------------------------------
    // Paint Bucket
    // -----------------------------------------------------------------------

    /// Flood-fill from a document point with the foreground colour.
    pub fn bucket_fill_at(&mut self, at: Vec2) {
        let options = self.bucket;
        let colour = self.foreground;
        let Some(source) = self.sample_source(options.sample_all_layers) else {
            let why = self.why_not("The Paint Bucket works on raster layers");
            self.fail(why);
            return;
        };

        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        if layer.locks.blocks_pixels() {
            self.fail("The layer is locked");
            return;
        }
        let offset = layer.offset;
        let Some(px) = layer.pixels() else { return };

        let coverage =
            cshop_core::fill::bucket_coverage(&source, at.x as i32, at.y as i32, options);
        // The wand already tracked what it covered; asking the mask again
        // would mean scanning the whole document to rediscover it.
        let region = coverage.bounds();
        if region.is_empty() {
            self.notify("Nothing matched at that point");
            return;
        }

        // Only the matched area, clipped to the layer and the selection.
        let layer_rect = IRect::at(offset.0, offset.1, px.width(), px.height());
        let mut rect = region.intersect(&layer_rect);
        if let Some(selection) = &view.doc.selection {
            rect = rect.intersect(&selection.bounds());
        }
        if rect.is_empty() {
            self.notify("The fill falls outside the layer or the selection");
            return;
        }

        let mut patch = px.copy_rect(rect.translate(-offset.0, -offset.1));
        let preserve = layer.locks.transparency;
        let selection = view.doc.selection.as_ref();
        let opacity = options.opacity.clamp(0.0, 1.0);
        let mode = options.mode;

        // A row at a time across every core. Filling the background of a large
        // picture is one of the few operations that really does touch every
        // pixel, so it is worth spreading.
        {
            use rayon::prelude::*;
            let width = patch.width() as usize;
            patch.pixels_mut().par_chunks_mut(width).enumerate().for_each(|(y, row)| {
                let dy = rect.y0 + y as i32;
                for (x, slot) in row.iter_mut().enumerate() {
                    let dx = rect.x0 + x as i32;
                    let selected =
                        selection.map_or(1.0, |s| s.coverage(dx, dy) as f32 / 255.0);
                    let mut amount =
                        coverage.coverage(dx, dy) as f32 / 255.0 * selected * opacity;
                    if amount <= 0.0 {
                        continue;
                    }
                    let existing = *slot;
                    if preserve {
                        if existing.a == 0 {
                            continue;
                        }
                        amount *= existing.a as f32 / 255.0;
                    }
                    let out = cshop_core::blend::composite(
                        mode,
                        existing.to_f32(),
                        colour.to_f32(),
                        amount,
                    );
                    let out = if preserve {
                        cshop_core::color::Rgba { a: existing.a as f32 / 255.0, ..out }
                    } else {
                        out
                    };
                    *slot = out.to_u8();
                }
            });
        }

        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(ReplacePixels::new(id, rect, patch, "Paint Bucket")),
        );
        view.mark_dirty(dirty);
    }

    // -----------------------------------------------------------------------
    // Gradient
    // -----------------------------------------------------------------------

    /// Lay down the gradient currently being dragged.
    pub fn commit_gradient(&mut self) {
        let Some((from, to)) = self.gradient_drag.take() else { return };
        if from.distance(to) < 1.0 {
            return;
        }
        let gradient = self.gradient.clone();

        let Some(view) = self.doc_mut() else { return };
        let Some(id) = view.doc.active else { return };
        let Some(layer) = view.doc.tree.get(id) else { return };
        if layer.locks.blocks_pixels() {
            self.fail("The layer is locked");
            return;
        }
        let offset = layer.offset;
        let preserve = layer.locks.transparency;
        let Some(px) = layer.pixels() else {
            let why = self.why_not("The Gradient tool works on raster layers");
            self.fail(why);
            return;
        };

        // A gradient fills the selection, or the whole layer without one.
        let layer_rect = IRect::at(offset.0, offset.1, px.width(), px.height());
        let rect = match &view.doc.selection {
            Some(s) => layer_rect.intersect(&s.bounds()),
            None => layer_rect,
        };
        if rect.is_empty() {
            self.notify("The selection does not overlap this layer");
            return;
        }

        let mut patch = px.copy_rect(rect.translate(-offset.0, -offset.1));
        let coverage = view.doc.selection.as_ref().map(|s| s.to_mask());
        gradient.render(
            &mut patch,
            (rect.x0, rect.y0),
            from,
            to,
            coverage.as_ref(),
            preserve,
        );

        let dirty = view
            .history
            .apply(&mut view.doc, Box::new(ReplacePixels::new(id, rect, patch, "Gradient")));
        view.mark_dirty(dirty);
    }

    // -----------------------------------------------------------------------
    // Clone Stamp
    // -----------------------------------------------------------------------

    /// Alt-click sets where the Clone Stamp copies from.
    /// Where the Clone Stamp is sampling from right now.
    ///
    /// Before the first stroke that is the anchor itself; once a stroke has
    /// fixed the offset, the source travels with the brush, which is the only
    /// way to see that an aligned source has wandered off the canvas.
    pub fn clone_source_at(&self, pointer: Option<Vec2>) -> Option<Vec2> {
        let anchor = self.clone_anchor?;
        match (self.clone_offset, pointer) {
            (Some(o), Some(p)) => Some(Vec2::new(p.x + o.0 as f32, p.y + o.1 as f32)),
            _ => Some(anchor),
        }
    }

    pub fn set_clone_anchor(&mut self, at: Vec2) {
        self.clone_anchor = Some(at);
        // A fresh anchor restarts the alignment.
        self.clone_offset = None;
        self.notify("Clone source set");
    }

    /// The image the Clone Stamp and the Paint Bucket sample.
    ///
    /// Always full-document sized, so a layer that is offset or smaller than
    /// the canvas still lines up with document coordinates.
    fn sample_source(&mut self, all_layers: bool) -> Option<PixelBuffer> {
        let gpu = self.gpu.clone();
        let index = self.active?;
        if all_layers {
            let view = &mut self.docs[index];
            view.sync_composite_only(&gpu, &mut self.compositor);
            return Some(view.read_composite(&gpu));
        }
        let view = &self.docs[index];
        let id = view.doc.active?;
        let layer = view.doc.tree.get(id)?;
        let pixels = layer.pixels()?;
        if layer.offset == (0, 0)
            && pixels.width() == view.doc.width
            && pixels.height() == view.doc.height
        {
            return Some(pixels.clone());
        }
        let mut full = PixelBuffer::new(view.doc.width, view.doc.height);
        full.paste(pixels, layer.offset.0, layer.offset.1);
        Some(full)
    }
}

/// A path being drawn with the Pen tool.
///
/// Kept on the app rather than in a layer until it is finished, because an
/// unfinished path has no interior and nothing to composite — and because
/// abandoning it should leave the document exactly as it was.
#[derive(Debug, Clone, Default)]
pub struct PenDraft {
    pub anchors: Vec<cshop_core::path::Anchor>,
    /// The anchor being dragged out, if the button is still down. Held apart
    /// from the committed ones so releasing without moving leaves a corner.
    pub dragging: Option<usize>,
    /// Where the pointer is now, for the segment that follows the last anchor.
    pub cursor: Option<Vec2>,
}

impl PenDraft {
    /// How close to the first anchor a click has to be to close the path, in
    /// document pixels at 100%.
    pub const CLOSE_RADIUS: f32 = 8.0;

    pub fn first(&self) -> Option<Vec2> {
        self.anchors.first().map(|a| a.at)
    }

    /// Whether clicking here would close the path rather than extend it.
    pub fn would_close(&self, at: Vec2, zoom: f32) -> bool {
        // The radius is in screen pixels, so zooming in does not make the
        // target harder to hit.
        let radius = Self::CLOSE_RADIUS / zoom.max(0.05);
        self.anchors.len() >= 2 && self.first().is_some_and(|p| p.distance(at) <= radius)
    }

    /// The path as it stands, for drawing the work in progress.
    pub fn to_path(&self, closed: bool) -> cshop_core::path::PathShape {
        cshop_core::path::PathShape::new(vec![cshop_core::path::SubPath {
            anchors: self.anchors.clone(),
            closed,
        }])
    }
}

/// What a background segmentation came back with.
/// What the full-resolution lens pass came back with.
pub enum LensOutcome {
    /// The corrected pixels, and the rectangle to keep if a crop was asked
    /// for — measured on the real result rather than on the preview.
    Done { pixels: Box<PixelBuffer>, crop: Option<IRect> },
    Failed(String),
}

/// Which windows can be pushed aside, and what each is called.
///
/// `None` means a modal: the four questions that have to be answered before
/// anything else makes sense — where to save, how big a new document is, what
/// to call a layer, and the about box.
///
/// Everything else shows its answer on the canvas, one way or another. A
/// window that dims the picture and sits over the middle of it is no use for
/// judging what it just did, which is the whole reason these have previews.
///
/// The name is also the id the window remembers its position under, so each
/// keeps its own place rather than inheriting wherever the last one was put.
fn window_key(dialog: &Dialog) -> Option<&'static str> {
    Some(match dialog {
        Dialog::NewDocument(_) | Dialog::FileBrowser(_) | Dialog::Rename(_) | Dialog::About => {
            return None
        }
        Dialog::None => return None,
        Dialog::LayerStyle(_) => "window-layer-style",
        Dialog::Segment(_) => "window-segment",
        Dialog::ColorRange(_) => "window-color-range",
        Dialog::RefineEdge(_) => "window-refine-edge",
        Dialog::Filter(_) => "window-filter",
        Dialog::Adjustment(_) => "window-adjustment",
        Dialog::Lens(_) => "window-lens",
        Dialog::Denoise(_) => "window-denoise",
        Dialog::Relight(_) => "window-relight",
        Dialog::Separate(_) => "window-separate",
        Dialog::Upscale(_) => "window-upscale",
        Dialog::ColorProfile(_) => "window-colour-profile",
        Dialog::Modify(_) => "window-modify",
        Dialog::ImageSize(_) => "window-image-size",
        Dialog::Fill(_) => "window-fill",
        Dialog::ColorPicker(_) => "window-colour-picker",
    })
}

pub enum SegmentOutcome {
    Ready { selection: Box<cshop_core::selection::Selection>, coverage: f32 },
    Failed(String),
    /// The detector's answer, when that is what was asked for.
    Found(Vec<crate::vision::Found>),
}

/// Which point of an anchor a gesture has hold of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleKind {
    /// The anchor itself. Moving it takes its handles along.
    Anchor,
    /// The control point of the curve arriving at it.
    In,
    /// The control point of the curve leaving it.
    Out,
}

/// One anchor's address inside a path: which operand, which contour, which
/// point. Held by index rather than by reference so it survives the path being
/// rebuilt on every drag frame.
pub type AnchorRef = (usize, usize, usize);

/// The Direct Selection tool's state.
#[derive(Debug, Clone, Default)]
pub struct PathEdit {
    /// The layer being edited. Cleared when the selection moves elsewhere.
    pub layer: Option<LayerId>,
    pub selected: Vec<AnchorRef>,
    /// What the pointer has hold of, and where it took hold.
    pub drag: Option<(AnchorRef, HandleKind)>,
    /// Where the pointer was on the previous frame of a drag.
    pub last: Option<Vec2>,
    /// Identifies this drag, so every frame of it folds into one undo step
    /// and the next drag starts a new one.
    pub run: Option<u64>,
}

impl PathEdit {
    /// How close a click has to land, in screen pixels.
    pub const GRAB_RADIUS: f32 = 7.0;

    pub fn clear(&mut self) {
        *self = PathEdit::default();
    }

    pub fn is_selected(&self, at: AnchorRef) -> bool {
        self.selected.contains(&at)
    }
}
