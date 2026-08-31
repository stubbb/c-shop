//! The document viewport: the composited image, the checkerboard behind it,
//! and all pointer interaction.

use crate::app::{CShopApp, SelectionDrag, StrokeFrom};
use crate::commands::Action;
use crate::theme::Palette;

/// How near, in screen pixels, something must come to a guide before it
/// catches. Fixed on screen rather than in the picture, so that zooming in to
/// place something precisely does not make every guide grab it.
const SNAP_REACH: f32 = 6.0;
use crate::tools::Tool;
use cshop_core::geom::{IRect, Vec2};
use cshop_core::history::OffsetLayer;
use cshop_core::paint::PaintMode;
use cshop_core::selection::{Rectf, SelectionMode};
use cshop_core::transform::Handle;

/// Checkerboard square size in screen points. Fixed in *screen* space, like
/// convention, so zooming does not make the checks swell.
const CHECKER: f32 = 8.0;

pub fn show(app: &mut CShopApp, ui: &mut egui::Ui) {
    let outer = ui.available_rect_before_wrap();
    // The rulers take a strip off two edges and the picture gets what is left,
    // so everything below works in the area rather than the whole panel.
    let viewport = crate::rulers::canvas_area(app, outer);
    app.canvas_viewport = viewport;

    if app.docs.is_empty() {
        empty_state(app, ui, outer);
        return;
    }
    let Some(index) = app.active else { return };

    // Fit to the window the first time we know how big the viewport is.
    if !app.docs[index].zoom_initialised && viewport.width() > 1.0 {
        app.docs[index].fit_to(viewport);
        app.docs[index].zoom_initialised = true;
    }

    let response = ui.allocate_rect(viewport, egui::Sense::click_and_drag());
    let canvas_rect = app.docs[index].canvas_rect(viewport);

    // --- backdrop, checkerboard, image ------------------------------------
    let painter = ui.painter_at(viewport);
    painter.rect_filled(viewport, 0.0, Palette::DARK.canvas_backdrop);

    let visible = canvas_rect.intersect(viewport);
    if visible.is_positive() {
        draw_checkerboard(&painter, visible);
    }

    if let Some(id) = app.docs[index].texture_id() {
        painter.image(
            id,
            canvas_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    // A hairline around the document, so its edge is visible against a dark
    // background even when the image itself is dark.
    painter.rect_stroke(
        canvas_rect,
        0.0,
        egui::Stroke::new(1.0, Palette::DARK.canvas_border),
        egui::StrokeKind::Outside,
    );

    crate::rulers::draw_grid(app, &painter, outer);

    if app.quick_mask {
        draw_quick_mask(app, &painter, viewport);
    }

    // A transform preview stands in for the layer it hid.
    draw_transform_preview(app, ui, viewport);

    // A ruler or a guide has the pointer before any tool does, so dragging a
    // guide out over the picture does not also paint on it.
    let on_a_guide = crate::rulers::rulers(app, ui, outer);
    if !on_a_guide {
        interact(app, ui, &response, viewport);
    }
    // Right-click opens the tool's menu. Every interaction above is gated to
    // the primary button so opening it cannot also lay down a brush stroke.
    response.context_menu(|ui| crate::context_menus::canvas_menu(app, ui));
    crate::rulers::draw_guides(app, &painter, outer);
    draw_selection(app, ui, &painter, viewport);
    draw_transform_overlay(app, ui, &painter, viewport);
    draw_crop_overlay(app, ui, &painter, viewport);
    draw_gradient_guide(app, &painter, viewport);
    draw_clone_anchor(app, ui, &painter, viewport);
    draw_text_caret(app, ui, &painter, viewport);
    draw_shape_preview(app, ui, &painter, viewport);
    cursor(app, ui, &response, viewport);
}

/// Draw the transformed layer while a Free Transform is live.
///
/// The quad is subdivided and drawn as a textured mesh: two triangles would
/// interpolate the texture affinely, which shears the image visibly under a
/// perspective transform. A grid keeps the error below a pixel.
fn draw_transform_preview(app: &mut CShopApp, ui: &mut egui::Ui, viewport: egui::Rect) {
    let Some(index) = app.active else { return };
    let Some(active) = &app.transform else { return };

    // Upload the proxy once and keep it for the life of the transform.
    let handle = {
        let id = egui::Id::new(("transform-proxy", active.layer.0));
        let existing: Option<egui::TextureHandle> = ui.ctx().data(|d| d.get_temp(id));
        match existing {
            Some(h) => h,
            None => {
                let proxy = &active.proxy;
                let pixels: Vec<egui::Color32> = proxy
                    .pixels()
                    .iter()
                    .map(|p| egui::Color32::from_rgba_unmultiplied(p.r, p.g, p.b, p.a))
                    .collect();
                let h = ui.ctx().load_texture(
                    "transform-proxy",
                    egui::ColorImage {
                        size: [proxy.width() as usize, proxy.height() as usize],
                        source_size: egui::vec2(proxy.width() as f32, proxy.height() as f32),
                        pixels,
                    },
                    egui::TextureOptions::LINEAR,
                );
                ui.ctx().data_mut(|d| d.insert_temp(id, h.clone()));
                h
            }
        }
    };

    let view = &app.docs[index];
    let corners = active.corners;
    const N: usize = 12;

    let mut mesh = egui::Mesh::with_texture(handle.id());
    for row in 0..=N {
        let v = row as f32 / N as f32;
        for col in 0..=N {
            let u = col as f32 / N as f32;
            // Bilinear across the quad's corners approximates the projective
            // mapping closely at this density.
            let top = corners[0].lerp(corners[1], u);
            let bottom = corners[3].lerp(corners[2], u);
            let doc = top.lerp(bottom, v);
            let pos = view.doc_to_screen(viewport, egui::vec2(doc.x, doc.y));
            mesh.vertices.push(egui::epaint::Vertex {
                pos,
                uv: egui::pos2(u, v),
                color: egui::Color32::WHITE,
            });
        }
    }
    for row in 0..N {
        for col in 0..N {
            let i = (row * (N + 1) + col) as u32;
            let stride = (N + 1) as u32;
            mesh.indices.extend_from_slice(&[i, i + 1, i + stride]);
            mesh.indices.extend_from_slice(&[i + 1, i + stride + 1, i + stride]);
        }
    }
    ui.painter_at(viewport).add(egui::Shape::mesh(mesh));
}

/// The transform box: edges, handles and a readout.
fn draw_transform_overlay(
    app: &CShopApp,
    ui: &egui::Ui,
    painter: &egui::Painter,
    viewport: egui::Rect,
) {
    let Some(index) = app.active else { return };
    let Some(active) = &app.transform else { return };
    let view = &app.docs[index];
    let p = Palette::DARK;

    let screen: Vec<egui::Pos2> = active
        .corners
        .iter()
        .map(|c| view.doc_to_screen(viewport, egui::vec2(c.x, c.y)))
        .collect();

    let mut outline = screen.clone();
    outline.push(screen[0]);
    painter.add(egui::Shape::line(outline, egui::Stroke::new(1.0, p.accent)));

    for handle in Handle::ALL {
        let d = active.handle_position(handle);
        let pos = view.doc_to_screen(viewport, egui::vec2(d.x, d.y));
        let rect = egui::Rect::from_center_size(pos, egui::vec2(8.0, 8.0));
        painter.rect_filled(rect, 1.0, egui::Color32::WHITE);
        painter.rect_stroke(
            rect,
            1.0,
            egui::Stroke::new(1.0, egui::Color32::BLACK),
            egui::StrokeKind::Inside,
        );
    }

    // The pivot, and a readout of what the transform currently is.
    let centre = active.centre();
    let pivot = view.doc_to_screen(viewport, egui::vec2(centre.x, centre.y));
    painter.circle_stroke(pivot, 5.0, egui::Stroke::new(1.0, egui::Color32::WHITE));
    painter.line_segment(
        [pivot - egui::vec2(7.0, 0.0), pivot + egui::vec2(7.0, 0.0)],
        egui::Stroke::new(1.0, egui::Color32::WHITE),
    );
    painter.line_segment(
        [pivot - egui::vec2(0.0, 7.0), pivot + egui::vec2(0.0, 7.0)],
        egui::Stroke::new(1.0, egui::Color32::WHITE),
    );

    let (sw, sh) = active.scale_percent();
    let text = format!("{sw:.1}%  x  {sh:.1}%   ·   {:.1}°", active.rotation_degrees());
    let anchor = screen[0] + egui::vec2(0.0, -18.0);
    painter.rect_filled(
        egui::Rect::from_min_size(anchor - egui::vec2(4.0, 2.0), egui::vec2(170.0, 18.0)),
        2.0,
        egui::Color32::from_black_alpha(180),
    );
    painter.text(
        anchor,
        egui::Align2::LEFT_TOP,
        text,
        egui::FontId::proportional(11.0),
        egui::Color32::WHITE,
    );
    let _ = ui;
}

/// The crop overlay: a dimmed surround, a rule-of-thirds grid, and handles.
fn draw_crop_overlay(
    app: &CShopApp,
    ui: &egui::Ui,
    painter: &egui::Painter,
    viewport: egui::Rect,
) {
    let Some(index) = app.active else { return };
    let Some(crop) = &app.crop else { return };
    let view = &app.docs[index];

    let canvas = view.canvas_rect(viewport);
    let keep = egui::Rect::from_min_max(
        view.doc_to_screen(viewport, egui::vec2(crop.rect.x0 as f32, crop.rect.y0 as f32)),
        view.doc_to_screen(viewport, egui::vec2(crop.rect.x1 as f32, crop.rect.y1 as f32)),
    );

    // Dim everything outside the crop, in four bands around it.
    let shade = egui::Color32::from_black_alpha(140);
    let outer = canvas.intersect(viewport);
    for band in [
        egui::Rect::from_min_max(outer.min, egui::pos2(outer.max.x, keep.min.y)),
        egui::Rect::from_min_max(egui::pos2(outer.min.x, keep.max.y), outer.max),
        egui::Rect::from_min_max(
            egui::pos2(outer.min.x, keep.min.y),
            egui::pos2(keep.min.x, keep.max.y),
        ),
        egui::Rect::from_min_max(
            egui::pos2(keep.max.x, keep.min.y),
            egui::pos2(outer.max.x, keep.max.y),
        ),
    ] {
        if band.is_positive() {
            painter.rect_filled(band, 0.0, shade);
        }
    }

    painter.rect_stroke(
        keep,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::WHITE),
        egui::StrokeKind::Inside,
    );
    // Rule-of-thirds guides, which is what the crop tool is usually for.
    for i in 1..3 {
        let t = i as f32 / 3.0;
        let stroke = egui::Stroke::new(1.0, egui::Color32::from_white_alpha(90));
        painter.line_segment(
            [
                egui::pos2(keep.min.x + t * keep.width(), keep.min.y),
                egui::pos2(keep.min.x + t * keep.width(), keep.max.y),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(keep.min.x, keep.min.y + t * keep.height()),
                egui::pos2(keep.max.x, keep.min.y + t * keep.height()),
            ],
            stroke,
        );
    }

    for handle in Handle::ALL {
        let d = crop.handle_position(handle);
        let pos = view.doc_to_screen(viewport, egui::vec2(d.x, d.y));
        let rect = egui::Rect::from_center_size(pos, egui::vec2(9.0, 9.0));
        painter.rect_filled(rect, 0.0, egui::Color32::WHITE);
        painter.rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::BLACK),
            egui::StrokeKind::Inside,
        );
    }

    let label = format!("{} x {} px", crop.rect.width(), crop.rect.height());
    painter.text(
        keep.center_bottom() + egui::vec2(0.0, 14.0),
        egui::Align2::CENTER_TOP,
        label,
        egui::FontId::proportional(11.0),
        egui::Color32::WHITE,
    );
    let _ = ui;
}

