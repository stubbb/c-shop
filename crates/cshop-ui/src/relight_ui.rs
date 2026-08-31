//! The Relight window.
//!
//! The shape of it follows the cost, as with the other model tools: working
//! out the depth is the slow half and does not change while the lamp moves, so
//! it happens once behind a spinner and everything after it is arithmetic on
//! the picture — fast enough to drag a slider on.
//!
//! The lamp is placed on a pad rather than by two numbers. Azimuth and
//! elevation are perfectly good names and nobody thinks in them; a dot on a
//! circle is the same two numbers in the form the eye already reads, with the
//! middle meaning "straight at it" and the rim meaning "level with it".

use crate::commands::Action;
use crate::theme::Palette;
use cshop_core::color::Rgba8;
use cshop_core::layer::LayerId;
use cshop_core::pixels::PixelBuffer;
use cshop_core::relight::{DepthMap, Relight};
use std::sync::Arc;

/// How big the direction pad is drawn.
const PAD: f32 = 132.0;

pub struct RelightDialog {
    pub layer: LayerId,
    pub lamp: Relight,
    pub status: String,
    pub unavailable: bool,
    /// True while the depth is being worked out.
    pub busy: bool,
    /// The picture as it was, to light and to put back on cancel.
    pub before: PixelBuffer,
    /// How far away everything is, once that is known.
    pub depth: Option<Arc<DepthMap>>,
    /// The same, softened for the softness now chosen. Kept because softening
    /// is a pass over the whole picture and moving the lamp is not: the light
    /// has to stay instant to drag.
    softened: Option<(u32, Arc<DepthMap>)>,
    /// Set once a lighting has been shown on the canvas.
    pub showing: bool,
}

impl RelightDialog {
    pub fn new(layer: LayerId, before: PixelBuffer) -> RelightDialog {
        let unavailable = !crate::vision::is_available();
        RelightDialog {
            layer,
            lamp: Relight::default(),
            status: if unavailable {
                crate::vision::NOT_INSTALLED.to_string()
            } else {
                "Working out the shape…".into()
            },
            unavailable,
            busy: !unavailable,
            before,
            depth: None,
            softened: None,
            showing: false,
        }
    }

