//! The right-hand dock: Layers, History and Color.
//!
//! The Layers panel is the centre of gravity of the whole application, so it
//! gets the most attention here: thumbnails, drag-to-reorder, blend mode,
//! opacity, locks and group nesting.

use crate::app::CShopApp;
use crate::commands::Action;
use crate::icons::{self, Icon};
use crate::theme::Palette;
use cshop_core::blend::BlendMode;
use cshop_core::document::EditTarget;
use cshop_core::history::LayerProperty;
use cshop_core::layer::LayerId;
use cshop_core::tree::LayerPos;

/// Height of one row in the Layers panel.
const ROW_HEIGHT: f32 = 44.0;
const THUMB: f32 = 34.0;

pub fn dock(app: &mut CShopApp, ui: &mut egui::Ui) {
    // Layers is the panel people work in, so it gets the bottom half outright
    // and everything else shares a scrolling column above it. Without the
    // split, opening Properties on a Curves layer pushed Layers off-screen.
    let total = ui.available_height();
    let layers_height = (total * 0.5).clamp(180.0, total - 120.0);

    egui::ScrollArea::vertical()
        .id_salt("dock-scroll")
        .max_height(total - layers_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            // Properties comes first when there is something to edit, because
            // that is what the user just clicked on.
            if app.doc().is_some_and(|d| {
                d.doc.active_layer().is_some_and(|l| l.kind.is_adjustment())
            }) {
                section(ui, "Properties", true, |ui| properties_panel(app, ui));
            }
            // The stack sits next to the layer it is on, not behind a dialog:
            // its whole point is that you can see what is applied and change
            // your mind, and a stack you have to go looking for is a stack
            // people forget is there.
            if app
                .doc()
                .is_some_and(|d| d.doc.active_layer().is_some_and(|l| !l.filters.slots.is_empty()))
            {
                section(ui, "Smart Filters", true, |ui| smart_filters_panel(app, ui));
            }
            section(ui, "Layer States", false, |ui| layer_states_panel(app, ui));
            section(ui, "Color", true, |ui| color_panel(app, ui));
            section(ui, "History", true, |ui| history_panel(app, ui));
            section(ui, "Channels", false, |ui| channels_panel(app, ui));
        });

    section_fill(ui, "Layers", |ui| layers_panel(app, ui));
}

// ---------------------------------------------------------------------------
// Layer states
// ---------------------------------------------------------------------------

/// Named sets of what the layers are doing, so two versions of a design can
/// live in one document.
fn layer_states_panel(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;
    let Some(view) = app.doc() else {
        ui.label(egui::RichText::new("No document").color(p.text_dim).italics());
        return;
    };
    // Which one is showing, if any: a state records settings, so the answer is
    // simply whether the layers currently match it.
    let rows: Vec<(String, bool)> =
        view.doc.states.iter().map(|s| (s.name.clone(), s.matches(&view.doc.tree))).collect();

    let mut queued: Vec<Action> = Vec::new();
    if rows.is_empty() {
        ui.label(
            egui::RichText::new(
                "Save what the layers are doing now, then change them and save again.",
            )
            .color(p.text_dim)
            .small(),
        );
    }
    for (i, (name, showing)) in rows.iter().enumerate() {
        ui.horizontal(|ui| {
            let text = if *showing {
                egui::RichText::new(name).color(p.accent)
            } else {
                egui::RichText::new(name)
            };
            if ui
                .add(egui::Label::new(text).sense(egui::Sense::click()))
                .on_hover_text("Show this one")
                .clicked()
            {
                queued.push(Action::ApplyLayerState(i));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("✕").on_hover_text("Forget it").clicked() {
                    queued.push(Action::DeleteLayerState(i));
                }
                if ui
                    .small_button("⟳")
                    .on_hover_text("Replace it with what the layers are doing now")
                    .clicked()
                {
                    queued.push(Action::UpdateLayerState(i));
                }
            });
        });
    }
    ui.add_space(4.0);
    if ui.button("Save current state").clicked() {
        queued.push(Action::SaveLayerState(String::new()));
    }

    for action in queued {
        app.push(action);
    }
}

// ---------------------------------------------------------------------------
// Smart filters
// ---------------------------------------------------------------------------

