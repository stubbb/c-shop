//! The Separate by Content window.
//!
//! The labeller says what every pixel is; this turns that into one layer per
//! kind of thing, which is the form a layered editor can do something with —
//! grade the sky without touching the hillside, clean the foliage and leave
//! the buildings alone.
//!
//! It looks at the picture as soon as it opens, because looking is half a
//! second and there is nothing to decide until the list is there.

use crate::commands::Action;
use crate::theme::Palette;
use crate::vision::Region;

pub struct SeparateDialog {
    pub layer: cshop_core::layer::LayerId,
    /// What the labeller found, most of the picture first.
    pub found: Vec<Region>,
    /// Which of them to make layers from.
    pub chosen: Vec<bool>,
    pub feather: f32,
    pub status: String,
    pub unavailable: bool,
    /// True while the labeller is looking.
    pub busy: bool,
}

impl SeparateDialog {
    pub fn new(layer: cshop_core::layer::LayerId) -> SeparateDialog {
        let unavailable = !crate::vision::is_available();
        SeparateDialog {
            layer,
            found: Vec::new(),
            chosen: Vec::new(),
            feather: 2.0,
            status: if unavailable {
                crate::vision::NOT_INSTALLED.to_string()
            } else {
                "Looking…".into()
            },
            unavailable,
            busy: !unavailable,
        }
    }

    pub fn title(&self) -> &'static str {
        "Separate by Content"
    }

    /// What was found, with anything too small to be worth a layer left
    /// unticked rather than hidden — a two percent sliver is usually a
    /// mislabelling, and occasionally the thing you wanted.
    pub fn show(&mut self, found: Vec<Region>) {
        self.chosen = found.iter().map(|r| r.coverage >= 0.02).collect();
        self.status = if found.is_empty() {
            "Nothing recognised in this picture.".into()
        } else {
            format!("{} kinds of thing here.", found.len())
        };
        self.found = found;
        self.busy = false;
    }

    pub fn picked(&self) -> Vec<Region> {
        self.found
            .iter()
            .zip(&self.chosen)
            .filter(|(_, on)| **on)
            .map(|(r, _)| r.clone())
            .collect()
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
        } else {
            ui.label(egui::RichText::new(&self.status).color(p.text_dim));
        }

        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
            for (i, region) in self.found.iter().enumerate() {
                let on = &mut self.chosen[i];
                ui.horizontal(|ui| {
                    ui.checkbox(on, &region.class);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", region.coverage * 100.0))
                                .color(p.text_dim)
                                .small(),
                        );
                    });
                });
            }
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Feather");
            ui.add(egui::Slider::new(&mut self.feather, 0.0..=12.0).suffix(" px"))
                .on_hover_text(
                    "The model draws a boundary it was never certain about. A soft \
                     edge is the honest way to show that.",
                );
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ready = !self.busy && self.chosen.iter().any(|c| *c);
            if ui.add_enabled(ready, egui::Button::new("Separate")).clicked() {
                actions.push(Action::RunSeparate);
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
        close
    }
}

/// One class's pixels, with everything else made transparent.
///
/// The label map is a hard yes-or-no per pixel; feathering it turns that into
/// a soft edge, which is the honest way to draw a boundary the model was never
/// certain about in the first place.
pub fn separated_layer(
    source: &cshop_core::pixels::PixelBuffer,
    map: &cshop_core::pixels::PixelBuffer,
    id: u8,
    feather: f32,
) -> Option<cshop_core::pixels::PixelBuffer> {
    let (w, h) = (source.width(), source.height());
    let mut coverage = cshop_core::mask::MaskBuffer::hide_all(w, h);
    let mut any = false;
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if map.get(x, y).r == id {
                coverage.set(x, y, 255);
                any = true;
            }
        }
    }
    if !any {
        return None;
    }

    let mut selection = cshop_core::selection::Selection::from_mask(coverage);
    if feather > 0.0 {
        selection.feather(feather);
    }
    let mut out = source.clone();
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let keep = selection.coverage(x, y) as u32;
            let mut px = out.get(x, y);
            px.a = ((px.a as u32 * keep + 127) / 255) as u8;
            out.set(x, y, px);
        }
    }
    Some(out)
}
