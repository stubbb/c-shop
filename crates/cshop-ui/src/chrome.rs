//! Application chrome: menu bar, toolbox, tool options bar, document tabs and
//! status bar.

use crate::app::CShopApp;
use crate::shortcuts::keys as k;
use crate::commands::Action;
use crate::theme::Palette;
use crate::commands::{ModifySelection, ResizeEdge, TransformPreset, WindowCommand};
use crate::dialogs::{Dialog, ModifyDialog};
use crate::icons::{self, Icon};
use crate::tools::{Tool, TOOL_GROUPS};
use cshop_core::document::EditTarget;
use cshop_core::selection::SelectionMode;

// ---------------------------------------------------------------------------
// Title bar
// ---------------------------------------------------------------------------

/// Height of the custom title bar, in points.
pub const TITLE_BAR_HEIGHT: f32 = 30.0;
/// How close to an edge counts as a resize grab.
const RESIZE_MARGIN: f32 = 5.0;

/// The window's own title bar: logo, menus, document name and window buttons.
///
/// The platform's decorations are switched off so the window matches the rest
/// of the interface, which means moving, maximising and closing all have to be
/// handled here. The menus live in this bar too, and doing that
/// buys back a row of vertical space.
pub fn title_bar(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;
    let full = ui.max_rect();

    // Register the drag handle *first*. In egui the widget added last is the
    // one on top, so anything drawn after this — the menus, the window buttons
    // — takes the click instead. Registering it last made the whole bar a drag
    // surface and left its own menus and close button dead.
    //
    // It is created here but acted on at the end of this function, once the
    // menus and buttons have reported where they are: a menu button senses
    // clicks and not drags, so a press-and-drag on one would otherwise open
    // the menu *and* start moving the window.
    let drag = ui.interact(full, ui.id().with("titlebar-drag"), egui::Sense::click_and_drag());

    let menus_end = ui.horizontal_centered(|ui| {
        ui.add_space(6.0);

        // --- logo ---------------------------------------------------------
        let logo = app.logo(ui.ctx());
        let (logo_rect, _) = ui.allocate_exact_size(egui::vec2(19.0, 19.0), egui::Sense::hover());
        ui.painter().image(
            logo.id(),
            logo_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        ui.add_space(6.0);

        // --- menus --------------------------------------------------------
        menu_bar(app, ui)
    })
    .inner;
    let menus = egui::Rect::from_min_max(full.min, egui::pos2(menus_end, full.max.y));

    // --- window buttons ---------------------------------------------------
    // Laid out from the right edge of the whole bar rather than inside the
    // menu row, so the menus keep their natural width.
    let button_width = 30.0;
    let mut x = full.max.x;
    let mut buttons = egui::Rect::NOTHING;
    for (index, (icon, command)) in [
        (WindowIcon::Close, WindowCommand::Close),
        (
            if app.is_maximized { WindowIcon::Restore } else { WindowIcon::Maximize },
            WindowCommand::ToggleMaximize,
        ),
        (WindowIcon::Minimize, WindowCommand::Minimize),
    ]
    .into_iter()
    .enumerate()
    {
        let rect = egui::Rect::from_min_max(
            egui::pos2(x - button_width, full.min.y),
            egui::pos2(x, full.max.y),
        );
        x -= button_width;
        buttons = buttons.union(rect);

        let response = ui.interact(rect, ui.id().with(("winbtn", index)), egui::Sense::click());
        if response.hovered() {
            // Red for close, a neutral highlight for the rest, as every
            // desktop does it.
            let fill = if command == WindowCommand::Close {
                egui::Color32::from_rgb(0xc4, 0x2b, 0x1c)
            } else {
                p.widget_hover
            };
            ui.painter().rect_filled(rect, 0.0, fill);
        }
        let colour = if response.hovered() && command == WindowCommand::Close {
            egui::Color32::WHITE
        } else {
            p.text
        };
        window_icon(&ui.painter_at(rect), rect, icon, colour);
        if response.clicked() {
            app.window(command);
        }
    }

    // --- move and maximise ---------------------------------------------------
    // Only the bar's own empty space is a drag surface.
    let over_child = ui
        .ctx()
        .pointer_interact_pos()
        .is_some_and(|pos| menus.contains(pos) || buttons.contains(pos));
    if !over_child {
        if drag.drag_started_by(egui::PointerButton::Primary) {
            app.window(WindowCommand::StartDrag);
        }
        // Double-click to maximise, as on every platform.
        if drag.double_clicked_by(egui::PointerButton::Primary) {
            app.window(WindowCommand::ToggleMaximize);
        }
    }

    // --- document title, centred --------------------------------------------
    if let Some(view) = app.doc() {
        let mark = if view.doc.modified { " •" } else { "" };
        ui.painter().text(
            egui::pos2(full.center().x, full.center().y),
            egui::Align2::CENTER_CENTER,
            format!("{}{mark}  —  C-Shop", view.doc.name),
            egui::FontId::proportional(11.5),
            p.text_dim,
        );
    } else {
        ui.painter().text(
            egui::pos2(full.center().x, full.center().y),
            egui::Align2::CENTER_CENTER,
            "C-Shop",
            egui::FontId::proportional(11.5),
            p.text_dim,
        );
    }

}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WindowIcon {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// The window buttons, drawn rather than typed: these glyphs are not in egui's
/// bundled fonts either.
fn window_icon(painter: &egui::Painter, rect: egui::Rect, icon: WindowIcon, colour: egui::Color32) {
    let c = rect.center();
    let s = 4.5;
    let stroke = egui::Stroke::new(1.0, colour);
    match icon {
        WindowIcon::Minimize => {
            painter.line_segment(
                [egui::pos2(c.x - s, c.y + 4.0), egui::pos2(c.x + s, c.y + 4.0)],
                stroke,
            );
        }
        WindowIcon::Maximize => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, egui::vec2(s * 2.0, s * 2.0)),
                0.0,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        WindowIcon::Restore => {
            // Two offset squares, the usual "restore down" mark.
            painter.rect_stroke(
                egui::Rect::from_min_size(
                    egui::pos2(c.x - s, c.y - s + 2.0),
                    egui::vec2(s * 2.0 - 2.0, s * 2.0 - 2.0),
                ),
                0.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [egui::pos2(c.x - s + 2.0, c.y - s), egui::pos2(c.x + s, c.y - s)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x + s, c.y - s), egui::pos2(c.x + s, c.y + s - 2.0)],
                stroke,
            );
        }
        WindowIcon::Close => {
            painter.line_segment(
                [egui::pos2(c.x - s, c.y - s), egui::pos2(c.x + s, c.y + s)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x + s, c.y - s), egui::pos2(c.x - s, c.y + s)],
                stroke,
            );
        }
    }
}

/// Invisible strips along the window edges that start a resize.
///
/// Without the platform's decorations there are no resize borders, so the
/// application has to provide them. Drawn last, above every panel, or a panel
/// filling the window would swallow the edge.
pub fn resize_borders(app: &mut CShopApp, ui: &mut egui::Ui) {
    let screen = ui.ctx().viewport_rect();

    // Without a platform frame the window has no outline of its own, and a
    // dark interface on a dark desktop has no visible edge at all.
    ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("window-outline"),
    ))
    .rect_stroke(
        screen,
        0.0,
        egui::Stroke::new(1.0, Palette::DARK.window_edge),
        egui::StrokeKind::Inside,
    );

    // A maximised window has no edges to drag.
    if app.is_maximized {
        return;
    }
    let m = RESIZE_MARGIN;

    // Corners first: they overlap the sides and must win.
    let zones: [(egui::Rect, ResizeEdge); 8] = [
        (
            egui::Rect::from_min_max(screen.min, screen.min + egui::vec2(m, m)),
            ResizeEdge::NorthWest,
        ),
        (
            egui::Rect::from_min_max(
                egui::pos2(screen.max.x - m, screen.min.y),
                egui::pos2(screen.max.x, screen.min.y + m),
            ),
            ResizeEdge::NorthEast,
        ),
        (
            egui::Rect::from_min_max(
                egui::pos2(screen.min.x, screen.max.y - m),
                egui::pos2(screen.min.x + m, screen.max.y),
            ),
            ResizeEdge::SouthWest,
        ),
        (
            egui::Rect::from_min_max(screen.max - egui::vec2(m, m), screen.max),
            ResizeEdge::SouthEast,
        ),
        (
            egui::Rect::from_min_max(
                egui::pos2(screen.min.x + m, screen.min.y),
                egui::pos2(screen.max.x - m, screen.min.y + m),
            ),
            ResizeEdge::North,
        ),
        (
            egui::Rect::from_min_max(
                egui::pos2(screen.min.x + m, screen.max.y - m),
                egui::pos2(screen.max.x - m, screen.max.y),
            ),
            ResizeEdge::South,
        ),
        (
            egui::Rect::from_min_max(
                egui::pos2(screen.min.x, screen.min.y + m),
                egui::pos2(screen.min.x + m, screen.max.y - m),
            ),
            ResizeEdge::West,
        ),
        (
            egui::Rect::from_min_max(
                egui::pos2(screen.max.x - m, screen.min.y + m),
                egui::pos2(screen.max.x, screen.max.y - m),
            ),
            ResizeEdge::East,
        ),
    ];

    egui::Area::new(egui::Id::new("resize-borders"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            for (index, (rect, edge)) in zones.into_iter().enumerate() {
                let response =
                    ui.interact(rect, egui::Id::new(("resize", index)), egui::Sense::drag());
                if response.hovered() || response.dragged() {
                    ui.ctx().set_cursor_icon(edge.cursor());
                }
                if response.drag_started_by(egui::PointerButton::Primary) {
                    app.window(WindowCommand::StartResize(edge));
                }
            }
        });
}

// ---------------------------------------------------------------------------
// Menu bar
// ---------------------------------------------------------------------------

