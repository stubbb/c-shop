//! The window that shows every shortcut and lets them be changed.
//!
//! # Why this is worth a window
//!
//! Until now the only place the chords were written down was a document in the
//! repository, which is no use to someone with the program open. A list of
//! them in the program is worth having even if nobody ever rebinds anything —
//! and once the list exists, making a row editable is a small step from
//! showing it.
//!
//! # Capturing a chord
//!
//! Clicking a row puts it in listening mode, and the next key with its
//! modifiers becomes the binding. Modifiers alone are not a chord, so they are
//! ignored until a real key arrives; Escape cancels, which is why Escape
//! cannot itself be bound here.

use crate::commands::Action;
use crate::shortcuts::{bindings_with, Chord};
use crate::theme::Palette;
use std::collections::HashMap;

pub struct ShortcutDialog {
    /// Which command is listening for its new chord.
    listening: Option<String>,
    /// What the search box holds, since the list is long.
    filter: String,
    /// The conflict the last capture would have created, if any.
    clash: Option<(String, String)>,
}

impl Default for ShortcutDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl ShortcutDialog {
    pub fn new() -> ShortcutDialog {
        ShortcutDialog { listening: None, filter: String::new(), clash: None }
    }

    pub fn title(&self) -> &'static str {
        "Keyboard Shortcuts"
    }

    /// Returns `true` when the window should close.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        overrides: &HashMap<String, Chord>,
        actions: &mut Vec<Action>,
    ) -> bool {
        let p = Palette::DARK;
        let mut close = false;
        ui.set_min_width(460.0);

        ui.horizontal(|ui| {
            ui.label("Find:");
            ui.text_edit_singleline(&mut self.filter);
            if !overrides.is_empty()
                && ui
                    .button("Reset all")
                    .on_hover_text("Put every shortcut back to what it was")
                    .clicked()
            {
                actions.push(Action::ResetShortcuts);
                self.clash = None;
            }
        });

        if let Some(waiting) = self.listening.clone() {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("Press the new chord for {waiting} — Escape to stop"))
                    .color(p.accent),
            );
            // A modifier on its own is not a chord, so nothing is captured
            // until a real key arrives with it.
            let captured = ui.input(|i| {
                if i.key_pressed(egui::Key::Escape) {
                    return Some(None);
                }
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some(Some(Chord {
                        ctrl: modifiers.command,
                        shift: modifiers.shift,
                        alt: modifiers.alt,
                        key: *key,
                    })),
                    _ => None,
                })
            });
            match captured {
                Some(None) => {
                    self.listening = None;
                    self.clash = None;
                }
                Some(Some(chord)) => {
                    // Say what it would displace rather than silently taking
                    // it: two commands on one chord means one of them never
                    // runs, and which is not obvious.
                    self.clash = bindings_with(overrides)
                        .iter()
                        .find(|b| b.chord == chord && b.name != waiting)
                        .map(|b| (b.name.to_string(), chord.label()));
                    actions.push(Action::SetShortcut(waiting, Some(chord)));
                    self.listening = None;
                }
                None => {}
            }
        }

        if let Some((taken_by, chord)) = &self.clash {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(format!("{chord} was {taken_by}; it is not any more"))
                    .color(p.text_dim)
                    .small(),
            );
        }

        ui.add_space(6.0);
        ui.separator();
        let filter = self.filter.to_lowercase();
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            egui::Grid::new("shortcut-list").num_columns(3).striped(true).show(ui, |ui| {
                // Every command, including the ones that have been unbound —
                // `bindings_with` drops those, and a command that cannot be
                // seen cannot be given a chord back.
                let live = bindings_with(overrides);
                for default in crate::shortcuts::bindings() {
                    let chord = live
                        .iter()
                        .find(|b| b.name == default.name)
                        .map(|b| b.chord)
                        .unwrap_or(Chord::UNBOUND);
                    let binding = crate::shortcuts::Binding { chord, ..default };
                    if !filter.is_empty()
                        && !binding.name.to_lowercase().contains(&filter)
                        && !binding.chord.label().to_lowercase().contains(&filter)
                    {
                        continue;
                    }
                    let changed = overrides.contains_key(binding.name);
                    ui.label(binding.name);
                    let chord = egui::RichText::new(binding.chord.label()).monospace();
                    let chord = if changed { chord.color(p.accent) } else { chord };
                    if ui
                        .add(egui::Button::new(chord).min_size(egui::vec2(120.0, 0.0)))
                        .on_hover_text("Click, then press the chord you want")
                        .clicked()
                    {
                        self.listening = Some(binding.name.to_string());
                        self.clash = None;
                    }
                    if changed {
                        if ui.small_button("↺").on_hover_text("Back to the default").clicked() {
                            actions.push(Action::SetShortcut(binding.name.to_string(), None));
                        }
                    } else {
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Done").clicked() {
                close = true;
            }
            ui.label(
                egui::RichText::new(
                    "The tool letters, the arrows and the brackets are handled where the \
                     state they depend on lives, so they are not in this list.",
                )
                .color(p.text_dim)
                .small(),
            );
        });
        close
    }
}
