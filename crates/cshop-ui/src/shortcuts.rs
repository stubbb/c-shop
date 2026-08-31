//! Keyboard shortcuts, as one table.
//!
//! The chords live here rather than inline in the input handler because two
//! other things need to agree with them: the menus, which print the chord
//! beside each command, and the tests, which check that nothing is advertised
//! without being bound. A menu that promised Ctrl+E for Merge Down while
//! nothing listened for it is the bug this arrangement is meant to prevent.
//!
//! The chords follow the bindings this class of editor has used for decades,
//! because that is what hands already know. Where a conventional shortcut has
//! no counterpart here — Merge Visible, Hide Extras — it is left unbound
//! rather than pointed at something that merely resembles it.

use crate::commands::Action;
use egui::Key;

/// A key plus the modifiers that must be held with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

impl Chord {
    /// What a command gets when its chord is taken by another.
    ///
    /// One chord means one command — two on the same chord means one of them
    /// silently never runs — so rebinding displaces whatever held it. The
    /// displaced command has to be recorded as *having no chord* rather than
    /// simply left at its default, or the next run would give it back.
    pub const UNBOUND: Chord =
        Chord { ctrl: true, shift: true, alt: true, key: Key::F35 };

    pub fn is_bound(&self) -> bool {
        !(self.ctrl && self.shift && self.alt && self.key == Key::F35)
    }

    pub const fn plain(key: Key) -> Chord {
        Chord { ctrl: false, shift: false, alt: false, key }
    }
    pub const fn ctrl(key: Key) -> Chord {
        Chord { ctrl: true, shift: false, alt: false, key }
    }
    pub const fn ctrl_shift(key: Key) -> Chord {
        Chord { ctrl: true, shift: true, alt: false, key }
    }
    pub const fn ctrl_alt(key: Key) -> Chord {
        Chord { ctrl: true, shift: false, alt: true, key }
    }
    pub const fn shift(key: Key) -> Chord {
        Chord { ctrl: false, shift: true, alt: false, key }
    }
    pub const fn alt(key: Key) -> Chord {
        Chord { ctrl: false, shift: false, alt: true, key }
    }

    /// True only when exactly these modifiers are down.
    ///
    /// The exactness matters: without it Ctrl+Alt+I would fire Invert on its
    /// way to Image Size, and Ctrl+Shift+S would also save.
    pub fn pressed(&self, i: &egui::InputState) -> bool {
        let m = i.modifiers;
        m.command == self.ctrl && m.shift == self.shift && m.alt == self.alt && i.key_pressed(self.key)
    }

    /// Read a chord back from how it is written, so a rebinding survives the
    /// settings file. Unknown key names are refused rather than guessed at.
    pub fn parse(text: &str) -> Option<Chord> {
        let mut chord = Chord { ctrl: false, shift: false, alt: false, key: Key::A };
        let mut key = None;
        for part in text.split('+') {
            let part = part.trim();
            match part.to_ascii_lowercase().as_str() {
                "" => continue,
                "ctrl" | "cmd" | "command" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" | "option" => chord.alt = true,
                _ => key = Key::from_name(part),
            }
        }
        chord.key = key?;
        Some(chord)
    }

    /// How the chord is written in a menu, e.g. `Ctrl+Shift+S`.
    pub fn label(&self) -> String {
        if !self.is_bound() {
            return "—".into();
        }
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        s.push_str(match self.key {
            Key::Plus | Key::Equals => "+",
            Key::Minus => "-",
            Key::OpenBracket => "[",
            Key::CloseBracket => "]",
            Key::Backspace => "Backspace",
            other => other.name(),
        });
        s
    }
}

/// The chords, named so menus and the dispatch table cannot drift apart.
pub mod keys {
    use super::Chord;
    use egui::Key;

    // File
    pub const NEW: Chord = Chord::ctrl(Key::N);
    pub const OPEN: Chord = Chord::ctrl(Key::O);
    pub const SAVE: Chord = Chord::ctrl(Key::S);
    pub const SAVE_AS: Chord = Chord::ctrl_shift(Key::S);
    pub const CLOSE: Chord = Chord::ctrl(Key::W);
    pub const QUIT: Chord = Chord::ctrl(Key::Q);