/// Returns the x coordinate just past the last menu, so the title bar knows
/// which part of itself is free to drag the window by. `MenuBar` fills the
/// width it is given, so neither its response nor the surrounding layout's
/// cursor can answer that.
pub fn menu_bar(app: &mut CShopApp, ui: &mut egui::Ui) -> f32 {
    egui::MenuBar::new()
        .ui(ui, |ui| {
        let has_doc = app.doc().is_some();

        ui.menu_button("File", |ui| {
            if item(ui, "New…", &k::NEW.label()).clicked() {
                app.push(Action::NewDocument);
                ui.close();
            }
            if item(ui, "Open…", &k::OPEN.label()).clicked() {
                app.push(Action::ShowOpenDialog);
                ui.close();
            }
            // Read before the menu is built, so the borrow ends before the
            // items below need `app` mutably.
            let recent: Vec<std::path::PathBuf> = app.settings.recent.clone();
            ui.add_enabled_ui(!recent.is_empty(), |ui| {
                ui.menu_button("Open Recent", |ui| {
                    for path in &recent {
                        let shown = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string());
                        if item(ui, &shown, "").on_hover_text(path.display().to_string()).clicked()
                        {
                            app.push(Action::OpenPath(path.clone()));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if item(ui, "Clear", "").clicked() {
                        app.push(Action::ClearRecent);
                        ui.close();
                    }
                });
            });
            ui.separator();
            if item_enabled(ui, "Save", &k::SAVE.label(), has_doc).clicked() {
                app.push(Action::Save);
                ui.close();
            }
            if item_enabled(ui, "Save As…", &k::SAVE_AS.label(), has_doc).clicked() {
                app.push(Action::ShowSaveAsDialog);
                ui.close();
            }
            ui.separator();
            if item_enabled(ui, "Close", &k::CLOSE.label(), has_doc).clicked() {
                app.push(Action::CloseDocument(usize::MAX));
                ui.close();
            }
            if item(ui, "Quit", &k::QUIT.label()).clicked() {
                app.push(Action::Quit);
                ui.close();
            }
        });

        ui.menu_button("Edit", |ui| {
            let (undo, redo) = match app.doc() {
                Some(d) => (d.history.undo_name(), d.history.redo_name()),
                None => (None, None),
            };
            let undo_label =
                undo.clone().map(|n| format!("Undo {n}")).unwrap_or_else(|| "Undo".into());
            let redo_label =
                redo.clone().map(|n| format!("Redo {n}")).unwrap_or_else(|| "Redo".into());

            if item_enabled(ui, &undo_label, &k::UNDO.label(), undo.is_some()).clicked() {
                app.push(Action::Undo);
                ui.close();
            }
            if item_enabled(ui, &redo_label, &k::REDO.label(), redo.is_some()).clicked() {
                app.push(Action::Redo);
                ui.close();
            }
            ui.separator();
            if item_enabled(ui, "Cut", &k::CUT.label(), has_doc).clicked() {
                app.push(Action::Cut);
                ui.close();
            }
            if item_enabled(ui, "Copy", &k::COPY.label(), has_doc).clicked() {
                app.push(Action::Copy);
                ui.close();
            }
            if item_enabled(ui, "Copy Merged", &k::COPY_MERGED.label(), has_doc).clicked() {
                app.push(Action::CopyMerged);
                ui.close();
            }
            let can_paste = app.clipboard.has_content();
            if item_enabled(ui, "Paste", &k::PASTE.label(), can_paste).clicked() {
                app.push(Action::Paste);
                ui.close();
            }
            if item_enabled(ui, "Paste in Place", &k::PASTE_IN_PLACE.label(), can_paste).clicked() {
                app.push(Action::PasteInPlace);
                ui.close();
            }
            ui.separator();
            if item_enabled(ui, "Fill…", &k::FILL.label(), has_doc).clicked() {
                app.push(Action::ShowFillDialog);
                ui.close();
            }
            if item_enabled(ui, "Fill with Foreground", &k::FILL_FOREGROUND.label(), has_doc).clicked() {
                app.push(Action::fill_foreground(false));
                ui.close();
            }
            if item_enabled(ui, "Fill with Background", &k::FILL_BACKGROUND.label(), has_doc)
                .clicked()
            {
                app.push(Action::fill_background(false));
                ui.close();
            }
            if item_enabled(ui, "Clear", "Delete", has_doc).clicked() {
                app.push(Action::ClearLayer);
                ui.close();
            }
        });

        ui.menu_button("Image", |ui| {
            if item_enabled(ui, "Image Size…", &k::IMAGE_SIZE.label(), has_doc).clicked() {
                app.push(Action::ShowImageSize);
                ui.close();
            }
            if item_enabled(ui, "Canvas Size…", &k::CANVAS_SIZE.label(), has_doc).clicked() {
                app.push(Action::ShowCanvasSize);
                ui.close();
            }
            let has_selection = app.doc().is_some_and(|d| d.doc.has_selection());
            if item_enabled(ui, "Crop to Selection", "", has_selection).clicked() {
                app.push(Action::CropToSelection);
                ui.close();
            }
            ui.separator();
            if item_enabled(ui, "Fill In Selection", "", has_doc).clicked() {
                app.push(Action::FillInSelection);
                ui.close();
            }
            ui.separator();
            if item_enabled(ui, "Relight…", "", has_doc).clicked() {
                app.push(Action::ShowRelight);
                ui.close();
            }
            if item_enabled(ui, "Replace Sky", "", has_doc)
                .on_hover_text(
                    "Finds the sky, puts a new one in, and carries its light into the \
                     foreground — otherwise it is a grey day with a blue sky pasted on",
                )
                .clicked()
            {
                app.push(Action::ReplaceSky);
                ui.close();
            }
            if item_enabled(ui, "Retouch Skin", "", has_doc)
                .on_hover_text(
                    "Smooths skin and not eyes or hair, inside the selection or over \
                     whatever faces are found",
                )
                .clicked()
            {
                app.push(Action::RetouchSkin);
                ui.close();
            }
            if item_enabled(ui, "Depth Effects…", "", has_doc)
                .on_hover_text(
                    "Haze that thickens with distance, a shallow depth of field applied \
                     after the fact, and a shift of viewpoint — all from the same depth",
                )
                .clicked()
            {
                app.push(Action::ShowDepthFx);
                ui.close();
            }
            if item_enabled(ui, "Separate by Content…", "", has_doc).clicked() {
                app.push(Action::ShowSeparate);
                ui.close();
            }
            if item_enabled(ui, "Upscale…", "", has_doc).clicked() {
                app.push(Action::ShowUpscale);
                ui.close();
            }
            ui.separator();
            if item_enabled(ui, "Colour Profile…", "", has_doc).clicked() {
                app.push(Action::ShowColorProfile);
                ui.close();
            }
            let depth = app.doc().map_or(8, |d| d.doc.depth());
            ui.menu_button("Mode", |ui| {
                for bits in [8u8, 16] {
                    let label = format!("{bits} Bits/Channel");
                    let at = depth == bits;
                    if ui.selectable_label(at, label).clicked() {
                        app.push(Action::SetDepth(bits));
                        ui.close();
                    }
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(
                        "Sixteen bits holds what eight rounds off.\n\
                         Widening loses nothing; narrowing cannot be\n\
                         undone except from the history.",
                    )
                    .color(Palette::DARK.text_dim)
                    .small(),
                );
            });
            ui.separator();
            ui.menu_button("Adjustments", |ui| {
                ui.label(
                    egui::RichText::new("Applied to the layer's pixels")
                        .color(Palette::DARK.text_dim)
                        .small(),
                );
                ui.separator();
                for adjustment in cshop_core::adjust::Adjustment::all_defaults() {
                    // The ellipsis promises a dialog, which all but Invert have.
                    let suffix = if adjustment.has_settings() { "…" } else { "" };
                    let label = format!("{}{suffix}", adjustment.name());
                    let chord = adjustment_chord(&adjustment);
                    if item_enabled(ui, &label, &chord, has_doc).clicked() {
                        app.push(Action::ShowAdjustmentDialog(Box::new(adjustment)));
                        ui.close();
                    }
                }
            });
            ui.menu_button("New Adjustment Layer", |ui| {
                ui.label(
                    egui::RichText::new("Non-destructive, above the active layer")
                        .color(Palette::DARK.text_dim)
                        .small(),
                );
                ui.separator();
                for adjustment in cshop_core::adjust::Adjustment::all_defaults() {
                    if item_enabled(ui, adjustment.name(), "", has_doc).clicked() {
                        app.push(Action::AddAdjustmentLayer(Box::new(adjustment)));
                        ui.close();
                    }
                }
            });
        });

        ui.menu_button("Layer", |ui| {
            if item_enabled(ui, "New Layer", &k::NEW_LAYER.label(), has_doc).clicked() {
                app.push(Action::NewLayer);
                ui.close();
            }
            if item_enabled(ui, "New Group", "", has_doc).clicked() {
                app.push(Action::NewGroup);
                ui.close();
            }
            if item_enabled(ui, "Layer via Copy", &k::LAYER_VIA_COPY.label(), has_doc).clicked() {
                app.push(Action::LayerViaCopy);
                ui.close();
            }
            if item_enabled(ui, "Duplicate Layer", "", has_doc).clicked() {
                app.push(Action::DuplicateLayer);
                ui.close();
            }
            if item_enabled(ui, "Delete Layer", "", has_doc).clicked() {
                app.push(Action::DeleteLayer);
                ui.close();
            }
            if item_enabled(ui, "Keyboard Shortcuts…", "", true)
                .on_hover_text("Every chord, and where to change them")
                .clicked()
            {
                app.push(Action::ShowShortcuts);
                ui.close();
            }
            ui.separator();
            ui.menu_button("Transform", |ui| {
                if item_enabled(ui, "Free Transform", &k::FREE_TRANSFORM.label(), has_doc).clicked() {
                    app.push(Action::BeginTransform);
                    ui.close();
                }
                if item_enabled(ui, "Warp", "", has_doc)
                    .on_hover_text("A mesh over the layer; drag it to bend the middle")
                    .clicked()
                {
                    app.push(Action::BeginWarp { puppet: false });
                    ui.close();
                }
                if item_enabled(ui, "Puppet Warp", "", has_doc)
                    .on_hover_text(
                        "Pins where you want them. What is pinned stays, what is between \
                         them moves as rigidly as it can",
                    )
                    .clicked()
                {
                    app.push(Action::BeginWarp { puppet: true });
                    ui.close();
                }
                ui.separator();
                for preset in TransformPreset::ALL {
                    if item_enabled(ui, preset.name(), "", has_doc).clicked() {
                        app.push(Action::TransformPreset(preset));
                        ui.close();
                    }
                }
            });
            ui.separator();
            ui.menu_button("Layer Mask", |ui| {
                let has_mask = app.doc().is_some_and(|d| d.doc.active_has_mask());
                let has_selection = app.doc().is_some_and(|d| d.doc.has_selection());

                if item_enabled(ui, "Reveal All", "", has_doc && !has_mask).clicked() {
                    app.push(Action::AddLayerMask { hide_all: false });
                    ui.close();
                }
                if item_enabled(ui, "Hide All", "", has_doc && !has_mask).clicked() {
                    app.push(Action::AddLayerMask { hide_all: true });
                    ui.close();
                }
                if item_enabled(ui, "Reveal Selection", "", has_selection && !has_mask).clicked() {
                    app.push(Action::AddLayerMaskFromSelection { invert: false });
                    ui.close();
                }
                if item_enabled(ui, "Hide Selection", "", has_selection && !has_mask).clicked() {
                    app.push(Action::AddLayerMaskFromSelection { invert: true });
                    ui.close();
                }
                if item_enabled(ui, "From Depth (near)", "", has_doc && !has_mask).clicked() {
                    app.push(Action::AddLayerMaskFromDepth { invert: false });
                    ui.close();
                }
                if item_enabled(ui, "From Depth (far)", "", has_doc && !has_mask).clicked() {
                    app.push(Action::AddLayerMaskFromDepth { invert: true });
                    ui.close();
                }
                ui.separator();
                let drawing = app.pen.as_ref().is_some_and(|d| d.anchors.len() >= 3);
                if item_enabled(ui, "From Path", "", drawing && !has_mask)
                    .on_hover_text(
                        "The path you are drawing becomes the mask, and stays a path: \
                         resizing the document draws it again rather than resampling it",
                    )
                    .clicked()
                {
                    app.push(Action::AddVectorMask { invert: false });
                    ui.close();
                }
                if item_enabled(ui, "From Path (hide inside)", "", drawing && !has_mask).clicked()
                {
                    app.push(Action::AddVectorMask { invert: true });
                    ui.close();
                }
                ui.separator();
                if item_enabled(ui, "Layer to Mask", "", has_doc).clicked() {
                    app.push(Action::LayerToMask);
                    ui.close();
                }
                if item_enabled(ui, "Mask to Selection", "", has_mask).clicked() {
                    app.push(Action::SelectionFromMask);
                    ui.close();
                }
                ui.separator();
                if item_enabled(ui, "Disable / Enable", "", has_mask).clicked() {
                    app.push(Action::ToggleMaskEnabled);
                    ui.close();
                }
                if item_enabled(ui, "Apply", "", has_mask).clicked() {
                    app.push(Action::ApplyLayerMask);
                    ui.close();
                }
                if item_enabled(ui, "Delete", "", has_mask).clicked() {
                    app.push(Action::DeleteLayerMask);
                    ui.close();
                }
            });
            ui.separator();
            // Read what the menu needs, then let the borrow end: the items
            // below push actions, which needs `app` mutably.
            let (can_style, has_style, active_id) = match app.doc() {
                Some(v) => match v.doc.active.and_then(|id| v.doc.tree.get(id)) {
                    Some(l) => (l.pixels().is_some(), l.effects.any(), v.doc.active),
                    None => (false, false, None),
                },
                None => (false, false, None),
            };
            if item_enabled(ui, "Layer Style…", "", can_style).clicked() {
                app.push(Action::ShowLayerStyle);
                ui.close();
            }
            if item_enabled(ui, "Clear Layer Style", "", has_style).clicked() {
                if let Some(id) = active_id {
                    app.push(Action::ClearLayerEffects(id));
                }
                ui.close();
            }
            ui.separator();
            // Read what the two items need before either can push an action,
            // since pushing borrows the app and the layer came out of it.
            let (has_pixels, is_smart, rendered, label) = match app
                .doc()
                .and_then(|v| v.doc.active.and_then(|id| v.doc.tree.get(id)))
            {
                Some(l) => (
                    l.pixels().is_some(),
                    l.smart().is_some(),
                    l.is_rendered(),
                    match &l.kind {
                        cshop_core::layer::LayerKind::Text(_) => "Rasterize Type",
                        cshop_core::layer::LayerKind::Smart(_) => "Rasterize Smart Object",
                        _ => "Rasterize Shape",
                    },
                ),
                None => (false, false, false, "Rasterize Shape"),
            };
            if item_enabled(
                ui,
                "Convert to Smart Object",
                "",
                has_pixels && !is_smart,
            )
            .on_hover_text(
                "Keeps the picture it was made from, so scaling and rotating \
                 can be changed as often as you like without wearing it out",
            )
            .clicked()
            {
                app.push(Action::ConvertToSmartObject);
                ui.close();
            }
            // How many layers share this one's picture, which is the only
            // thing "linked" means here.
            let shared = app.smart_link().map(|(_, n)| n).unwrap_or(0);
            if item_enabled(ui, "Replace Contents…", "", is_smart)
                .on_hover_text(if shared > 1 {
                    format!(
                        "Puts a different picture behind this smart object — and behind \
                         the {} other layers sharing it, each at its own placement",
                        shared - 1
                    )
                } else {
                    "Puts a different picture behind this smart object, keeping its \
                     placement"
                        .to_string()
                })
                .clicked()
            {
                app.push(Action::ShowReplaceContents);
                ui.close();
            }
            if item_enabled(ui, "Make Unique", "", shared > 1)
                .on_hover_text(
                    "Gives this layer its own copy of the picture, so replacing one \
                     no longer changes the other",
                )
                .clicked()
            {
                app.push(Action::MakeSmartUnique);
                ui.close();
            }
            if item_enabled(ui, label, "", rendered).clicked() {
                app.push(Action::RasterizeLayer);
                ui.close();
            }
            ui.separator();
            // Boolean operations on shape layers. Enabled only with two or
            // more selected, since combining one shape with nothing is not an
            // operation.
            let combinable = app
                .doc()
                .is_some_and(|d| {
                    d.doc.selected_layers.len() >= 2
                        && d.doc
                            .selected_layers
                            .iter()
                            .all(|id| d.doc.tree.get(*id).is_some_and(|l| l.shape().is_some()))
                });
            ui.menu_button("Combine Shapes", |ui| {
                for op in cshop_core::path::BoolOp::all() {
                    if item_enabled(ui, op.name(), "", combinable).clicked() {
                        app.push(Action::CombineShapes(op));
                        ui.close();
                    }
                }
            });
            ui.separator();
            if item_enabled(ui, "Merge Down", &k::MERGE_DOWN.label(), has_doc).clicked() {
                app.push(Action::MergeDown);
                ui.close();
            }
            let several = app.doc().is_some_and(|d| d.doc.tree.len() >= 2);
            let animated = app.doc().is_some_and(|d| d.doc.timeline.is_some());
            if item_enabled(
                ui,
                if animated { "Stop Animating" } else { "Make Frames from Layers" },
                "",
                several,
            )
            .on_hover_text(
                "A frame is a layer shown on its own, so everything that works on a \
                 layer works on a frame",
            )
            .clicked()
            {
                app.push(Action::ToggleTimeline);
                ui.close();
            }
            ui.separator();
            ui.menu_button("Align Layers", |ui| {
                ui.label(
                    egui::RichText::new("Every layer moved onto the bottom one")
                        .color(Palette::DARK.text_dim)
                        .small(),
                );
                ui.separator();
                for (name, motion, hint) in [
                    (
                        "Shift only",
                        cshop_core::align::Motion::Translation,
                        "Two numbers to fit, so the least that can go wrong. Right for a tripod.",
                    ),
                    (
                        "Shift, turn and scale",
                        cshop_core::align::Motion::Similarity,
                        "What a hand-held sequence of the same scene usually needs.",
                    ),
                    (
                        "Camera turned",
                        cshop_core::align::Motion::Homography,
                        "The full projective fit, for a panorama. It can distort as well as \
                         move, so it is the wrong one for stacking.",
                    ),
                ] {
                    if item_enabled(ui, name, "", several).on_hover_text(hint).clicked() {
                        app.push(Action::AlignLayers { motion });
                        ui.close();
                    }
                }
                ui.separator();
                if item_enabled(ui, "Align and Stack", "", several)
                    .on_hover_text(
                        "Align, then average into a new layer — the picture is the same in \
                         every frame and the noise is not, so the noise averages away",
                    )
                    .clicked()
                {
                    app.push(Action::StackLayers);
                    ui.close();
                }
            });
            ui.separator();
            if item_enabled(ui, "Flatten Image", "", has_doc).clicked() {
                app.push(Action::FlattenImage);
                ui.close();
            }
        });

        ui.menu_button("Select", |ui| {
            let has_selection = app.doc().is_some_and(|d| d.doc.has_selection());
            let can_reselect = app.doc().is_some_and(|d| d.doc.last_selection.is_some());

            // The models are optional, so the entry is always there and says
            // what is missing when it is not installed, rather than vanishing.
            if item_enabled(ui, "Segment Object…", "", has_doc).clicked() {
                app.push(Action::ShowSegment);
                ui.close();
            }
            if item_enabled(ui, "Colour Range…", "", has_doc)
                .on_hover_text(
                    "Selects a colour wherever it appears, not just where it is joined \
                     to what you clicked — and partly, where it is partly there",
                )
                .clicked()
            {
                app.push(Action::ShowColorRange);
                ui.close();
            }
            if item_enabled(ui, "Refine Edge…", "", has_selection)
                .on_hover_text(
                    "Fits the selection's edge to the one in the picture, which is what \
                     hair and fur need and what growing or feathering cannot do",
                )
                .clicked()
            {
                app.push(Action::ShowRefineEdge);
                ui.close();
            }
            ui.separator();
            let drawing_path = app.pen.as_ref().is_some_and(|d| d.anchors.len() >= 3);
            if item_enabled(ui, "Selection from Path", "", drawing_path).clicked() {
                app.push(Action::SelectionFromPath);
                ui.close();
            }
            if item_enabled(ui, "Path from Selection", "", has_selection)
                .on_hover_text("Traces the outline as a path you can edit with the Pen")
                .clicked()
            {
                app.push(Action::PathFromSelection);
                ui.close();
            }
            ui.separator();
            if item_enabled(ui, "All", &k::SELECT_ALL.label(), has_doc).clicked() {
                app.push(Action::SelectAll);
                ui.close();
            }
            if item_enabled(ui, "Deselect", &k::DESELECT.label(), has_selection).clicked() {
                app.push(Action::Deselect);
                ui.close();
            }
            if item_enabled(ui, "Reselect", &k::RESELECT.label(), can_reselect).clicked() {
                app.push(Action::Reselect);
                ui.close();
            }
            // Feather is the one Modify entry people reach for constantly, so
            // it gets a place of its own here as well as inside the submenu.
            if item_enabled(ui, "Feather…", &k::FEATHER.label(), has_selection).clicked() {
                app.push(Action::ShowModifyDialog(ModifyKind::Feather));
                ui.close();
            }
            if item_enabled(ui, "Inverse", &k::INVERSE.label(), has_doc).clicked() {
                app.push(Action::InverseSelection);
                ui.close();
            }
            ui.separator();

            ui.add_enabled_ui(has_selection, |ui| {
                ui.menu_button("Modify", |ui| {
                    for (label, make) in [
                        ("Feather…", ModifyKind::Feather),
                        ("Expand…", ModifyKind::Expand),
                        ("Contract…", ModifyKind::Contract),
                        ("Border…", ModifyKind::Border),
                        ("Smooth…", ModifyKind::Smooth),
                    ] {
                        let chord = match make {
                            ModifyKind::Feather => k::FEATHER.label(),
                            _ => String::new(),
                        };
                        if item(ui, label, &chord).clicked() {
                            app.dialog = Dialog::Modify(ModifyDialog::new(make));
                            ui.close();
                        }
                    }
                });
            });

            if item_enabled(ui, "Grow", "", has_selection).clicked() {
                app.push(Action::GrowSelection);
                ui.close();
            }
            if item_enabled(ui, "Similar", "", has_selection).clicked() {
                app.push(Action::SimilarSelection);
                ui.close();
            }
            ui.separator();

            if item_enabled(ui, "Save Selection as Channel", "", has_selection).clicked() {
                app.push(Action::SaveSelectionAsChannel);
                ui.close();
            }
            let label = if app.quick_mask { "Exit Quick Mask" } else { "Edit in Quick Mask" };
            if item_enabled(ui, label, "Q", has_doc).clicked() {
                app.push(Action::ToggleQuickMask);
                ui.close();
            }
        });

        ui.menu_button("Filter", |ui| {
            let repeat = app
                .last_filter
                .as_ref()
                .map(|f| format!("{} Again", f.name()))
                .unwrap_or_else(|| "Last Filter".into());
            if item_enabled(ui, &repeat, &k::LAST_FILTER.label(), has_doc && app.last_filter.is_some())
                .clicked()
            {
                app.push(Action::RepeatLastFilter);
                ui.close();
            }
            ui.separator();
            // Above the categories rather than inside one, because it is not a
            // filter: it changes the geometry of the picture and can change
            // its size, which nothing under Distort does.
            if item_enabled(ui, "Lens Correction…", "", has_doc).clicked() {
                app.push(Action::ShowLens);
                ui.close();
            }
            if item_enabled(ui, "Remove Noise…", "", has_doc).clicked() {
                app.push(Action::ShowDenoise);
                ui.close();
            }
            ui.separator();

            // Grouped exactly as the Filter menu is, so the shape is familiar.
            let all = cshop_core::filters::Filter::all_defaults();
            for category in cshop_core::filters::Category::ALL {
                ui.menu_button(category.name(), |ui| {
                    for filter in all.iter().filter(|f| f.category() == category) {
                        let suffix = if filter.has_settings() { "…" } else { "" };
                        let label = format!("{}{suffix}", filter.name());
                        if item_enabled(ui, &label, "", has_doc).clicked() {
                            app.push(Action::ShowFilterDialog(Box::new(filter.clone())));
                            ui.close();
                        }
                    }
                });
            }
        });

        ui.menu_button("View", |ui| {
            // Colour first, because it changes what everything below it looks
            // like rather than where it is.
            let display_name = app
                .display_profile
                .as_ref()
                .map(|p| p.name().to_string())
                .unwrap_or_else(|| "sRGB (assumed)".into());
            ui.menu_button("Screen Profile", |ui| {
                ui.label(
                    egui::RichText::new(format!("Showing for: {display_name}"))
                        .color(Palette::DARK.text_dim)
                        .small(),
                );
                ui.separator();
                if ui.button("sRGB (assume)").clicked() {
                    app.push(Action::SetDisplayProfile(None));
                    ui.close();
                }
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for (name, path) in crate::profile_ui::discover() {
                        if ui.button(&name).clicked() {
                            app.push(Action::SetDisplayProfile(Some(path)));
                            ui.close();
                        }
                    }
                });
            });
            let proofing = app.proof_profile.is_some();
            ui.menu_button(if proofing { "Proof Colours ✓" } else { "Proof Colours" }, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Shows the picture as another space would render it — what a \
                         press can reach and what it cannot",
                    )
                    .color(Palette::DARK.text_dim)
                    .small(),
                );
                ui.separator();
                if item_enabled(ui, "Off", "", proofing).clicked() {
                    app.push(Action::SetProofProfile(None));
                    ui.close();
                }
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for (name, path) in crate::profile_ui::discover() {
                        if ui.button(&name).clicked() {
                            app.push(Action::SetProofProfile(Some(path)));
                            ui.close();
                        }
                    }
                });
            });
            ui.separator();
            if item_enabled(ui, "Zoom In", &k::ZOOM_IN.label(), has_doc).clicked() {
                app.push(Action::ZoomIn);
                ui.close();
            }
            if item_enabled(ui, "Zoom Out", &k::ZOOM_OUT.label(), has_doc).clicked() {
                app.push(Action::ZoomOut);
                ui.close();
            }
            if item_enabled(ui, "Fit on Screen", &k::ZOOM_FIT.label(), has_doc).clicked() {
                app.push(Action::ZoomFit);
                ui.close();
            }
            if item_enabled(ui, "Actual Pixels", &k::ZOOM_ACTUAL.label(), has_doc).clicked() {
                app.push(Action::ZoomActual);
                ui.close();
            }
            ui.separator();
            if item(ui, if app.show_rulers { "Hide Rulers" } else { "Rulers" }, "").clicked() {
                app.push(Action::ToggleRulers);
                ui.close();
            }
            if item(ui, if app.show_guides { "Hide Guides" } else { "Show Guides" }, "").clicked() {
                app.push(Action::ToggleGuides);
                ui.close();
            }
            if item(ui, if app.show_grid { "Hide Grid" } else { "Show Grid" }, "").clicked() {
                app.push(Action::ToggleGrid);
                ui.close();
            }
            if item(ui, if app.snap { "Turn Snapping Off" } else { "Snap to Guides" }, "").clicked() {
                app.push(Action::ToggleSnap);
                ui.close();
            }
            if item_enabled(ui, "Clear Guides", "", has_doc).clicked() {
                app.push(Action::ClearGuides);
                ui.close();
            }
        });

        ui.menu_button("Window", |ui| {
            let label = if app.show_panels { "Hide Panels" } else { "Show Panels" };
            if item(ui, label, &k::TOGGLE_PANELS.label()).clicked() {
                app.push(Action::TogglePanels);
                ui.close();
            }
        });

        ui.menu_button("Help", |ui| {
            if item(ui, "About C-Shop", "").clicked() {
                app.dialog = crate::dialogs::Dialog::About;
                ui.close();
            }
        });

            // The x just past the last menu, which is where the title bar's
            // free-to-drag area begins.
            ui.cursor().min.x
        })
        .inner
}

