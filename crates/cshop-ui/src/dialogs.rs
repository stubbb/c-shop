//! Modal dialogs, drawn by us rather than by the platform.
//!
//! Editors of this kind use in-application dialogs. Doing the same
//! keeps the look consistent and, on Linux, avoids depending on GTK or on an
//! xdg-desktop-portal that may not be running.

use crate::commands::{Action, Anchor};
use crate::theme::Palette;
use cshop_core::color::Rgba8;
use cshop_core::document::Background;
use cshop_io::ImageFormat;
use std::path::{Path, PathBuf};

/// Which modal is up, if any.
pub enum Dialog {
    None,
    NewDocument(NewDocument),
    FileBrowser(FileBrowser),
    Modify(ModifyDialog),
    ImageSize(SizeDialog),
    // Boxed: these two carry a preview image, and one an entire histogram, so
    // inline they would make every `Dialog::None` kilobytes wide.
    Filter(Box<crate::filter_ui::FilterDialog>),
    Rename(RenameDialog),
    Adjustment(Box<crate::adjust_ui::AdjustmentDialog>),
    LayerStyle(Box<crate::layer_style::LayerStyleDialog>),
    Fill(FillDialog),
    ColorPicker(ColorPickerDialog),
    About,
}

impl Dialog {
    pub fn is_open(&self) -> bool {
        !matches!(self, Dialog::None)
    }
}

// ---------------------------------------------------------------------------
// New document
// ---------------------------------------------------------------------------

pub struct NewDocument {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub dpi: f32,
    pub background: usize,
}

/// Document presets: the sizes people actually reach for.
const PRESETS: &[(&str, u32, u32, f32)] = &[
    ("Default (1920 x 1080)", 1920, 1080, 72.0),
    ("Default", 1000, 1000, 72.0),
    ("A4 (300 dpi)", 2480, 3508, 300.0),
    ("A4 Landscape (300 dpi)", 3508, 2480, 300.0),
    ("US Letter (300 dpi)", 2550, 3300, 300.0),
    ("4K UHD", 3840, 2160, 72.0),
    ("1080p", 1920, 1080, 72.0),
    ("Square 2048", 2048, 2048, 72.0),
    ("Instagram Post", 1080, 1080, 72.0),
    ("Instagram Story", 1080, 1920, 72.0),
    ("Web Banner", 1200, 628, 72.0),
    ("Icon 512", 512, 512, 72.0),
];

const BACKGROUNDS: &[&str] = &["White", "Black", "Transparent"];

impl Default for NewDocument {
    fn default() -> Self {
        Self { name: String::new(), width: 1920, height: 1080, dpi: 72.0, background: 0 }
    }
}

impl NewDocument {
    pub fn background(&self) -> Background {
        match self.background {
            1 => Background::Color(Rgba8::BLACK),
            2 => Background::Transparent,
            _ => Background::White,
        }
    }

    /// Returns `true` when the dialog should close.
    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let mut close = false;

        egui::Grid::new("new-doc-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Name:");
            ui.add(
                egui::TextEdit::singleline(&mut self.name)
                    .hint_text("Untitled-1")
                    .desired_width(220.0),
            );
            ui.end_row();

            ui.label("Preset:");
            egui::ComboBox::from_id_salt("preset")
                .width(220.0)
                .selected_text("Choose a preset…")
                .show_ui(ui, |ui| {
                    for (label, w, h, dpi) in PRESETS {
                        if ui.selectable_label(false, *label).clicked() {
                            self.width = *w;
                            self.height = *h;
                            self.dpi = *dpi;
                        }
                    }
                });
            ui.end_row();