/// The active layer's filter stack, bottom-first as it is applied.
fn smart_filters_panel(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;
    let Some(view) = app.doc() else { return };
    let Some(layer) = view.doc.active_layer() else { return };
    let stack_on = layer.filters.enabled;
    let rows: Vec<(String, bool, f32)> = layer
        .filters
        .slots
        .iter()
        .map(|s| (s.filter.name().to_string(), s.enabled, s.opacity))
        .collect();

    let mut queued: Vec<Action> = Vec::new();
    ui.horizontal(|ui| {
        let mut on = stack_on;
        if ui.checkbox(&mut on, "").changed() {
            queued.push(Action::ToggleAttachedFilters);
        }
        ui.label(
            egui::RichText::new(if stack_on { "Applied" } else { "Switched off" })
                .color(p.text_dim)
                .small(),
        );
        if ui
            .small_button("Apply")
            .on_hover_text("Run the stack into the pixels; after this it stops being editable")
            .clicked()
        {
            queued.push(Action::ApplyAttachedFilters);
        }
    });
    ui.add_space(2.0);

    // Bottom-first, which is the order they run in.
    for (i, (name, on, opacity)) in rows.iter().enumerate() {
        ui.horizontal(|ui| {
            let mut enabled = *on;
            if ui.checkbox(&mut enabled, "").changed() {
                queued.push(Action::ToggleAttachedFilter(i));
            }
            let text = if stack_on && *on {
                egui::RichText::new(name)
            } else {
                egui::RichText::new(name).color(p.text_dim)
            };
            if ui.add(egui::Label::new(text).sense(egui::Sense::click())).clicked() {
                queued.push(Action::EditAttachedFilter(i));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("✕").on_hover_text("Remove").clicked() {
                    queued.push(Action::RemoveAttachedFilter(i));
                }
                let mut k = *opacity;
                if ui
                    .add(
                        egui::DragValue::new(&mut k)
                            .range(0.0..=1.0)
                            .speed(0.01)
                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                    )
                    .changed()
                {
                    queued.push(Action::SetAttachedFilterOpacity(i, k));
                }
            });
        });
    }

    for action in queued {
        app.push(action);
    }
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

/// Editors for the active layer. Today that means adjustment layers.
fn properties_panel(app: &mut CShopApp, ui: &mut egui::Ui) {
    let Some(view) = app.doc() else { return };
    let Some(layer) = view.doc.active_layer() else { return };
    let Some(current) = layer.adjustment_settings() else { return };

    let mut edited = current.clone();
    ui.label(egui::RichText::new(edited.name()).strong());
    ui.add_space(4.0);

    let histogram = app.histogram();
    let changed = egui::ScrollArea::vertical()
        .id_salt("properties-scroll")
        .max_height(360.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            crate::properties::adjustment_editor(ui, &mut edited, histogram)
        })
        .inner;

    if changed {
        app.push(Action::SetAdjustment(Box::new(edited)));
    }
}

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

/// The colour channels, plus any saved selections.
///
/// Per-channel editing is not implemented; what is here is the part that earns
/// its place today — storing selections and loading them back.
fn channels_panel(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;
    let Some(view) = app.doc() else {
        ui.label(egui::RichText::new("No document").color(p.text_dim).italics());
        return;
    };

    ui.label(egui::RichText::new("RGB").color(p.text));
    for (name, colour) in [
        ("Red", egui::Color32::from_rgb(0xd0, 0x50, 0x50)),
        ("Green", egui::Color32::from_rgb(0x50, 0xc0, 0x60)),
        ("Blue", egui::Color32::from_rgb(0x60, 0x80, 0xd8)),
    ] {
        ui.label(egui::RichText::new(name).color(colour).small());
    }

    let channels: Vec<(String, bool)> =
        view.doc.channels.iter().map(|c| (c.name.clone(), c.visible)).collect();

    if !channels.is_empty() {
        ui.add_space(4.0);
        ui.separator();
    }
    let mut load = None;
    let mut delete = None;
    let mut toggle = None;
    for (i, (name, visible)) in channels.iter().enumerate() {
        ui.horizontal(|ui| {
            let (rect, r) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
            icons::icon(
                &ui.painter_at(rect),
                rect.shrink(1.0),
                if *visible { Icon::Eye } else { Icon::EyeOff },
                if *visible { p.text } else { p.text_dim },
            );
            if r.on_hover_text("Show this channel").clicked() {
                toggle = Some(i);
            }
            if ui
                .selectable_label(false, name)
                .on_hover_text("Click to load as a selection")
                .clicked()
            {
                load = Some(i);
            }
            if icons::icon_button(ui, Icon::Trash, 16.0, "Delete channel").clicked() {
                delete = Some(i);
            }
        });
    }

    ui.add_space(4.0);
    let has_selection = app.doc().is_some_and(|d| d.doc.has_selection());
    ui.add_enabled_ui(has_selection, |ui| {
        if ui.button("Save Selection").clicked() {
            app.push(Action::SaveSelectionAsChannel);
        }
    });

    if let Some(i) = toggle {
        app.push(Action::ToggleChannelVisible(i));
    }
    if let Some(i) = load {
        app.push(Action::LoadChannelAsSelection(i));
    }
    if let Some(i) = delete {
        app.push(Action::DeleteChannel(i));
    }
}

