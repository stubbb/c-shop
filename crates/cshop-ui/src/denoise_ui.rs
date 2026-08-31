//! The Remove Noise window.
//!
//! # Why there is no live preview
//!
//! The model costs about a second for every hundred thousand pixels, on every
//! core the machine has. There is no viewport small enough to make that feel
//! live, so pretending otherwise would mean a preview that lagged a slider by
//! seconds — worse than none.
//!
//! What there is instead is the shape the cost actually has. The expensive
//! part runs **once**, behind a progress bar, and produces the model's answer
//! at full strength. Strength is then a blend between that answer and the
//! original, which is instant, so the judgement everybody actually wants to
//! make — how much of this do I want — is made after the waiting rather than
//! before it, against the real picture at full size.
//!
//! A selection narrows what is worked on, and is the difference between a few
//! seconds and several minutes on a large photograph. The window says which it
//! is going to be before anyone commits to it.

use crate::commands::Action;
use crate::theme::Palette;
use cshop_core::geom::IRect;
use cshop_core::layer::LayerId;
use cshop_core::pixels::PixelBuffer;

/// Roughly how many pixels a second the model manages.
///
/// Measured rather than guessed: 900x600 through the whole path takes about
/// twenty seconds on this sixteen-core machine, which is where the number
/// comes from. It counts the picture's own pixels, so the cost of the tile
/// overlap is already inside it.
///
/// Only used to warn someone before they wait, so being out by a factor of two
/// on a different machine is survivable and being silent is not.
const PIXELS_PER_SECOND: f32 = 26_000.0;

pub struct DenoiseDialog {
    pub layer: LayerId,
    /// What is being cleaned, in the layer's own frame.
    pub region: IRect,
    /// How much of the model's answer to keep.
    pub strength: f32,
    pub status: String,
    /// True when the pack is not installed, which is a message rather than a
    /// failure.
    pub unavailable: bool,
    /// The region as it was, to blend against and to put back on cancel.
    pub before: PixelBuffer,
    /// The model's answer at full strength, once there is one.
    pub cleaned: Option<PixelBuffer>,
    /// Live while the model runs.
    pub progress: Option<cshop_core::progress::Progress>,
    /// Set once a result has been shown on the canvas, so cancel knows there
    /// is something to undo by hand.
    pub showing: bool,
}

impl DenoiseDialog {
    pub fn new(layer: LayerId, region: IRect, before: PixelBuffer) -> DenoiseDialog {
        let unavailable = !crate::vision::is_available();
        DenoiseDialog {
            layer,
            region,
            strength: 1.0,
            status: if unavailable {
                crate::vision::NOT_INSTALLED.to_string()
            } else {
                String::new()
            },
            unavailable,
            before,
            cleaned: None,
            progress: None,
            showing: false,
        }
    }

    pub fn title(&self) -> &'static str {
        "Remove Noise"
    }

    pub fn is_working(&self) -> bool {
        self.progress.is_some()
    }

    /// The blend of original and answer at the current strength.
    pub fn blended(&self) -> Option<PixelBuffer> {
        let cleaned = self.cleaned.as_ref()?;
        if self.strength >= 1.0 {
            return Some(cleaned.clone());
        }
        let mut out = self.before.clone();
        let s = self.strength.clamp(0.0, 1.0);
        for (dst, src) in out.pixels_mut().iter_mut().zip(cleaned.pixels()) {
            let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * s).round() as u8;
            *dst = cshop_core::color::Rgba8::new(
                mix(dst.r, src.r),
                mix(dst.g, src.g),
                mix(dst.b, src.b),
                // Alpha is coverage, not something a camera measured, so the
                // model never touched it and neither does the blend.
                dst.a,
            );
        }
        Some(out)
    }

    /// What the wait is likely to be, before anyone commits to it.
    fn estimate(&self) -> String {
        let pixels = self.region.width() as f32 * self.region.height() as f32;
        let seconds = pixels / PIXELS_PER_SECOND;
        if seconds < 90.0 {
            format!("about {} seconds", (seconds.max(1.0)).round() as u32)
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

        let whole = format!("{}x{}", self.region.width(), self.region.height());
        ui.label(
            egui::RichText::new(if self.region.x0 == 0 && self.region.y0 == 0 {
                format!("The whole layer, {whole}")
            } else {
                format!("The selection, {whole}")
            })
            .color(p.text_dim),
        );

        ui.add_space(8.0);
        if let Some(progress) = &self.progress {
            let f = progress.fraction().unwrap_or(0.0);
            ui.add(egui::ProgressBar::new(f).show_percentage().desired_height(14.0));
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
        } else if self.cleaned.is_some() {
            // The expensive part is done, so strength is free to move.
            ui.horizontal(|ui| {
                ui.label("Strength");
                let r = ui.add(egui::Slider::new(&mut self.strength, 0.0..=1.0).fixed_decimals(2));
                if r.drag_stopped() || r.lost_focus() {
                    actions.push(Action::DenoiseRestrength);
                }
            });
            ui.label(
                egui::RichText::new(
                    "The model has answered; this mixes its answer back over the original.",
                )
                .color(p.text_dim)
                .small(),
            );
        } else {
            ui.label(
                egui::RichText::new(format!("This will take {}.", self.estimate()))
                    .color(p.text_dim),
            );
            ui.label(
                egui::RichText::new(
                    "Select part of the picture first to clean only that, which is much quicker.",
                )
                .color(p.text_dim)
                .small(),
            );
        }

        if !self.status.is_empty() && !self.unavailable {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(&self.status).color(p.text_dim));
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let working = self.is_working();
            if self.cleaned.is_some() {
                if ui.add_enabled(!working, egui::Button::new("Keep")).clicked() {
                    actions.push(Action::DenoiseKeep);
                    close = true;
                }
            } else if ui
                .add_enabled(!working, egui::Button::new("Remove Noise"))
                .clicked()
            {
                actions.push(Action::RunDenoise);
            }
            if ui.add_enabled(!working, egui::Button::new("Cancel")).clicked() {
                actions.push(Action::DenoiseCancel);
                close = true;
            }
        });
        close
    }
}