/// A menu row with a right-aligned shortcut hint.
/// Five adjustments have a chord of their own; the rest have none.
fn adjustment_chord(adjustment: &cshop_core::adjust::Adjustment) -> String {
    use cshop_core::adjust::Adjustment as A;
    match adjustment {
        A::Levels { .. } => k::LEVELS.label(),
        A::Curves { .. } => k::CURVES.label(),
        A::HueSaturation { .. } => k::HUE_SATURATION.label(),
        A::ColorBalance { .. } => k::COLOR_BALANCE.label(),
        A::Invert => k::INVERT.label(),
        _ => String::new(),
    }
}

fn item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> egui::Response {
    item_enabled(ui, label, shortcut, true)
}

fn item_enabled(
    ui: &mut egui::Ui,
    label: &str,
    shortcut: &str,
    enabled: bool,
) -> egui::Response {
    let button = egui::Button::new(label)
        .shortcut_text(egui::RichText::new(shortcut).color(Palette::DARK.text_dim))
        .min_size(egui::vec2(190.0, 0.0));
    ui.add_enabled(enabled, button)
}

// ---------------------------------------------------------------------------
// Toolbox
// ---------------------------------------------------------------------------

pub fn toolbox(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.vertical_centered(|ui| {
        for group in TOOL_GROUPS {
            // Show whichever tool in this group is currently selected, so the
            // slot remembers the user's last choice.
            let shown =
                group.tools.iter().copied().find(|t| *t == app.tool).unwrap_or(group.tools[0]);
            let selected = group.tools.contains(&app.tool);

            let (rect, r) =
                ui.allocate_exact_size(egui::vec2(30.0, 26.0), egui::Sense::click());
            let fill = if selected {
                p.accent
            } else if r.hovered() {
                p.widget_hover
            } else {
                p.chrome
            };
            ui.painter().rect_filled(rect, 2.0, fill);

            let tint = if selected {
                egui::Color32::WHITE
            } else if shown.is_implemented() {
                p.text
            } else {
                p.text_dim
            };
            icons::tool(&ui.painter_at(rect), rect.shrink(5.0), shown, tint);

            // A corner dot marks slots that hold more than one tool.
            if group.tools.len() > 1 {
                ui.painter().circle_filled(
                    egui::pos2(rect.max.x - 3.0, rect.max.y - 3.0),
                    1.5,
                    tint.gamma_multiply(0.8),
                );
            }

            let mut hover = format!("{}  ({})", shown.name(), group.label);
            if group.tools.len() > 1 {
                hover.push_str(&format!("\nPress {} again to cycle", group.label));
            }
            if !shown.is_implemented() {
                hover.push_str("\nNot implemented yet");
            }
            let r = r.on_hover_text(hover);
            if r.clicked() {
                app.push(Action::SelectTool(crate::tools::cycle(group, app.tool)));
            }
            // Right-click opens the flyout for multi-tool slots.
            if group.tools.len() > 1 {
                r.context_menu(|ui| {
                    for t in group.tools {
                        if ui.selectable_label(app.tool == *t, t.name()).clicked() {
                            app.push(Action::SelectTool(*t));
                            ui.close();
                        }
                    }
                });
            }
        }

        ui.add_space(10.0);
        color_swatches(app, ui);

        ui.add_space(8.0);
        // Quick Mask toggle, below the colour swatches where it belongs.
        let (rect, r) = ui.allocate_exact_size(egui::vec2(30.0, 24.0), egui::Sense::click());
        let fill = if app.quick_mask {
            p.accent
        } else if r.hovered() {
            p.widget_hover
        } else {
            p.chrome
        };
        ui.painter().rect_filled(rect, 2.0, fill);
        icons::icon(
            &ui.painter_at(rect),
            rect.shrink(5.0),
            Icon::QuickMask,
            if app.quick_mask { egui::Color32::WHITE } else { p.text },
        );
        let hint = if app.quick_mask {
            "Exit Quick Mask (Q)"
        } else {
            "Edit in Quick Mask (Q)"
        };
        if r.on_hover_text(hint).clicked() {
            app.push(Action::ToggleQuickMask);
        }
    });
}