/// Marching ants: the selection outline, and the live preview of a gesture in
/// progress.
fn draw_selection(app: &mut CShopApp, ui: &mut egui::Ui, painter: &egui::Painter, viewport: egui::Rect) {
    let Some(index) = app.active else { return };

    // Animate the dash phase. Requesting a repaint keeps the ants marching
    // while a selection exists, and only then — the editor is otherwise idle.
    let time = ui.input(|i| i.time) as f32;
    let has_selection = app.docs[index].doc.has_selection();
    if has_selection || app.drag.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
    }

    // The preview of the shape currently being dragged out.
    // The path being edited: its anchors, and the handles of whichever are
    // selected. Handles are shown only for selected anchors, so a complicated
    // path does not become a thicket of lines.
    if app.tool == Tool::DirectSelect {
        if let Some((_, path, origin)) = app.editable_path() {
            let to_screen = |p: cshop_core::geom::Vec2| {
                app.docs[index]
                    .doc_to_screen(viewport, egui::vec2(origin.x + p.x, origin.y + p.y))
            };
            let accent = egui::Color32::from_rgb(90, 170, 255);
            // The outline itself, so the points have something to sit on.
            for part in &path.parts {
                for sub in &part.subpaths {
                    let line = sub.flatten(0.5);
                    let pts: Vec<egui::Pos2> = line.iter().map(|p| to_screen(*p)).collect();
                    if pts.len() >= 2 {
                        painter.add(egui::Shape::line(
                            pts,
                            egui::Stroke::new(1.0, accent.gamma_multiply(0.7)),
                        ));
                    }
                }
            }
            for (pi, part) in path.parts.iter().enumerate() {
                for (si, sub) in part.subpaths.iter().enumerate() {
                    for (ai, a) in sub.anchors.iter().enumerate() {
                        let at = to_screen(a.at);
                        let selected = app.path_edit.is_selected((pi, si, ai));
                        if selected && a.at.distance(a.out_handle) > 0.5 {
                            for h in [a.in_handle, a.out_handle] {
                                let hp = to_screen(h);
                                painter.line_segment(
                                    [at, hp],
                                    egui::Stroke::new(1.0, accent.gamma_multiply(0.8)),
                                );
                                painter.circle_filled(hp, 3.0, accent);
                            }
                        }
                        // Selected anchors are filled, unselected hollow —
                        // the convention every vector editor uses.
                        if selected {
                            painter.circle_filled(at, 4.0, accent);
                            painter.circle_stroke(
                                at,
                                4.0,
                                egui::Stroke::new(1.0, egui::Color32::WHITE),
                            );
                        } else {
                            painter.circle_filled(at, 3.5, egui::Color32::WHITE);
                            painter.circle_stroke(at, 3.5, egui::Stroke::new(1.2, accent));
                        }
                    }
                }
            }
        }
    }

    // The Pen tool's work in progress: the curve so far, its anchors, and the
    // segment that would follow the pointer. Drawn here rather than as a layer
    // because an unfinished path is not part of the document yet.
    if let Some(pen) = &app.pen {
        let to_screen = |p: cshop_core::geom::Vec2| {
            app.docs[index].doc_to_screen(viewport, egui::vec2(p.x, p.y))
        };
        let zoom = app.docs[index].zoom;
        let closing = pen.cursor.is_some_and(|c| pen.would_close(c, zoom));

        // The committed curve, flattened the same way it will be rendered.
        if pen.anchors.len() >= 2 {
            let flat = pen.to_path(false).flatten(0.4);
            for part in &flat.parts {
                for line in part.open.iter().chain(part.closed.iter()) {
                    let pts: Vec<egui::Pos2> = line.iter().map(|p| to_screen(*p)).collect();
                    if pts.len() >= 2 {
                        painter.add(egui::Shape::line(
                            pts,
                            egui::Stroke::new(1.5, egui::Color32::from_rgb(90, 170, 255)),
                        ));
                    }
                }
            }
        }
        // The segment being placed, so the shape of the next curve is visible
        // before the button goes down.
        if let (Some(last), Some(cursor)) = (pen.anchors.last(), pen.cursor) {
            let target = if closing { pen.first().unwrap_or(cursor) } else { cursor };
            painter.line_segment(
                [to_screen(last.at), to_screen(target)],
                egui::Stroke::new(1.0, egui::Color32::from_rgb(90, 170, 255).gamma_multiply(0.6)),
            );
        }
        for (i, a) in pen.anchors.iter().enumerate() {
            let p = to_screen(a.at);
            // Handles, drawn only where they are actually pulled out.
            if a.at.distance(a.out_handle) > 0.5 {
                for h in [a.in_handle, a.out_handle] {
                    painter.line_segment(
                        [p, to_screen(h)],
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 200, 255).gamma_multiply(0.7)),
                    );
                    painter.circle_filled(to_screen(h), 2.5, egui::Color32::from_rgb(120, 200, 255));
                }
            }
            // The first anchor grows when clicking it would close the path.
            let first_and_closing = i == 0 && closing;
            let r = if first_and_closing { 6.0 } else { 3.5 };
            painter.circle_filled(p, r, egui::Color32::WHITE);
            painter.circle_stroke(p, r, egui::Stroke::new(1.5, egui::Color32::from_rgb(40, 90, 160)));
        }
    }

    if let Some(drag) = &app.drag {
        let points = drag.preview();
        if points.len() >= 2 {
            let screen: Vec<egui::Pos2> = points
                .iter()
                .map(|p| app.docs[index].doc_to_screen(viewport, egui::vec2(p.x, p.y)))
                .collect();
            draw_ants(painter, &screen, drag.is_closed(), time, viewport);
        }
    }

    if !has_selection {
        return;
    }
    // Outlines are cached on the selection and only retraced when it changes.
    let zoom = app.docs[index].zoom;
    let origin = app.docs[index].canvas_rect(viewport).min;
    let Some(selection) = app.docs[index].doc.selection.as_mut() else { return };
    for contour in selection.contours() {
        if contour.len() < 2 {
            continue;
        }
        let screen: Vec<egui::Pos2> =
            contour.iter().map(|p| origin + egui::vec2(p.x, p.y) * zoom).collect();
        draw_ants(painter, &screen, true, time, viewport);
    }
}