            ui.label("Width:");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.width).range(1..=32768).suffix(" px"));
                if crate::icons::icon_button(ui, crate::icons::Icon::Swap, 20.0, "Swap width and height").clicked() {
                    std::mem::swap(&mut self.width, &mut self.height);
                }
            });
            ui.end_row();

            ui.label("Height:");
            ui.add(egui::DragValue::new(&mut self.height).range(1..=32768).suffix(" px"));
            ui.end_row();

            ui.label("Resolution:");
            ui.add(egui::DragValue::new(&mut self.dpi).range(1.0..=2400.0).suffix(" ppi"));
            ui.end_row();

            ui.label("Background:");
            egui::ComboBox::from_id_salt("bg")
                .width(220.0)
                .selected_text(BACKGROUNDS[self.background.min(BACKGROUNDS.len() - 1)])
                .show_ui(ui, |ui| {
                    for (i, name) in BACKGROUNDS.iter().enumerate() {
                        ui.selectable_value(&mut self.background, i, *name);
                    }
                });
            ui.end_row();
        });

        // Give the user the memory cost before they commit to a huge canvas.
        let bytes = self.width as u64 * self.height as u64 * 4;
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(format!(
                "{} x {} px  ·  {}",
                self.width,
                self.height,
                crate::format_bytes(bytes)
            ))
            .color(Palette::DARK.text_dim)
            .small(),
        );

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Create").clicked() {
                actions.push(Action::NewDocument);
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
        close
    }
}

// ---------------------------------------------------------------------------
// Edit > Fill
// ---------------------------------------------------------------------------

/// What a fill is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillContents {
    Foreground,
    Background,
    Black,
    Gray,
    White,
    Custom,
}

impl FillContents {
    pub fn name(self) -> &'static str {
        match self {
            FillContents::Foreground => "Foreground Colour",
            FillContents::Background => "Background Colour",
            FillContents::Black => "Black",
            FillContents::Gray => "50% Gray",
            FillContents::White => "White",
            FillContents::Custom => "Custom…",
        }
    }

    pub const ALL: [FillContents; 6] = [
        FillContents::Foreground,
        FillContents::Background,
        FillContents::Black,
        FillContents::Gray,
        FillContents::White,
        FillContents::Custom,
    ];
}

/// Edit > Fill: choose a colour and lay it into the selection.
pub struct FillDialog {
    pub contents: FillContents,
    pub custom: crate::color_picker::ColorPickerState,
    pub opacity: f32,
    pub mode: cshop_core::blend::BlendMode,
    pub preserve_transparency: bool,
    foreground: Rgba8,
    background: Rgba8,
    /// Whether the custom picker is expanded.
    picking: bool,
}

impl FillDialog {
    pub fn new(foreground: Rgba8, background: Rgba8) -> Self {
        Self {
            contents: FillContents::Foreground,
            custom: crate::color_picker::ColorPickerState::from_color(foreground),
            opacity: 1.0,
            mode: cshop_core::blend::BlendMode::Normal,
            preserve_transparency: false,
            foreground,
            background,
            picking: false,
        }
    }

    /// The colour the current settings would lay down.
    pub fn color(&self) -> Rgba8 {
        match self.contents {
            FillContents::Foreground => self.foreground,
            FillContents::Background => self.background,
            FillContents::Black => Rgba8::BLACK,
            FillContents::Gray => Rgba8::opaque(128, 128, 128),
            FillContents::White => Rgba8::WHITE,
            FillContents::Custom => self.custom.to_color(),
        }
    }

    /// Returns `true` when the dialog should close.
    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let p = Palette::DARK;
        let mut close = false;