/// Ids for the two toolbox swatches. Fixed rather than derived from the
/// enclosing `Ui`, so a test can ask egui where they actually ended up instead
/// of recomputing the layout and hoping it agrees.
pub fn foreground_swatch_id() -> egui::Id {
    egui::Id::new("toolbox-foreground-swatch")
}

pub fn background_swatch_id() -> egui::Id {
    egui::Id::new("toolbox-background-swatch")
}

/// Foreground/background swatches, with the swap and reset affordances.
fn color_swatches(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(32.0, 34.0), egui::Sense::hover());
    let painter = ui.painter();

    let fg_rect = egui::Rect::from_min_size(rect.min, egui::vec2(20.0, 20.0));
    let bg_rect =
        egui::Rect::from_min_size(rect.min + egui::vec2(11.0, 12.0), egui::vec2(20.0, 20.0));

    let to32 = |c: cshop_core::color::Rgba8| egui::Color32::from_rgb(c.r, c.g, c.b);

    // Background sits behind, offset down-right.
    painter.rect_filled(bg_rect, 0.0, to32(app.background));
    painter.rect_stroke(bg_rect, 0.0, egui::Stroke::new(1.0, p.separator), egui::StrokeKind::Inside);
    painter.rect_filled(fg_rect, 0.0, to32(app.foreground));
    painter.rect_stroke(fg_rect, 0.0, egui::Stroke::new(1.0, p.separator), egui::StrokeKind::Inside);

    // The background sits behind the foreground, so it is registered first:
    // in egui the widget added last is the one on top, and the foreground has
    // to win the overlap.
    let bg = ui
        .interact(bg_rect, background_swatch_id(), egui::Sense::click())
        .on_hover_text("Background colour — click to swap it to the front, right-click to choose");
    let fg = ui
        .interact(fg_rect, foreground_swatch_id(), egui::Sense::click())
        .on_hover_text("Foreground colour — click to choose");

    // Right-click opens the picker for whichever swatch was hit. The
    // foreground also opens it on a plain click; the
    // background keeps its click for the swap, which is the commoner move.
    if fg.clicked() || fg.secondary_clicked() {
        app.push(Action::ShowColorPicker(crate::dialogs::PickerTarget::Foreground));
    }
    if bg.secondary_clicked() {
        app.push(Action::ShowColorPicker(crate::dialogs::PickerTarget::Background));
    } else if bg.clicked() {
        app.push(Action::SwapColors);
    }

    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if icons::icon_button(ui, Icon::Swap, 15.0, "Swap colours (X)").clicked() {
            app.push(Action::SwapColors);
        }
        if icons::icon_button(ui, Icon::Reset, 15.0, "Default colours (D)").clicked() {
            app.push(Action::ResetColors);
        }
    });
}