/// A collapsible panel with a compact title bar.
///
/// `open_by_default` sets the initial state; the user's choice is remembered
/// per panel for the session.
fn section(
    ui: &mut egui::Ui,
    title: &str,
    open_by_default: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    let p = Palette::DARK;
    let id = ui.id().with(("section", title));
    let mut open: bool = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(open_by_default);

    let header = egui::Frame::NONE
        .fill(p.header)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                icons::icon(
                    &ui.painter_at(rect),
                    rect,
                    if open { Icon::ChevronDown } else { Icon::ChevronRight },
                    p.text_dim,
                );
                ui.label(egui::RichText::new(title).strong().small());
            });
        });

    // The whole title bar toggles, not just the chevron.
    let response = ui.interact(header.response.rect, id.with("hit"), egui::Sense::click());
    if response.clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_temp(id, open));
    }

    if open {
        egui::Frame::NONE
            .fill(p.panel)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, body);
    }
}

fn section_fill(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    let p = Palette::DARK;
    egui::Frame::NONE.fill(p.header).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
        ui.label(egui::RichText::new(title).strong().small());
    });
    egui::Frame::NONE.fill(p.panel).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_min_height(ui.available_height());
        body(ui);
    });
}

// ---------------------------------------------------------------------------
// Color
// ---------------------------------------------------------------------------

fn color_panel(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;

    // The two swatches, each opening the picker on click.
    ui.horizontal(|ui| {
        for (target, colour, hint) in [
            (
                crate::dialogs::PickerTarget::Foreground,
                app.foreground,
                "Foreground colour — click to choose",
            ),
            (
                crate::dialogs::PickerTarget::Background,
                app.background,
                "Background colour — click to choose",
            ),
        ] {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(30.0, 22.0), egui::Sense::click());
            ui.painter().rect_filled(
                rect,
                2.0,
                egui::Color32::from_rgb(colour.r, colour.g, colour.b),
            );
            ui.painter().rect_stroke(
                rect,
                2.0,
                egui::Stroke::new(1.0, p.separator),
                egui::StrokeKind::Inside,
            );
            if response.on_hover_text(hint).clicked() {
                app.push(Action::ShowColorPicker(target));
            }
        }

        let mut hex = app.foreground.to_hex();
        let r = ui.add(egui::TextEdit::singleline(&mut hex).desired_width(66.0));
        if r.changed() {
            if let Some(c) = cshop_core::color::Rgba8::from_hex(&hex) {
                app.foreground = c;
            }
        }
        if icons::icon_button(ui, Icon::Swap, 18.0, "Swap (X)").clicked() {
            app.push(Action::SwapColors);
        }
    });

    ui.add_space(3.0);
    if ui
        .button("Custom…")
        .on_hover_text("Choose the foreground colour")
        .clicked()
    {
        app.push(Action::ShowColorPicker(crate::dialogs::PickerTarget::Foreground));
    }

    // A compact swatch strip, standing in for the full Swatches panel.
    ui.add_space(4.0);
    const SWATCHES: &[[u8; 3]] = &[
        [0, 0, 0],
        [255, 255, 255],
        [128, 128, 128],
        [237, 28, 36],
        [255, 127, 39],
        [255, 242, 0],
        [34, 177, 76],
        [0, 162, 232],
        [63, 72, 204],
        [163, 73, 164],
    ];
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
        for c in SWATCHES {
            let (rect, r) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
            ui.painter().rect_filled(rect, 1.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
            ui.painter().rect_stroke(
                rect,
                1.0,
                egui::Stroke::new(1.0, Palette::DARK.separator),
                egui::StrokeKind::Inside,
            );
            if r.clicked() {
                app.foreground = cshop_core::color::Rgba8::opaque(c[0], c[1], c[2]);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

fn history_panel(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;
    let Some(view) = app.doc() else {
        ui.label(egui::RichText::new("No document").color(p.text_dim).italics());
        return;
    };

    let origin = view.history.origin().to_string();
    let labels = view.history.labels();
    let cursor = view.history.cursor();
    let marked = view.history_source.as_ref().map(|(at, ..)| *at);
    let mut jump = None;
    let mut mark = None;

    // A state's name, with a marker in front when the History Brush paints
    // back to it. Clicking the name goes there; the marker column sets where
    // the brush paints from, which is a different thing and needs its own
    // target rather than a modifier nobody would guess.
    let brush_mark = |ui: &mut egui::Ui, state: usize, marked: Option<usize>| {
        let on = marked == Some(state);
        let glyph = if on { "◉" } else { "○" };
        let hint = if on {
            "The History Brush paints back to this state"
        } else {
            "Paint back to this state with the History Brush"
        };
        ui.add(egui::Label::new(egui::RichText::new(glyph).monospace()).sense(egui::Sense::click()))
            .on_hover_text(hint)
            .clicked()
    };

    egui::ScrollArea::vertical()
        .id_salt("history-scroll")
        .max_height(140.0)
        .auto_shrink([false, true])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            // Row 0 is the document as it was opened or created.
            ui.horizontal(|ui| {
                if brush_mark(ui, 0, marked) {
                    mark = Some(0);
                }
                if ui.selectable_label(cursor == 0, origin.clone()).clicked() {
                    jump = Some(0);
                }
            });
            for (i, label) in labels.iter().enumerate() {
                let applied = i < cursor;
                let text = if applied {
                    egui::RichText::new(label)
                } else {
                    // Undone states stay listed but dimmed, so redo is discoverable.
                    egui::RichText::new(label).color(p.text_dim)
                };
                ui.horizontal(|ui| {
                    if brush_mark(ui, i + 1, marked) {
                        mark = Some(i + 1);
                    }
                    if ui.selectable_label(cursor == i + 1, text).clicked() {
                        jump = Some(i + 1);
                    }
                });
            }
        });

    if let Some(target) = jump {
        app.push(Action::HistoryJump(target));
    }
    if let Some(target) = mark {
        app.push(Action::SetHistorySource(target));
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let (can_undo, can_redo) = match app.doc() {
            Some(v) => (v.history.can_undo(), v.history.can_redo()),
            None => (false, false),
        };
        ui.add_enabled_ui(can_undo, |ui| {
            if icons::icon_button(ui, Icon::Undo, 18.0, "Undo (Ctrl+Z)").clicked() {
                app.push(Action::Undo);
            }
        });
        ui.add_enabled_ui(can_redo, |ui| {
            if icons::icon_button(ui, Icon::Redo, 18.0, "Redo (Ctrl+Shift+Z)").clicked() {
                app.push(Action::Redo);
            }
        });
        ui.label(
            egui::RichText::new(format!("{cursor} of {}", labels.len())).color(p.text_dim).small(),
        );
    });
}