    // Edit
    pub const UNDO: Chord = Chord::ctrl(Key::Z);
    pub const REDO: Chord = Chord::ctrl_shift(Key::Z);
    /// The legacy redo, still bound because the muscle memory persists.
    pub const REDO_LEGACY: Chord = Chord::ctrl(Key::Y);
    /// Step Backward. A linear history makes this the same as Undo.
    pub const STEP_BACKWARD: Chord = Chord::ctrl_alt(Key::Z);
    pub const COPY: Chord = Chord::ctrl(Key::C);
    pub const COPY_MERGED: Chord = Chord::ctrl_shift(Key::C);
    pub const CUT: Chord = Chord::ctrl(Key::X);
    pub const PASTE: Chord = Chord::ctrl(Key::V);
    pub const PASTE_IN_PLACE: Chord = Chord::ctrl_shift(Key::V);
    pub const FILL: Chord = Chord::shift(Key::Backspace);
    pub const FILL_F5: Chord = Chord::shift(Key::F5);
    pub const FILL_FOREGROUND: Chord = Chord::alt(Key::Backspace);
    pub const FILL_BACKGROUND: Chord = Chord::ctrl(Key::Backspace);
    pub const FILL_FOREGROUND_LOCKED: Chord =
        Chord { ctrl: false, shift: true, alt: true, key: Key::Backspace };
    pub const FILL_BACKGROUND_LOCKED: Chord = Chord::ctrl_shift(Key::Backspace);
    pub const FREE_TRANSFORM: Chord = Chord::ctrl(Key::T);

    // Image
    pub const IMAGE_SIZE: Chord = Chord::ctrl_alt(Key::I);
    pub const CANVAS_SIZE: Chord = Chord::ctrl_alt(Key::C);
    pub const LEVELS: Chord = Chord::ctrl(Key::L);
    pub const CURVES: Chord = Chord::ctrl(Key::M);
    pub const HUE_SATURATION: Chord = Chord::ctrl(Key::U);
    pub const COLOR_BALANCE: Chord = Chord::ctrl(Key::B);
    pub const DESATURATE: Chord = Chord::ctrl_shift(Key::U);
    pub const INVERT: Chord = Chord::ctrl(Key::I);

    // Layer
    pub const NEW_LAYER: Chord = Chord::ctrl_shift(Key::N);
    pub const LAYER_VIA_COPY: Chord = Chord::ctrl(Key::J);
    pub const MERGE_DOWN: Chord = Chord::ctrl(Key::E);
    pub const CLIPPING_MASK: Chord = Chord::ctrl_alt(Key::G);
    pub const LAYER_FORWARD: Chord = Chord::ctrl(Key::CloseBracket);
    pub const LAYER_BACKWARD: Chord = Chord::ctrl(Key::OpenBracket);
    pub const LAYER_TO_FRONT: Chord = Chord::ctrl_shift(Key::CloseBracket);
    pub const LAYER_TO_BACK: Chord = Chord::ctrl_shift(Key::OpenBracket);
    pub const SELECT_LAYER_ABOVE: Chord = Chord::alt(Key::CloseBracket);
    pub const SELECT_LAYER_BELOW: Chord = Chord::alt(Key::OpenBracket);

    // Select
    pub const SELECT_ALL: Chord = Chord::ctrl(Key::A);
    pub const DESELECT: Chord = Chord::ctrl(Key::D);
    pub const RESELECT: Chord = Chord::ctrl_shift(Key::D);
    pub const INVERSE: Chord = Chord::ctrl_shift(Key::I);
    pub const FEATHER: Chord = Chord::shift(Key::F6);

    // Filter
    pub const LAST_FILTER: Chord = Chord::ctrl(Key::F);

    // View
    pub const ZOOM_IN: Chord = Chord::ctrl(Key::Plus);
    pub const ZOOM_IN_EQUALS: Chord = Chord::ctrl(Key::Equals);
    pub const ZOOM_OUT: Chord = Chord::ctrl(Key::Minus);
    pub const ZOOM_FIT: Chord = Chord::ctrl(Key::Num0);
    pub const ZOOM_ACTUAL: Chord = Chord::ctrl(Key::Num1);
    pub const TOGGLE_PANELS: Chord = Chord::plain(Key::Tab);

    // Colours and modes
    pub const SWAP_COLORS: Chord = Chord::plain(Key::X);
    pub const RESET_COLORS: Chord = Chord::plain(Key::D);
    pub const QUICK_MASK: Chord = Chord::plain(Key::Q);
}