// ---------------------------------------------------------------------------
// Tool options bar
// ---------------------------------------------------------------------------

pub fn options_bar(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;
    ui.horizontal_centered(|ui| {
        let (icon_rect, _) =
            ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
        icons::tool(&ui.painter_at(icon_rect), icon_rect.shrink(1.0), app.tool, p.text);
        ui.label(egui::RichText::new(app.tool.name()).strong());
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        // A live transform or warp owns the options bar whatever tool is
        // selected.
        if app.warp.is_some() {
            warp_options(app, ui);
            return;
        }
        if app.transform.is_some() {
            transform_options(app, ui);
            return;
        }
        if app.tool == Tool::Crop {
            crop_options(app, ui);
            return;
        }
        if app.tool.is_selection_tool() {
            selection_options(app, ui);
            return;
        }

        match app.tool {
            Tool::PaintBucket => {
                ui.label("Tolerance:");
                ui.add(egui::DragValue::new(&mut app.bucket.tolerance).range(0..=255));
                ui.add_space(6.0);
                ui.label("Opacity:");
                ui.add(percent_slider(&mut app.bucket.opacity));
                ui.add_space(6.0);
                ui.label("Mode:");
                blend_combo(ui, "bucket-mode", &mut app.bucket.mode);
                ui.checkbox(&mut app.bucket.contiguous, "Contiguous");
                ui.checkbox(&mut app.bucket.antialias, "Anti-alias");
                ui.checkbox(&mut app.bucket.sample_all_layers, "All Layers");
            }

            Tool::Gradient => {
                ui.label("Type:");
                for kind in cshop_core::fill::GradientKind::ALL {
                    if ui
                        .selectable_label(app.gradient.kind == kind, kind.name())
                        .clicked()
                    {
                        app.gradient.kind = kind;
                    }
                }
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);

                // The presets people reach for, built from the current colours.
                let (fg, bg) = (app.foreground, app.background);
                if ui.button("FG to BG").clicked() {
                    app.gradient.stops = cshop_core::fill::Gradient::between(fg, bg).stops;
                }
                if ui.button("FG to Clear").clicked() {
                    app.gradient.stops = cshop_core::fill::Gradient::to_transparent(fg).stops;
                }
                if ui.button("Black to White").clicked() {
                    app.gradient.stops = cshop_core::fill::Gradient::between(
                        cshop_core::color::Rgba8::BLACK,
                        cshop_core::color::Rgba8::WHITE,
                    )
                    .stops;
                }
                ui.add_space(6.0);
                gradient_preview(ui, &app.gradient);

                ui.add_space(6.0);
                ui.label("Opacity:");
                ui.add(percent_slider(&mut app.gradient.opacity));
                ui.label("Mode:");
                blend_combo(ui, "gradient-mode", &mut app.gradient.mode);
                ui.checkbox(&mut app.gradient.reverse, "Reverse");
                ui.checkbox(&mut app.gradient.dither, "Dither");
            }

            Tool::Text => {
                text_options(app, ui);
            }

            Tool::Shape => {
                shape_options(app, ui);
            }

            Tool::CloneStamp => {
                ui.label("Size:");
                ui.add(
                    egui::DragValue::new(&mut app.brush.size)
                        .range(1.0..=2000.0)
                        .speed(0.5)
                        .suffix(" px"),
                );
                ui.add_space(6.0);
                ui.label("Hardness:");
                ui.add(percent_slider(&mut app.brush.hardness));
                ui.add_space(6.0);
                ui.label("Opacity:");
                ui.add(percent_slider(&mut app.brush.opacity));
                ui.add_space(6.0);
                ui.label("Flow:");
                ui.add(percent_slider(&mut app.brush.flow));
                ui.add_space(6.0);
                ui.label("Spacing:");
                ui.add(
                    egui::DragValue::new(&mut app.brush.spacing)
                        .range(0.01..=2.0)
                        .speed(0.005)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                ui.add_space(6.0);
                ui.checkbox(&mut app.clone_aligned, "Aligned");
                ui.checkbox(&mut app.sample_all_layers, "All Layers");
                ui.label(
                    egui::RichText::new(if app.clone_anchor.is_some() {
                        "Alt-click to move the source"
                    } else {
                        "Alt-click to set the source"
                    })
                    .color(p.text_dim)
                    .small(),
                );
            }

            Tool::Brush | Tool::Pencil | Tool::Eraser => {
                ui.label("Size:");
                ui.add(
                    egui::DragValue::new(&mut app.brush.size)
                        .range(1.0..=2000.0)
                        .speed(0.5)
                        .suffix(" px"),
                );
                ui.add_space(6.0);

                ui.label("Hardness:");
                ui.add(percent_slider(&mut app.brush.hardness));
                ui.add_space(6.0);

                ui.label("Opacity:");
                ui.add(percent_slider(&mut app.brush.opacity));
                ui.add_space(6.0);

                ui.label("Flow:");
                ui.add(percent_slider(&mut app.brush.flow));
                ui.add_space(6.0);

                ui.label("Spacing:");
                ui.add(
                    egui::DragValue::new(&mut app.brush.spacing)
                        .range(0.01..=2.0)
                        .speed(0.005)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
            }

            Tool::Dodge | Tool::Burn | Tool::Sponge => {
                use cshop_core::retouch::{RetouchKind, Tones};
                ui.label("Size:");
                ui.add(
                    egui::DragValue::new(&mut app.brush.size)
                        .range(1.0..=2000.0)
                        .speed(0.5)
                        .suffix(" px"),
                );
                ui.add_space(6.0);
                ui.label("Hardness:");
                ui.add(percent_slider(&mut app.brush.hardness));
                ui.add_space(6.0);

                let sponge = app.tool == Tool::Sponge;
                if sponge {
                    // The sponge acts on colour, so a tonal range means
                    // nothing to it; it chooses a direction instead.
                    ui.label("Mode:");
                    for (soak, name) in [(true, "Saturate"), (false, "Desaturate")] {
                        if ui.selectable_label(app.retouch.soak == soak, name).clicked() {
                            app.retouch.soak = soak;
                        }
                    }
                } else {
                    ui.label("Range:");
                    for range in [Tones::Shadows, Tones::Midtones, Tones::Highlights] {
                        if ui.selectable_label(app.retouch.range == range, range.name()).clicked() {
                            app.retouch.range = range;
                        }
                    }
                }
                ui.add_space(6.0);

                ui.label(if sponge { "Flow:" } else { "Exposure:" });
                ui.add(percent_slider(&mut app.retouch.exposure));
                ui.add_space(6.0);

                ui.label(
                    egui::RichText::new(match app.tool.retouches() {
                        Some(RetouchKind::Dodge) => "Alt to burn instead",
                        Some(RetouchKind::Burn) => "Alt to dodge instead",
                        _ => "Alt for the other direction",
                    })
                    .color(p.text_dim)
                    .small(),
                );
            }

            Tool::HistoryBrush => {
                ui.label("Size:");
                ui.add(
                    egui::DragValue::new(&mut app.brush.size)
                        .range(1.0..=2000.0)
                        .speed(0.5)
                        .suffix(" px"),
                );
                ui.add_space(6.0);
                ui.label("Hardness:");
                ui.add(percent_slider(&mut app.brush.hardness));
                ui.add_space(6.0);
                ui.label("Opacity:");
                ui.add(percent_slider(&mut app.brush.opacity));
                ui.add_space(6.0);
                let source = app
                    .doc()
                    .and_then(|v| v.history_source.as_ref().map(|(at, ..)| *at))
                    .and_then(|at| app.doc().map(|v| v.history.label_at(at)));
                ui.label(
                    egui::RichText::new(match source {
                        Some(name) => format!("Painting back to {name}"),
                        None => "Mark a state in the History panel to paint back to".into(),
                    })
                    .color(p.text_dim)
                    .small(),
                );
            }

            Tool::HealingBrush | Tool::SpotHealing => {
                ui.label("Size:");
                ui.add(
                    egui::DragValue::new(&mut app.brush.size)
                        .range(1.0..=2000.0)
                        .speed(0.5)
                        .suffix(" px"),
                );
                ui.add_space(6.0);
                ui.label("Hardness:");
                ui.add(percent_slider(&mut app.brush.hardness));
                ui.add_space(6.0);
                ui.label("Opacity:");
                ui.add(percent_slider(&mut app.brush.opacity));
                ui.add_space(6.0);
                if app.tool == Tool::HealingBrush {
                    ui.checkbox(&mut app.clone_aligned, "Aligned");
                    ui.label(
                        egui::RichText::new(if app.clone_anchor.is_some() {
                            "Alt-click to move the source"
                        } else {
                            "Alt-click to set the source"
                        })
                        .color(p.text_dim)
                        .small(),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("Finds its own source; make the brush a little larger than the mark")
                            .color(p.text_dim)
                            .small(),
                    );
                }
            }

            Tool::Blur | Tool::Sharpen | Tool::Smudge => {
                ui.label("Size:");
                ui.add(
                    egui::DragValue::new(&mut app.brush.size)
                        .range(1.0..=2000.0)
                        .speed(0.5)
                        .suffix(" px"),
                );
                ui.add_space(6.0);
                ui.label("Hardness:");
                ui.add(percent_slider(&mut app.brush.hardness));
                ui.add_space(6.0);
                ui.label("Strength:");
                ui.add(percent_slider(&mut app.brush_filter_strength));
                ui.add_space(6.0);
                if app.tool == Tool::Smudge {
                    ui.label("Flow:");
                    ui.add(percent_slider(&mut app.brush.flow));
                    ui.add_space(6.0);
                }
                ui.label(
                    egui::RichText::new(match app.tool {
                        Tool::Blur => "Softens what is under the brush; the size sets how far it reaches",
                        Tool::Sharpen => "Puts the edges back; past about half, it starts to halo",
                        _ => "Drags colour along with the pointer, letting go as it goes",
                    })
                    .color(p.text_dim)
                    .small(),
                );
            }

            Tool::Zoom => {
                if ui.button("Fit on Screen").clicked() {
                    app.push(Action::ZoomFit);
                }
                if ui.button("Actual Pixels").clicked() {
                    app.push(Action::ZoomActual);
                }
                ui.label(
                    egui::RichText::new("Alt-click to zoom out").color(p.text_dim).small(),
                );
            }

            Tool::Move => {
                ui.label(
                    egui::RichText::new(
                        "Drag to move the active layer · arrow keys nudge, Shift for 10 px",
                    )
                    .color(p.text_dim)
                    .small(),
                );
            }

            Tool::Eyedropper => {
                ui.label(
                    egui::RichText::new("Click to sample the composited colour")
                        .color(p.text_dim)
                        .small(),
                );
            }

            other => {
                ui.label(
                    egui::RichText::new(format!(
                        "The {} tool is not implemented yet.",
                        other.name()
                    ))
                    .color(p.text_dim)
                    .small(),
                );
            }
        }
    });
}

/// Controls for a warp in progress.
fn warp_options(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;
    let puppet = app.warp.as_ref().is_some_and(|w| w.puppet);
    ui.label(egui::RichText::new(if puppet { "Puppet Warp" } else { "Warp" }).strong());
    ui.separator();

    let mut changed = false;
    if let Some(active) = &mut app.warp {
        ui.label("Falloff:");
        changed |= ui
            .add(egui::Slider::new(&mut active.warp.falloff, 0.3..=3.0).fixed_decimals(1))
            .on_hover_text("Higher keeps each point's effect closer to it")
            .changed();
        ui.add_space(6.0);
        let mut rigid = active.warp.rigid;
        if ui
            .checkbox(&mut rigid, "Rigid")
            .on_hover_text(
                "Only rotate and move what is between the points. Off lets them stretch \
                 to reach, which is quicker to pose and easier to make look wrong",
            )
            .changed()
        {
            active.warp.rigid = rigid;
            changed = true;
        }
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(if puppet {
                "Click to add a pin · Alt-click to remove one · double-click to finish"
            } else {
                "Drag the mesh · double-click to finish"
            })
            .color(p.text_dim)
            .small(),
        );
    }
    if changed {
        app.refresh_warp();
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.button("Cancel").clicked() {
            app.push(Action::CancelWarp);
        }
        if ui.button("Apply").clicked() {
            app.push(Action::CommitWarp);
        }
    });
}

