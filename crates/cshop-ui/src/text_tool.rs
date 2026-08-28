//! Editing type on the canvas.
//!
//! Typing edits the layer directly so the canvas shows the result immediately,
//! which means the whole session bypasses the undo stack until it is
//! committed. Committing then records one `Edit Type` step covering everything
//! that was typed, which is what people expect from
//! Ctrl+Z after typing a word.

use cshop_core::geom::Vec2;
use cshop_core::layer::LayerId;
use cshop_core::text::TextContent;

/// A type layer being edited.
pub struct TextEdit {
    pub layer: LayerId,
    /// Caret position, as a byte index into the text.
    pub caret: usize,
    /// Content and layer offset as they were when editing began. `None` means
    /// the layer was created by this session, so cancelling deletes it.
    pub before: Option<(TextContent, (i32, i32))>,
    /// Where the text is anchored in document space, so re-rendering can put
    /// it back after the raster changes size.
    pub anchor: Vec2,
    /// When the caret last had reason to be visible. The blink is measured
    /// from here, so it does not flick off mid-keystroke.
    pub blink_epoch: f64,
}

impl TextEdit {
    pub fn is_new(&self) -> bool {
        self.before.is_none()
    }
}

/// What a key press means while type is being edited.
#[derive(Debug, Clone, PartialEq)]
pub enum TextInput {
    Insert(String),
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Newline,
    Commit,
    Cancel,
}

/// Byte index of the character before `at`.
pub fn prev_char(text: &str, at: usize) -> usize {
    text[..at.min(text.len())].chars().next_back().map_or(0, |c| at - c.len_utf8())
}

/// Byte index of the character after `at`.
pub fn next_char(text: &str, at: usize) -> usize {
    text[at.min(text.len())..].chars().next().map_or(text.len(), |c| at + c.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_moves_by_whole_characters() {
        // Multi-byte characters must not be split, or the string stops being
        // valid UTF-8 the moment Backspace is pressed.
        let s = "aé漢z";
        let mut at = 0;
        let mut stops = vec![0];
        while at < s.len() {
            at = next_char(s, at);
            stops.push(at);
        }
        assert_eq!(stops, vec![0, 1, 3, 6, 7]);

        let mut back = vec![s.len()];
        let mut at = s.len();
        while at > 0 {
            at = prev_char(s, at);
            back.push(at);
        }
        back.reverse();
        assert_eq!(back, stops);
    }

    #[test]
    fn stepping_past_the_ends_stays_put() {
        assert_eq!(prev_char("abc", 0), 0);
        assert_eq!(next_char("abc", 3), 3);
        assert_eq!(next_char("", 0), 0);
    }
}