// ---------------------------------------------------------------------------
// Layers
// ---------------------------------------------------------------------------

fn layers_panel(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;
    let Some(index) = app.active else {
        ui.label(egui::RichText::new("No document").color(p.text_dim).italics());
        return;
    };

    // --- blend mode and opacity for the active layer ----------------------
    let active = app.docs[index].doc.active;
    if let Some(id) = active {
        let (mode, opacity, fill, locks) = {
            let l = app.docs[index].doc.tree.get(id);
            match l {
                Some(l) => (l.blend_mode, l.opacity, l.fill_opacity, l.locks),
                None => (BlendMode::Normal, 1.0, 1.0, Default::default()),
            }
        };

        ui.horizontal(|ui| {
            let mut selected = mode;
            egui::ComboBox::from_id_salt("blend")
                .width(130.0)
                .selected_text(mode.name())
                .show_ui(ui, |ui| {
                    for entry in BlendMode::MENU {
                        match entry {
                            Some(m) => {
                                ui.selectable_value(&mut selected, *m, m.name());
                            }
                            None => {
                                ui.separator();
                            }
                        }
                    }
                });
            if selected != mode {
                app.push(Action::SetLayerProperty(id, LayerProperty::Blend(selected)));
            }

            ui.label("Opacity:");
            let mut o = opacity;
            if ui.add(opacity_drag(&mut o)).changed() {
                app.push(Action::SetLayerProperty(id, LayerProperty::Opacity(o)));
            }
        });

        ui.horizontal(|ui| {
            ui.label("Lock:");
            ui.spacing_mut().item_spacing.x = 2.0;
            for (icon, on, hover, make) in [
                (
                    Icon::LockTransparency,
                    locks.transparency,
                    "Lock transparent pixels",
                    LayerProperty::LockTransparency as fn(bool) -> LayerProperty,
                ),
                (Icon::LockPixels, locks.pixels, "Lock image pixels", LayerProperty::LockPixels),
                (
                    Icon::LockPosition,
                    locks.position,
                    "Lock position",
                    LayerProperty::LockPosition,
                ),
                (Icon::Lock, locks.all, "Lock all", LayerProperty::LockAll),
            ] {
                if lock_toggle(ui, icon, on, hover) {
                    app.push(Action::SetLayerProperty(id, make(!on)));
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut f = fill;
                if ui.add(opacity_drag(&mut f)).changed() {
                    app.push(Action::SetLayerProperty(id, LayerProperty::FillOpacity(f)));
                }
                ui.label("Fill:");
            });
        });
    }

    ui.add_space(4.0);
    ui.separator();

    // --- the stack --------------------------------------------------------
    let rows = app.docs[index].doc.tree.visible_rows();
    let mut clicked = None;
    let mut toggled = None;
    let mut expanded = None;
    let mut drop: Option<(LayerId, LayerPos)> = None;
    let mut row_rects: Vec<egui::Rect> = Vec::new();
    let mut edit_target = None;
    let mut toggle_mask = false;

    // Reserve room for the button row before the list takes what is left.
    // Letting the list fill the panel pushed the footer off the bottom, where
    // it was drawn but never visible.
    const FOOTER_HEIGHT: f32 = 34.0;
    let list_height = (ui.available_height() - FOOTER_HEIGHT).max(60.0);

    egui::ScrollArea::vertical()
        .id_salt("layers-scroll")
        .auto_shrink([false, false])
        .max_height(list_height)
        .show(ui, |ui| {
            ui.set_min_height(list_height);
            for (id, depth) in rows.iter().copied() {
                let r = layer_row(app, ui, index, id, depth);
                // The row has given `app` back, so the menu can borrow it.
                if let Some(response) = &r.response {
                    response.context_menu(|ui| {
                        crate::context_menus::layer_menu(app, ui, id);
                    });
                }
                if r.select {
                    clicked = Some(id);
                }
                if let Some(v) = r.visible {
                    toggled = Some((id, v));
                }
                if let Some(v) = r.expanded {
                    expanded = Some((id, v));
                }
                if let Some(rect) = r.rect {
                    row_rects.push(rect);
                }
                if let Some(t) = r.edit_target {
                    edit_target = Some(t);
                }
                if r.toggle_mask {
                    toggle_mask = true;
                }
            }

            // Resolve the drag here rather than in a row: only the panel can
            // see every row, and a row that cleared the drag state before the
            // row under the pointer had been drawn used to swallow the drop.
            drop = resolve_layer_drag(app, ui, index, &rows, &row_rects);
        });

    if let Some(id) = clicked {
        app.push(Action::SelectLayer(id));
    }
    if let Some((id, v)) = toggled {
        app.push(Action::SetLayerProperty(id, LayerProperty::Visible(v)));
    }
    if let Some((id, v)) = expanded {
        app.push(Action::SetLayerProperty(id, LayerProperty::Expanded(v)));
    }
    if let Some((id, pos)) = drop {
        app.push(Action::MoveLayer(id, pos));
    }
    if let Some(t) = edit_target {
        app.push(Action::SetEditTarget(t));
    }
    if toggle_mask {
        app.push(Action::ToggleMaskEnabled);
    }

    // --- footer buttons ---------------------------------------------------
    ui.add_space(3.0);
    ui.separator();
    layer_buttons(app, ui);
}

