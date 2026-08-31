//! Rebinding shortcuts.

use cshop_core::document::{Background, Document};
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::shortcuts::{bindings, bindings_with, Chord};
use cshop_ui::CShopApp;

fn app() -> Option<CShopApp> {
    let gpu = GpuContext::headless().ok()?;
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("t", 16, 16, Background::Transparent));
    Some(app)
}

fn chord_of(app: &CShopApp, name: &str) -> Option<Chord> {
    bindings_with(&app.shortcut_overrides)
        .iter()
        .find(|b| b.name == name)
        .map(|b| b.chord)
}

#[test]
fn a_rebinding_takes_effect_and_the_default_stops_working() {
    let Some(mut app) = app() else { return };
    let was = chord_of(&app, "Undo").expect("Undo is bound");
    let want = Chord::ctrl(egui::Key::G);

    app.dispatch(Action::SetShortcut("Undo".into(), Some(want)));
    assert_eq!(chord_of(&app, "Undo"), Some(want));
    assert_ne!(was, want, "and it is not what it was");
}

/// One chord, one command: two on the same chord means one of them silently
/// never runs, and which is not obvious.
#[test]
fn taking_a_chord_takes_it_from_whoever_had_it() {
    let Some(mut app) = app() else { return };
    let redo = chord_of(&app, "Redo").expect("Redo is bound");
    app.dispatch(Action::SetShortcut("Undo".into(), Some(redo)));

    assert_eq!(chord_of(&app, "Undo"), Some(redo));
    assert_eq!(chord_of(&app, "Redo"), None, "Redo lost it rather than sharing it");
    // And nothing is listening on that chord twice.
    let live = bindings_with(&app.shortcut_overrides);
    let on_that_chord = live.iter().filter(|b| b.chord == redo).count();
    assert_eq!(on_that_chord, 1);
}

#[test]
fn a_rebinding_can_be_put_back() {
    let Some(mut app) = app() else { return };
    let was = chord_of(&app, "Save").unwrap();
    app.dispatch(Action::SetShortcut("Save".into(), Some(Chord::ctrl(egui::Key::J))));
    assert_ne!(chord_of(&app, "Save"), Some(was));

    app.dispatch(Action::SetShortcut("Save".into(), None));
    assert_eq!(chord_of(&app, "Save"), Some(was));
}

#[test]
fn resetting_puts_every_one_back() {
    let Some(mut app) = app() else { return };
    app.dispatch(Action::SetShortcut("Undo".into(), Some(Chord::ctrl(egui::Key::G))));
    app.dispatch(Action::SetShortcut("Save".into(), Some(Chord::ctrl(egui::Key::J))));
    // At least the two that were set — and possibly more, since taking a
    // chord unbinds whoever held it, and Ctrl+J was Layer Via Copy.
    assert!(app.shortcut_overrides.len() >= 2);

    app.dispatch(Action::ResetShortcuts);
    assert!(app.shortcut_overrides.is_empty());
    let defaults: Vec<(&str, Chord)> = bindings().iter().map(|b| (b.name, b.chord)).collect();
    let now: Vec<(&str, Chord)> =
        bindings_with(&app.shortcut_overrides).iter().map(|b| (b.name, b.chord)).collect();
    assert_eq!(defaults, now);
}

/// Only the changed ones are written down, so a later build's new defaults
/// reach everyone who has not overridden them.
#[test]
fn only_the_changed_ones_are_remembered_and_they_survive() {
    let Some(mut app) = app() else { return };
    let want = Chord::ctrl_shift(egui::Key::G);
    app.dispatch(Action::SetShortcut("Undo".into(), Some(want)));

    let settings = app.current_settings();
    assert_eq!(settings.shortcuts.len(), 1, "only the one that changed");
    assert_eq!(settings.shortcuts[0].0, "Undo");

    // Through the settings file and back.
    let json = cshop_core::json::parse(&settings.to_json().write()).unwrap();
    let back = cshop_ui::settings::Settings::from_json(&json);
    assert_eq!(back.shortcuts, settings.shortcuts);
    let parsed = Chord::parse(&back.shortcuts[0].1).expect("it should read back");
    assert_eq!(parsed, want);
}

#[test]
fn a_chord_reads_back_as_the_chord_it_was_written_as() {
    for chord in [
        Chord::ctrl(egui::Key::S),
        Chord::ctrl_shift(egui::Key::S),
        Chord::ctrl_alt(egui::Key::I),
        Chord::plain(egui::Key::Tab),
        Chord::alt(egui::Key::Backspace),
        Chord::shift(egui::Key::F6),
    ] {
        let text = chord.label();
        assert_eq!(Chord::parse(&text), Some(chord), "{text}");
    }
}

#[test]
fn nonsense_in_the_settings_file_is_ignored_rather_than_fatal() {
    assert_eq!(Chord::parse(""), None);
    assert_eq!(Chord::parse("Ctrl+"), None);
    assert_eq!(Chord::parse("Ctrl+Nonsense"), None);
}