/// A chord and the command it runs.
///
/// The action is built on demand because some carry owned data — an adjustment
/// to open a dialog for — which cannot live in a `const`.
pub struct Binding {
    /// What the command is called, for the shortcuts window and for the
    /// settings file. Stable: it is the key a rebinding is stored under, so
    /// renaming one silently drops whatever someone had bound to it.
    pub name: &'static str,
    pub chord: Chord,
    pub make: fn() -> Action,
}

const fn bind(name: &'static str, chord: Chord, make: fn() -> Action) -> Binding {
    Binding { name, chord, make }
}

/// Every chord that maps straight to one command.
///
/// The context-sensitive keys are not here: Escape and Enter mean different
/// things during a transform, a crop and a drag; the arrows nudge; the tool
/// letters cycle within their group; and the brackets and digits adjust the
/// brush. Those stay in the input handler where the state they depend on is.
/// Every chord as it is bound now, with whatever has been changed applied.
///
/// Overrides are keyed by name rather than by position, so adding a command
/// does not shuffle everyone's rebindings along by one.
pub fn bindings_with(overrides: &std::collections::HashMap<String, Chord>) -> Vec<Binding> {
    let mut out = bindings();
    if overrides.is_empty() {
        return out;
    }
    for binding in &mut out {
        if let Some(chord) = overrides.get(binding.name) {
            binding.chord = *chord;
        }
    }
    // A command whose chord was taken by another has none, and something with
    // no chord must not be listening for one.
    out.retain(|b| b.chord.is_bound());
    out
}

