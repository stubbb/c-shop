//! Right-click menus.
//!
//! Two of them: the canvas shows whatever the active tool is configured by, and
//! a layer row shows what can be done to that layer. Both put controls where
//! the pointer already is, which for a brush size or a blend mode is worth more
//! than a trip to the options bar or the top of the panel.

use crate::app::CShopApp;
use crate::commands::{Action, TransformPreset};
use crate::theme::Palette;
use crate::tools::Tool;
use cshop_core::blend::BlendMode;
use cshop_core::document::EditTarget;
use cshop_core::history::LayerProperty;
use cshop_core::layer::LayerId;
use cshop_core::selection::SelectionMode;

/// Header text at the top of a context menu.
fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).strong().small());
    ui.separator();
}

/// A labelled slider sized for a menu rather than a panel.
fn slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::Slider::new(value, range).suffix(suffix).max_decimals(1)).changed()
    })
    .inner
}

// ---------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------

/// The canvas menu: the active tool's settings, plus the commands that go with
/// it.
pub fn canvas_menu(app: &mut CShopApp, ui: &mut egui::Ui) {
    ui.set_min_width(230.0);

    // A live transform or crop owns the canvas, so it owns the menu too.
    if app.transform.is_some() {
        heading(ui, "Free Transform");
        if ui.button("Apply").clicked() {
            app.push(Action::CommitTransform);
            ui.close();
        }
        if ui.button("Reset").clicked() {
            if let Some(t) = &mut app.transform {
                t.reset();
            }
            ui.close();
        }
        if ui.button("Cancel").clicked() {
            app.push(Action::CancelTransform);
            ui.close();
        }
        return;
    }

    heading(ui, app.tool.name());

    match app.tool {
        Tool::Brush | Tool::Pencil | Tool::Eraser => brush_menu(app, ui),
        Tool::RectangularMarquee
        | Tool::EllipticalMarquee
        | Tool::Lasso
        | Tool::PolygonalLasso => marquee_menu(app, ui),
        Tool::MagicWand => wand_menu(app, ui),
        Tool::Crop => crop_menu(app, ui),
        Tool::Zoom | Tool::Hand => zoom_menu(app, ui),
        Tool::Move => move_menu(app, ui),
        Tool::Eyedropper => {
            ui.label(
                egui::RichText::new("Click the canvas to sample the composited colour.")
                    .color(Palette::DARK.text_dim)
                    .small(),
            );
        }
        other => {
            ui.label(
                egui::RichText::new(format!("The {} tool is not implemented yet.", other.name()))
                    .color(Palette::DARK.text_dim)
                    .small(),
            );
        }
    }
}

fn brush_menu(app: &mut CShopApp, ui: &mut egui::Ui) {
    slider(ui, "Size", &mut app.brush.size, 1.0..=500.0, " px");
    let mut hardness = app.brush.hardness * 100.0;
    if slider(ui, "Hardness", &mut hardness, 0.0..=100.0, "%") {
        app.brush.hardness = hardness / 100.0;
    }
    let mut opacity = app.brush.opacity * 100.0;
    if slider(ui, "Opacity", &mut opacity, 0.0..=100.0, "%") {
        app.brush.opacity = opacity / 100.0;
    }
    let mut flow = app.brush.flow * 100.0;
    if slider(ui, "Flow", &mut flow, 0.0..=100.0, "%") {
        app.brush.flow = flow / 100.0;
    }

    ui.separator();
    // The sizes people actually reach for, so a menu click can replace a drag.
    ui.horizontal(|ui| {
        ui.label("Preset:");
        for size in [1.0f32, 5.0, 20.0, 60.0, 150.0] {
            if ui.small_button(format!("{size:.0}")).clicked() {
                app.brush.size = size;
            }
        }
    });

    ui.separator();
    if ui.button("Reset brush").clicked() {
        app.brush = cshop_core::paint::Brush::default();
        ui.close();
    }
}

