//! The Properties panel: editors for whatever the active layer is.
//!
//! Today that means adjustment layers. Each editor returns `true` when the
//! user changed something, and the caller turns that into a history entry —
//! drags collapse into one, so scrubbing a slider does not fill the History
//! panel with a hundred steps.

use crate::icons::{self, Icon};
use crate::theme::Palette;
use cshop_core::adjust::{Adjustment, GradientStop, LevelsChannel};
use cshop_core::color::Rgba8;
use cshop_core::curve::Curve;
use cshop_core::pixels::PixelBuffer;

/// A 256-bin luminance histogram of the composited image, drawn behind the
/// Levels and Curves editors.
#[derive(Debug, Clone)]
pub struct Histogram {
    pub luma: [u32; 256],
    pub peak: u32,
}

impl Default for Histogram {
    fn default() -> Self {
        // `[u32; 256]` is past the size the standard library derives Default
        // for, so it has to be spelled out.
        Self { luma: [0; 256], peak: 1 }
    }
}

impl Histogram {
    pub fn of(pixels: &PixelBuffer) -> Histogram {
        let mut h = Histogram::default();
        for px in pixels.pixels() {
            if px.a == 0 {
                // Transparent pixels have no colour worth counting.
                continue;
            }
            let luma = (0.30 * px.r as f32 + 0.59 * px.g as f32 + 0.11 * px.b as f32) as usize;
            h.luma[luma.min(255)] += 1;
        }
        // Ignore the extreme bins when scaling: a large flat background at
        // pure black or white would otherwise flatten everything else.
        h.peak = h.luma[1..255].iter().copied().max().unwrap_or(1).max(1);
        h
    }

    fn paint(&self, ui: &egui::Ui, rect: egui::Rect) {
        let painter = ui.painter_at(rect);
        let p = Palette::DARK;
        for (i, &count) in self.luma.iter().enumerate() {
            if count == 0 {
                continue;
            }
            // Square root keeps the small counts visible next to a tall peak.
            let t = ((count as f32 / self.peak as f32).sqrt()).min(1.0);
            let x = rect.min.x + (i as f32 / 255.0) * rect.width();
            let h = t * rect.height();
            painter.line_segment(
                [egui::pos2(x, rect.max.y), egui::pos2(x, rect.max.y - h)],
                egui::Stroke::new(1.0, p.widget_hover),
            );
        }
    }
}