/// A dashed outline in two alternating colours, so it reads against any image.
fn draw_ants(
    painter: &egui::Painter,
    points: &[egui::Pos2],
    close: bool,
    time: f32,
    viewport: egui::Rect,
) {
    let mut pts = points.to_vec();
    if close {
        if let Some(first) = pts.first().copied() {
            pts.push(first);
        }
    }
    // Reject outlines entirely off screen; a magic wand selection can have
    // hundreds of contours and most are usually out of view.
    let visible = pts.iter().any(|p| viewport.expand(4.0).contains(*p));
    if !visible {
        return;
    }

    const DASH: f32 = 4.0;
    // A moving phase is what makes the ants march.
    let phase = (time * 12.0) % (DASH * 2.0);

    // White underneath, black dashes on top: the classic two-tone outline that
    // stays visible on both light and dark pixels.
    painter.add(egui::Shape::line(pts.clone(), egui::Stroke::new(1.0, egui::Color32::WHITE)));
    painter.add(egui::Shape::dashed_line_with_offset(
        &pts,
        egui::Stroke::new(1.0, egui::Color32::BLACK),
        &[DASH],
        &[DASH],
        phase,
    ));
}

/// Quick Mask paints everything *outside* the selection with translucent red,
/// the traditional rubylith red.
fn draw_quick_mask(app: &mut CShopApp, painter: &egui::Painter, viewport: egui::Rect) {
    let Some(index) = app.active else { return };
    let view = &app.docs[index];
    let Some(selection) = &view.doc.selection else { return };

    let canvas = view.canvas_rect(viewport).intersect(viewport);
    if !canvas.is_positive() {
        return;
    }
    let zoom = view.zoom;
    // Sample coarsely and draw blocks: an exact overlay would need a texture
    // upload per stroke, and this is a transient editing aid.
    let step = (6.0 / zoom).max(1.0);
    let origin = view.canvas_rect(viewport).min;

    let mut y = (canvas.min.y - origin.y) / zoom;
    let y_end = (canvas.max.y - origin.y) / zoom;
    while y < y_end {
        let mut x = (canvas.min.x - origin.x) / zoom;
        let x_end = (canvas.max.x - origin.x) / zoom;
        while x < x_end {
            let cov = selection.coverage(x as i32, y as i32);
            if cov < 255 {
                let alpha = ((255 - cov) as f32 * 0.45) as u8;
                let rect = egui::Rect::from_min_size(
                    origin + egui::vec2(x, y) * zoom,
                    egui::vec2(step * zoom, step * zoom),
                );
                painter.rect_filled(
                    rect.intersect(canvas),
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(255, 40, 40, alpha),
                );
            }
            x += step;
        }
        y += step;
    }
}

