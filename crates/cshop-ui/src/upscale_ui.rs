//! The Upscale window.
//!
//! Small, because there is only one decision to make: how much bigger. The
//! model knows one factor — four — and anything less is reached by reducing
//! its answer afterwards, which is why 2× comes out sharper than a model
//! trained for 2× would have made it.
//!
//! There is no preview. An enlargement is judged at one pixel to one pixel and
//! there is nowhere in a dialog to show that; what the window can honestly do
//! is say how big the result will be, roughly how long it will take, and then
//! get on with it behind a progress bar.

use crate::commands::Action;
use crate::theme::Palette;

/// Output pixels a second, measured: a 300×400 picture to four times that is
/// under two seconds, and the cost follows the *output* rather than the input.
const OUTPUT_PIXELS_PER_SECOND: f32 = 900_000.0;

pub struct UpscaleDialog {
    pub scale: f32,
    pub from: (u32, u32),
    /// How many raster layers will go through the model.
    pub layers: usize,
    pub status: String,
    pub unavailable: bool,
    pub progress: Option<cshop_core::progress::Progress>,
}

impl UpscaleDialog {
    pub fn new(from: (u32, u32), layers: usize) -> UpscaleDialog {
        let unavailable = !crate::vision::is_available();
        UpscaleDialog {
            scale: 2.0,
            from,
            layers,
            status: if unavailable {
                crate::vision::NOT_INSTALLED.to_string()
            } else {
                String::new()
            },
            unavailable,
            progress: None,
        }
    }

    pub fn title(&self) -> &'static str {
        "Upscale"
    }

    pub fn is_working(&self) -> bool {
        self.progress.is_some()
    }

    pub fn to(&self) -> (u32, u32) {
        (
            ((self.from.0 as f32 * self.scale).round() as u32).max(1),
            ((self.from.1 as f32 * self.scale).round() as u32).max(1),
        )
    }

    fn estimate(&self) -> String {
        let (w, h) = self.to();
        // The model always works at four times and reduces afterwards, so the
        // cost is set by that rather than by what was asked for.
        let worked = (self.from.0 as f32 * 4.0) * (self.from.1 as f32 * 4.0);
        let _ = (w, h);
        let seconds = worked / OUTPUT_PIXELS_PER_SECOND;
        if seconds < 90.0 {
            format!("about {} seconds", seconds.max(1.0).round() as u32)
        } else {
            format!("about {} minutes", (seconds / 60.0).round().max(2.0) as u32)
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let p = Palette::DARK;
        let mut close = false;

        if self.unavailable {
            ui.label(egui::RichText::new(&self.status).color(p.text_dim));
            ui.add_space(8.0);
            if ui.button("Close").clicked() {
                close = true;
            }
            return close;
        }

        let working = self.is_working();
        ui.add_enabled_ui(!working, |ui| {
            ui.horizontal(|ui| {
                ui.label("Scale");
                for s in [1.5f32, 2.0, 3.0, 4.0] {
                    let label = if s.fract() == 0.0 {
                        format!("{s:.0}×")
                    } else {
                        format!("{s:.1}×")
                    };
                    if ui.selectable_label((self.scale - s).abs() < 0.01, label).clicked() {
                        self.scale = s;
                    }
                }
            });
            ui.add(egui::Slider::new(&mut self.scale, 1.1..=4.0).fixed_decimals(2));
        });

        let (w, h) = self.to();
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(format!(
                "{}×{} → {w}×{h}",
                self.from.0, self.from.1
            ))
            .strong(),
        );
        if self.layers > 1 {
            ui.label(
                egui::RichText::new(format!("{} raster layers, each through the model", self.layers))
                    .color(p.text_dim)
                    .small(),
            );
        }

        ui.add_space(8.0);
        if let Some(progress) = &self.progress {
            ui.add(egui::ProgressBar::new(progress.fraction().unwrap_or(0.0)).show_percentage().desired_height(14.0));
            let total = progress.total();
            ui.label(
                egui::RichText::new(if total == 0 {
                    "Starting the model…".to_string()
                } else {
                    format!(
                        "{} of {total} tiles",
                        progress.done()
                    )
                })
                .color(p.text_dim)
                .small(),
            );
        } else {
            ui.label(
                egui::RichText::new(format!("This will take {}.", self.estimate()))
                    .color(p.text_dim)
                    .small(),
            );
        }

        if !self.status.is_empty() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(&self.status).color(p.text_dim));
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!working, egui::Button::new("Upscale")).clicked() {
                actions.push(Action::RunUpscale);
            }
            if ui.add_enabled(!working, egui::Button::new("Cancel")).clicked() {
                close = true;
            }
        });
        close
    }
}
