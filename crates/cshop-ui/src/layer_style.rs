//! The Layer Style dialog.
//!
//! A list of effects down the left with a checkbox each, and the selected
//! one's controls on the right. Changes apply to the document as they are
//! made, so the canvas is the preview; Cancel puts back what was there before.

use crate::commands::Action;
use crate::theme::Palette;
use cshop_core::color::Rgba8;
use cshop_core::effects::*;
use cshop_core::layer::LayerId;

/// Which effect's controls are showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    DropShadow,
    OuterGlow,
    Bevel,
    InnerShadow,
    InnerGlow,
    Satin,
    ColorOverlay,
    Stroke,
}

/// Listed the way they stack, topmost effect first.
const PAGES: &[(Page, &str)] = &[
    (Page::Stroke, "Stroke"),
    (Page::ColorOverlay, "Color Overlay"),
    (Page::Satin, "Satin"),
    (Page::InnerGlow, "Inner Glow"),
    (Page::InnerShadow, "Inner Shadow"),
    (Page::Bevel, "Bevel & Emboss"),
    (Page::OuterGlow, "Outer Glow"),
    (Page::DropShadow, "Drop Shadow"),
];

pub struct LayerStyleDialog {
    pub layer: LayerId,
    pub effects: LayerEffects,
    /// What the layer had when the dialog opened, for Cancel.
    before: LayerEffects,
    name: String,
    page: Page,
    /// Set once the first change has been pushed, so Cancel knows whether
    /// there is anything to put back.
    touched: bool,
}

impl LayerStyleDialog {
    pub fn new(layer: LayerId, effects: LayerEffects, name: String) -> Self {
        // Open on whatever is already on, so an existing style is not hidden
        // behind a page nobody thought to click.
        let page = PAGES
            .iter()
            .find(|(p, _)| is_on(&effects, *p))
            .map(|(p, _)| *p)
            .unwrap_or(Page::DropShadow);
        Self { layer, effects, before: effects, name, page, touched: false }
    }

    pub fn title(&self) -> String {
        format!("Layer Style — {}", self.name)
    }

    /// Returns `true` when the dialog should close.
    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let p = Palette::DARK;
        let mut close = false;
        let before = self.effects;

        ui.horizontal_top(|ui| {
            // --- the list ---------------------------------------------------
            ui.vertical(|ui| {
                ui.set_min_width(170.0);
                ui.set_max_width(170.0);
                ui.checkbox(&mut self.effects.enabled, "Effects on");
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(2.0);
                for (page, label) in PAGES {
                    ui.horizontal(|ui| {
                        let mut on = is_on(&self.effects, *page);
                        if ui.checkbox(&mut on, "").changed() {
                            set_on(&mut self.effects, *page, on);
                            // Ticking one is also a request to look at it.
                            self.page = *page;
                        }
                        if ui.selectable_label(self.page == *page, *label).clicked() {
                            self.page = *page;
                        }
                    });
                }
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Global light")
                        .color(p.text_dim)
                        .small(),
                );
                ui.horizontal(|ui| {
                    ui.label("Angle:");
                    angle_drag(ui, &mut self.effects.global_light_angle);
                });
                ui.horizontal(|ui| {
                    ui.label("Altitude:");
                    ui.add(
                        egui::DragValue::new(&mut self.effects.global_light_altitude)
                            .range(0.0..=90.0)
                            .speed(0.5)
                            .suffix("°"),
                    );
                });
            });

            // Plain spacing, not a separator: a vertical rule inside a
            // horizontal layout stretches to the tallest column, and here that
            // made the whole dialog degenerate.
            ui.add_space(16.0);

            // --- the selected effect's controls -----------------------------
            ui.vertical(|ui| {
                ui.set_min_width(340.0);
                ui.set_max_width(360.0);
                egui::ScrollArea::vertical()
                    .id_salt("layer-style-controls")
                    .max_height(420.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_min_width(330.0);
                        self.page_ui(ui);
                    });
            });
        });

        // Apply as they go, so the canvas is the preview.
        if self.effects != before {
            self.touched = true;
            actions.push(Action::SetLayerEffects(self.layer, Box::new(self.effects)));
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                close = true;
            }
            if ui.button("Cancel").clicked() {
                if self.touched {
                    actions.push(Action::SetLayerEffects(self.layer, Box::new(self.before)));
                }
                close = true;
            }
            if ui.button("Clear All").clicked() {
                self.effects = LayerEffects {
                    enabled: true,
                    global_light_angle: self.effects.global_light_angle,
                    global_light_altitude: self.effects.global_light_altitude,
                    ..Default::default()
                };
                self.touched = true;
                actions.push(Action::SetLayerEffects(self.layer, Box::new(self.effects)));
            }
        });
        close
    }

    fn page_ui(&mut self, ui: &mut egui::Ui) {
        let p = Palette::DARK;
        let mut on = is_on(&self.effects, self.page);
        let label = PAGES.iter().find(|(x, _)| *x == self.page).map(|(_, l)| *l).unwrap_or("");
        ui.horizontal(|ui| {
            if ui.checkbox(&mut on, "").changed() {
                set_on(&mut self.effects, self.page, on);
            }
            ui.heading(label);
        });
        if !on {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Switch this effect on to change it.")
                    .color(p.text_dim)
                    .small(),
            );
            return;
        }
        ui.add_space(6.0);

        let global = (self.effects.global_light_angle, self.effects.global_light_altitude);
        match self.page {
            Page::DropShadow => {
                if let Some(s) = self.effects.drop_shadow.as_mut() {
                    shadow_ui(ui, s, global.0, "Spread");
                }
            }
            Page::InnerShadow => {
                if let Some(s) = self.effects.inner_shadow.as_mut() {
                    shadow_ui(ui, s, global.0, "Choke");
                }
            }
            Page::OuterGlow => {
                if let Some(g) = self.effects.outer_glow.as_mut() {
                    glow_ui(ui, g, false);
                }
            }
            Page::InnerGlow => {
                if let Some(g) = self.effects.inner_glow.as_mut() {
                    glow_ui(ui, g, true);
                }
            }
            Page::Bevel => {
                if let Some(b) = self.effects.bevel.as_mut() {
                    bevel_ui(ui, b);
                }
            }
            Page::Satin => {
                if let Some(s) = self.effects.satin.as_mut() {
                    satin_ui(ui, s);
                }
            }
            Page::ColorOverlay => {
                if let Some(o) = self.effects.color_overlay.as_mut() {
                    blend_row(ui, &mut o.mode, &mut o.opacity, "overlay");
                    color_row(ui, "Color:", &mut o.color);
                }
            }
            Page::Stroke => {
                if let Some(s) = self.effects.stroke.as_mut() {
                    row(ui, "Size:", |ui| {
                        ui.add(px_drag(&mut s.size, 0.1..=250.0));
                    });
                    row(ui, "Position:", |ui| {
                        for pos in
                            [StrokePosition::Outside, StrokePosition::Center, StrokePosition::Inside]
                        {
                            if ui.selectable_label(s.position == pos, pos.name()).clicked() {
                                s.position = pos;
                            }
                        }
                    });
                    blend_row(ui, &mut s.mode, &mut s.opacity, "stroke");
                    color_row(ui, "Color:", &mut s.color);
                }
            }
        }
    }
}