        ui.horizontal(|ui| {
            ui.label("Contents:");
            egui::ComboBox::from_id_salt("fill-contents")
                .width(200.0)
                .selected_text(self.contents.name())
                .show_ui(ui, |ui| {
                    for option in FillContents::ALL {
                        ui.selectable_value(&mut self.contents, option, option.name());
                    }
                });

            // A swatch of whatever the choice resolves to.
            let c = self.color();
            let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 20.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, egui::Color32::from_rgb(c.r, c.g, c.b));
            ui.painter().rect_stroke(
                rect,
                2.0,
                egui::Stroke::new(1.0, p.separator),
                egui::StrokeKind::Inside,
            );
        });

        // Choosing Custom opens the picker in place rather than stacking a
        // second dialog on top of this one.
        if self.contents == FillContents::Custom {
            self.picking = true;
        }
        if self.picking {
            ui.add_space(8.0);
            let original = self.custom.to_color();
            if crate::color_picker::color_picker(ui, &mut self.custom, original, false) {
                self.contents = FillContents::Custom;
            }
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label("Mode:");
            egui::ComboBox::from_id_salt("fill-mode")
                .width(160.0)
                .selected_text(self.mode.name())
                .show_ui(ui, |ui| {
                    for entry in cshop_core::blend::BlendMode::MENU {
                        match entry {
                            Some(m) => {
                                ui.selectable_value(&mut self.mode, *m, m.name());
                            }
                            None => {
                                ui.separator();
                            }
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Opacity:");
            ui.add(
                egui::Slider::new(&mut self.opacity, 0.0..=1.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
            );
        });
        ui.checkbox(&mut self.preserve_transparency, "Preserve transparency");

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                actions.push(Action::FillWith {
                    color: self.color(),
                    mode: self.mode,
                    opacity: self.opacity,
                    preserve_transparency: self.preserve_transparency,
                });
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
        close
    }
}

// ---------------------------------------------------------------------------
// Colour picker
// ---------------------------------------------------------------------------

/// Which swatch the picker is editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerTarget {
    Foreground,
    Background,
}

/// The Color panel's *Custom* button opens this.
pub struct ColorPickerDialog {
    pub target: PickerTarget,
    pub state: crate::color_picker::ColorPickerState,
    original: Rgba8,
}

impl ColorPickerDialog {
    pub fn new(target: PickerTarget, current: Rgba8) -> Self {
        Self {
            target,
            state: crate::color_picker::ColorPickerState::from_color(current),
            original: current,
        }
    }

    pub fn title(&self) -> &'static str {
        match self.target {
            PickerTarget::Foreground => "Foreground Colour",
            PickerTarget::Background => "Background Colour",
        }
    }

    /// Returns `true` when the dialog should close.
    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let mut close = false;
        crate::color_picker::color_picker(ui, &mut self.state, self.original, false);

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                actions.push(Action::SetColor {
                    target: self.target,
                    color: self.state.to_color(),
                });
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
        close
    }
}

// ---------------------------------------------------------------------------
// Rename layer
// ---------------------------------------------------------------------------

/// Renames a layer. Small enough to be a dialog rather than an inline editor,
/// which would mean putting a text field inside a custom-painted row.
pub struct RenameDialog {
    pub layer: cshop_core::layer::LayerId,
    pub name: String,
    /// Set on the first frame so the field takes focus without the user
    /// having to click into it.
    focused: bool,
}

impl RenameDialog {
    pub fn new(layer: cshop_core::layer::LayerId, name: String) -> Self {
        Self { layer, name, focused: false }
    }

    /// Returns `true` when the dialog should close.
    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let mut close = false;
        let mut commit = false;

        ui.horizontal(|ui| {
            ui.label("Name:");
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.name).desired_width(260.0),
            );
            if !self.focused {
                field.request_focus();
                self.focused = true;
            }
            // Enter accepts, which is what everyone expects of a rename box.
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                commit = true;
            }
        });

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                commit = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });

        if commit {
            let name = self.name.trim();
            if !name.is_empty() {
                actions.push(Action::SetLayerProperty(
                    self.layer,
                    cshop_core::history::LayerProperty::Name(name.to_string()),
                ));
            }
            close = true;
        }
        close
    }
}

// ---------------------------------------------------------------------------
// Image Size and Canvas Size
// ---------------------------------------------------------------------------

/// Collects new dimensions for Image Size (resamples) or Canvas Size (does
/// not).
pub struct SizeDialog {
    /// `true` for Image Size, `false` for Canvas Size.
    pub resample: bool,
    pub width: u32,
    pub height: u32,
    pub original: (u32, u32),
    pub link_aspect: bool,
    pub filter: cshop_core::resample::Resampling,
    pub anchor: Anchor,
    /// Editing in percent rather than pixels.
    pub percent: bool,
}

impl SizeDialog {
    pub fn image(width: u32, height: u32) -> Self {
        Self {
            resample: true,
            width,
            height,
            original: (width, height),
            link_aspect: true,
            filter: Default::default(),
            anchor: Anchor::Center,
            percent: false,
        }
    }

