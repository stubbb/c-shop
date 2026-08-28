//! The dialog behind Image > Adjustments.
//!
//! Most adjustments are neutral at their defaults — a Curves with the identity
//! curve, Levels with the full range — so the menu cannot simply apply one and
//! be done. It has to ask for settings first, and show what they do.
//!
//! Same shape as the filter dialog: a downscaled proxy of the affected region
//! is re-rendered whenever a setting changes, and only OK touches the real
//! pixels. Adjustments are pointwise, so unlike filters the proxy needs no
//! scaling — a Curves means the same thing at any resolution.

use crate::commands::Action;
use crate::properties::{adjustment_editor, Histogram};
use crate::theme::Palette;
use cshop_core::adjust::Adjustment;
use cshop_core::pixels::PixelBuffer;
use cshop_core::resample::Resampling;

/// Longest edge of the preview proxy, in pixels.
const PROXY_MAX: u32 = 320;

pub struct AdjustmentDialog {
    pub adjustment: Adjustment,
    proxy: PixelBuffer,
    /// Histogram of the region being adjusted, drawn behind Levels and Curves.
    histogram: Histogram,
    preview: Option<egui::TextureHandle>,
    /// The settings the cached preview was rendered with.
    rendered: Option<Adjustment>,
    show_original: bool,
}

impl AdjustmentDialog {
    /// `source` is the affected region of the layer at full resolution.
    pub fn new(adjustment: Adjustment, source: &PixelBuffer) -> Self {
        let longest = source.width().max(source.height()).max(1);
        let scale = (PROXY_MAX as f32 / longest as f32).min(1.0);
        let proxy = if scale < 1.0 {
            cshop_core::resample::resize(
                source,
                ((source.width() as f32 * scale) as u32).max(1),
                ((source.height() as f32 * scale) as u32).max(1),
                Resampling::Bilinear,
            )
        } else {
            source.clone()
        };
        // Take the histogram from the full-resolution source: a downscaled
        // proxy has already averaged away the very peaks a Levels adjustment
        // is usually chasing.
        let histogram = Histogram::of(source);
        Self {
            adjustment,
            proxy,
            histogram,
            preview: None,
            rendered: None,
            show_original: false,
        }
    }

    pub fn title(&self) -> String {
        self.adjustment.name().to_string()
    }

    /// Returns `true` when the dialog should close.
    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let p = Palette::DARK;
        let mut close = false;

        ui.horizontal_top(|ui| {
            // --- preview ----------------------------------------------------
            ui.vertical(|ui| {
                self.refresh_preview(ui.ctx());
                let size = egui::vec2(self.proxy.width() as f32, self.proxy.height() as f32);
                let scale = (300.0 / size.x.max(1.0)).min(1.0);
                let (rect, response) =
                    ui.allocate_exact_size(size * scale, egui::Sense::click_and_drag());
                ui.painter().rect_filled(rect, 2.0, p.canvas_backdrop);

                self.show_original = response.is_pointer_button_down_on();
                let handle = if self.show_original {
                    Some(upload(ui.ctx(), "adjust-original", &self.proxy))
                } else {
                    self.preview.clone()
                };
                if let Some(handle) = handle {
                    ui.painter().image(
                        handle.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                ui.painter().rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(1.0, p.separator),
                    egui::StrokeKind::Inside,
                );
                ui.label(
                    egui::RichText::new(if self.show_original {
                        "Original"
                    } else {
                        "Preview — press and hold to compare"
                    })
                    .color(p.text_dim)
                    .small(),
                );
            });

            ui.add_space(16.0);

            // --- controls ---------------------------------------------------
            ui.vertical(|ui| {
                ui.set_min_width(300.0);
                ui.set_max_width(340.0);
                egui::ScrollArea::vertical()
                    .id_salt("adjust-controls")
                    .max_height(420.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_min_width(290.0);
                        adjustment_editor(ui, &mut self.adjustment, Some(&self.histogram));
                    });
            });
        });

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                actions.push(Action::ApplyAdjustment(Box::new(self.adjustment.clone())));
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
            if ui.button("Reset").clicked() {
                // Back to the neutral settings this adjustment started from.
                if let Some(fresh) = Adjustment::all_defaults()
                    .into_iter()
                    .find(|a| a.name() == self.adjustment.name())
                {
                    self.adjustment = fresh;
                }
            }
        });
        close
    }

    fn refresh_preview(&mut self, ctx: &egui::Context) {
        if self.rendered.as_ref() == Some(&self.adjustment) && self.preview.is_some() {
            return;
        }
        let mut result = self.proxy.clone();
        self.adjustment.prepare().apply_buffer(result.pixels_mut());
        // Update the existing texture rather than allocating a new one: this
        // runs on every drag of a curve point.
        match &mut self.preview {
            Some(handle) => handle.set(to_image(&result), egui::TextureOptions::LINEAR),
            None => self.preview = Some(upload(ctx, "adjust-preview", &result)),
        }
        self.rendered = Some(self.adjustment.clone());
    }
}

fn to_image(image: &PixelBuffer) -> egui::ColorImage {
    let pixels: Vec<egui::Color32> = image
        .pixels()
        .iter()
        .map(|p| egui::Color32::from_rgba_unmultiplied(p.r, p.g, p.b, p.a))
        .collect();
    egui::ColorImage {
        size: [image.width() as usize, image.height() as usize],
        source_size: egui::vec2(image.width() as f32, image.height() as f32),
        pixels,
    }
}

fn upload(ctx: &egui::Context, name: &str, image: &PixelBuffer) -> egui::TextureHandle {
    ctx.load_texture(name, to_image(image), egui::TextureOptions::LINEAR)
}
