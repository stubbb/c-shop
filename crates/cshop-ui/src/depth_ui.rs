//! The window for effects that read how far away things are.
//!
//! Three effects sharing one depth map, because working the depth out is the
//! expensive part and doing it three times would be three waits for the same
//! answer. Fog, focus and parallax are cheap once it is known, so they preview
//! together and go on together.

use crate::commands::Action;
use crate::theme::Palette;
use cshop_core::color::Rgba8;
use cshop_core::depth_fx::{Fog, Focus, Parallax};
use cshop_core::layer::LayerId;
use cshop_core::pixels::PixelBuffer;
use cshop_core::relight::DepthMap;
use std::sync::Arc;

pub struct DepthFxDialog {
    pub layer: LayerId,
    /// The picture as it was, to apply to and to put back on cancel.
    pub before: PixelBuffer,
    /// How far away everything is, once that is known.
    pub depth: Option<Arc<DepthMap>>,
    pub status: String,
    pub unavailable: bool,
    pub busy: bool,
    /// Set once a result has been shown on the canvas.
    pub showing: bool,

    pub fog_on: bool,
    pub fog: Fog,
    pub focus_on: bool,
    pub focus: Focus,
    pub parallax_on: bool,
    pub parallax: Parallax,
}

impl DepthFxDialog {
    pub fn new(layer: LayerId, before: PixelBuffer) -> DepthFxDialog {
        let unavailable = !crate::vision::is_available();
        DepthFxDialog {
            layer,
            before,
            depth: None,
            status: if unavailable {
                crate::vision::NOT_INSTALLED.to_string()
            } else {
                "Working out how far away everything is…".into()
            },
            unavailable,
            busy: !unavailable,
            showing: false,
            fog_on: false,
            fog: Fog::default(),
            focus_on: true,
            focus: Focus::default(),
            parallax_on: false,
            parallax: Parallax::default(),
        }
    }

    pub fn title(&self) -> &'static str {
        "Depth Effects"
    }

    /// The picture with whatever is switched on applied to it.
    ///
    /// In that order deliberately: focus is a property of the lens and applies
    /// to the scene as it was; fog is between the camera and the scene, so it
    /// goes over the blur rather than under it; and parallax moves the whole
    /// result, since moving the camera moves everything it can see.
    pub fn rendered(&self) -> Option<PixelBuffer> {
        let depth = self.depth.as_ref()?;
        if !self.fog_on && !self.focus_on && !self.parallax_on {
            return Some(self.before.clone());
        }
        let mut out = self.before.clone();
        if self.focus_on {
            out = self.focus.apply(&out, depth);
        }
        if self.fog_on {
            out = self.fog.apply(&out, depth);
        }
        if self.parallax_on {
            out = self.parallax.apply(&out, depth);
        }
        Some(out)
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let p = Palette::DARK;
        let mut close = false;
        let mut changed = false;

        if self.unavailable || self.busy || self.depth.is_none() {
            ui.label(egui::RichText::new(&self.status).color(p.text_dim));
            if self.busy {
                ui.add_space(4.0);
                ui.spinner();
            }
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);
            if ui.button("Cancel").clicked() {
                actions.push(Action::CancelDepthFx);
                close = true;
            }
            return close;
        }

        ui.set_min_width(360.0);

        changed |= ui.checkbox(&mut self.focus_on, "Depth of field").changed();
        if self.focus_on {
            ui.indent("focus", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Focus on:");
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut self.focus.at, 0.0..=1.0)
                                .custom_formatter(|v, _| {
                                    if v > 0.66 {
                                        "near".into()
                                    } else if v > 0.33 {
                                        "middle".into()
                                    } else {
                                        "far".into()
                                    }
                                }),
                        )
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Sharp range:");
                    changed |= ui.add(egui::Slider::new(&mut self.focus.range, 0.0..=0.6)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Blur:");
                    changed |= ui
                        .add(egui::Slider::new(&mut self.focus.blur, 0.0..=48.0).suffix(" px"))
                        .changed();
                });
                changed |= ui
                    .checkbox(&mut self.focus.both_ways, "Blur nearer things too")
                    .on_hover_text("Which is what a real lens does; off is what a long one looks like")
                    .changed();
            });
        }

        ui.add_space(6.0);
        changed |= ui.checkbox(&mut self.fog_on, "Haze").changed();
        if self.fog_on {
            ui.indent("fog", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Colour:");
                    let mut c = [
                        self.fog.colour.r as f32 / 255.0,
                        self.fog.colour.g as f32 / 255.0,
                        self.fog.colour.b as f32 / 255.0,
                    ];
                    if ui.color_edit_button_rgb(&mut c).changed() {
                        self.fog.colour = Rgba8::opaque(
                            (c[0] * 255.0) as u8,
                            (c[1] * 255.0) as u8,
                            (c[2] * 255.0) as u8,
                        );
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Density:");
                    changed |= ui.add(egui::Slider::new(&mut self.fog.density, 0.0..=1.0)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Starts at:");
                    changed |= ui.add(egui::Slider::new(&mut self.fog.start, 0.0..=1.0)).changed();
                });
            });
        }

        ui.add_space(6.0);
        changed |= ui.checkbox(&mut self.parallax_on, "Shift the viewpoint").changed();
        if self.parallax_on {
            ui.indent("parallax", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Shift:");
                    changed |= ui
                        .add(egui::Slider::new(&mut self.parallax.shift, -40.0..=40.0).suffix(" px"))
                        .changed();
                });
                changed |= ui.checkbox(&mut self.parallax.vertical, "Vertically").changed();
                ui.label(
                    egui::RichText::new(
                        "Moving the near things reveals what was behind them, and a \
                         photograph does not know. The gap is filled by stretching what \
                         is beside it.",
                    )
                    .color(p.text_dim)
                    .small(),
                );
            });
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Apply").clicked() {
                actions.push(Action::KeepDepthFx);
                close = true;
            }
            if ui.button("Cancel").clicked() {
                actions.push(Action::CancelDepthFx);
                close = true;
            }
        });

        if changed {
            actions.push(Action::PreviewDepthFx);
        }
        close
    }
}