    pub fn canvas(width: u32, height: u32) -> Self {
        Self { resample: false, ..Self::image(width, height) }
    }

    pub fn title(&self) -> &'static str {
        if self.resample { "Image Size" } else { "Canvas Size" }
    }

    /// Returns `true` when the dialog should close.
    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let p = Palette::DARK;
        let mut close = false;
        let aspect = self.original.0 as f32 / self.original.1.max(1) as f32;

        ui.label(
            egui::RichText::new(format!(
                "Current: {} x {} px  ·  {}",
                self.original.0,
                self.original.1,
                crate::format_bytes(self.original.0 as u64 * self.original.1 as u64 * 4)
            ))
            .color(p.text_dim)
            .small(),
        );
        ui.add_space(8.0);

        ui.checkbox(&mut self.percent, "Enter as percentage");

        egui::Grid::new("size-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Width:");
            let mut w_changed = false;
            if self.percent {
                let mut pct = self.width as f32 / self.original.0.max(1) as f32 * 100.0;
                if ui.add(egui::DragValue::new(&mut pct).range(1.0..=2000.0).suffix(" %")).changed()
                {
                    self.width = ((self.original.0 as f32 * pct / 100.0) as u32).max(1);
                    w_changed = true;
                }
            } else {
                w_changed = ui
                    .add(egui::DragValue::new(&mut self.width).range(1..=32768).suffix(" px"))
                    .changed();
            }
            if w_changed && self.link_aspect {
                self.height = ((self.width as f32 / aspect).round() as u32).max(1);
            }
            ui.end_row();

            ui.label("Height:");
            let mut h_changed = false;
            if self.percent {
                let mut pct = self.height as f32 / self.original.1.max(1) as f32 * 100.0;
                if ui.add(egui::DragValue::new(&mut pct).range(1.0..=2000.0).suffix(" %")).changed()
                {
                    self.height = ((self.original.1 as f32 * pct / 100.0) as u32).max(1);
                    h_changed = true;
                }
            } else {
                h_changed = ui
                    .add(egui::DragValue::new(&mut self.height).range(1..=32768).suffix(" px"))
                    .changed();
            }
            if h_changed && self.link_aspect {
                self.width = ((self.height as f32 * aspect).round() as u32).max(1);
            }
            ui.end_row();

            ui.label("");
            ui.checkbox(&mut self.link_aspect, "Constrain proportions");
            ui.end_row();
        });

        if self.resample {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Resample:");
                egui::ComboBox::from_id_salt("resize-filter")
                    .width(180.0)
                    .selected_text(self.filter.name())
                    .show_ui(ui, |ui| {
                        for f in cshop_core::resample::Resampling::ALL {
                            ui.selectable_value(&mut self.filter, f, f.name());
                        }
                    });
            });
        } else {
            ui.add_space(8.0);
            ui.label("Anchor:");
            // A three-by-three grid, as a canvas-size anchor wants.
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    for row in Anchor::GRID {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
                            for cell in row {
                                let selected = self.anchor == cell;
                                let (rect, r) = ui.allocate_exact_size(
                                    egui::vec2(22.0, 22.0),
                                    egui::Sense::click(),
                                );
                                ui.painter().rect_filled(
                                    rect,
                                    2.0,
                                    if selected { p.accent } else { p.widget },
                                );
                                if r.clicked() {
                                    self.anchor = cell;
                                }
                            }
                        });
                    }
                });
            });
        }

        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!(
                "New: {} x {} px  ·  {}",
                self.width,
                self.height,
                crate::format_bytes(self.width as u64 * self.height as u64 * 4)
            ))
            .color(p.text_dim)
            .small(),
        );

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                if self.resample {
                    actions.push(Action::ResizeImage {
                        width: self.width,
                        height: self.height,
                        filter: self.filter,
                    });
                } else {
                    actions.push(Action::ResizeCanvas {
                        width: self.width,
                        height: self.height,
                        anchor: self.anchor,
                    });
                }
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
        close
    }
}