fn marquee_menu(app: &mut CShopApp, ui: &mut egui::Ui) {
    let mut mode = app.selection_mode;
    ui.horizontal(|ui| {
        ui.label("Mode:");
        for m in [
            SelectionMode::Replace,
            SelectionMode::Add,
            SelectionMode::Subtract,
            SelectionMode::Intersect,
        ] {
            let label = match m {
                SelectionMode::Replace => "New",
                SelectionMode::Add => "Add",
                SelectionMode::Subtract => "Sub",
                SelectionMode::Intersect => "Int",
            };
            if ui.selectable_label(mode == m, label).on_hover_text(m.name()).clicked() {
                mode = m;
            }
        }
    });
    app.selection_mode = mode;

    slider(ui, "Feather", &mut app.selection_feather, 0.0..=100.0, " px");
    ui.checkbox(&mut app.selection_antialias, "Anti-alias");

    ui.separator();
    selection_commands(app, ui);
}

fn wand_menu(app: &mut CShopApp, ui: &mut egui::Ui) {
    let mut tolerance = app.wand.tolerance as f32;
    if slider(ui, "Tolerance", &mut tolerance, 0.0..=255.0, "") {
        app.wand.tolerance = tolerance as u8;
    }
    ui.checkbox(&mut app.wand.contiguous, "Contiguous");
    ui.checkbox(&mut app.wand.antialias, "Anti-alias");
    ui.checkbox(&mut app.sample_all_layers, "Sample all layers");

    ui.separator();
    selection_commands(app, ui);
}

/// The selection commands worth reaching from the canvas.
fn selection_commands(app: &mut CShopApp, ui: &mut egui::Ui) {
    let has_selection = app.doc().is_some_and(|d| d.doc.has_selection());

    if ui.button("Select All").clicked() {
        app.push(Action::SelectAll);
        ui.close();
    }
    ui.add_enabled_ui(has_selection, |ui| {
        if ui.button("Deselect").clicked() {
            app.push(Action::Deselect);
            ui.close();
        }
        if ui.button("Inverse").clicked() {
            app.push(Action::InverseSelection);
            ui.close();
        }
        if ui.button("Crop to Selection").clicked() {
            app.push(Action::CropToSelection);
            ui.close();
        }
        if ui.button("Layer Mask from Selection").clicked() {
            app.push(Action::AddLayerMaskFromSelection { invert: false });
            ui.close();
        }
        if ui.button("Fill with Foreground").clicked() {
            app.push(Action::fill_foreground(false));
            ui.close();
        }
    });
}

fn crop_menu(app: &mut CShopApp, ui: &mut egui::Ui) {
    let mut aspect = app.crop.as_ref().and_then(|c| c.aspect);
    ui.horizontal(|ui| {
        ui.label("Ratio:");
        for (label, value) in [
            ("Free", None),
            ("1:1", Some(1.0f32)),
            ("4:3", Some(4.0 / 3.0)),
            ("16:9", Some(16.0 / 9.0)),
        ] {
            if ui.selectable_label(aspect == value, label).clicked() {
                aspect = value;
            }
        }
    });
    if let Some(crop) = &mut app.crop {
        crop.aspect = aspect;
    }

    ui.separator();
    ui.add_enabled_ui(app.crop.is_some(), |ui| {
        if ui.button("Crop").clicked() {
            app.push(Action::CommitCrop);
            ui.close();
        }
        if ui.button("Cancel").clicked() {
            app.push(Action::CancelCrop);
            ui.close();
        }
    });
}

fn zoom_menu(app: &mut CShopApp, ui: &mut egui::Ui) {
    for (label, action) in [
        ("Fit on Screen", Action::ZoomFit),
        ("Actual Pixels", Action::ZoomActual),
        ("Zoom In", Action::ZoomIn),
        ("Zoom Out", Action::ZoomOut),
    ] {
        if ui.button(label).clicked() {
            app.push(action);
            ui.close();
        }
    }
}