    pub fn title(&self) -> &'static str {
        "Relight"
    }

    pub fn ready(&self) -> bool {
        self.depth.is_some() && !self.busy
    }

    /// The picture as this lamp would light it.
    ///
    /// Takes `&mut self` because the softened shape is worked out on demand
    /// and then kept: azimuth, strength and colour cost nothing, and softness
    /// costs one pass over the picture.
    pub fn lit(&mut self) -> Option<PixelBuffer> {
        let depth = self.depth.clone()?;
        let radius = depth.softening_radius(self.lamp.softness);
        let shape = match &self.softened {
            Some((had, map)) if *had == radius => map.clone(),
            _ => {
                let map = Arc::new(depth.smoothed(radius));
                self.softened = Some((radius, map.clone()));
                map
            }
        };
        Some(cshop_core::relight::apply(&self.before, &shape, self.lamp))
    }

    /// The lamp as a point on the pad: the middle is straight on, the rim is
    /// level with the subject.
    fn dot(&self) -> egui::Vec2 {
        let a = self.lamp.azimuth.to_radians();
        let r = (1.0 - self.lamp.elevation.clamp(0.0, 90.0) / 90.0).clamp(0.0, 1.0);
        // 0° is to the left and it goes round clockwise, which on screen — y
        // downward — is this way about.
        egui::vec2(-a.cos() * r, -a.sin() * r)
    }

    fn set_from_dot(&mut self, v: egui::Vec2) {
        let len = v.length().min(1.0);
        if len > 1e-4 {
            self.lamp.azimuth = (-v.y).atan2(-v.x).to_degrees().rem_euclid(360.0);
        }
        self.lamp.elevation = (1.0 - len) * 90.0;
    }

    /// A word for where the lamp is, because a number of degrees is not a
    /// picture of anything.
    fn where_from(&self) -> &'static str {
        let a = self.lamp.azimuth.rem_euclid(360.0);
        match a {
            _ if self.lamp.elevation > 72.0 => "straight on",
            a if a < 22.5 || a >= 337.5 => "from the left",
            a if a < 67.5 => "from the top left",
            a if a < 112.5 => "from above",
            a if a < 157.5 => "from the top right",
            a if a < 202.5 => "from the right",
            a if a < 247.5 => "from the bottom right",
            a if a < 292.5 => "from below",
            _ => "from the bottom left",
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

        if self.busy {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(16.0));
                ui.label(egui::RichText::new(&self.status).color(p.text_dim));
            });
            ui.add_space(8.0);
        }

        let mut moved = false;
        ui.add_enabled_ui(self.ready(), |ui| {
            ui.horizontal(|ui| {
                // --- the pad ---------------------------------------------
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(PAD, PAD), egui::Sense::click_and_drag());
                let centre = rect.center();
                let radius = PAD * 0.5 - 6.0;
                let painter = ui.painter();
                painter.circle_filled(centre, radius, egui::Color32::from_rgb(0x22, 0x22, 0x26));
                painter.circle_stroke(
                    centre,
                    radius,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(0x44, 0x44, 0x4a)),
                );
                // A cross, so the middle is findable without hunting.
                painter.line_segment(
                    [centre - egui::vec2(radius, 0.0), centre + egui::vec2(radius, 0.0)],
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(0x33, 0x33, 0x38)),
                );
                painter.line_segment(
                    [centre - egui::vec2(0.0, radius), centre + egui::vec2(0.0, radius)],
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(0x33, 0x33, 0x38)),
                );
                let dot = centre + self.dot() * radius;
                painter.line_segment(
                    [centre, dot],
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(0x50, 0x80, 0xb0)),
                );
                painter.circle_filled(dot, 5.0, egui::Color32::from_rgb(0x60, 0xc0, 0xff));

                if response.dragged() || response.clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        self.set_from_dot((pos - centre) / radius);
                        moved = true;
                    }
                }

                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(self.where_from()).strong());
                    ui.label(
                        egui::RichText::new(format!(
                            "{:.0}° round, {:.0}° up",
                            self.lamp.azimuth, self.lamp.elevation
                        ))
                        .color(p.text_dim)
                        .small(),
                    );
                    ui.add_space(6.0);
                    let mut colour = [
                        self.lamp.color.r as f32 / 255.0,
                        self.lamp.color.g as f32 / 255.0,
                        self.lamp.color.b as f32 / 255.0,
                    ];
                    if ui.color_edit_button_rgb(&mut colour).changed() {
                        self.lamp.color = Rgba8::opaque(
                            (colour[0] * 255.0) as u8,
                            (colour[1] * 255.0) as u8,
                            (colour[2] * 255.0) as u8,
                        );
                        moved = true;
                    }
                });
            });

            ui.add_space(8.0);
            egui::Grid::new("relight-sliders").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("Strength");
                moved |= ui
                    .add(egui::Slider::new(&mut self.lamp.intensity, 0.0..=2.0).fixed_decimals(2))
                    .drag_stopped();
                ui.end_row();

                ui.label("Ambient");
                moved |= ui
                    .add(egui::Slider::new(&mut self.lamp.ambient, 0.0..=1.5).fixed_decimals(2))
                    .on_hover_text(
                        "What survives where the lamp does not reach. At 1 this only ever \
                         adds light; below it, the unlit side falls away.",
                    )
                    .drag_stopped();
                ui.end_row();

                ui.label("Relief");
                moved |= ui
                    .add(egui::Slider::new(&mut self.lamp.relief, 0.0..=4.0).fixed_decimals(2))
                    .on_hover_text(
                        "How much shape to read into the depth. The depth has no unit, so \
                         this is a choice rather than a measurement.",
                    )
                    .drag_stopped();
                ui.end_row();

                ui.label("Softness");
                moved |= ui
                    .add(
                        egui::Slider::new(&mut self.lamp.softness, 0.0..=0.12)
                            .fixed_decimals(3),
                    )
                    .on_hover_text(
                        "How far to soften the shape before lighting it. The model draws a \
                         cliff at the edge of an object, and lighting a cliff outlines it; \
                         softening turns that outline into shading.",
                    )
                    .drag_stopped();
                ui.end_row();
            });
        });

        if moved && self.ready() {
            actions.push(Action::RelightPreview);
        }

        if !self.status.is_empty() && !self.busy {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(&self.status).color(p.text_dim).small());
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(self.ready(), egui::Button::new("Keep")).clicked() {
                actions.push(Action::RelightKeep);
                close = true;
            }
            if ui.button("Cancel").clicked() {
                actions.push(Action::RelightCancel);
                close = true;
            }
        });
        close
    }
}