/// Readouts and controls for a Free Transform in progress.
fn transform_options(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;
    let Some(active) = &app.transform else { return };
    let (sw, sh) = active.scale_percent();
    let rotation = active.rotation_degrees();
    let mut filter = active.filter;

    ui.label(egui::RichText::new("Free Transform").strong());
    ui.separator();
    ui.label(format!("W: {sw:.1}%"));
    ui.label(format!("H: {sh:.1}%"));
    ui.label(format!("Angle: {rotation:.1}°"));
    ui.separator();

    ui.label("Interpolation:");
    egui::ComboBox::from_id_salt("transform-filter")
        .width(140.0)
        .selected_text(filter.name())
        .show_ui(ui, |ui| {
            for f in cshop_core::resample::Resampling::ALL {
                ui.selectable_value(&mut filter, f, f.name());
            }
        });
    if let Some(active) = &mut app.transform {
        active.filter = filter;
    }

    ui.separator();
    if ui.button("Reset").clicked() {
        if let Some(active) = &mut app.transform {
            active.reset();
        }
    }
    if ui.button("Apply").clicked() {
        app.push(Action::CommitTransform);
    }
    if ui.button("Cancel").clicked() {
        app.push(Action::CancelTransform);
    }
    ui.label(
        egui::RichText::new("Shift constrains · Alt from centre · Ctrl distorts a corner")
            .color(p.text_dim)
            .small(),
    );
}

/// Controls for the crop tool.
fn crop_options(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;
    ui.label(egui::RichText::new("Crop").strong());
    ui.separator();

    // Perspective first, because it changes what every other control means:
    // a quadrilateral has no aspect ratio to fix.
    let mut perspective = app.crop.as_ref().is_some_and(|c| c.is_perspective());
    if ui
        .checkbox(&mut perspective, "Perspective")
        .on_hover_text(
            "Put the four corners on something rectangular in the photograph and \
             cropping straightens it as well as cutting it",
        )
        .changed()
    {
        if app.crop.is_none() {
            // Nothing to put corners on yet; start from the whole canvas.
            let bounds = app.doc().map(|v| v.doc.bounds()).unwrap_or_default();
            app.crop = Some(crate::transform_tool::ActiveCrop::new(bounds));
        }
        if let Some(crop) = &mut app.crop {
            crop.set_perspective(perspective);
        }
    }
    ui.separator();

    if perspective {
        ui.label(
            egui::RichText::new("Drag the corners · double-click to straighten")
                .color(p.text_dim)
                .small(),
        );
        return;
    }

    let mut aspect = app.crop.as_ref().and_then(|c| c.aspect);
    let label = match aspect {
        None => "Free".to_string(),
        Some(a) if (a - 1.0).abs() < 1e-3 => "1:1".into(),
        Some(a) if (a - 4.0 / 3.0).abs() < 1e-3 => "4:3".into(),
        Some(a) if (a - 16.0 / 9.0).abs() < 1e-3 => "16:9".into(),
        Some(a) => format!("{a:.2}:1"),
    };
    egui::ComboBox::from_id_salt("crop-aspect").width(110.0).selected_text(label).show_ui(
        ui,
        |ui| {
            for (name, value) in [
                ("Free", None),
                ("1:1", Some(1.0)),
                ("4:3", Some(4.0 / 3.0)),
                ("3:4", Some(3.0 / 4.0)),
                ("16:9", Some(16.0 / 9.0)),
            ] {
                ui.selectable_value(&mut aspect, value, name);
            }
        },
    );
    if let Some(crop) = &mut app.crop {
        crop.aspect = aspect;
    }

    ui.separator();
    let active = app.crop.is_some();
    if ui.add_enabled(active, egui::Button::new("Crop")).clicked() {
        app.push(Action::CommitCrop);
    }
    if ui.add_enabled(active, egui::Button::new("Cancel")).clicked() {
        app.push(Action::CancelCrop);
    }
    ui.label(
        egui::RichText::new(if active {
            "Drag the handles · Enter to crop · Esc to cancel"
        } else {
            "Drag on the canvas to set the crop area"
        })
        .color(p.text_dim)
        .small(),
    );
}

