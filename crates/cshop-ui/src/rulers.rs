//! Rulers along two edges, the guides you drag out of them, and the grid.
//!
//! The rulers are drawn inside the viewport rather than beside it, and the
//! canvas is given what is left. That keeps the whole thing in one rectangle
//! and means turning them off gives the space straight back to the picture.
//!
//! A guide is made by dragging out of a ruler, moved by dragging it, and
//! deleted by dragging it back out of the picture — which is how everyone
//! expects it to work and needs no menu.

use crate::app::CShopApp;
use crate::theme::Palette;
use cshop_core::guides::{ruler_step, Guide};

/// How wide the ruler strips are.
pub const RULER: f32 = 18.0;

/// How near the pointer must come to a guide to take hold of it, in screen
/// pixels.
const GRAB: f32 = 5.0;

/// The part of the viewport the picture gets once the rulers have had theirs.
pub fn canvas_area(app: &CShopApp, viewport: egui::Rect) -> egui::Rect {
    if app.show_rulers {
        egui::Rect::from_min_max(viewport.min + egui::vec2(RULER, RULER), viewport.max)
    } else {
        viewport
    }
}

/// Draw the grid, under everything the canvas puts on top of it.
pub fn draw_grid(app: &CShopApp, painter: &egui::Painter, viewport: egui::Rect) {
    if !app.show_grid || app.grid_spacing <= 0.0 {
        return;
    }
    let Some(view) = app.doc() else { return };
    let area = canvas_area(app, viewport);
    let rect = view.canvas_rect(area).intersect(area);
    if rect.width() < 1.0 {
        return;
    }
    let step = app.grid_spacing * view.zoom;
    // A grid finer than a few pixels on screen is a grey wash rather than a
    // grid, so it is left undrawn until there is room for it.
    if step < 4.0 {
        return;
    }
    let faint = egui::Stroke::new(1.0, Palette::DARK.separator.gamma_multiply(0.6));
    let origin = view.doc_to_screen(area, egui::vec2(0.0, 0.0));

    let mut x = origin.x;
    while x < rect.min.x {
        x += step;
    }
    while x <= rect.max.x {
        painter.line_segment([egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)], faint);
        x += step;
    }
    let mut y = origin.y;
    while y < rect.min.y {
        y += step;
    }
    while y <= rect.max.y {
        painter.line_segment([egui::pos2(rect.min.x, y), egui::pos2(rect.max.x, y)], faint);
        y += step;
    }
}

/// Draw the guides themselves.
pub fn draw_guides(app: &CShopApp, painter: &egui::Painter, viewport: egui::Rect) {
    if !app.show_guides {
        return;
    }
    let Some(view) = app.doc() else { return };
    let area = canvas_area(app, viewport);
    let colour = egui::Color32::from_rgb(0x3f, 0xa8, 0xe8);
    let stroke = egui::Stroke::new(1.0, colour);

    for guide in &view.doc.guides {
        let at = view.doc_to_screen(area, guide_point(guide));
        if guide.vertical {
            if at.x >= area.min.x && at.x <= area.max.x {
                painter.line_segment(
                    [egui::pos2(at.x, area.min.y), egui::pos2(at.x, area.max.y)],
                    stroke,
                );
            }
        } else if at.y >= area.min.y && at.y <= area.max.y {
            painter
                .line_segment([egui::pos2(area.min.x, at.y), egui::pos2(area.max.x, at.y)], stroke);
        }
    }
}

fn guide_point(guide: &Guide) -> egui::Vec2 {
    if guide.vertical {
        egui::vec2(guide.at, 0.0)
    } else {
        egui::vec2(0.0, guide.at)
    }
}