fn draw_checkerboard(painter: &egui::Painter, rect: egui::Rect) {
    let p = Palette::DARK;
    painter.rect_filled(rect, 0.0, p.checker_light);

    // Align the pattern to the viewport origin so it does not shimmer as the
    // canvas moves.
    let x0 = (rect.min.x / CHECKER).floor() as i32;
    let y0 = (rect.min.y / CHECKER).floor() as i32;
    let x1 = (rect.max.x / CHECKER).ceil() as i32;
    let y1 = (rect.max.y / CHECKER).ceil() as i32;

    for gy in y0..y1 {
        for gx in x0..x1 {
            if (gx + gy) % 2 == 0 {
                continue;
            }
            let square = egui::Rect::from_min_size(
                egui::pos2(gx as f32 * CHECKER, gy as f32 * CHECKER),
                egui::vec2(CHECKER, CHECKER),
            )
            .intersect(rect);
            if square.is_positive() {
                painter.rect_filled(square, 0.0, p.checker_dark);
            }
        }
    }
}

fn interact(
    app: &mut CShopApp,
    ui: &mut egui::Ui,
    response: &egui::Response,
    viewport: egui::Rect,
) {
    let Some(index) = app.active else { return };

    // A dialog that is not modal — the Layer Style window, which the user can
    // push aside to watch the canvas — leaves clicks reaching the tools. The
    // canvas may still be scrolled and zoomed to look at the preview, but a
    // stray click must not paint on it.
    let dialog_open = app.dialog.is_open();

    // The Segment window takes the canvas as its input: a click says "this",
    // Alt-click says "not this". Handled before the tools, since while it is
    // open that is what a click means.
    // Colour Range takes its samples off the canvas, so while it is open a
    // click means "that colour" rather than whatever the tool would have done.
    if matches!(app.dialog, crate::dialogs::Dialog::ColorRange(_)) {
        if response.clicked_by(egui::PointerButton::Primary) {
            if let Some(p) = response.interact_pointer_pos() {
                let v = app.docs[index].screen_to_doc(viewport, p);
                let alt = ui.input(|i| i.modifiers.alt);
                let gpu = app.gpu.clone();
                let sampled =
                    app.docs[index].sample_composite(&gpu, v.x as i32, v.y as i32);
                if let (Some(c), crate::dialogs::Dialog::ColorRange(d)) = (sampled, &mut app.dialog)
                {
                    // Alt adds without having to reach for the checkbox, which
                    // is what a second sample almost always means.
                    let was = d.adding;
                    d.adding = was || alt;
                    d.sample(c);
                    d.adding = was;
                }
            }
        }
        return;
    }

    if matches!(app.dialog, crate::dialogs::Dialog::Segment(_)) {
        if response.clicked_by(egui::PointerButton::Primary) {
            if let Some(p) = response.interact_pointer_pos() {
                let v = app.docs[index].screen_to_doc(viewport, p);
                let alt = ui.input(|i| i.modifiers.alt);
                let mut accepted = false;
                if let crate::dialogs::Dialog::Segment(d) = &mut app.dialog {
                    // A click while one is already running would queue a
                    // second and answer with whichever finished last.
                    if !d.busy {
                        d.add_hint(cshop_core::geom::Vec2::new(v.x, v.y), !alt);
                        accepted = true;
                    }
                }
                if accepted {
                    app.push(Action::SegmentPreview);
                }
            }
        }
        return;
    }

    // --- scroll and zoom ---------------------------------------------------
    if response.hovered() {
        let (scroll, zoom_delta, modifiers) = ui.input(|i| {
            (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers)
        });
        let pointer = ui.input(|i| i.pointer.hover_pos()).unwrap_or(viewport.center());

        if zoom_delta != 1.0 {
            let z = app.docs[index].zoom * zoom_delta;
            app.docs[index].zoom_to(viewport, z, pointer);
        } else if scroll != egui::Vec2::ZERO {
            if modifiers.command {
                // Ctrl+wheel zooms, as in every image editor.
                let factor = (scroll.y * 0.005).exp();
                let z = app.docs[index].zoom * factor;
                app.docs[index].zoom_to(viewport, z, pointer);
            } else if modifiers.alt {
                // Alt+wheel resizes the brush, which is the usual binding.
                app.brush.size = (app.brush.size + scroll.y * 0.25).clamp(1.0, 2000.0);
            } else {
                let zoom = app.docs[index].zoom;
                // Shift swaps the scroll axis for horizontal panning.
                let delta = if modifiers.shift {
                    egui::vec2(scroll.y, scroll.x)
                } else {
                    scroll
                };
                app.docs[index].center -= delta / zoom;
            }
        }
    }

    // --- space-drag pans, whatever the active tool -------------------------
    let space_held = ui.input(|i| i.key_down(egui::Key::Space));
    // Middle-drag pans whatever the active tool is.
    let panning =
        space_held || app.tool == Tool::Hand || response.dragged_by(egui::PointerButton::Middle);

    if panning {
        // Either button, since the middle-drag case is a pan by definition.
        if response.dragged() {
            let zoom = app.docs[index].zoom;
            app.docs[index].center -= response.drag_delta() / zoom;
        }
        return;
    }

    if dialog_open {
        return;
    }

    // A live transform or crop takes every pointer event before the tools do.
    if app.transform.is_some() {
        transform_interact(app, ui, response, viewport);
        return;
    }
    if app.tool == Tool::Crop {
        crop_interact(app, ui, response, viewport);
        return;
    }

    let Some(pointer) = response.interact_pointer_pos() else {
        // Releasing outside the canvas must still finish the stroke.
        if app.is_painting() && ui.input(|i| i.pointer.primary_released()) {
            app.end_stroke();
        }
        return;
    };
    let doc_point = {
        let v = app.docs[index].screen_to_doc(viewport, pointer);
        Vec2::new(v.x, v.y)
    };

    match app.tool {
        Tool::Brush
        | Tool::Pencil
        | Tool::Eraser
        | Tool::CloneStamp
        | Tool::Dodge
        | Tool::Burn
        | Tool::Sponge
        | Tool::Blur
        | Tool::Sharpen
        | Tool::Smudge
        | Tool::HealingBrush
        | Tool::SpotHealing
        | Tool::HistoryBrush => {
            let alt_held = ui.input(|i| i.modifiers.alt);
            let mode = match app.tool.retouches() {
                // Holding Alt swaps dodge for burn and back, which is how the
                // pair is actually used: you lighten, see you went too far,
                // and darken the same spot without leaving the stroke.
                Some(kind) => {
                    use cshop_core::retouch::RetouchKind;
                    let kind = match (kind, alt_held) {
                        (RetouchKind::Dodge, true) => RetouchKind::Burn,
                        (RetouchKind::Burn, true) => RetouchKind::Dodge,
                        (RetouchKind::Sponge, true) => RetouchKind::Sponge,
                        (k, false) => k,
                    };
                    let soak = if kind == RetouchKind::Sponge && alt_held {
                        !app.retouch.soak
                    } else {
                        app.retouch.soak
                    };
                    PaintMode::Retouch(cshop_core::retouch::Retouch { kind, soak, ..app.retouch })
                }
                None if app.tool == Tool::Eraser => PaintMode::Erase,
                None => PaintMode::Paint,
            };
            let clone = app.tool == Tool::CloneStamp;
            let alt = alt_held;

            // Alt-click sets where the Clone Stamp and the Healing Brush copy
            // from. Spot Healing finds its own, so Alt means nothing to it.
            if (clone || app.tool == Tool::HealingBrush) && alt {
                if response.clicked_by(egui::PointerButton::Primary)
                    || response.drag_started_by(egui::PointerButton::Primary)
                {
                    app.set_clone_anchor(doc_point);
                }
                return;
            }

            if response.drag_started_by(egui::PointerButton::Primary) || (response.clicked_by(egui::PointerButton::Primary) && !app.is_painting()) {
                match app.tool {
                    Tool::Smudge => {
                        let strength = app.brush_filter_strength;
                        app.begin_smudge(doc_point, strength);
                    }
                    Tool::HealingBrush => {
                        app.begin_stroke_from(doc_point, mode, StrokeFrom::Heal)
                    }
                    Tool::SpotHealing => {
                        app.begin_stroke_from(doc_point, mode, StrokeFrom::HealSpot)
                    }
                    Tool::HistoryBrush => {
                        app.begin_stroke_from(doc_point, mode, StrokeFrom::History)
                    }
                    _ => match app.brush_filter() {
                        Some(filter) => {
                            app.begin_stroke_from(doc_point, mode, StrokeFrom::Filter(filter))
                        }
                        None => app.begin_stroke_with(doc_point, mode, clone),
                    },
                }
            } else if response.dragged_by(egui::PointerButton::Primary) && app.is_painting() {
                app.continue_stroke(doc_point);
            }
            if response.drag_stopped_by(egui::PointerButton::Primary)
                || ui.input(|i| i.pointer.primary_released())
            {
                app.end_stroke();
            }
        }

        Tool::Shape => {
            if response.drag_started_by(egui::PointerButton::Primary) {
                app.drag_start = Some(doc_point);
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                if let Some(from) = app.drag_start.take() {
                    let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
                    app.push(Action::DrawShape {
                        from,
                        to: doc_point,
                        from_centre: alt,
                        constrain: shift,
                    });
                }
            }
        }

        // Direct Selection: pick an anchor or a handle, and move it.
        Tool::DirectSelect => {
            let zoom = app.doc().map_or(1.0, |v| v.zoom);
            let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));

            // The hit is taken when the button goes down, not when egui
            // decides a drag has begun — by then the pointer has moved past
            // the drag threshold and is no longer over the anchor it grabbed.
            let pressed = ui.input(|i| i.pointer.primary_pressed());
            if pressed {
                match app.path_hit(doc_point, zoom) {
                    Some((at, kind)) => {
                        if kind == crate::app::HandleKind::Anchor {
                            if shift {
                                if let Some(i) =
                                    app.path_edit.selected.iter().position(|s| *s == at)
                                {
                                    app.path_edit.selected.remove(i);
                                } else {
                                    app.path_edit.selected.push(at);
                                }
                            } else if !app.path_edit.is_selected(at) {
                                app.path_edit.selected = vec![at];
                            }
                        }
                        app.path_edit.drag = Some((at, kind));
                        app.path_edit.last = Some(doc_point);
                        // A fresh identifier, so this drag is its own step.
                        app.path_edit.run = Some(app.next_edit_run());
                    }
                    // Clicking away from every point clears the selection, the
                    // way clicking off a selection does everywhere else.
                    None if !shift => app.path_edit.selected.clear(),
                    None => {}
                }
            }
            if response.dragged_by(egui::PointerButton::Primary) {
                if let (Some(last), Some(_)) = (app.path_edit.last, app.path_edit.drag) {
                    let delta = cshop_core::geom::Vec2::new(
                        doc_point.x - last.x,
                        doc_point.y - last.y,
                    );
                    app.drag_path(delta, alt);
                    app.path_edit.last = Some(doc_point);
                }
            }
            if ui.input(|i| i.pointer.primary_released()) {
                app.path_edit.drag = None;
                app.path_edit.last = None;
                app.path_edit.run = None;
            }
        }

        // The Pen: click for a corner, drag for a curve, click the first
        // anchor again to close.
        Tool::Pen => {
            let zoom = app.doc().map_or(1.0, |v| v.zoom);
            if response.drag_started_by(egui::PointerButton::Primary)
                || response.clicked_by(egui::PointerButton::Primary)
            {
                let draft = app.pen.get_or_insert_with(Default::default);
                if draft.would_close(doc_point, zoom) {
                    app.push(Action::FinishPath { closed: true });
                } else {
                    draft.anchors.push(cshop_core::path::Anchor::corner(doc_point));
                    draft.dragging = Some(draft.anchors.len() - 1);
                }
            }
            if response.dragged_by(egui::PointerButton::Primary) {
                if let Some(draft) = app.pen.as_mut() {
                    if let Some(i) = draft.dragging {
                        // Dragging pulls a handle out of the anchor just
                        // placed, mirrored so the curve runs smoothly through.
                        let at = draft.anchors[i].at;
                        draft.anchors[i] = cshop_core::path::Anchor::smooth(at, doc_point);
                    }
                }
            }
            if response.drag_stopped_by(egui::PointerButton::Primary)
                || response.clicked_by(egui::PointerButton::Primary)
            {
                if let Some(draft) = app.pen.as_mut() {
                    draft.dragging = None;
                }
            }
            if let Some(draft) = app.pen.as_mut() {
                draft.cursor = Some(doc_point);
            }
        }

        Tool::Text => {
            // Dragging draws a paragraph box; a plain click makes point text.
            if response.drag_started_by(egui::PointerButton::Primary) {
                app.drag_start = Some(doc_point);
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                let start = app.drag_start.take().unwrap_or(doc_point);
                let width = (doc_point.x - start.x).abs();
                let at = Vec2::new(start.x.min(doc_point.x), start.y.min(doc_point.y));
                app.push(Action::BeginText { at, wrap: Some(width) });
            } else if response.clicked_by(egui::PointerButton::Primary) {
                app.drag_start = None;
                // Clicking the type already being edited moves the caret;
                // clicking other type opens it; anywhere else starts afresh.
                if app.editing_text_contains(doc_point) {
                    app.push(Action::TextCaretAt(doc_point));
                } else if let Some(id) = app.text_layer_at(doc_point) {
                    app.push(Action::EditTextLayer(id));
                } else {
                    app.push(Action::BeginText { at: doc_point, wrap: None });
                }
            }
        }

        Tool::Eyedropper => {
            if response.dragged_by(egui::PointerButton::Primary) || response.clicked_by(egui::PointerButton::Primary) {
                app.pick_color(doc_point);
            }
        }

        Tool::Zoom => {
            if response.clicked_by(egui::PointerButton::Primary) {
                let alt = ui.input(|i| i.modifiers.alt);
                let z = app.docs[index].stepped_zoom(!alt);
                app.docs[index].zoom_to(viewport, z, pointer);
            }
        }

        Tool::RectangularMarquee | Tool::EllipticalMarquee => {
            let ellipse = app.tool == Tool::EllipticalMarquee;
            let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));

            if response.drag_started_by(egui::PointerButton::Primary) {
                // Modifiers at the *start* of the drag choose the boolean mode;
                // during the drag they constrain the shape instead, which is
                // why the mode is captured once here.
                let mode = if app.selection_mode == SelectionMode::Replace {
                    SelectionMode::from_modifiers(shift, alt)
                } else {
                    app.selection_mode
                };
                app.drag = Some(SelectionDrag::Marquee {
                    start: doc_point,
                    current: doc_point,
                    ellipse,
                    mode,
                });
            } else if response.dragged_by(egui::PointerButton::Primary) {
                if let Some(SelectionDrag::Marquee { start, current, .. }) = &mut app.drag {
                    // Shift constrains to a square, Alt grows from the centre.
                    *current = doc_point;
                    let (s, c) = (*start, *current);
                    let rect = if shift {
                        Rectf::constrain_square(s, c)
                    } else {
                        Rectf::from_points(s, c)
                    };
                    let rect = if alt { Rectf::from_center(s, c) } else { rect };
                    *start = Vec2::new(rect.x0, rect.y0);
                    *current = Vec2::new(rect.x1, rect.y1);
                }
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                app.finish_selection_drag();
            } else if response.clicked_by(egui::PointerButton::Primary) && app.drag.is_none() {
                // A bare click clears the selection.
                app.push(Action::Deselect);
            }
        }

        Tool::Lasso => {
            let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
            if response.drag_started_by(egui::PointerButton::Primary) {
                let mode = if app.selection_mode == SelectionMode::Replace {
                    SelectionMode::from_modifiers(shift, alt)
                } else {
                    app.selection_mode
                };
                app.drag = Some(SelectionDrag::Lasso { points: vec![doc_point], mode });
            } else if response.dragged_by(egui::PointerButton::Primary) {
                if let Some(SelectionDrag::Lasso { points, .. }) = &mut app.drag {
                    // Drop samples closer than a pixel: a slow drag would
                    // otherwise pile up thousands of near-identical vertices.
                    if points.last().is_none_or(|p| p.distance(doc_point) > 1.0) {
                        points.push(doc_point);
                    }
                }
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                app.finish_selection_drag();
            }
        }

        Tool::PolygonalLasso => {
            let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
            if response.clicked_by(egui::PointerButton::Primary) {
                match &mut app.drag {
                    Some(SelectionDrag::Polygon { points, .. }) => {
                        // Clicking back on the first vertex closes the shape.
                        let close = points
                            .first()
                            .is_some_and(|p| p.distance(doc_point) * app.docs[index].zoom < 8.0);
                        if close || response.double_clicked_by(egui::PointerButton::Primary) {
                            app.finish_selection_drag();
                        } else {
                            points.push(doc_point);
                        }
                    }
                    _ => {
                        let mode = if app.selection_mode == SelectionMode::Replace {
                            SelectionMode::from_modifiers(shift, alt)
                        } else {
                            app.selection_mode
                        };
                        app.drag = Some(SelectionDrag::Polygon {
                            points: vec![doc_point],
                            cursor: doc_point,
                            mode,
                        });
                    }
                }
            }
            if let Some(SelectionDrag::Polygon { cursor, .. }) = &mut app.drag {
                *cursor = doc_point;
            }
        }

        Tool::PaintBucket => {
            if response.clicked_by(egui::PointerButton::Primary) {
                app.bucket_fill_at(doc_point);
            }
        }

        Tool::Gradient => {
            if response.drag_started_by(egui::PointerButton::Primary) {
                app.gradient_drag = Some((doc_point, doc_point));
            } else if response.dragged_by(egui::PointerButton::Primary) {
                if let Some((start, current)) = &mut app.gradient_drag {
                    // Shift constrains to 45-degree steps.
                    *current = if ui.input(|i| i.modifiers.shift) {
                        constrain_angle(*start, doc_point)
                    } else {
                        doc_point
                    };
                }
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                app.commit_gradient();
            }
        }

        Tool::MagicWand => {
            if response.clicked_by(egui::PointerButton::Primary) {
                let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
                let mode = if app.selection_mode == SelectionMode::Replace {
                    SelectionMode::from_modifiers(shift, alt)
                } else {
                    app.selection_mode
                };
                app.magic_wand_at(doc_point, mode);
            }
        }

        Tool::Move => {
            if response.dragged_by(egui::PointerButton::Primary) {
                let zoom = app.docs[index].zoom;
                let delta = response.drag_delta() / zoom;
                // Only whole pixels; sub-pixel layer offsets would need
                // resampling, which belongs with the transform tool.
                let (mut dx, mut dy) = (delta.x.round() as i32, delta.y.round() as i32);

                // Where the layer would land, offered to the guides. Snapping
                // the *destination* each frame rather than the movement is
                // what gives it the magnetic feel: a small drag away is pulled
                // back, and a larger one lets go.
                if app.snap && (dx != 0 || dy != 0) {
                    let view = &app.docs[index];
                    if let Some(bounds) = view.doc.active.and_then(|id| {
                        view.doc.tree.get(id).map(|l| l.bounds())
                    }) {
                        let mut lines = cshop_core::guides::SnapLines::for_document(
                            view.doc.width,
                            view.doc.height,
                        );
                        lines.add_guides(&view.doc.guides);
                        if app.show_grid {
                            let near = cshop_core::geom::Vec2::new(
                                (bounds.x0 + dx) as f32,
                                (bounds.y0 + dy) as f32,
                            );
                            lines.add_grid(app.grid_spacing, near, app.grid_spacing * 2.0);
                        }
                        let proposed = cshop_core::geom::IRect::new(
                            bounds.x0 + dx,
                            bounds.y0 + dy,
                            bounds.x1 + dx,
                            bounds.y1 + dy,
                        );
                        // The reach is fixed on screen, so a guide is equally
                        // easy to catch at any magnification.
                        let shift = cshop_core::guides::snap_offset(
                            proposed,
                            &lines,
                            SNAP_REACH / zoom,
                        );
                        dx += shift.x.round() as i32;
                        dy += shift.y.round() as i32;
                    }
                }

                if dx != 0 || dy != 0 {
                    let view = &mut app.docs[index];
                    if let Some(id) = view.doc.active {
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
            }
        }

        other => {
            if response.clicked_by(egui::PointerButton::Primary) && !other.is_implemented() {
                app.push(Action::SelectTool(other));
            }
        }
    }
}