// ---------------------------------------------------------------------------
// Select > Modify
// ---------------------------------------------------------------------------

/// Collects the amount for one of the Select > Modify operations.
pub struct ModifyDialog {
    pub kind: crate::chrome::ModifyKind,
    pub amount: f32,
}

impl ModifyDialog {
    pub fn new(kind: crate::chrome::ModifyKind) -> Self {
        // Sensible defaults, which differ per operation.
        let amount = match kind {
            crate::chrome::ModifyKind::Feather => 5.0,
            crate::chrome::ModifyKind::Border => 10.0,
            _ => 3.0,
        };
        Self { kind, amount }
    }

    pub fn title(&self) -> &'static str {
        self.kind.title()
    }

    /// Returns `true` when the dialog should close.
    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let mut close = false;
        ui.horizontal(|ui| {
            ui.label(self.kind.field());
            ui.add(
                egui::DragValue::new(&mut self.amount)
                    .range(0.0..=1000.0)
                    .speed(0.25)
                    .suffix(" px"),
            );
        });
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                actions.push(Action::ModifySelection(self.kind.build(self.amount)));
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
        close
    }
}

// ---------------------------------------------------------------------------
// File browser
// ---------------------------------------------------------------------------

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum BrowserMode {
    Open,
    Save,
}

pub struct FileBrowser {
    pub mode: BrowserMode,
    pub dir: PathBuf,
    pub filename: String,
    pub format: ImageFormat,
    entries: Vec<Entry>,
    error: Option<String>,
    /// Bumped to force a re-read after navigating.
    needs_refresh: bool,
}

struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
}

impl FileBrowser {
    pub fn new(mode: BrowserMode, start: Option<PathBuf>) -> Self {
        let dir = start
            .and_then(|p| if p.is_dir() { Some(p) } else { p.parent().map(Path::to_path_buf) })
            .or_else(dirs_home)
            .unwrap_or_else(|| PathBuf::from("/"));

        let mut b = Self {
            mode,
            dir,
            filename: String::new(),
            format: ImageFormat::Png,
            entries: Vec::new(),
            error: None,
            needs_refresh: true,
        };
        b.refresh();
        b
    }