pub fn bindings() -> Vec<Binding> {
    use cshop_core::adjust::Adjustment;
    use keys as k;

    vec![
        bind("New", k::NEW, || Action::NewDocument),
        bind("Open", k::OPEN, || Action::ShowOpenDialog),
        bind("Save", k::SAVE, || Action::Save),
        bind("Save as", k::SAVE_AS, || Action::ShowSaveAsDialog),
        bind("Close", k::CLOSE, || Action::CloseDocument(usize::MAX)),
        bind("Quit", k::QUIT, || Action::Quit),
        bind("Undo", k::UNDO, || Action::Undo),
        bind("Redo", k::REDO, || Action::Redo),
        bind("Redo (legacy)", k::REDO_LEGACY, || Action::Redo),
        bind("Step Backward", k::STEP_BACKWARD, || Action::Undo),
        bind("Copy", k::COPY, || Action::Copy),
        bind("Copy Merged", k::COPY_MERGED, || Action::CopyMerged),
        bind("Cut", k::CUT, || Action::Cut),
        bind("Paste", k::PASTE, || Action::Paste),
        bind("Paste in Place", k::PASTE_IN_PLACE, || Action::PasteInPlace),
        bind("Fill", k::FILL, || Action::ShowFillDialog),
        bind("Fill (F5)", k::FILL_F5, || Action::ShowFillDialog),
        bind("Fill Foreground", k::FILL_FOREGROUND, || Action::fill_foreground(false)),
        bind("Fill Background", k::FILL_BACKGROUND, || Action::fill_background(false)),
        bind("Fill Foreground Locked", k::FILL_FOREGROUND_LOCKED, || Action::fill_foreground(true)),
        bind("Fill Background Locked", k::FILL_BACKGROUND_LOCKED, || Action::fill_background(true)),
        bind("Free Transform", k::FREE_TRANSFORM, || Action::BeginTransform),
        bind("Image Size", k::IMAGE_SIZE, || Action::ShowImageSize),
        bind("Canvas Size", k::CANVAS_SIZE, || Action::ShowCanvasSize),
        bind("Levels", k::LEVELS, || {
            Action::ShowAdjustmentDialog(Box::new(Adjustment::Levels {
                rgb: Default::default(),
                channels: Default::default(),
            }))
        }),
        bind("Curves", k::CURVES, || {
            Action::ShowAdjustmentDialog(Box::new(Adjustment::Curves { curves: Default::default() }))
        }),
        bind("Hue Saturation", k::HUE_SATURATION, || {
            Action::ShowAdjustmentDialog(Box::new(Adjustment::HueSaturation {
                hue: 0.0,
                saturation: 0.0,
                lightness: 0.0,
                colorize: false,
            }))
        }),
        bind("Color Balance", k::COLOR_BALANCE, || {
            Action::ShowAdjustmentDialog(Box::new(Adjustment::ColorBalance {
                shadows: [0.0; 3],
                midtones: [0.0; 3],
                highlights: [0.0; 3],
                preserve_luminosity: true,
            }))
        }),
        // Desaturate is Hue/Saturation pulled to the bottom of its range, which
        // is what a Desaturate menu entry amounts to.
        bind("Desaturate", k::DESATURATE, || {
            Action::ApplyAdjustment(Box::new(Adjustment::HueSaturation {
                hue: 0.0,
                saturation: -1.0,
                lightness: 0.0,
                colorize: false,
            }))
        }),
        bind("Invert", k::INVERT, || Action::ApplyAdjustment(Box::new(Adjustment::Invert))),
        bind("New Layer", k::NEW_LAYER, || Action::NewLayer),
        bind("Layer Via Copy", k::LAYER_VIA_COPY, || Action::LayerViaCopy),
        bind("Merge Down", k::MERGE_DOWN, || Action::MergeDown),
        bind("Clipping Mask", k::CLIPPING_MASK, || Action::ToggleClippingMask),
        bind("Layer Forward", k::LAYER_FORWARD, || Action::ReorderActiveLayer(1)),
        bind("Layer Backward", k::LAYER_BACKWARD, || Action::ReorderActiveLayer(-1)),
        bind("Layer to Front", k::LAYER_TO_FRONT, || Action::ReorderActiveLayer(i32::MAX)),
        bind("Layer to Back", k::LAYER_TO_BACK, || Action::ReorderActiveLayer(i32::MIN)),
        bind("Select Layer Above", k::SELECT_LAYER_ABOVE, || Action::StepActiveLayer(1)),
        bind("Select Layer Below", k::SELECT_LAYER_BELOW, || Action::StepActiveLayer(-1)),
        bind("Select All", k::SELECT_ALL, || Action::SelectAll),
        bind("Deselect", k::DESELECT, || Action::Deselect),
        bind("Reselect", k::RESELECT, || Action::Reselect),
        bind("Inverse", k::INVERSE, || Action::InverseSelection),
        bind("Feather", k::FEATHER, || Action::ShowModifyDialog(crate::chrome::ModifyKind::Feather)),
        bind("Last Filter", k::LAST_FILTER, || Action::RepeatLastFilter),
        bind("Zoom in", k::ZOOM_IN, || Action::ZoomIn),
        bind("Zoom in Equals", k::ZOOM_IN_EQUALS, || Action::ZoomIn),
        bind("Zoom Out", k::ZOOM_OUT, || Action::ZoomOut),
        bind("Zoom Fit", k::ZOOM_FIT, || Action::ZoomFit),
        bind("Zoom Actual", k::ZOOM_ACTUAL, || Action::ZoomActual),
        bind("Toggle Panels", k::TOGGLE_PANELS, || Action::TogglePanels),
        bind("Swap Colors", k::SWAP_COLORS, || Action::SwapColors),
        bind("Reset Colors", k::RESET_COLORS, || Action::ResetColors),
        bind("Quick Mask", k::QUICK_MASK, || Action::ToggleQuickMask),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every chord named in `keys`, so the checks below cannot quietly miss
    /// one that was added later.
    fn every_named_chord() -> Vec<(&'static str, Chord)> {
        use keys as k;
        vec![
            ("NEW", k::NEW),
            ("OPEN", k::OPEN),
            ("SAVE", k::SAVE),
            ("SAVE_AS", k::SAVE_AS),
            ("CLOSE", k::CLOSE),
            ("QUIT", k::QUIT),
            ("UNDO", k::UNDO),
            ("REDO", k::REDO),
            ("REDO_LEGACY", k::REDO_LEGACY),
            ("STEP_BACKWARD", k::STEP_BACKWARD),
            ("COPY", k::COPY),
            ("COPY_MERGED", k::COPY_MERGED),
            ("CUT", k::CUT),
            ("PASTE", k::PASTE),
            ("PASTE_IN_PLACE", k::PASTE_IN_PLACE),
            ("FILL", k::FILL),
            ("FILL_F5", k::FILL_F5),
            ("FILL_FOREGROUND", k::FILL_FOREGROUND),
            ("FILL_BACKGROUND", k::FILL_BACKGROUND),
            ("FILL_FOREGROUND_LOCKED", k::FILL_FOREGROUND_LOCKED),
            ("FILL_BACKGROUND_LOCKED", k::FILL_BACKGROUND_LOCKED),
            ("FREE_TRANSFORM", k::FREE_TRANSFORM),
            ("IMAGE_SIZE", k::IMAGE_SIZE),
            ("CANVAS_SIZE", k::CANVAS_SIZE),
            ("LEVELS", k::LEVELS),
            ("CURVES", k::CURVES),
            ("HUE_SATURATION", k::HUE_SATURATION),
            ("COLOR_BALANCE", k::COLOR_BALANCE),
            ("DESATURATE", k::DESATURATE),
            ("INVERT", k::INVERT),
            ("NEW_LAYER", k::NEW_LAYER),
            ("LAYER_VIA_COPY", k::LAYER_VIA_COPY),
            ("MERGE_DOWN", k::MERGE_DOWN),
            ("CLIPPING_MASK", k::CLIPPING_MASK),
            ("LAYER_FORWARD", k::LAYER_FORWARD),
            ("LAYER_BACKWARD", k::LAYER_BACKWARD),
            ("LAYER_TO_FRONT", k::LAYER_TO_FRONT),
            ("LAYER_TO_BACK", k::LAYER_TO_BACK),
            ("SELECT_LAYER_ABOVE", k::SELECT_LAYER_ABOVE),
            ("SELECT_LAYER_BELOW", k::SELECT_LAYER_BELOW),
            ("SELECT_ALL", k::SELECT_ALL),
            ("DESELECT", k::DESELECT),
            ("RESELECT", k::RESELECT),
            ("INVERSE", k::INVERSE),
            ("FEATHER", k::FEATHER),
            ("LAST_FILTER", k::LAST_FILTER),
            ("ZOOM_IN", k::ZOOM_IN),
            ("ZOOM_IN_EQUALS", k::ZOOM_IN_EQUALS),
            ("ZOOM_OUT", k::ZOOM_OUT),
            ("ZOOM_FIT", k::ZOOM_FIT),
            ("ZOOM_ACTUAL", k::ZOOM_ACTUAL),
            ("TOGGLE_PANELS", k::TOGGLE_PANELS),
            ("SWAP_COLORS", k::SWAP_COLORS),
            ("RESET_COLORS", k::RESET_COLORS),
            ("QUICK_MASK", k::QUICK_MASK),
        ]
    }

    /// A named chord that nothing dispatches is a shortcut the menus may well
    /// be advertising — which is exactly how Merge Down came to promise Ctrl+E
    /// while nothing listened for it.
    #[test]
    fn every_named_chord_is_dispatched() {
        let bound: Vec<Chord> = bindings().iter().map(|b| b.chord).collect();
        for (name, chord) in every_named_chord() {
            assert!(
                bound.contains(&chord),
                "keys::{name} ({}) is named but nothing runs it",
                chord.label()
            );
        }
    }

    #[test]
    fn no_two_commands_share_a_chord() {
        let mut seen: Vec<Chord> = Vec::new();
        for binding in bindings() {
            assert!(
                !seen.contains(&binding.chord),
                "{} is bound twice",
                binding.chord.label()
            );
            seen.push(binding.chord);
        }
    }

    /// The plain tool letters must stay clear of the unmodified command
    /// chords, or pressing B would both pick the Brush and do something else.
    #[test]
    fn the_tool_letters_do_not_collide_with_plain_chords() {
        let plain: Vec<Chord> =
            bindings().iter().map(|b| b.chord).filter(|c| !c.ctrl && !c.shift && !c.alt).collect();
        for group in crate::tools::TOOL_GROUPS {
            let chord = Chord::plain(group.key);
            assert!(
                !plain.contains(&chord),
                "tool key {} is also a command shortcut",
                group.label
            );
        }
    }

    #[test]
    fn chords_read_the_way_they_are_conventionally_written() {
        use keys as k;
        assert_eq!(k::SAVE_AS.label(), "Ctrl+Shift+S");
        assert_eq!(k::IMAGE_SIZE.label(), "Ctrl+Alt+I");
        assert_eq!(k::LAYER_FORWARD.label(), "Ctrl+]");
        assert_eq!(k::FILL_FOREGROUND.label(), "Alt+Backspace");
        assert_eq!(k::ZOOM_OUT.label(), "Ctrl+-");
        assert_eq!(k::TOGGLE_PANELS.label(), "Tab");
    }
}