/// Snap a drag to the nearest 45 degrees.
fn constrain_angle(from: Vec2, to: Vec2) -> Vec2 {
    let d = to - from;
    let length = d.length();
    if length < 1e-4 {
        return to;
    }
    let step = std::f32::consts::FRAC_PI_4;
    let angle = (d.y.atan2(d.x) / step).round() * step;
    Vec2::new(from.x + angle.cos() * length, from.y + angle.sin() * length)
}

/// Dragging the transform box.
fn transform_interact(
    app: &mut CShopApp,
    ui: &mut egui::Ui,
    response: &egui::Response,
    viewport: egui::Rect,
) {
    let Some(index) = app.active else { return };
    let zoom = app.docs[index].zoom;
    let Some(pointer) = response.interact_pointer_pos() else {
        if let Some(t) = &mut app.transform {
            t.end_drag();
        }
        return;
    };
    let doc = {
        let v = app.docs[index].screen_to_doc(viewport, pointer);
        Vec2::new(v.x, v.y)
    };
    let (shift, alt, ctrl) =
        ui.input(|i| (i.modifiers.shift, i.modifiers.alt, i.modifiers.command));

    let Some(active) = &mut app.transform else { return };
    if response.drag_started_by(egui::PointerButton::Primary) {
        if let Some(handle) = active.hit(doc, zoom) {
            active.begin_drag(handle, doc);
        }
    } else if response.dragged_by(egui::PointerButton::Primary) {
        active.drag_to(doc, ctrl, shift, alt);
    }
    if response.drag_stopped_by(egui::PointerButton::Primary) {
        active.end_drag();
    }
    // Double-clicking inside the box applies it.
    if response.double_clicked_by(egui::PointerButton::Primary) && active.contains(doc) {
        app.push(Action::CommitTransform);
    }
}