fn is_on(fx: &LayerEffects, page: Page) -> bool {
    match page {
        Page::DropShadow => fx.drop_shadow.is_some(),
        Page::OuterGlow => fx.outer_glow.is_some(),
        Page::Bevel => fx.bevel.is_some(),
        Page::InnerShadow => fx.inner_shadow.is_some(),
        Page::InnerGlow => fx.inner_glow.is_some(),
        Page::Satin => fx.satin.is_some(),
        Page::ColorOverlay => fx.color_overlay.is_some(),
        Page::Stroke => fx.stroke.is_some(),
    }
}

/// Switching an effect off keeps nothing; switching it on starts from the
/// default, which is a visible effect rather than a no-op.
fn set_on(fx: &mut LayerEffects, page: Page, on: bool) {
    match page {
        Page::DropShadow => fx.drop_shadow = on.then(Shadow::default),
        Page::OuterGlow => fx.outer_glow = on.then(Glow::default),
        Page::Bevel => fx.bevel = on.then(Bevel::default),
        Page::InnerShadow => {
            fx.inner_shadow = on.then(|| Shadow { distance: 5.0, size: 5.0, ..Default::default() })
        }
        Page::InnerGlow => {
            fx.inner_glow = on.then(|| Glow { mode: cshop_core::blend::BlendMode::Screen, ..Default::default() })
        }
        Page::Satin => fx.satin = on.then(Satin::default),
        Page::ColorOverlay => fx.color_overlay = on.then(ColorOverlay::default),
        Page::Stroke => fx.stroke = on.then(Stroke::default),
    }
}

fn row(ui: &mut egui::Ui, label: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_width(320.0);
        ui.label(label);
        body(ui);
    });
}

fn px_drag(v: &mut f32, range: std::ops::RangeInclusive<f32>) -> egui::DragValue<'_> {
    egui::DragValue::new(v).range(range).speed(0.2).suffix(" px")
}

fn percent(v: &mut f32) -> egui::DragValue<'_> {
    egui::DragValue::new(v)
        .range(0.0..=1.0)
        .speed(0.004)
        .custom_formatter(|x, _| format!("{:.0}%", x * 100.0))
}

fn angle_drag(ui: &mut egui::Ui, v: &mut f32) {
    ui.add(egui::DragValue::new(v).range(-180.0..=360.0).speed(0.5).suffix("°"));
}

fn color_row(ui: &mut egui::Ui, label: &str, c: &mut Rgba8) {
    row(ui, label, |ui| {
        let mut rgb = [c.r, c.g, c.b];
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            *c = Rgba8::new(rgb[0], rgb[1], rgb[2], c.a);
        }
    });
}