/// Draw the two rulers, and let them be dragged from.
///
/// Returns true when the pointer is doing something with a ruler or a guide,
/// so the canvas knows to leave the event alone.
pub fn rulers(app: &mut CShopApp, ui: &mut egui::Ui, viewport: egui::Rect) -> bool {
    if !app.show_rulers {
        return guide_dragging(app, ui, viewport);
    }
    let area = canvas_area(app, viewport);
    let Some(view) = app.doc() else { return false };
    let (zoom, width, height) = (view.zoom, view.doc.width as f32, view.doc.height as f32);
    let origin = view.doc_to_screen(area, egui::vec2(0.0, 0.0));

    let p = Palette::DARK;
    let painter = ui.painter_at(viewport);
    let top = egui::Rect::from_min_max(
        egui::pos2(area.min.x, viewport.min.y),
        egui::pos2(viewport.max.x, viewport.min.y + RULER),
    );
    let left = egui::Rect::from_min_max(
        egui::pos2(viewport.min.x, area.min.y),
        egui::pos2(viewport.min.x + RULER, viewport.max.y),
    );
    let corner = egui::Rect::from_min_size(viewport.min, egui::vec2(RULER, RULER));
    for r in [top, left, corner] {
        painter.rect_filled(r, 0.0, p.panel);
    }
    let edge = egui::Stroke::new(1.0, p.separator);
    painter.line_segment([top.left_bottom(), top.right_bottom()], edge);
    painter.line_segment([left.right_top(), left.right_bottom()], edge);

    let step = ruler_step(zoom);
    let tick = egui::Stroke::new(1.0, p.text_dim);
    let font = egui::FontId::proportional(9.0);

    // Along the top: every step, labelled, with a shorter mark at halves.
    let mut at = 0.0f32;
    while at <= width {
        let x = origin.x + at * zoom;
        if x >= top.min.x && x <= top.max.x {
            painter.line_segment([egui::pos2(x, top.max.y - 6.0), egui::pos2(x, top.max.y)], tick);
            painter.text(
                egui::pos2(x + 2.0, top.min.y + 1.0),
                egui::Align2::LEFT_TOP,
                format!("{at:.0}"),
                font.clone(),
                p.text_dim,
            );
        }
        let half = origin.x + (at + step / 2.0) * zoom;
        if half >= top.min.x && half <= top.max.x {
            painter
                .line_segment([egui::pos2(half, top.max.y - 3.0), egui::pos2(half, top.max.y)], tick);
        }
        at += step;
    }

    // Down the side. The numbers are drawn upright rather than turned, which
    // is less pretty and a great deal easier to read.
    let mut at = 0.0f32;
    while at <= height {
        let y = origin.y + at * zoom;
        if y >= left.min.y && y <= left.max.y {
            painter
                .line_segment([egui::pos2(left.max.x - 6.0, y), egui::pos2(left.max.x, y)], tick);
            painter.text(
                egui::pos2(left.min.x + 1.0, y + 1.0),
                egui::Align2::LEFT_TOP,
                format!("{at:.0}"),
                font.clone(),
                p.text_dim,
            );
        }
        let half = origin.y + (at + step / 2.0) * zoom;
        if half >= left.min.y && half <= left.max.y {
            painter.line_segment(
                [egui::pos2(left.max.x - 3.0, half), egui::pos2(left.max.x, half)],
                tick,
            );
        }
        at += step;
    }

    // Where the pointer is, on both rulers.
    if let Some(pos) = ui.ctx().pointer_hover_pos() {
        let mark = egui::Stroke::new(1.0, p.text);
        if pos.x > area.min.x {
            painter.line_segment(
                [egui::pos2(pos.x, top.min.y), egui::pos2(pos.x, top.max.y)],
                mark,
            );
        }
        if pos.y > area.min.y {
            painter.line_segment(
                [egui::pos2(left.min.x, pos.y), egui::pos2(left.max.x, pos.y)],
                mark,
            );
        }
    }

    // Dragging out of a ruler makes a guide.
    let from_top = ui.interact(top, ui.id().with("ruler-top"), egui::Sense::drag());
    let from_left = ui.interact(left, ui.id().with("ruler-left"), egui::Sense::drag());
    for (response, vertical) in [(&from_top, false), (&from_left, true)] {
        if response.drag_started() {
            let at = pointer_in_doc(app, viewport, ui);
            let value = if vertical { at.x } else { at.y };
            if let Some(view) = app.doc_mut() {
                view.doc.guides.push(Guide { vertical, at: value });
            }
            app.dragging_guide = app.doc().map(|v| v.doc.guides.len() - 1);
            app.show_guides = true;
        }
    }

    guide_dragging(app, ui, viewport) || from_top.dragged() || from_left.dragged()
}

fn pointer_in_doc(app: &CShopApp, viewport: egui::Rect, ui: &egui::Ui) -> egui::Vec2 {
    let area = canvas_area(app, viewport);
    let pos = ui.ctx().pointer_hover_pos().unwrap_or(area.min);
    match app.doc() {
        Some(view) => view.screen_to_doc(area, pos),
        None => egui::vec2(0.0, 0.0),
    }
}

/// Move a guide that has been taken hold of, and drop it off the edge to
/// delete it.
fn guide_dragging(app: &mut CShopApp, ui: &mut egui::Ui, viewport: egui::Rect) -> bool {
    let area = canvas_area(app, viewport);
    let pointer = ui.ctx().pointer_hover_pos();

    if let Some(index) = app.dragging_guide {
        let held = ui.ctx().input(|i| i.pointer.primary_down());
        let at = pointer_in_doc(app, viewport, ui);
        if held {
            if let Some(view) = app.doc_mut() {
                if let Some(guide) = view.doc.guides.get_mut(index) {
                    guide.at = if guide.vertical { at.x } else { at.y };
                }
            }
            return true;
        }
        // Let go. Off the picture means it was being thrown away.
        let outside = pointer.is_none_or(|p| !area.contains(p));
        if outside {
            if let Some(view) = app.doc_mut() {
                if index < view.doc.guides.len() {
                    view.doc.guides.remove(index);
                }
            }
        }
        app.dragging_guide = None;
        return true;
    }

    // Take hold of one the pointer is over.
    if !app.show_guides || !ui.ctx().input(|i| i.pointer.primary_pressed()) {
        return false;
    }
    let Some(pos) = pointer else { return false };
    if !area.contains(pos) {
        return false;
    }
    let Some(view) = app.doc() else { return false };
    let found = view.doc.guides.iter().position(|g| {
        let at = view.doc_to_screen(area, guide_point(g));
        if g.vertical {
            (at.x - pos.x).abs() <= GRAB
        } else {
            (at.y - pos.y).abs() <= GRAB
        }
    });
    if let Some(index) = found {
        app.dragging_guide = Some(index);
        return true;
    }
    false
}