/// Dragging the crop rectangle.
fn crop_interact(
    app: &mut CShopApp,
    ui: &mut egui::Ui,
    response: &egui::Response,
    viewport: egui::Rect,
) {
    let Some(index) = app.active else { return };
    let zoom = app.docs[index].zoom;
    let bounds = app.docs[index].doc.bounds();
    let Some(pointer) = response.interact_pointer_pos() else { return };
    let doc = {
        let v = app.docs[index].screen_to_doc(viewport, pointer);
        Vec2::new(v.x, v.y)
    };
    let _ = ui;

    match &mut app.crop {
        Some(crop) => {
            if response.drag_started_by(egui::PointerButton::Primary) {
                match crop.hit(doc, zoom) {
                    Some(handle) => crop.begin_drag(handle, doc),
                    // Starting a drag outside the current rectangle begins a
                    // fresh one.
                    None => {
                        crop.rect = IRect::from_points(
                            doc.x as i32,
                            doc.y as i32,
                            doc.x as i32 + 1,
                            doc.y as i32 + 1,
                        );
                        crop.begin_drag(Handle::BottomRight, doc);
                    }
                }
            } else if response.dragged_by(egui::PointerButton::Primary) {
                crop.drag_to(doc, bounds);
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                crop.end_drag();
            }
            if response.double_clicked_by(egui::PointerButton::Primary) {
                app.push(Action::CommitCrop);
            }
        }
        None => {
            if response.drag_started_by(egui::PointerButton::Primary) {
                let mut crop = crate::transform_tool::ActiveCrop::new(IRect::from_points(
                    doc.x as i32,
                    doc.y as i32,
                    doc.x as i32 + 1,
                    doc.y as i32 + 1,
                ));
                crop.begin_drag(Handle::BottomRight, doc);
                app.crop = Some(crop);
            }
        }
    }
}

