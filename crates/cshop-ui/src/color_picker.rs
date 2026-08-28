//! A colour picker, drawn to match the rest of the interface.
//!
//! egui has a built-in one, but it is a small popup in its own visual
//! language. This is the familiar shape — a saturation/value square beside a
//! hue strip, with numeric and hex entry and a before/after swatch — and it is
//! shared by the Color panel's *Custom* button and by Edit > Fill.

use crate::theme::Palette;
use cshop_core::color::{hsv_to_rgb, rgb_to_hsv, Rgba8};

/// Editing state that has to survive between frames.
///
/// Hue and saturation cannot be recovered from the colour alone: black has no
/// hue, and white has no saturation, so dragging into either corner would
/// otherwise lose where the user came from.
#[derive(Debug, Clone, Copy)]
pub struct ColorPickerState {
    pub hue: f32,
    pub saturation: f32,
    pub value: f32,
    pub alpha: u8,
}

impl ColorPickerState {
    pub fn from_color(c: Rgba8) -> Self {
        let (h, s, v) = rgb_to_hsv(c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0);
        Self { hue: h, saturation: s, value: v, alpha: c.a }
    }

    pub fn to_color(self) -> Rgba8 {
        let (r, g, b) = hsv_to_rgb(self.hue, self.saturation, self.value);
        Rgba8::new(
            (r * 255.0 + 0.5) as u8,
            (g * 255.0 + 0.5) as u8,
            (b * 255.0 + 0.5) as u8,
            self.alpha,
        )
    }

    fn set_color(&mut self, c: Rgba8) {
        let (h, s, v) = rgb_to_hsv(c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0);
        // Keep the existing hue when the new colour has none to offer.
        if s > 1e-4 {
            self.hue = h;
        }
        if v > 1e-4 {
            self.saturation = s;
        }
        self.value = v;
        self.alpha = c.a;
    }
}

/// Draw the picker. Returns `true` when the colour changed.
///
/// `original` is drawn beside the current colour so the user can see what they
/// started from.
pub fn color_picker(
    ui: &mut egui::Ui,
    state: &mut ColorPickerState,
    original: Rgba8,
    with_alpha: bool,
) -> bool {
    let p = Palette::DARK;
    let mut changed = false;

    ui.horizontal(|ui| {
        // --- saturation / value square -------------------------------------
        let size = 200.0;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click_and_drag());
        paint_sv_square(ui, rect, state.hue);

        if response.is_pointer_button_down_on() || response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                state.saturation = ((pos.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
                // Value runs bottom-to-top, as it does everywhere else.
                state.value = 1.0 - ((pos.y - rect.min.y) / rect.height()).clamp(0.0, 1.0);
                changed = true;
            }
        }

        // The marker, ringed in both black and white so it reads anywhere.
        let marker = egui::pos2(
            rect.min.x + state.saturation * rect.width(),
            rect.min.y + (1.0 - state.value) * rect.height(),
        );
        ui.painter().circle_stroke(marker, 6.0, egui::Stroke::new(1.5, egui::Color32::WHITE));
        ui.painter().circle_stroke(marker, 7.5, egui::Stroke::new(1.0, egui::Color32::BLACK));

        ui.add_space(8.0);

        // --- hue strip ------------------------------------------------------
        let (hue_rect, hue_response) =
            ui.allocate_exact_size(egui::vec2(22.0, size), egui::Sense::click_and_drag());
        paint_hue_strip(ui, hue_rect);
        if hue_response.is_pointer_button_down_on() || hue_response.clicked() {
            if let Some(pos) = hue_response.interact_pointer_pos() {
                state.hue = ((pos.y - hue_rect.min.y) / hue_rect.height()).clamp(0.0, 1.0);
                changed = true;
            }
        }
        let y = hue_rect.min.y + state.hue * hue_rect.height();
        ui.painter().rect_stroke(
            egui::Rect::from_min_max(
                egui::pos2(hue_rect.min.x - 2.0, y - 3.0),
                egui::pos2(hue_rect.max.x + 2.0, y + 3.0),
            ),
            1.0,
            egui::Stroke::new(1.5, egui::Color32::WHITE),
            egui::StrokeKind::Inside,
        );

        ui.add_space(10.0);

        // --- numbers ---------------------------------------------------------
        ui.vertical(|ui| {
            let current = state.to_color();

            // Before and after, split down the middle.
            let (swatch, _) =
                ui.allocate_exact_size(egui::vec2(120.0, 40.0), egui::Sense::hover());
            let half = swatch.width() / 2.0;
            ui.painter().rect_filled(
                egui::Rect::from_min_size(swatch.min, egui::vec2(half, swatch.height())),
                0.0,
                egui::Color32::from_rgb(original.r, original.g, original.b),
            );
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(swatch.min.x + half, swatch.min.y),
                    egui::vec2(half, swatch.height()),
                ),
                0.0,
                egui::Color32::from_rgb(current.r, current.g, current.b),
            );
            ui.painter().rect_stroke(
                swatch,
                0.0,
                egui::Stroke::new(1.0, p.separator),
                egui::StrokeKind::Inside,
            );
            ui.label(egui::RichText::new("before  ·  after").color(p.text_dim).small());

            ui.add_space(6.0);

            let mut rgb = [current.r, current.g, current.b];
            for (label, index) in [("R", 0usize), ("G", 1), ("B", 2)] {
                ui.horizontal(|ui| {
                    ui.label(label);
                    if ui.add(egui::DragValue::new(&mut rgb[index]).range(0..=255)).changed() {
                        state.set_color(Rgba8::new(rgb[0], rgb[1], rgb[2], state.alpha));
                        changed = true;
                    }
                });
            }

            if with_alpha {
                ui.horizontal(|ui| {
                    ui.label("A");
                    if ui.add(egui::DragValue::new(&mut state.alpha).range(0..=255)).changed() {
                        changed = true;
                    }
                });
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("#");
                let mut hex = current.to_hex();
                if ui.add(egui::TextEdit::singleline(&mut hex).desired_width(70.0)).changed() {
                    if let Some(c) = Rgba8::from_hex(&hex) {
                        state.set_color(Rgba8::new(c.r, c.g, c.b, state.alpha));
                        changed = true;
                    }
                }
            });
        });
    });

    ui.add_space(6.0);
    // A row of the colours people reach for most.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
        for c in PRESETS {
            let colour = Rgba8::opaque(c[0], c[1], c[2]);
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
            ui.painter().rect_filled(rect, 1.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
            ui.painter().rect_stroke(
                rect,
                1.0,
                egui::Stroke::new(1.0, p.separator),
                egui::StrokeKind::Inside,
            );
            if response.clicked() {
                state.set_color(colour);
                changed = true;
            }
        }
    });

    changed
}