/// Draw the editor for `adjustment`. Returns `true` if it changed.
pub fn adjustment_editor(
    ui: &mut egui::Ui,
    adjustment: &mut Adjustment,
    histogram: Option<&Histogram>,
) -> bool {
    match adjustment {
        Adjustment::BrightnessContrast { brightness, contrast } => {
            let mut changed = slider(ui, "Brightness", brightness, -1.0..=1.0);
            changed |= slider(ui, "Contrast", contrast, -1.0..=1.0);
            changed
        }

        Adjustment::Levels { rgb, channels } => levels_editor(ui, rgb, channels, histogram),
        Adjustment::Curves { curves } => curves_editor(ui, curves, histogram),

        Adjustment::Exposure { exposure, offset, gamma } => {
            let mut changed = slider(ui, "Exposure", exposure, -5.0..=5.0);
            changed |= slider(ui, "Offset", offset, -0.5..=0.5);
            changed |= slider(ui, "Gamma", gamma, 0.1..=4.0);
            changed
        }

        Adjustment::Vibrance { vibrance, saturation } => {
            let mut changed = slider(ui, "Vibrance", vibrance, -1.0..=1.0);
            changed |= slider(ui, "Saturation", saturation, -1.0..=1.0);
            changed
        }

        Adjustment::HueSaturation { hue, saturation, lightness, colorize } => {
            let mut changed = slider(ui, "Hue", hue, -0.5..=0.5);
            changed |= slider(ui, "Saturation", saturation, -1.0..=1.0);
            changed |= slider(ui, "Lightness", lightness, -1.0..=1.0);
            changed |= ui.checkbox(colorize, "Colorize").changed();
            changed
        }

        Adjustment::ColorBalance { shadows, midtones, highlights, preserve_luminosity } => {
            let mut changed = false;
            for (label, band) in
                [("Shadows", shadows), ("Midtones", midtones), ("Highlights", highlights)]
            {
                ui.label(egui::RichText::new(label).strong().small());
                for (i, name) in ["Cyan / Red", "Magenta / Green", "Yellow / Blue"]
                    .into_iter()
                    .enumerate()
                {
                    changed |= slider(ui, name, &mut band[i], -1.0..=1.0);
                }
                ui.add_space(2.0);
            }
            changed |= ui.checkbox(preserve_luminosity, "Preserve Luminosity").changed();
            changed
        }

        Adjustment::BlackAndWhite { weights, tint } => {
            let mut changed = false;
            for (i, name) in
                ["Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas"].into_iter().enumerate()
            {
                changed |= slider(ui, name, &mut weights[i], -1.0..=3.0);
            }
            let mut tinted = tint.is_some();
            if ui.checkbox(&mut tinted, "Tint").changed() {
                *tint = tinted.then(|| Rgba8::opaque(215, 190, 155));
                changed = true;
            }
            if let Some(colour) = tint {
                let mut rgb = [colour.r, colour.g, colour.b];
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    *colour = Rgba8::opaque(rgb[0], rgb[1], rgb[2]);
                    changed = true;
                }
            }
            changed
        }

        Adjustment::ChannelMixer { matrix, monochrome } => {
            let mut changed = ui.checkbox(monochrome, "Monochrome").changed();
            let rows: &[(&str, usize)] = if *monochrome {
                &[("Grey", 0)]
            } else {
                &[("Red", 0), ("Green", 1), ("Blue", 2)]
            };
            for (label, row) in rows {
                ui.label(egui::RichText::new(format!("Output: {label}")).strong().small());
                for (i, name) in ["Red", "Green", "Blue"].into_iter().enumerate() {
                    changed |= slider(ui, name, &mut matrix[*row][i], -2.0..=2.0);
                }
                changed |= slider(ui, "Constant", &mut matrix[*row][3], -1.0..=1.0);
                ui.add_space(2.0);
            }
            changed
        }

        Adjustment::PhotoFilter { color, density, preserve_luminosity } => {
            let mut rgb = [color.r, color.g, color.b];
            let mut changed = ui.color_edit_button_srgb(&mut rgb).changed();
            if changed {
                *color = Rgba8::opaque(rgb[0], rgb[1], rgb[2]);
            }
            changed |= slider(ui, "Density", density, 0.0..=1.0);
            changed |= ui.checkbox(preserve_luminosity, "Preserve Luminosity").changed();
            changed
        }

        Adjustment::Invert => {
            ui.label(
                egui::RichText::new("Invert has no settings.").color(Palette::DARK.text_dim),
            );
            false
        }

        Adjustment::Posterize { levels } => {
            let mut v = *levels as f32;
            if ui.add(egui::Slider::new(&mut v, 2.0..=64.0).text("Levels").integer()).changed() {
                *levels = v as u32;
                true
            } else {
                false
            }
        }

        Adjustment::Threshold { level } => {
            if let Some(h) = histogram {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), 60.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 2.0, Palette::DARK.canvas_backdrop);
                h.paint(ui, rect);
                // Mark where the threshold falls on the histogram.
                let x = rect.min.x + *level * rect.width();
                ui.painter().line_segment(
                    [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)],
                    egui::Stroke::new(1.0, Palette::DARK.accent),
                );
            }
            slider(ui, "Threshold", level, 0.0..=1.0)
        }

        Adjustment::GradientMap { stops } => gradient_editor(ui, stops),
    }
}

fn slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::Slider::new(value, range).show_value(true).max_decimals(2)).changed()
        })
        .inner
    })
    .inner
}

// ---------------------------------------------------------------------------
// Levels
// ---------------------------------------------------------------------------