/// The row of layer commands along the bottom of the Layers panel.
fn layer_buttons(app: &mut CShopApp, ui: &mut egui::Ui) {
    let has_doc = app.doc().is_some();
    // Merging needs something below to merge into, and flattening needs more
    // than one layer to be worth doing.
    let layer_count = app.doc().map(|d| d.doc.tree.len()).unwrap_or(0);
    let can_merge_down = app
        .doc()
        .and_then(|d| d.doc.active.and_then(|id| d.doc.tree.position(id)))
        .is_some_and(|pos| pos.index > 0);
    let can_delete = layer_count > 1;

    let mask_action = if app.doc().is_some_and(|d| d.doc.has_selection()) {
        Action::AddLayerMaskFromSelection { invert: false }
    } else {
        Action::AddLayerMask { hide_all: false }
    };

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (icon, hover, action, enabled) in [
            (
                Icon::Mask,
                "Add layer mask — from the selection when there is one",
                mask_action,
                has_doc,
            ),
            (Icon::Folder, "New group", Action::NewGroup, has_doc),
            (Icon::Plus, "New layer  (Ctrl+Shift+N)", Action::NewLayer, has_doc),
            (Icon::Duplicate, "Duplicate layer", Action::DuplicateLayer, has_doc),
            (
                Icon::MergeDown,
                "Merge down  (Ctrl+E)",
                Action::MergeDown,
                can_merge_down,
            ),
            (
                Icon::Flatten,
                "Merge all layers into one",
                Action::FlattenImage,
                layer_count > 1,
            ),
            (Icon::Trash, "Delete layer", Action::DeleteLayer, can_delete),
        ] {
            // Greyed out rather than hidden, so the row stays in one place and
            // the reason a command is unavailable is visible.
            ui.add_enabled_ui(enabled, |ui| {
                if icons::icon_button(ui, icon, 22.0, hover).clicked() {
                    app.push(action);
                }
            });
        }
    });
}

fn opacity_drag(value: &mut f32) -> egui::DragValue<'_> {
    egui::DragValue::new(value)
        .range(0.0..=1.0)
        .speed(0.005)
        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
        .custom_parser(|s| s.trim_end_matches('%').parse::<f64>().ok().map(|v| v / 100.0))
}

/// A small toggle that draws a vector icon; returns `true` when clicked.
fn lock_toggle(ui: &mut egui::Ui, which: Icon, on: bool, hover: &str) -> bool {
    let p = Palette::DARK;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(20.0, 18.0), egui::Sense::click());
    if on {
        ui.painter().rect_filled(rect, 2.0, p.accent);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 2.0, p.widget_hover);
    }
    let color = if on { egui::Color32::WHITE } else { p.text };
    icons::icon(&ui.painter_at(rect), rect.shrink(3.0), which, color);
    response.on_hover_text(hover).clicked()
}