const PRESETS: &[[u8; 3]] = &[
    [0, 0, 0],
    [64, 64, 64],
    [128, 128, 128],
    [192, 192, 192],
    [255, 255, 255],
    [237, 28, 36],
    [255, 127, 39],
    [255, 242, 0],
    [34, 177, 76],
    [0, 162, 232],
    [63, 72, 204],
    [163, 73, 164],
];

/// The saturation/value square for one hue.
fn paint_sv_square(ui: &egui::Ui, rect: egui::Rect, hue: f32) {
    // Built as a mesh with the four corners coloured: saturation runs left to
    // right and value bottom to top, which is exactly bilinear between them.
    let painter = ui.painter_at(rect);
    let mut mesh = egui::Mesh::default();
    const N: usize = 24;
    for row in 0..=N {
        let v = 1.0 - row as f32 / N as f32;
        for col in 0..=N {
            let s = col as f32 / N as f32;
            let (r, g, b) = hsv_to_rgb(hue, s, v);
            mesh.vertices.push(egui::epaint::Vertex {
                pos: egui::pos2(
                    rect.min.x + s * rect.width(),
                    rect.min.y + (row as f32 / N as f32) * rect.height(),
                ),
                uv: egui::epaint::WHITE_UV,
                color: egui::Color32::from_rgb(
                    (r * 255.0) as u8,
                    (g * 255.0) as u8,
                    (b * 255.0) as u8,
                ),
            });
        }
    }
    let stride = (N + 1) as u32;
    for row in 0..N as u32 {
        for col in 0..N as u32 {
            let i = row * stride + col;
            mesh.indices.extend_from_slice(&[i, i + 1, i + stride]);
            mesh.indices.extend_from_slice(&[i + 1, i + stride + 1, i + stride]);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, Palette::DARK.separator),
        egui::StrokeKind::Inside,
    );
}

/// The vertical hue ramp.
fn paint_hue_strip(ui: &egui::Ui, rect: egui::Rect) {
    let painter = ui.painter_at(rect);
    let steps = rect.height() as usize;
    for i in 0..steps {
        let h = i as f32 / steps.max(1) as f32;
        let (r, g, b) = hsv_to_rgb(h, 1.0, 1.0);
        let y = rect.min.y + i as f32;
        painter.line_segment(
            [egui::pos2(rect.min.x, y), egui::pos2(rect.max.x, y)],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8),
            ),
        );
    }
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, Palette::DARK.separator),
        egui::StrokeKind::Inside,
    );
}
