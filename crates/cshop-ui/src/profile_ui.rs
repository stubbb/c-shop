//! Choosing what a document's colours mean.
//!
//! Two buttons rather than one, because the two things anyone wants to do here
//! are opposites and the difference is the whole subject. See
//! [`cshop_core::profile`] — the window says the same thing in fewer words,
//! since nobody reads documentation from inside a dialog.

use crate::commands::Action;
use crate::theme::Palette;
use cshop_core::profile::{Profile, Space};
use std::path::PathBuf;

/// Where profiles live on a system that has any. Scanned rather than
/// configured: a machine set up for printing already has these, and asking
/// someone to find an `.icc` file by hand is asking them to give up.
const SEARCH: &[&str] = &[
    "/usr/share/color/icc",
    "/usr/local/share/color/icc",
    "/var/lib/color/icc",
];

pub struct ProfileDialog {
    /// What the document is in now.
    pub current: String,
    /// The profiles found on this machine, by name and path.
    pub found: Vec<(String, PathBuf)>,
    /// Which of them is selected. `None` means the built-in sRGB.
    pub chosen: Option<usize>,
    /// A path typed in, for a profile that is not where the others are.
    pub typed: String,
    pub status: String,
}

impl ProfileDialog {
    pub fn new(current: &Profile) -> ProfileDialog {
        ProfileDialog {
            current: format!("{} ({})", current.name(), current.space().name()),
            found: discover(),
            chosen: None,
            typed: String::new(),
            status: String::new(),
        }
    }

    pub fn title(&self) -> &'static str {
        "Colour Profile"
    }

    /// The profile the buttons would act on.
    fn picked(&self) -> Option<PathBuf> {
        if !self.typed.trim().is_empty() {
            return Some(PathBuf::from(self.typed.trim()));
        }
        self.chosen.and_then(|i| self.found.get(i)).map(|(_, p)| p.clone())
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let p = Palette::DARK;
        let mut close = false;

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Working in").color(p.text_dim));
            ui.label(egui::RichText::new(&self.current).strong());
        });
        ui.add_space(6.0);

        ui.label(egui::RichText::new("Change to").color(p.text_dim));
        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
            if ui.selectable_label(self.chosen.is_none(), "sRGB — built in").clicked() {
                self.chosen = None;
                self.typed.clear();
            }
            for (i, (name, _)) in self.found.iter().enumerate() {
                if ui.selectable_label(self.chosen == Some(i), name).clicked() {
                    self.chosen = Some(i);
                    self.typed.clear();
                }
            }
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("or a path").color(p.text_dim));
            ui.add(
                egui::TextEdit::singleline(&mut self.typed)
                    .hint_text("/path/to/profile.icc")
                    .desired_width(240.0),
            );
        });

        if !self.status.is_empty() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(&self.status).color(p.text_dim));
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let to = self.picked();
            if ui
                .add_enabled(true, egui::Button::new("Convert"))
                .on_hover_text(
                    "Rewrite the pixels so the picture looks the same in the new space. \
                     This is what moving a document between spaces means.",
                )
                .clicked()
            {
                actions.push(Action::SetColorProfile { path: to.clone(), convert: true });
                close = true;
            }
            if ui
                .add_enabled(true, egui::Button::new("Assign"))
                .on_hover_text(
                    "Leave the pixels and change what they mean. The picture will look \
                     different. This is the repair for a file that arrived labelled wrongly.",
                )
                .clicked()
            {
                actions.push(Action::SetColorProfile { path: to, convert: false });
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
        close
    }
}

/// Every readable RGB profile on the machine, named by what it calls itself
/// rather than by its filename.
///
/// CMYK and grey profiles are left out on purpose: a document works in RGB, so
/// offering a press profile here would only offer a way to fail. Ink is made
/// on the way out, not on the way in.
fn discover() -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let mut roots: Vec<PathBuf> = SEARCH.iter().map(PathBuf::from).collect();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(&home).join(".local/share/icc"));
        roots.push(PathBuf::from(&home).join(".color/icc"));
    }

    for root in roots {
        walk(&root, 0, &mut out);
    }
    out.sort_by_key(|(name, _)| name.to_lowercase());
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

fn walk(dir: &std::path::Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    // Two levels is enough for the way these directories are laid out, and
    // stops a symlink loop from turning the menu into a hang.
    if depth > 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, depth + 1, out);
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if ext != "icc" && ext != "icm" {
            continue;
        }
        if let Ok(profile) = Profile::load(&path) {
            if profile.space() == Space::Rgb {
                out.push((profile.name().to_string(), path));
            }
        }
    }
}