fn levels_editor(
    ui: &mut egui::Ui,
    rgb: &mut LevelsChannel,
    channels: &mut [LevelsChannel; 3],
    histogram: Option<&Histogram>,
) -> bool {
    let mut changed = false;

    // Which plate the sliders below edit.
    let id = ui.id().with("levels-channel");
    let mut selected: usize = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(0);
    egui::ComboBox::from_id_salt("levels-ch")
        .selected_text(["RGB", "Red", "Green", "Blue"][selected])
        .show_ui(ui, |ui| {
            for (i, name) in ["RGB", "Red", "Green", "Blue"].into_iter().enumerate() {
                ui.selectable_value(&mut selected, i, name);
            }
        });
    ui.ctx().data_mut(|d| d.insert_temp(id, selected));

    let target = if selected == 0 { rgb } else { &mut channels[selected - 1] };

    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 70.0),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, 2.0, Palette::DARK.canvas_backdrop);
    if let Some(h) = histogram {
        h.paint(ui, rect);
    }
    // The three input markers, drawn on the histogram they refer to.
    let painter = ui.painter_at(rect);
    for (v, colour) in [
        (target.input_black, egui::Color32::BLACK),
        (
            target.input_black
                + (target.input_white - target.input_black)
                    * (0.5f32).powf(target.gamma.clamp(0.01, 9.99)),
            egui::Color32::GRAY,
        ),
        (target.input_white, egui::Color32::WHITE),
    ] {
        let x = rect.min.x + v.clamp(0.0, 1.0) * rect.width();
        painter.line_segment(
            [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)],
            egui::Stroke::new(2.0, colour),
        );
    }

    ui.add_space(4.0);
    ui.label(egui::RichText::new("Input Levels").small().strong());
    changed |= slider(ui, "Black", &mut target.input_black, 0.0..=1.0);
    changed |= slider(ui, "Gamma", &mut target.gamma, 0.1..=9.99);
    changed |= slider(ui, "White", &mut target.input_white, 0.0..=1.0);

    ui.add_space(4.0);
    ui.label(egui::RichText::new("Output Levels").small().strong());
    changed |= slider(ui, "Black", &mut target.output_black, 0.0..=1.0);
    changed |= slider(ui, "White", &mut target.output_white, 0.0..=1.0);

    // A crossed-over input range divides by almost zero; keep them ordered.
    if target.input_white <= target.input_black + 0.004 {
        target.input_white = (target.input_black + 0.004).min(1.0);
    }

    ui.add_space(6.0);
    if ui.button("Auto").on_hover_text("Stretch to the histogram").clicked() {
        if let Some(h) = histogram {
            let total: u32 = h.luma.iter().sum();
            // Clip the darkest and brightest 0.1%, which is what Auto Levels
            // does — using the absolute extremes would key off a single stray
            // pixel.
            let cut = (total as f32 * 0.001) as u32;
            let mut acc = 0;
            let mut lo = 0usize;
            for (i, &c) in h.luma.iter().enumerate() {
                acc += c;
                if acc > cut {
                    lo = i;
                    break;
                }
            }
            acc = 0;
            let mut hi = 255usize;
            for (i, &c) in h.luma.iter().enumerate().rev() {
                acc += c;
                if acc > cut {
                    hi = i;
                    break;
                }
            }
            if hi > lo {
                target.input_black = lo as f32 / 255.0;
                target.input_white = hi as f32 / 255.0;
                changed = true;
            }
        }
    }
    changed
}

// ---------------------------------------------------------------------------
// Curves
// ---------------------------------------------------------------------------