/// The Shape tool's options: which shape, and how it is painted.
fn shape_options(app: &mut CShopApp, ui: &mut egui::Ui) {
    use cshop_core::shape::{ShapeKind, StrokeAlign};
    let p = Palette::DARK;
    // Adopt the selected shape's settings before showing them, so the bar
    // describes the layer rather than overwriting it.
    app.sync_shape_options();
    let before = (app.shape_kind.clone(), app.shape_style);

    // The kinds, as the Gradient tool lists its own.
    let kinds = [
        ("Rectangle", ShapeKind::Rectangle { radius: 0.0 }),
        ("Rounded", ShapeKind::Rectangle { radius: 12.0 }),
        ("Ellipse", ShapeKind::Ellipse),
        ("Polygon", ShapeKind::Polygon { sides: 6, star: false, inner: 0.5 }),
        ("Star", ShapeKind::Polygon { sides: 5, star: true, inner: 0.45 }),
        ("Line", ShapeKind::Line { thickness: 3.0, from: (0.0, 0.0), to: (1.0, 1.0) }),
    ];
    for (label, kind) in kinds {
        // Compare by shape, not by settings, so switching back keeps whatever
        // radius or side count was chosen.
        let selected = std::mem::discriminant(&app.shape_kind) == std::mem::discriminant(&kind)
            && match (&app.shape_kind, &kind) {
                (
                    ShapeKind::Rectangle { radius: a },
                    ShapeKind::Rectangle { radius: b },
                ) => (*a > 0.0) == (*b > 0.0),
                (ShapeKind::Polygon { star: a, .. }, ShapeKind::Polygon { star: b, .. }) => a == b,
                _ => true,
            };
        if ui.selectable_label(selected, label).clicked() && !selected {
            app.shape_kind = kind;
        }
    }

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);

    // Only the controls the chosen shape actually has.
    match &mut app.shape_kind {
        ShapeKind::Rectangle { radius } if *radius > 0.0 => {
            ui.label("Radius:");
            ui.add(egui::DragValue::new(radius).range(0.1..=400.0).speed(0.4).suffix(" px"));
            ui.add_space(6.0);
        }
        ShapeKind::Polygon { sides, star, inner } => {
            ui.label("Sides:");
            ui.add(egui::DragValue::new(sides).range(3..=32).speed(0.08));
            if *star {
                ui.add_space(6.0);
                ui.label("Indent:");
                ui.add(
                    egui::DragValue::new(inner)
                        .range(0.05..=0.95)
                        .speed(0.004)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
            }
            ui.add_space(6.0);
        }
        ShapeKind::Line { thickness, .. } => {
            ui.label("Weight:");
            ui.add(egui::DragValue::new(thickness).range(0.2..=200.0).speed(0.2).suffix(" px"));
            ui.add_space(6.0);
        }
        _ => {}
    }

    // Fill and stroke, each toggled by its own checkbox so "none" is a state
    // rather than a colour.
    let swatch = |ui: &mut egui::Ui, c: cshop_core::color::Rgba8| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 16.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, egui::Color32::from_rgb(c.r, c.g, c.b));
        ui.painter().rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0, p.separator),
            egui::StrokeKind::Inside,
        );
    };

    let mut has_fill = app.shape_style.fill.is_some();
    if ui.checkbox(&mut has_fill, "Fill").changed() {
        app.shape_style.fill = has_fill.then_some(app.foreground);
    }
    if let Some(c) = app.shape_style.fill {
        swatch(ui, c);
        if ui.small_button("Set").on_hover_text("Fill with the foreground colour").clicked() {
            app.shape_style.fill = Some(app.foreground);
        }
    }

    ui.add_space(8.0);
    let mut has_stroke = app.shape_style.stroke.is_some();
    if ui.checkbox(&mut has_stroke, "Stroke").changed() {
        app.shape_style.stroke = has_stroke.then_some(app.foreground);
    }
    if let Some(c) = app.shape_style.stroke {
        swatch(ui, c);
        if ui.small_button("Set").on_hover_text("Stroke with the foreground colour").clicked() {
            app.shape_style.stroke = Some(app.foreground);
        }
        ui.add_space(4.0);
        ui.add(
            egui::DragValue::new(&mut app.shape_style.stroke_width)
                .range(0.1..=200.0)
                .speed(0.2)
                .suffix(" px"),
        );
        for align in [StrokeAlign::Inside, StrokeAlign::Center, StrokeAlign::Outside] {
            if ui
                .selectable_label(app.shape_style.stroke_align == align, align.name())
                .on_hover_text(format!("{} stroke", align.name()))
                .clicked()
            {
                app.shape_style.stroke_align = align;
            }
        }
    }

    ui.add_space(6.0);
    ui.checkbox(&mut app.shape_style.antialias, "Anti-alias");
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new("Shift for a square · Alt from the centre")
            .color(p.text_dim)
            .small(),
    );

    // A selected shape layer follows the options, so it stays editable.
    if (app.shape_kind.clone(), app.shape_style) != before {
        app.refresh_shape_style();
    }
}

/// The Type tool's options: family, size, style, alignment and colour.
fn text_options(app: &mut CShopApp, ui: &mut egui::Ui) {
    let db = cshop_core::font::FontDb::global();
    if app.text_style.family.is_empty() {
        app.text_style.family = db.default_family();
    }
    let before = app.text_style.clone();

    // 180-odd families is too many to scroll blind, so the list filters.
    let filter_id = ui.id().with("font-filter");
    let mut filter: String = ui.data(|d| d.get_temp(filter_id).unwrap_or_default());
    egui::ComboBox::from_id_salt("font-family")
        .selected_text(app.text_style.family.clone())
        .width(190.0)
        .show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut filter).hint_text("Filter…").desired_width(180.0));
            ui.separator();
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                let needle = filter.to_lowercase();
                for family in db.families() {
                    if !needle.is_empty() && !family.name.to_lowercase().contains(&needle) {
                        continue;
                    }
                    ui.selectable_value(
                        &mut app.text_style.family,
                        family.name.clone(),
                        &family.name,
                    );
                }
            });
        });
    ui.data_mut(|d| d.insert_temp(filter_id, filter));

    ui.add_space(4.0);
    ui.add(
        egui::DragValue::new(&mut app.text_style.size)
            .range(1.0..=1400.0)
            .speed(0.4)
            .suffix(" px"),
    );

    ui.add_space(4.0);
    let family = app.text_style.family.clone();
    let has = db.family(&family);
    // Say when a weight is being faked rather than loaded.
    let bold = ui.selectable_label(app.text_style.bold, "B").on_hover_text(
        if has.is_some_and(|f| f.has_bold) { "Bold" } else { "Bold (synthesised — this family has no bold)" },
    );
    if bold.clicked() {
        app.text_style.bold = !app.text_style.bold;
    }
    let italic = ui.selectable_label(app.text_style.italic, "I").on_hover_text(
        if has.is_some_and(|f| f.has_italic) { "Italic" } else { "Italic (slanted — this family has no italic)" },
    );
    if italic.clicked() {
        app.text_style.italic = !app.text_style.italic;
    }

    ui.add_space(6.0);
    for align in [
        cshop_core::text::TextAlign::Left,
        cshop_core::text::TextAlign::Center,
        cshop_core::text::TextAlign::Right,
    ] {
        if ui
            .selectable_label(app.text_style.align == align, align.name())
            .on_hover_text(format!("Align {}", align.name().to_lowercase()))
            .clicked()
        {
            app.text_style.align = align;
        }
    }

    ui.add_space(6.0);
    ui.label("Leading:");
    // Zero means auto, which is how the font's own metrics get used.
    let mut leading = app.text_style.leading.unwrap_or(0.0);
    if ui
        .add(
            egui::DragValue::new(&mut leading)
                .range(0.0..=2000.0)
                .speed(0.3)
                .custom_formatter(|v, _| if v <= 0.0 { "Auto".into() } else { format!("{v:.0} px") }),
        )
        .changed()
    {
        app.text_style.leading = (leading > 0.0).then_some(leading);
    }

    ui.add_space(6.0);
    ui.label("Tracking:");
    ui.add(egui::DragValue::new(&mut app.text_style.tracking).range(-200.0..=800.0).speed(2.0));

    ui.add_space(6.0);
    ui.checkbox(&mut app.text_style.antialias, "Anti-alias");

    ui.add_space(6.0);
    // The colour comes from the foreground swatch.
    let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 18.0), egui::Sense::hover());
    let fg = app.foreground;
    ui.painter().rect_filled(rect, 2.0, egui::Color32::from_rgb(fg.r, fg.g, fg.b));
    ui.painter().rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, Palette::DARK.separator),
        egui::StrokeKind::Inside,
    );
    if ui.button("Colour…").on_hover_text("Type takes the foreground colour").clicked() {
        app.push(Action::ShowColorPicker(crate::dialogs::PickerTarget::Foreground));
    }

    if app.text_style != before {
        app.refresh_text_style();
    }
}

/// Boolean mode buttons, feather and antialias — the controls every selection
/// tool shares — plus the wand's own settings.
fn selection_options(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;

    ui.spacing_mut().item_spacing.x = 2.0;
    for (mode, icon) in [
        (SelectionMode::Replace, Icon::SelectNew),
        (SelectionMode::Add, Icon::SelectAdd),
        (SelectionMode::Subtract, Icon::SelectSubtract),
        (SelectionMode::Intersect, Icon::SelectIntersect),
    ] {
        let selected = app.selection_mode == mode;
        let (rect, r) = ui.allocate_exact_size(egui::vec2(24.0, 22.0), egui::Sense::click());
        let fill = if selected {
            p.accent
        } else if r.hovered() {
            p.widget_hover
        } else {
            p.widget
        };
        ui.painter().rect_filled(rect, 2.0, fill);
        icons::icon(
            &ui.painter_at(rect),
            rect.shrink(4.0),
            icon,
            if selected { egui::Color32::WHITE } else { p.text },
        );
        if r.on_hover_text(mode.name()).clicked() {
            app.selection_mode = mode;
        }
    }
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);
    ui.spacing_mut().item_spacing.x = 6.0;

    if app.tool == Tool::MagicWand {
        ui.label("Tolerance:");
        ui.add(egui::DragValue::new(&mut app.wand.tolerance).range(0..=255));
        ui.checkbox(&mut app.wand.contiguous, "Contiguous");
        ui.checkbox(&mut app.wand.antialias, "Anti-alias");
        ui.checkbox(&mut app.sample_all_layers, "Sample All Layers");
    } else {
        ui.label("Feather:");
        ui.add(
            egui::DragValue::new(&mut app.selection_feather)
                .range(0.0..=250.0)
                .speed(0.2)
                .suffix(" px"),
        );
        ui.checkbox(&mut app.selection_antialias, "Anti-alias");
    }

    ui.add_space(8.0);
    let hint = match app.tool {
        Tool::PolygonalLasso => {
            "Click to add points · click the first point or press Enter to close · Esc cancels"
        }
        Tool::MagicWand => "Shift adds · Alt subtracts",
        _ => "Shift adds · Alt subtracts · Shift-drag constrains",
    };
    ui.label(egui::RichText::new(hint).color(p.text_dim).small());
}