    fn refresh(&mut self) {
        self.needs_refresh = false;
        self.entries.clear();
        self.error = None;

        let read = match std::fs::read_dir(&self.dir) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(format!("Cannot open {}: {e}", self.dir.display()));
                return;
            }
        };

        for entry in read.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            // Hidden files stay hidden; there is no toggle yet.
            if name.starts_with('.') {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let is_dir = meta.is_dir();

            if !is_dir && self.mode == BrowserMode::Open {
                let ok = path
                    .extension()
                    .map(|e| {
                        let e = e.to_string_lossy().to_ascii_lowercase();
                        ImageFormat::OPENABLE_EXTENSIONS.contains(&e.as_str())
                    })
                    .unwrap_or(false);
                if !ok {
                    continue;
                }
            }
            self.entries.push(Entry { name, path, is_dir, size: meta.len() });
        }

        // Directories first, then alphabetically, case-insensitively.
        self.entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }

    fn navigate(&mut self, to: PathBuf) {
        self.dir = to;
        self.needs_refresh = true;
    }

    /// Full path the Save button would write to.
    fn target(&self) -> PathBuf {
        let mut name = self.filename.trim().to_string();
        if name.is_empty() {
            name = "Untitled".into();
        }
        if ImageFormat::from_path(Path::new(&name)) != Some(self.format) {
            name = format!("{name}.{}", self.format.default_extension());
        }
        self.dir.join(name)
    }

    /// Returns `true` when the dialog should close.
    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        if self.needs_refresh {
            self.refresh();
        }
        let mut close = false;
        let p = Palette::DARK;

        // --- breadcrumb ----------------------------------------------------
        ui.horizontal(|ui| {
            if crate::icons::icon_button(ui, crate::icons::Icon::Home, 20.0, "Home").clicked() {
                if let Some(h) = dirs_home() {
                    self.navigate(h);
                }
            }
            if crate::icons::icon_button(ui, crate::icons::Icon::Up, 20.0, "Parent folder").clicked() {
                if let Some(parent) = self.dir.parent().map(Path::to_path_buf) {
                    self.navigate(parent);
                }
            }
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(self.dir.display().to_string()).color(p.text_dim).small(),
            );
        });

        ui.add_space(4.0);

        // --- listing -------------------------------------------------------
        let mut navigate_to = None;
        let mut chosen = None;

        egui::Frame::NONE.fill(p.canvas_backdrop).inner_margin(4).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, false]).show(
                ui,
                |ui| {
                    ui.set_min_width(520.0);
                    if let Some(err) = &self.error {
                        ui.colored_label(egui::Color32::from_rgb(0xe0, 0x6c, 0x60), err);
                        return;
                    }
                    if self.entries.is_empty() {
                        ui.label(
                            egui::RichText::new("Nothing here").color(p.text_dim).italics(),
                        );
                        return;
                    }
                    for entry in &self.entries {
                        let label = if entry.is_dir {
                            format!("      {}", entry.name)
                        } else {
                            format!(
                                "      {}          {}",
                                entry.name,
                                crate::format_bytes(entry.size)
                            )
                        };
                        let selected = !entry.is_dir && self.filename == entry.name;
                        let r = ui.selectable_label(selected, label);
                        // Draw the icon into the space the label left for it.
                        let glyph = egui::Rect::from_min_size(
                            egui::pos2(r.rect.min.x + 3.0, r.rect.center().y - 7.0),
                            egui::vec2(14.0, 14.0),
                        );
                        crate::icons::icon(
                            &ui.painter_at(r.rect),
                            glyph,
                            if entry.is_dir {
                                crate::icons::Icon::Folder
                            } else {
                                crate::icons::Icon::File
                            },
                            Palette::DARK.text_dim,
                        );
                        if r.clicked() {
                            if entry.is_dir {
                                navigate_to = Some(entry.path.clone());
                            } else {
                                self.filename = entry.name.clone();
                            }
                        }
                        if r.double_clicked() {
                            if entry.is_dir {
                                navigate_to = Some(entry.path.clone());
                            } else {
                                chosen = Some(entry.path.clone());
                            }
                        }
                    }
                },
            );
        });

        if let Some(dir) = navigate_to {
            self.navigate(dir);
        }
        if let Some(path) = chosen {
            actions.push(Action::OpenPath(path));
            return true;
        }

        // --- filename and format ------------------------------------------
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("File name:");
            ui.add(egui::TextEdit::singleline(&mut self.filename).desired_width(300.0));

            if self.mode == BrowserMode::Save {
                egui::ComboBox::from_id_salt("save-format")
                    .width(160.0)
                    .selected_text(self.format.display_name())
                    .show_ui(ui, |ui| {
                        for f in ImageFormat::WRITABLE {
                            ui.selectable_value(&mut self.format, *f, f.display_name());
                        }
                    });
            }
        });

        if self.mode == BrowserMode::Save && !self.format.supports_alpha() {
            ui.label(
                egui::RichText::new(
                    "JPEG has no alpha channel; transparent areas will be filled with white.",
                )
                .color(p.text_dim)
                .small(),
            );
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            let verb = if self.mode == BrowserMode::Open { "Open" } else { "Save" };
            let ready = !self.filename.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new(verb)).clicked() {
                match self.mode {
                    BrowserMode::Open => {
                        actions.push(Action::OpenPath(self.dir.join(self.filename.trim())))
                    }
                    BrowserMode::Save => actions.push(Action::SavePath(self.target())),
                }
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }

            if self.mode == BrowserMode::Save && ready {
                let target = self.target();
                if target.exists() {
                    ui.label(
                        egui::RichText::new("⚠ This file already exists and will be replaced.")
                            .color(egui::Color32::from_rgb(0xd8, 0xa0, 0x50))
                            .small(),
                    );
                }
            }
        });

        close
    }
}

/// The user's home directory, without pulling in a crate for it.
fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_dir())
}