/// The line a gradient is being dragged along.
fn draw_gradient_guide(app: &CShopApp, painter: &egui::Painter, viewport: egui::Rect) {
    let Some(index) = app.active else { return };
    let Some((from, to)) = app.gradient_drag else { return };
    let view = &app.docs[index];
    let a = view.doc_to_screen(viewport, egui::vec2(from.x, from.y));
    let b = view.doc_to_screen(viewport, egui::vec2(to.x, to.y));

    // Two-tone, so it reads against whatever is underneath.
    painter.line_segment([a, b], egui::Stroke::new(3.0, egui::Color32::from_black_alpha(150)));
    painter.line_segment([a, b], egui::Stroke::new(1.0, egui::Color32::WHITE));
    for end in [a, b] {
        painter.circle_stroke(end, 4.0, egui::Stroke::new(1.0, egui::Color32::WHITE));
        painter.circle_stroke(end, 5.0, egui::Stroke::new(1.0, egui::Color32::from_black_alpha(150)));
    }
}

/// The rubber band while a shape is being dragged out.
fn draw_shape_preview(
    app: &CShopApp,
    ui: &egui::Ui,
    painter: &egui::Painter,
    viewport: egui::Rect,
) {
    if app.tool != Tool::Shape {
        return;
    }
    let Some(from) = app.drag_start else { return };
    let Some(index) = app.active else { return };
    let Some(pointer) = ui.ctx().pointer_latest_pos() else { return };
    let view = &app.docs[index];
    let to = view.screen_to_doc(viewport, pointer);
    let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
    let (origin, size) = CShopApp::shape_rect(from, Vec2::new(to.x, to.y), alt, shift);

    let a = view.doc_to_screen(viewport, egui::vec2(origin.x, origin.y));
    let b = view.doc_to_screen(viewport, egui::vec2(origin.x + size.0, origin.y + size.1));
    let rect = egui::Rect::from_two_pos(a, b);
    // Two strokes so the band reads over any content underneath.
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(3.0, egui::Color32::from_black_alpha(110)),
        egui::StrokeKind::Outside,
    );
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::WHITE),
        egui::StrokeKind::Outside,
    );
}