fn blend_row(
    ui: &mut egui::Ui,
    mode: &mut cshop_core::blend::BlendMode,
    opacity: &mut f32,
    salt: &str,
) {
    row(ui, "Blend:", |ui| {
        crate::chrome::blend_combo(ui, salt, mode);
        ui.add_space(6.0);
        ui.label("Opacity:");
        ui.add(percent(opacity));
    });
}

fn shadow_ui(ui: &mut egui::Ui, s: &mut Shadow, global_angle: f32, spread_label: &str) {
    blend_row(ui, &mut s.mode, &mut s.opacity, spread_label);
    color_row(ui, "Color:", &mut s.color);
    row(ui, "Angle:", |ui| {
        let mut shown = if s.use_global_light { global_angle } else { s.angle };
        if ui
            .add(egui::DragValue::new(&mut shown).range(-180.0..=360.0).speed(0.5).suffix("°"))
            .changed()
        {
            s.angle = shown;
            s.use_global_light = false;
        }
        ui.checkbox(&mut s.use_global_light, "Use global light");
    });
    row(ui, "Distance:", |ui| {
        ui.add(px_drag(&mut s.distance, 0.0..=250.0));
    });
    row(ui, &format!("{spread_label}:"), |ui| {
        ui.add(percent(&mut s.spread));
    });
    row(ui, "Size:", |ui| {
        ui.add(px_drag(&mut s.size, 0.0..=250.0));
    });
}

fn glow_ui(ui: &mut egui::Ui, g: &mut Glow, inner: bool) {
    blend_row(ui, &mut g.mode, &mut g.opacity, if inner { "inner-glow" } else { "outer-glow" });
    color_row(ui, "Color:", &mut g.color);
    if inner {
        row(ui, "Source:", |ui| {
            for (src, label) in [(GlowSource::Edge, "Edge"), (GlowSource::Center, "Center")] {
                if ui.selectable_label(g.source == src, label).clicked() {
                    g.source = src;
                }
            }
        });
    }
    row(ui, if inner { "Choke:" } else { "Spread:" }, |ui| {
        ui.add(percent(&mut g.spread));
    });
    row(ui, "Size:", |ui| {
        ui.add(px_drag(&mut g.size, 0.0..=250.0));
    });
}

fn bevel_ui(ui: &mut egui::Ui, b: &mut Bevel) {
    row(ui, "Style:", |ui| {
        egui::ComboBox::from_id_salt("bevel-style")
            .selected_text(b.style.name())
            .width(150.0)
            .show_ui(ui, |ui| {
                for style in
                    [BevelStyle::Inner, BevelStyle::Outer, BevelStyle::Emboss, BevelStyle::Pillow]
                {
                    ui.selectable_value(&mut b.style, style, style.name());
                }
            });
    });
    row(ui, "Depth:", |ui| {
        ui.add(egui::DragValue::new(&mut b.depth).range(0.0..=8.0).speed(0.02));
        ui.checkbox(&mut b.down, "Down");
    });
    row(ui, "Size:", |ui| {
        ui.add(px_drag(&mut b.size, 0.5..=250.0));
    });
    row(ui, "Soften:", |ui| {
        ui.add(px_drag(&mut b.soften, 0.0..=40.0));
    });
    row(ui, "Angle:", |ui| {
        angle_drag(ui, &mut b.angle);
        ui.checkbox(&mut b.use_global_light, "Global");
    });
    row(ui, "Altitude:", |ui| {
        ui.add(egui::DragValue::new(&mut b.altitude).range(0.0..=90.0).speed(0.5).suffix("°"));
    });
    ui.separator();
    row(ui, "Highlight:", |ui| {
        crate::chrome::blend_combo(ui, "bevel-hi", &mut b.highlight_mode);
        ui.add(percent(&mut b.highlight_opacity));
    });
    color_row(ui, "  Colour:", &mut b.highlight);
    row(ui, "Shadow:", |ui| {
        crate::chrome::blend_combo(ui, "bevel-lo", &mut b.shadow_mode);
        ui.add(percent(&mut b.shadow_opacity));
    });
    color_row(ui, "  Colour:", &mut b.shadow);
}

fn satin_ui(ui: &mut egui::Ui, s: &mut Satin) {
    blend_row(ui, &mut s.mode, &mut s.opacity, "satin");
    color_row(ui, "Color:", &mut s.color);
    row(ui, "Angle:", |ui| {
        angle_drag(ui, &mut s.angle);
    });
    row(ui, "Distance:", |ui| {
        ui.add(px_drag(&mut s.distance, 0.0..=250.0));
    });
    row(ui, "Size:", |ui| {
        ui.add(px_drag(&mut s.size, 0.0..=250.0));
    });
    row(ui, "", |ui| {
        ui.checkbox(&mut s.invert, "Invert");
    });
}