/// Stable per-row id, so the panel and the tests can both find a row rather
/// than recomputing where the list thinks it put one.
pub fn layer_row_id(id: LayerId) -> egui::Id {
    egui::Id::new(("layer-row", id.0))
}

/// Where the id of the layer being dragged is parked between frames.
const DRAG_KEY: &str = "layer-drag";

fn drag_id() -> egui::Id {
    egui::Id::new(DRAG_KEY)
}

#[derive(Default)]
struct RowResult {
    select: bool,
    visible: Option<bool>,
    expanded: Option<bool>,
    /// Where the row was laid out, so the panel can hit-test the drag.
    rect: Option<egui::Rect>,
    edit_target: Option<EditTarget>,
    toggle_mask: bool,
    /// Returned so the caller can hang a context menu on it once the row's
    /// borrow of `app` has ended.
    response: Option<egui::Response>,
}

fn layer_row(
    app: &mut CShopApp,
    ui: &mut egui::Ui,
    doc_index: usize,
    id: LayerId,
    depth: usize,
) -> RowResult {
    let p = Palette::DARK;
    let mut out = RowResult::default();

    let (
        name,
        visible,
        is_group,
        group_expanded,
        is_active,
        clipping,
        has_mask,
        locked,
        mask_on,
        is_smart,
    ) = {
        let view = &app.docs[doc_index];
        let Some(l) = view.doc.tree.get(id) else { return out };
        (
            l.name.clone(),
            l.visible,
            l.kind.is_group(),
            l.expanded,
            view.doc.active == Some(id),
            l.clipping,
            l.mask.is_some(),
            l.locks.any(),
            l.mask.as_ref().is_some_and(|m| m.enabled),
            l.smart().is_some(),
        )
    };
    // Effects get an "fx" mark and, when the group is open, a line each — the
    // only way to see what a style is made of without opening the dialog.
    let effect_names: Vec<&'static str> = app.docs[doc_index]
        .doc
        .tree
        .get(id)
        .map(|l| l.effects.active_names())
        .unwrap_or_default();
    // The Background is pinned to the bottom of the stack.
    let is_background = app.docs[doc_index]
        .doc
        .tree
        .get(id)
        .is_some_and(|l| l.is_background || l.locks.blocks_move());

    // Only the active layer shows which of its plates is being edited.
    let editing_mask =
        is_active && app.docs[doc_index].doc.effective_edit_target() == EditTarget::Mask;

    let thumbnail = app.docs[doc_index].thumbnail(ui.ctx(), id);
    let mask_thumbnail =
        if has_mask { app.docs[doc_index].mask_thumbnail(ui.ctx(), id) } else { None };

    let full = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(full, ROW_HEIGHT), egui::Sense::hover());
    // A fixed id rather than an allocated one, so the panel's drag logic and
    // the tests can both ask egui where a given layer's row ended up.
    let response = ui.interact(rect, layer_row_id(id), egui::Sense::click_and_drag());
    out.rect = Some(rect);
    // Right-clicking a row selects it as well as opening its menu, so the
    // commands in that menu act on what was clicked.
    if response.secondary_clicked() {
        out.select = true;
    }

    let painter = ui.painter_at(rect);
    let bg = if is_active {
        p.row_selected
    } else if response.hovered() {
        p.row_hover
    } else {
        p.panel
    };
    painter.rect_filled(rect, 0.0, bg);

    let indent = depth as f32 * 12.0;
    let mut x = rect.min.x + 2.0;

    // --- eye ---------------------------------------------------------------
    let eye_rect = egui::Rect::from_min_size(
        egui::pos2(x, rect.center().y - 9.0),
        egui::vec2(18.0, 18.0),
    );
    let eye = ui.interact(eye_rect, ui.id().with((id.0, "eye")), egui::Sense::click());
    icons::icon(
        &painter,
        eye_rect.shrink(2.0),
        if visible { Icon::Eye } else { Icon::EyeOff },
        if visible { p.text } else { p.text_dim },
    );
    if eye.clicked() {
        out.visible = Some(!visible);
    }
    x += 20.0 + indent;

    // --- group disclosure --------------------------------------------------
    if is_group {
        let tri_rect =
            egui::Rect::from_min_size(egui::pos2(x, rect.center().y - 7.0), egui::vec2(14.0, 14.0));
        let tri = ui.interact(tri_rect, ui.id().with((id.0, "tri")), egui::Sense::click());
        icons::icon(
            &painter,
            tri_rect.shrink(2.0),
            if group_expanded { Icon::ChevronDown } else { Icon::ChevronRight },
            p.text,
        );
        if tri.clicked() {
            out.expanded = Some(!group_expanded);
        }
        x += 15.0;
    }

    // --- thumbnail ---------------------------------------------------------
    let thumb_rect =
        egui::Rect::from_min_size(egui::pos2(x, rect.center().y - THUMB / 2.0), egui::vec2(THUMB, THUMB));
    painter.rect_filled(thumb_rect, 0.0, egui::Color32::from_gray(0x20));
    match (&thumbnail, is_group) {
        (Some(handle), _) => {
            // Letterbox the thumbnail inside the square cell.
            let size = handle.size_vec2();
            let scale = (THUMB / size.x).min(THUMB / size.y);
            let draw = egui::Rect::from_center_size(thumb_rect.center(), size * scale);
            painter.image(
                handle.id(),
                draw,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        (None, true) => {
            icons::icon(&painter, thumb_rect.shrink(8.0), Icon::Folder, p.text_dim);
        }
        (None, false) => {}
    }
    // The active plate gets a bright outline.
    let pixel_selected = is_active && !editing_mask;
    painter.rect_stroke(
        thumb_rect,
        0.0,
        egui::Stroke::new(
            if pixel_selected { 2.0 } else { 1.0 },
            if pixel_selected { egui::Color32::WHITE } else { p.separator },
        ),
        egui::StrokeKind::Inside,
    );
    if ui
        .interact(thumb_rect, ui.id().with((id.0, "px")), egui::Sense::click())
        .on_hover_text("Edit the layer's pixels")
        .clicked()
    {
        out.select = true;
        out.edit_target = Some(EditTarget::Pixels);
    }
    x += THUMB + 4.0;

    // --- mask thumbnail ----------------------------------------------------
    if has_mask {
        // The link icon between the plates.
        let link = egui::Rect::from_min_size(
            egui::pos2(x, rect.center().y - 5.0),
            egui::vec2(10.0, 10.0),
        );
        icons::icon(&painter, link, Icon::MaskLink, p.text_dim);
        x += 12.0;

        let mask_rect = egui::Rect::from_min_size(
            egui::pos2(x, rect.center().y - THUMB / 2.0),
            egui::vec2(THUMB, THUMB),
        );
        painter.rect_filled(mask_rect, 0.0, egui::Color32::from_gray(0x20));
        if let Some(handle) = &mask_thumbnail {
            let size = handle.size_vec2();
            let scale = (THUMB / size.x).min(THUMB / size.y);
            painter.image(
                handle.id(),
                egui::Rect::from_center_size(mask_rect.center(), size * scale),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        painter.rect_stroke(
            mask_rect,
            0.0,
            egui::Stroke::new(
                if editing_mask { 2.0 } else { 1.0 },
                if editing_mask { egui::Color32::WHITE } else { p.separator },
            ),
            egui::StrokeKind::Inside,
        );
        // A disabled mask is struck through rather than hidden, so it is clear
        // the mask still exists.
        if !mask_on {
            painter.line_segment(
                [mask_rect.left_top(), mask_rect.right_bottom()],
                egui::Stroke::new(2.0, egui::Color32::from_rgb(0xd0, 0x50, 0x50)),
            );
        }

        let r = ui.interact(mask_rect, ui.id().with((id.0, "mask")), egui::Sense::click());
        if r.on_hover_text("Edit the layer mask · Shift-click to disable it").clicked() {
            out.select = true;
            if ui.input(|i| i.modifiers.shift) {
                out.toggle_mask = true;
            } else {
                out.edit_target = Some(EditTarget::Mask);
            }
        }
        x += THUMB + 8.0;
    } else {
        x += 4.0;
    }

    // --- name and badges ---------------------------------------------------
    painter.text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &name,
        egui::FontId::proportional(12.0),
        if visible { p.text } else { p.text_dim },
    );

    let mut badge_x = rect.max.x - 6.0;
    // A smart object looks like any other layer in the panel and behaves
    // unlike one — it cannot be painted on, and its transforms are free — so
    // it says which it is.
    if is_smart {
        let g = painter.layout_no_wrap(
            "◇".to_string(),
            egui::FontId::proportional(12.0),
            p.accent,
        );
        badge_x -= g.size().x + 4.0;
        painter.galley(egui::pos2(badge_x, rect.center().y - g.size().y / 2.0), g, p.accent);
        badge_x -= 2.0;
    }
    // An "fx" mark rather than an icon: it is what the effects are called
    // everywhere else in the interface, and it reads at this size.
    if !effect_names.is_empty() {
        let g = painter.layout_no_wrap(
            "fx".to_string(),
            egui::FontId::proportional(11.0),
            p.accent,
        );
        badge_x -= g.size().x + 4.0;
        painter.galley(
            egui::pos2(badge_x, rect.center().y - g.size().y / 2.0),
            g,
            p.accent,
        );
        badge_x -= 2.0;
    }
    for (show, which) in
        [(locked, Icon::Lock), (has_mask, Icon::Mask), (clipping, Icon::Clip)]
    {
        if !show {
            continue;
        }
        let badge = egui::Rect::from_min_size(
            egui::pos2(badge_x - 13.0, rect.center().y - 6.5),
            egui::vec2(13.0, 13.0),
        );
        icons::icon(&painter, badge, which, p.text_dim);
        badge_x -= 15.0;
    }

    // --- interaction -------------------------------------------------------
    if response.clicked() {
        out.select = true;
    }
    if response.double_clicked() {
        // Renaming is what a double-click means everywhere else, and until now
        // there was no way to rename a layer at all.
        out.select = true;
        app.push(Action::RenameLayer(id));
    }

    // The drag itself is run by the panel, which is the only place that can
    // see every row at once. All a row does is say that one started on it.
    if response.drag_started() && !is_background {
        ui.ctx().data_mut(|d| d.insert_temp(drag_id(), id.0));
    }

    out.response = Some(response);
    out
}

/// Which gap the pointer is nearest, as an index into `rows` meaning "insert
/// above this row". `rows.len()` means below everything.
///
/// Each row is split in half so a drop lands on the side of it the insertion
/// line was drawn, which is the whole point of showing the line.
fn insertion_gap(rects: &[egui::Rect], pointer: egui::Pos2) -> usize {
    for (i, rect) in rects.iter().enumerate() {
        if pointer.y < rect.center().y {
            return i;
        }
        if pointer.y < rect.max.y {
            return i + 1;
        }
    }
    rects.len()
}

/// Turn a gap into a position in the tree.
///
/// The panel lists layers top-first while the tree stores them bottom-first,
/// so "above row i" is a *higher* index in that row's parent. The index is the
/// one [`cshop_core::tree::LayerTree::move_to`] wants — a slot in the sibling
/// list as it stands *before* the layer is lifted out — which is exactly what
/// a gap between two rows describes.
fn gap_position(view: &crate::doc_view::DocView, rows: &[(LayerId, usize)], gap: usize) -> LayerPos {
    let root_bottom = LayerPos { parent: None, index: 0 };
    if rows.is_empty() {
        return root_bottom;
    }
    if gap < rows.len() {
        // Directly above rows[gap].
        return match view.doc.tree.position(rows[gap].0) {
            Some(pos) => LayerPos { parent: pos.parent, index: pos.index + 1 },
            None => root_bottom,
        };
    }
    // Past the last row: directly below it.
    match view.doc.tree.position(rows[rows.len() - 1].0) {
        Some(pos) => LayerPos { parent: pos.parent, index: pos.index },
        None => root_bottom,
    }
}

/// Draw the insertion line and, on release, report where to move the layer.
fn resolve_layer_drag(
    app: &CShopApp,
    ui: &egui::Ui,
    doc_index: usize,
    rows: &[(LayerId, usize)],
    rects: &[egui::Rect],
) -> Option<(LayerId, LayerPos)> {
    let dragged: u64 = ui.ctx().data(|d| d.get_temp(drag_id()))?;
    let dragged = LayerId(dragged);
    let released = ui.input(|i| i.pointer.any_released());
    let clear = || ui.ctx().data_mut(|d| d.remove::<u64>(drag_id()));

    let Some(pointer) = ui.ctx().pointer_interact_pos() else {
        if released {
            clear();
        }
        return None;
    };

    let view = &app.docs[doc_index];
    let gap = insertion_gap(rects, pointer);
    let pos = gap_position(view, rows, gap);

    // A drop that would change nothing, or that the tree would refuse, should
    // not draw a line promising otherwise.
    let allowed = view.doc.tree.position(dragged).is_some_and(|from| {
        let same_place = from.parent == pos.parent
            && (from.index == pos.index || from.index + 1 == pos.index);
        let into_itself = pos.parent.is_some_and(|p| {
            p == dragged || view.doc.tree.is_ancestor(dragged, p)
        });
        // Nothing may pass below a pinned Background.
        let under_background = rows.last().is_some_and(|(last, _)| {
            gap == rows.len()
                && view.doc.tree.get(*last).is_some_and(|l| l.is_background)
        });
        !same_place && !into_itself && !under_background
    });

    if allowed {
        // The line is drawn on the gap the drop would use, so where it lands
        // is never a surprise.
        let y = match rects.get(gap) {
            Some(r) => r.min.y,
            None => rects.last().map(|r| r.max.y).unwrap_or(pointer.y),
        };
        let (x0, x1) = rects
            .first()
            .map(|r| (r.min.x, r.max.x))
            .unwrap_or((pointer.x - 40.0, pointer.x + 40.0));
        let p = Palette::DARK;
        ui.painter().line_segment(
            [egui::pos2(x0, y), egui::pos2(x1, y)],
            egui::Stroke::new(2.0, p.accent),
        );
        ui.painter().circle_filled(egui::pos2(x0 + 3.0, y), 3.0, p.accent);
    }

    if released {
        clear();
        if allowed {
            return Some((dragged, pos));
        }
    }
    None
}