fn curves_editor(
    ui: &mut egui::Ui,
    curves: &mut [Curve; 4],
    histogram: Option<&Histogram>,
) -> bool {
    let mut changed = false;
    let p = Palette::DARK;

    let id = ui.id().with("curve-channel");
    let mut selected: usize = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(0);
    egui::ComboBox::from_id_salt("curve-ch")
        .selected_text(["RGB", "Red", "Green", "Blue"][selected])
        .show_ui(ui, |ui| {
            for (i, name) in ["RGB", "Red", "Green", "Blue"].into_iter().enumerate() {
                ui.selectable_value(&mut selected, i, name);
            }
        });
    ui.ctx().data_mut(|d| d.insert_temp(id, selected));

    let size = ui.available_width().min(240.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, p.canvas_backdrop);

    if let Some(h) = histogram {
        h.paint(ui, rect);
    }

    // A four-by-four grid.
    for i in 1..4 {
        let t = i as f32 / 4.0;
        let stroke = egui::Stroke::new(1.0, p.separator);
        painter.line_segment(
            [
                egui::pos2(rect.min.x + t * rect.width(), rect.min.y),
                egui::pos2(rect.min.x + t * rect.width(), rect.max.y),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(rect.min.x, rect.min.y + t * rect.height()),
                egui::pos2(rect.max.x, rect.min.y + t * rect.height()),
            ],
            stroke,
        );
    }
    // The identity diagonal, for reference.
    painter.line_segment(
        [rect.left_bottom(), rect.right_top()],
        egui::Stroke::new(1.0, p.widget),
    );

    let curve = &mut curves[selected];
    // Curve space has y increasing upward; screen space does not.
    let to_screen = |x: f32, y: f32| {
        egui::pos2(rect.min.x + x * rect.width(), rect.max.y - y * rect.height())
    };
    let to_curve = |pos: egui::Pos2| {
        (
            ((pos.x - rect.min.x) / rect.width()).clamp(0.0, 1.0),
            ((rect.max.y - pos.y) / rect.height()).clamp(0.0, 1.0),
        )
    };

    let line: Vec<egui::Pos2> = (0..=64)
        .map(|i| {
            let x = i as f32 / 64.0;
            to_screen(x, curve.eval(x))
        })
        .collect();
    let colour = [p.text, egui::Color32::from_rgb(0xe0, 0x60, 0x60),
        egui::Color32::from_rgb(0x60, 0xd0, 0x70), egui::Color32::from_rgb(0x70, 0x90, 0xe8)]
        [selected];
    painter.add(egui::Shape::line(line, egui::Stroke::new(1.5, colour)));

    // --- interaction -------------------------------------------------------
    let drag_id = ui.id().with("curve-drag");
    let mut dragging: Option<usize> = ui.ctx().data(|d| d.get_temp(drag_id));

    if let Some(pos) = response.interact_pointer_pos() {
        let (cx, cy) = to_curve(pos);
        if response.drag_started() || (response.clicked() && dragging.is_none()) {
            let hit = curve.hit(cx, cy, 12.0 / rect.width());
            if ui.input(|i| i.modifiers.alt) {
                // Alt-click removes a point, matching the gradient editor.
                if let Some(i) = hit {
                    curve.remove(i);
                    changed = true;
                }
            } else {
                let index = match hit {
                    Some(i) => i,
                    None => {
                        changed = true;
                        curve.add(cx, cy)
                    }
                };
                dragging = Some(index);
                ui.ctx().data_mut(|d| d.insert_temp(drag_id, index));
            }
        } else if response.dragged() {
            if let Some(i) = dragging {
                curve.move_point(i, cx, cy);
                changed = true;
            }
        }
    }
    if response.drag_stopped() || ui.input(|i| i.pointer.any_released()) {
        ui.ctx().data_mut(|d| d.remove::<usize>(drag_id));
    }

    for (i, point) in curve.points().iter().enumerate() {
        let pos = to_screen(point.0, point.1);
        let selected_point = dragging == Some(i);
        painter.circle_filled(pos, 4.0, if selected_point { p.accent } else { colour });
        painter.circle_stroke(pos, 4.0, egui::Stroke::new(1.0, egui::Color32::BLACK));
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Click to add · drag to move · Alt-click to remove")
                .color(p.text_dim)
                .small(),
        );
    });
    if ui.button("Reset channel").clicked() {
        *curve = Curve::default();
        changed = true;
    }

    changed
}

// ---------------------------------------------------------------------------
// Gradient map
// ---------------------------------------------------------------------------

fn gradient_editor(ui: &mut egui::Ui, stops: &mut Vec<GradientStop>) -> bool {
    let mut changed = false;
    let p = Palette::DARK;

    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
    // Preview the ramp by sampling it.
    let painter = ui.painter_at(rect);
    let steps = rect.width() as usize;
    let adjustment = Adjustment::GradientMap { stops: stops.clone() };
    let adjustment = adjustment.prepare();
    for i in 0..steps {
        let t = i as f32 / (steps.max(2) - 1) as f32;
        let c = adjustment.apply_rgb([t, t, t]);
        let x = rect.min.x + i as f32;
        painter.line_segment(
            [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgb(
                    (c[0] * 255.0) as u8,
                    (c[1] * 255.0) as u8,
                    (c[2] * 255.0) as u8,
                ),
            ),
        );
    }
    painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, p.separator), egui::StrokeKind::Inside);

    ui.add_space(4.0);
    let mut remove = None;
    for (i, stop) in stops.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let mut rgb = [stop.color.r, stop.color.g, stop.color.b];
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                stop.color = Rgba8::opaque(rgb[0], rgb[1], rgb[2]);
                changed = true;
            }
            changed |= ui
                .add(egui::Slider::new(&mut stop.position, 0.0..=1.0).show_value(true))
                .changed();
            if icons::icon_button(ui, Icon::Trash, 16.0, "Remove stop").clicked() {
                remove = Some(i);
            }
        });
    }
    // A gradient needs at least two stops to interpolate between.
    if let Some(i) = remove {
        if stops.len() > 2 {
            stops.remove(i);
            changed = true;
        }
    }
    if ui.button("Add stop").clicked() {
        stops.push(GradientStop { position: 0.5, color: Rgba8::opaque(128, 128, 128) });
        changed = true;
    }
    changed
}