fn move_menu(app: &mut CShopApp, ui: &mut egui::Ui) {
    if ui.button("Free Transform").clicked() {
        app.push(Action::BeginTransform);
        ui.close();
    }
    ui.menu_button("Transform", |ui| {
        for preset in TransformPreset::ALL {
            if ui.button(preset.name()).clicked() {
                app.push(Action::TransformPreset(preset));
                ui.close();
            }
        }
    });
    ui.separator();
    ui.label(
        egui::RichText::new("Arrow keys nudge by a pixel, Shift by ten.")
            .color(Palette::DARK.text_dim)
            .small(),
    );
}

// ---------------------------------------------------------------------------
// Layers
// ---------------------------------------------------------------------------

/// The menu for one layer row.
pub fn layer_menu(app: &mut CShopApp, ui: &mut egui::Ui, id: LayerId) {
    let has_pixels = app
        .doc()
        .and_then(|v| v.doc.tree.get(id))
        .is_some_and(|l| l.pixels().is_some());
    let has_fx =
        app.doc().and_then(|v| v.doc.tree.get(id)).is_some_and(|l| l.effects.any());
    if ui.add_enabled(has_pixels, egui::Button::new("Layer Style…")).clicked() {
        app.push(Action::SelectLayer(id));
        app.push(Action::ShowLayerStyle);
        ui.close();
    }
    if ui.add_enabled(has_fx, egui::Button::new("Clear Layer Style")).clicked() {
        app.push(Action::ClearLayerEffects(id));
        ui.close();
    }
    ui.separator();
    ui.set_min_width(210.0);

    let Some(view) = app.doc() else { return };
    let Some(layer) = view.doc.tree.get(id) else { return };
    let name = layer.name.clone();
    let is_group = layer.kind.is_group();
    let is_adjustment = layer.kind.is_adjustment();
    let has_mask = layer.mask.is_some();
    let mask_enabled = layer.mask.as_ref().is_some_and(|m| m.enabled);
    let clipping = layer.clipping;
    let visible = layer.visible;
    let locks = layer.locks;
    let mode = layer.blend_mode;
    let opacity = layer.opacity;
    let fill_opacity = layer.fill_opacity;
    let can_merge_down =
        view.doc.tree.position(id).is_some_and(|pos| pos.index > 0) && !is_group;
    let layer_count = view.doc.tree.len();
    let has_selection = view.doc.has_selection();

    heading(ui, &name);

    // The menu always acts on the layer that was clicked, which may not be the
    // one that was active.
    if app.doc().and_then(|d| d.doc.active) != Some(id) {
        app.push(Action::SelectLayer(id));
    }

    // --- blending -----------------------------------------------------------
    blend_mode_menu(app, ui, id, mode);

    let mut opacity_pct = opacity * 100.0;
    if slider(ui, "Opacity", &mut opacity_pct, 0.0..=100.0, "%") {
        app.push(Action::SetLayerProperty(id, LayerProperty::Opacity(opacity_pct / 100.0)));
    }
    let mut fill_pct = fill_opacity * 100.0;
    if slider(ui, "Fill", &mut fill_pct, 0.0..=100.0, "%") {
        app.push(Action::SetLayerProperty(id, LayerProperty::FillOpacity(fill_pct / 100.0)));
    }

    ui.separator();

    // --- visibility and clipping -------------------------------------------
    if ui.button(if visible { "Hide Layer" } else { "Show Layer" }).clicked() {
        app.push(Action::SetLayerProperty(id, LayerProperty::Visible(!visible)));
        ui.close();
    }
    if ui
        .button(if clipping { "Release Clipping Mask" } else { "Create Clipping Mask" })
        .on_hover_text("Clip this layer into the alpha of the one below")
        .clicked()
    {
        app.push(Action::SetLayerProperty(id, LayerProperty::Clipping(!clipping)));
        ui.close();
    }

    // --- masks --------------------------------------------------------------
    ui.menu_button("Layer Mask", |ui| {
        if !has_mask {
            if ui.button("Reveal All").clicked() {
                app.push(Action::AddLayerMask { hide_all: false });
                ui.close();
            }
            if ui.button("Hide All").clicked() {
                app.push(Action::AddLayerMask { hide_all: true });
                ui.close();
            }
            ui.add_enabled_ui(has_selection, |ui| {
                if ui.button("From Selection").clicked() {
                    app.push(Action::AddLayerMaskFromSelection { invert: false });
                    ui.close();
                }
                if ui.button("From Selection, Inverted").clicked() {
                    app.push(Action::AddLayerMaskFromSelection { invert: true });
                    ui.close();
                }
            });
        } else {
            if ui.button(if mask_enabled { "Disable Mask" } else { "Enable Mask" }).clicked() {
                app.push(Action::ToggleMaskEnabled);
                ui.close();
            }
            if ui.button("Edit Mask").clicked() {
                app.push(Action::SetEditTarget(EditTarget::Mask));
                ui.close();
            }
            if ui.button("Edit Pixels").clicked() {
                app.push(Action::SetEditTarget(EditTarget::Pixels));
                ui.close();
            }
            ui.separator();
            if ui.button("Apply Mask").clicked() {
                app.push(Action::ApplyLayerMask);
                ui.close();
            }
            if ui.button("Delete Mask").clicked() {
                app.push(Action::DeleteLayerMask);
                ui.close();
            }
        }
    });

    // --- locks --------------------------------------------------------------
    ui.menu_button("Lock", |ui| {
        for (label, on, make) in [
            (
                "Transparent Pixels",
                locks.transparency,
                LayerProperty::LockTransparency as fn(bool) -> LayerProperty,
            ),
            ("Image Pixels", locks.pixels, LayerProperty::LockPixels),
            ("Position", locks.position, LayerProperty::LockPosition),
            ("All", locks.all, LayerProperty::LockAll),
        ] {
            let mut value = on;
            if ui.checkbox(&mut value, label).changed() {
                app.push(Action::SetLayerProperty(id, make(value)));
            }
        }
    });

    ui.separator();

    // --- structural ---------------------------------------------------------
    if ui.button("Rename…").clicked() {
        app.push(Action::RenameLayer(id));
        ui.close();
    }
    ui.add_enabled_ui(!is_group, |ui| {
        if ui.button("Duplicate Layer").clicked() {
            app.push(Action::DuplicateLayer);
            ui.close();
        }
    });
    ui.add_enabled_ui(can_merge_down, |ui| {
        if ui.button("Merge Down").clicked() {
            app.push(Action::MergeDown);
            ui.close();
        }
    });
    ui.add_enabled_ui(layer_count > 1, |ui| {
        if ui.button("Merge All").clicked() {
            app.push(Action::FlattenImage);
            ui.close();
        }
        if ui.button("Delete Layer").clicked() {
            app.push(Action::DeleteLayer);
            ui.close();
        }
    });

    if is_adjustment {
        ui.separator();
        ui.label(
            egui::RichText::new("Settings are in the Properties panel.")
                .color(Palette::DARK.text_dim)
                .small(),
        );
    }
}

/// The blend mode submenu, grouped by family like the dropdown.
fn blend_mode_menu(app: &mut CShopApp, ui: &mut egui::Ui, id: LayerId, current: BlendMode) {
    ui.menu_button(format!("Blend Mode:  {}", current.name()), |ui| {
        // Groups nest, so Pass Through is only meaningful for them and is not
        // in the shared menu list.
        let is_group = app
            .doc()
            .and_then(|d| d.doc.tree.get(id))
            .is_some_and(|l| l.kind.is_group());
        if is_group
            && ui.selectable_label(current == BlendMode::PassThrough, "Pass Through").clicked()
        {
            app.push(Action::SetLayerProperty(id, LayerProperty::Blend(BlendMode::PassThrough)));
            ui.close();
        }
        if is_group {
            ui.separator();
        }

        for entry in BlendMode::MENU {
            match entry {
                Some(mode) => {
                    if ui.selectable_label(current == *mode, mode.name()).clicked() {
                        app.push(Action::SetLayerProperty(id, LayerProperty::Blend(*mode)));
                        ui.close();
                    }
                }
                None => {
                    ui.separator();
                }
            }
        }
    });
}