/// The caret, and the outline of the type being edited.
fn draw_text_caret(
    app: &CShopApp,
    ui: &egui::Ui,
    painter: &egui::Painter,
    viewport: egui::Rect,
) {
    let Some(edit) = app.text_edit.as_ref() else { return };
    let Some(index) = app.active else { return };
    let view = &app.docs[index];

    // A dashed outline shows what is being edited, since empty type has
    // nothing else to see.
    if let Some(layer) = view.doc.tree.get(edit.layer) {
        let b = layer.bounds();
        let a = view.doc_to_screen(viewport, egui::vec2(b.x0 as f32, b.y0 as f32));
        let c = view.doc_to_screen(viewport, egui::vec2(b.x1 as f32, b.y1 as f32));
        painter.rect_stroke(
            egui::Rect::from_two_pos(a, c),
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(120, 170, 255, 110)),
            egui::StrokeKind::Inside,
        );
    }

    let Some((top, bottom)) = app.text_caret_rect() else { return };
    // Blink at a typical editor's rate, measured from the last keystroke so
    // the caret is solid while someone is typing rather than flickering.
    let since = ui.input(|i| i.time) - edit.blink_epoch;
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
    if since > 0.6 && (since * 1.6).fract() > 0.5 {
        return;
    }
    let a = view.doc_to_screen(viewport, egui::vec2(top.x, top.y));
    let b = view.doc_to_screen(viewport, egui::vec2(bottom.x, bottom.y));
    // Drawn twice so it stays visible over both light and dark type.
    painter.line_segment([a, b], egui::Stroke::new(3.0, egui::Color32::from_black_alpha(120)));
    painter.line_segment([a, b], egui::Stroke::new(1.5, egui::Color32::WHITE));
}

/// Where the Clone Stamp is copying from.
fn draw_clone_anchor(
    app: &CShopApp,
    ui: &egui::Ui,
    painter: &egui::Painter,
    viewport: egui::Rect,
) {
    if app.tool != Tool::CloneStamp {
        return;
    }
    let Some(index) = app.active else { return };
    let view = &app.docs[index];

    // Track the pointer, so the crosshair shows where the *next* dab will come
    // from rather than where the anchor was dropped.
    let pointer = ui.ctx().pointer_latest_pos().filter(|p| viewport.contains(*p)).map(|p| {
        let v = view.screen_to_doc(viewport, p);
        Vec2::new(v.x, v.y)
    });
    let Some(source) = app.clone_source_at(pointer) else { return };
    let p = view.doc_to_screen(viewport, egui::vec2(source.x, source.y));

    // Red once the source has left the image, because from there on the tool
    // deposits nothing and the reason is otherwise invisible.
    let outside = !view.doc.bounds().contains(source.x as i32, source.y as i32);
    let colour =
        if outside { egui::Color32::from_rgb(0xff, 0x60, 0x50) } else { egui::Color32::WHITE };

    // A crosshair, so the source is visible without obscuring it.
    let stroke = egui::Stroke::new(1.0, colour);
    let shadow = egui::Stroke::new(3.0, egui::Color32::from_black_alpha(140));
    for (a, b) in [
        (p - egui::vec2(8.0, 0.0), p + egui::vec2(8.0, 0.0)),
        (p - egui::vec2(0.0, 8.0), p + egui::vec2(0.0, 8.0)),
    ] {
        painter.line_segment([a, b], shadow);
        painter.line_segment([a, b], stroke);
    }
    painter.circle_stroke(p, 5.0, stroke);
}

/// Cursor feedback: a brush outline where it helps, a system cursor elsewhere.
fn cursor(app: &CShopApp, ui: &mut egui::Ui, response: &egui::Response, viewport: egui::Rect) {
    let Some(index) = app.active else { return };
    let space_held = ui.input(|i| i.key_down(egui::Key::Space));

    let icon = if space_held || app.tool == Tool::Hand {
        egui::CursorIcon::Grab
    } else {
        match app.tool {
            Tool::Move => egui::CursorIcon::Move,
            Tool::Zoom => egui::CursorIcon::ZoomIn,
            Tool::Text => egui::CursorIcon::Text,
            Tool::Eyedropper => egui::CursorIcon::Crosshair,
            Tool::Brush | Tool::Pencil | Tool::Eraser | Tool::CloneStamp => {
                egui::CursorIcon::None
            }
            _ => egui::CursorIcon::Crosshair,
        }
    };
    if response.hovered() {
        ui.ctx().set_cursor_icon(icon);
    }

    // Draw the brush footprint so the user can see its true size.
    let paints =
        matches!(app.tool, Tool::Brush | Tool::Pencil | Tool::Eraser | Tool::CloneStamp);
    if paints && response.hovered() && !space_held {
        if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
            let r = app.brush.size / 2.0 * app.docs[index].zoom;
            let painter = ui.painter_at(viewport);
            // Two rings, light over dark, so the outline reads on any image.
            painter.circle_stroke(p, r, egui::Stroke::new(1.0, egui::Color32::from_black_alpha(160)));
            painter.circle_stroke(
                p,
                r - 1.0,
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(190)),
            );
            if r < 3.0 {
                painter.circle_filled(p, 1.0, egui::Color32::from_white_alpha(190));
            }
        }
    }
}

fn empty_state(app: &mut CShopApp, ui: &mut egui::Ui, viewport: egui::Rect) {
    let p = Palette::DARK;
    ui.painter().rect_filled(viewport, 0.0, p.canvas_backdrop);

    ui.scope_builder(egui::UiBuilder::new().max_rect(viewport), |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(viewport.height() * 0.3);
            ui.label(
                egui::RichText::new("C-Shop").size(38.0).color(egui::Color32::from_gray(0x55)),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("A native, GPU-accelerated layered image editor")
                    .color(p.text_dim),
            );
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                // Centre the button row inside the available width.
                let w = 260.0;
                ui.add_space((ui.available_width() - w).max(0.0) / 2.0);
                if ui.add_sized([120.0, 28.0], egui::Button::new("New Document…")).clicked() {
                    app.push(Action::NewDocument);
                }
                if ui.add_sized([120.0, 28.0], egui::Button::new("Open…")).clicked() {
                    app.push(Action::ShowOpenDialog);
                }
            });
            ui.add_space(20.0);
            ui.label(
                egui::RichText::new("Ctrl+N  new    ·    Ctrl+O  open    ·    Tab  hide panels")
                    .color(egui::Color32::from_gray(0x4a))
                    .small(),
            );
        });
    });
}