/// Which Modify operation a dialog is collecting an amount for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifyKind {
    Feather,
    Expand,
    Contract,
    Border,
    Smooth,
}

impl ModifyKind {
    pub fn title(self) -> &'static str {
        match self {
            ModifyKind::Feather => "Feather Selection",
            ModifyKind::Expand => "Expand Selection",
            ModifyKind::Contract => "Contract Selection",
            ModifyKind::Border => "Border Selection",
            ModifyKind::Smooth => "Smooth Selection",
        }
    }

    pub fn field(self) -> &'static str {
        match self {
            ModifyKind::Feather => "Feather Radius:",
            ModifyKind::Expand => "Expand By:",
            ModifyKind::Contract => "Contract By:",
            ModifyKind::Border => "Width:",
            ModifyKind::Smooth => "Sample Radius:",
        }
    }

    pub fn build(self, amount: f32) -> ModifySelection {
        match self {
            ModifyKind::Feather => ModifySelection::Feather(amount),
            ModifyKind::Expand => ModifySelection::Expand(amount as u32),
            ModifyKind::Contract => ModifySelection::Contract(amount as u32),
            ModifyKind::Border => ModifySelection::Border(amount as u32),
            ModifyKind::Smooth => ModifySelection::Smooth(amount as u32),
        }
    }
}

/// A blend-mode dropdown, grouped by family.
pub(crate) fn blend_combo(ui: &mut egui::Ui, id: &str, mode: &mut cshop_core::blend::BlendMode) {
    egui::ComboBox::from_id_salt(id).width(130.0).selected_text(mode.name()).show_ui(ui, |ui| {
        for entry in cshop_core::blend::BlendMode::MENU {
            match entry {
                Some(m) => {
                    ui.selectable_value(mode, *m, m.name());
                }
                None => {
                    ui.separator();
                }
            }
        }
    });
}

/// A swatch showing the gradient's current ramp.
fn gradient_preview(ui: &mut egui::Ui, gradient: &cshop_core::fill::Gradient) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(90.0, 18.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    // A checkerboard behind it, so a ramp into transparency is legible.
    let p = Palette::DARK;
    painter.rect_filled(rect, 2.0, p.checker_light);
    for i in 0..(rect.width() as usize / 5 + 1) {
        for j in 0..2 {
            if (i + j) % 2 == 0 {
                continue;
            }
            let cell = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + i as f32 * 5.0, rect.min.y + j as f32 * 9.0),
                egui::vec2(5.0, 9.0),
            )
            .intersect(rect);
            if cell.is_positive() {
                painter.rect_filled(cell, 0.0, p.checker_dark);
            }
        }
    }
    let steps = rect.width() as usize;
    for i in 0..steps {
        let c = gradient.color_at(i as f32 / steps.max(1) as f32);
        let x = rect.min.x + i as f32;
        painter.line_segment(
            [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)],
            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)),
        );
    }
    painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, p.separator), egui::StrokeKind::Inside);
}

fn percent_slider(value: &mut f32) -> egui::DragValue<'_> {
    egui::DragValue::new(value)
        .range(0.0..=1.0)
        .speed(0.005)
        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
        .custom_parser(|s| s.trim_end_matches('%').parse::<f64>().ok().map(|v| v / 100.0))
}

// ---------------------------------------------------------------------------
// Document tabs
// ---------------------------------------------------------------------------

pub fn document_tabs(app: &mut CShopApp, ui: &mut egui::Ui) {
    if app.docs.is_empty() {
        return;
    }
    let p = Palette::DARK;
    egui::Frame::NONE.fill(p.chrome).inner_margin(egui::Margin::symmetric(4, 2)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let mut close = None;
            for i in 0..app.docs.len() {
                let view = &app.docs[i];
                let selected = app.active == Some(i);
                let label = format!(
                    "{}{}  {:.0}%",
                    view.doc.name,
                    if view.doc.modified { " •" } else { "" },
                    view.zoom * 100.0
                );

                let button = egui::Button::new(label)
                    .fill(if selected { p.panel } else { p.chrome })
                    .stroke(egui::Stroke::NONE);
                if ui.add(button).clicked() {
                    app.push(Action::SelectDocument(i));
                }
                if icons::icon_button(ui, Icon::Close, 16.0, "Close").clicked() {
                    close = Some(i);
                }
            }
            if let Some(i) = close {
                app.push(Action::CloseDocument(i));
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

pub fn status_bar(app: &mut CShopApp, ui: &mut egui::Ui) {
    let p = Palette::DARK;
    // Everything running on a worker, in the one place that draws it. A
    // filter, a resize and an alignment all take seconds on a large picture
    // and all look like a hang without this.
    //
    // Nothing appears for the first fifth of a second. Most operations are
    // over by then, and a bar that flashes up and vanishes is noise rather
    // than information — it is only a wait long enough to be noticed that
    // needs saying anything about.
    let running = app.jobs.running();
    ui.horizontal_centered(|ui| {
        for job in &running {
            if job.elapsed < std::time::Duration::from_millis(200) {
                continue;
            }
            let what = job.progress.what();
            match job.progress.fraction() {
                Some(done) => {
                    ui.add(
                        egui::ProgressBar::new(done)
                            .desired_width(150.0)
                            .text(format!("{what} {:.0}%", done * 100.0)),
                    );
                }
                // Some work cannot say how far along it is — a model in
                // another process, most of all — so it says that rather than
                // guessing at a number.
                None => {
                    ui.add(egui::Spinner::new().size(13.0));
                    ui.label(egui::RichText::new(&what).color(p.text_dim));
                }
            }
            if ui
                .small_button("✕")
                .on_hover_text(format!("Stop {what}"))
                .clicked()
            {
                job.progress.cancel();
            }
            ui.separator();
        }
        match app.doc() {
            Some(view) => {
                ui.label(format!("{:.1}%", view.zoom * 100.0));
                ui.separator();
                ui.label(format!("{} x {} px", view.doc.width, view.doc.height));
                ui.separator();
                ui.label(format!("{} layers", view.doc.tree.len()));
                if let Some(selection) = &view.doc.selection {
                    ui.separator();
                    let b = selection.bounds();
                    ui.label(
                        egui::RichText::new(format!("sel {} x {}", b.width(), b.height()))
                            .color(p.accent),
                    );
                    // Say so when the outline is not the whole story.
                    let dropped = selection.dropped_contours();
                    if dropped > 0 {
                        ui.label(
                            egui::RichText::new(format!("(outline simplified, {dropped} hidden)"))
                                .color(p.text_dim)
                                .small(),
                        )
                        .on_hover_text(
                            "The selection has more islands than can be outlined at frame rate. \
                             Only the largest are drawn; the selection itself is unaffected.",
                        );
                    }
                }
                if app.quick_mask {
                    ui.separator();
                    ui.label(egui::RichText::new("Quick Mask").color(p.accent));
                }
                if view.doc.effective_edit_target() == EditTarget::Mask {
                    ui.separator();
                    ui.label(egui::RichText::new("editing mask").color(p.accent));
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("GPU {}", crate::format_bytes(view.vram_bytes())))
                        .color(p.text_dim),
                );
                // What undo is holding. On a large canvas this is the largest
                // thing in the process after the layers themselves, and it is
                // the only one the user can do anything about.
                // The pictures behind the smart objects. On a document with a
                // few placed photographs this is the largest thing in the
                // process after the layers, and unlike them it is invisible —
                // one picture can be behind four layers or none.
                let sources = view.doc.sources.bytes();
                if sources > 0 {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!(
                            "sources {}",
                            crate::format_bytes(sources)
                        ))
                        .color(p.text_dim),
                    )
                    .on_hover_text(format!(
                        "{} picture{} behind the smart objects, {} of which {} placed",
                        view.doc.sources.len(),
                        if view.doc.sources.len() == 1 { "" } else { "s" },
                        view.doc.used_sources().len(),
                        if view.doc.used_sources().len() == 1 { "is" } else { "are" },
                    ));
                }
                let undo = view.history.memory_bytes();
                if undo > 0 {
                    ui.separator();
                    let forgotten = view.history.forgotten();
                    let text = if forgotten > 0 {
                        format!(
                            "undo {} — {forgotten} older step{} dropped",
                            crate::format_bytes(undo),
                            if forgotten == 1 { "" } else { "s" }
                        )
                    } else {
                        format!("undo {}", crate::format_bytes(undo))
                    };
                    ui.label(egui::RichText::new(text).color(p.text_dim))
                        .on_hover_text(
                            "History is bounded by memory as well as by depth: the oldest \
                             steps are dropped once it would hold more than its budget.",
                        );
                }
                // A document that does not fit in the GPU budget renders
                // incompletely, so say so rather than let it look like a bug.
                if let Some((needed, budget)) = view.cache().over_budget() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!(
                            "⚠ needs {} of {} GPU memory — canvas incomplete",
                            crate::format_bytes(needed),
                            crate::format_bytes(budget)
                        ))
                        .color(egui::Color32::from_rgb(0xe0, 0x6c, 0x60)),
                    );
                }
            }
            None => {
                ui.label(egui::RichText::new("No document open").color(p.text_dim));
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            match &app.toast {
                Some((msg, is_error)) => {
                    let color = if *is_error {
                        egui::Color32::from_rgb(0xe0, 0x6c, 0x60)
                    } else {
                        p.text_dim
                    };
                    ui.label(egui::RichText::new(msg).color(color));
                }
                None => {
                    ui.label(
                        egui::RichText::new(app.gpu.adapter_name()).color(p.text_dim).small(),
                    );
                }
            }
        });
    });
}
