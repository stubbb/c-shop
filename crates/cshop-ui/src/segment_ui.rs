//! The Segment Object window.
//!
//! Click the thing you want; the model returns a mask; the mask becomes the
//! selection. Clicking again refines it, and Alt-clicking says "not this" —
//! which is how a subject sitting on something else gets separated from it,
//! since a single click cannot know where you meant to stop.
//!
//! The window is deliberately not modal. The canvas *is* the control here, so
//! a dimmed sheet over the middle of it would be in the way of the only thing
//! worth looking at.

use cshop_core::geom::Vec2;

use crate::commands::Action;
use crate::theme::Palette;
use crate::vision::Found;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hint {
    pub at: Vec2,
    /// False for a point that says "not this".
    pub include: bool,
}

#[derive(Debug, Default)]
pub struct SegmentDialog {
    pub hints: Vec<Hint>,
    /// What the detector found, once it has been asked.
    pub found: Vec<Found>,
    /// Which of those is being segmented, if the choice came from the list.
    pub chosen: Option<usize>,
    pub feather: f32,
    /// Grow the mask outward by this many pixels before softening it.
    pub expand: u32,
    /// The last thing that happened, in words.
    pub status: String,
    /// Whether anything has been applied to the document yet, so Cancel knows
    /// whether there is something to undo.
    pub applied: bool,
    pub coverage: Option<f32>,
    /// Whether a model is running right now, so the window can say so and the
    /// controls that would start another are held.
    pub busy: bool,
    /// Set when the model is not installed, so the window says so once rather
    /// than failing on every click.
    pub unavailable: bool,
}

impl SegmentDialog {
    pub fn new() -> SegmentDialog {
        let available = crate::vision::is_available();
        SegmentDialog {
            feather: 0.0,
            unavailable: !available,
            status: if available {
                "Click the object on the canvas. Alt-click to exclude part of it.".into()
            } else {
                crate::vision::NOT_INSTALLED.into()
            },
            ..Default::default()
        }
    }

    pub fn title(&self) -> &'static str {
        "Segment Object"
    }

    /// Record a click on the canvas and ask for a new mask.
    pub fn add_hint(&mut self, at: Vec2, include: bool) {
        self.chosen = None;
        self.hints.push(Hint { at, include });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let p = Palette::DARK;
        let mut close = false;

        if self.unavailable {
            ui.label(egui::RichText::new(&self.status).color(p.accent));
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "The models are an optional pack: run vision/setup.sh once, then \
                     reopen this window.",
                )
                .color(p.text_dim),
            );
            ui.add_space(10.0);
            if ui.button("Close").clicked() {
                close = true;
            }
            return close;
        }

        // While a model is running, the spinner and the message are the whole
        // point of the window: the work takes about a second, which is long
        // enough that silence reads as a hang.
        ui.horizontal(|ui| {
            if self.busy {
                ui.add(egui::Spinner::new().size(14.0));
                ui.add_space(2.0);
            }
            ui.label(
                egui::RichText::new(&self.status)
                    .color(if self.busy { p.accent } else { p.text_dim }),
            );
        });
        ui.add_space(8.0);

        // --- what the detector can offer -----------------------------------
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.busy, egui::Button::new("Find objects")).clicked() {
                actions.push(Action::SegmentDetect);
            }
            if !self.hints.is_empty()
                && ui.add_enabled(!self.busy, egui::Button::new("Clear points")).clicked()
            {
                self.hints.clear();
                actions.push(Action::SegmentPreview);
            }
        });

        if !self.found.is_empty() {
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                for (i, f) in self.found.iter().enumerate() {
                    let label = format!("{}  {:.0}%", f.class, f.score * 100.0);
                    if ui.selectable_label(self.chosen == Some(i), label).clicked() {
                        self.chosen = Some(i);
                        self.hints.clear();
                        actions.push(Action::SegmentPreview);
                    }
                }
            });
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // --- the edge -------------------------------------------------------
        //
        // Both are applied to the mask after the model has spoken, so moving
        // either costs nothing but the redraw and needs no second look at the
        // picture. Expand runs first: growing a softened edge would harden it
        // again, which is not what either control is for.
        let before = (self.feather, self.expand);
        // A grid rather than two rows, so the two sliders start at the same
        // place however long their labels are.
        egui::Grid::new("segment-edge").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("Expand");
            ui.add(egui::Slider::new(&mut self.expand, 0..=50).suffix(" px")).on_hover_text(
                "Grow the selection outward. Useful when the edge has cut inside \
                 the subject, or to leave room for a stroke.",
            );
            ui.end_row();
            ui.label("Feather");
            ui.add(egui::Slider::new(&mut self.feather, 0.0..=40.0).suffix(" px"))
                .on_hover_text("Soften the edge of the selection.");
            ui.end_row();
        });
        if (self.feather - before.0).abs() > f32::EPSILON || self.expand != before.1 {
            actions.push(Action::SegmentPreview);
        }

        if let Some(c) = self.coverage {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("covers {:.1}% of the image", c * 100.0))
                    .color(p.text_dim),
            );
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let ready = self.applied && !self.busy;
            if ui.add_enabled(ready, egui::Button::new("Keep selection")).clicked() {
                close = true;
            }
            if ui.button("Cancel").clicked() {
                if self.applied {
                    actions.push(Action::SegmentCancel);
                }
                close = true;
            }
        });
        close
    }

    /// The prompt these hints make, if any.
    pub fn prompt(&self) -> Option<crate::vision::Prompt> {
        if let Some(i) = self.chosen {
            let f = self.found.get(i)?;
            return Some(crate::vision::Prompt::Box(f.box_));
        }
        if self.hints.is_empty() {
            return None;
        }
        let yes: Vec<(f32, f32)> =
            self.hints.iter().filter(|h| h.include).map(|h| (h.at.x, h.at.y)).collect();
        let no: Vec<(f32, f32)> =
            self.hints.iter().filter(|h| !h.include).map(|h| (h.at.x, h.at.y)).collect();
        if yes.is_empty() {
            // Only exclusions is not a prompt; the model has nothing to start
            // from and would return the whole picture or none of it.
            return None;
        }
        Some(crate::vision::Prompt::Points(yes, no))
    }
}
