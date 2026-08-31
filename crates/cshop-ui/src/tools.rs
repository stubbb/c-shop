//! The toolbox: which tools exist, how they are grouped, and their shortcuts.
//!
//! Grouping follows the long-standing convention: one slot in the single-column
//! toolbar
//! holds several related tools reachable by a press-and-hold flyout, and the
//! group shares one keyboard shortcut that cycles through it.

/// A tool the user can select.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Move,
    RectangularMarquee,
    EllipticalMarquee,
    Lasso,
    PolygonalLasso,
    MagicWand,
    Crop,
    Eyedropper,
    Brush,
    Pencil,
    Eraser,
    CloneStamp,
    Dodge,
    Burn,
    Sponge,
    Blur,
    Sharpen,
    Smudge,
    HealingBrush,
    SpotHealing,
    HistoryBrush,
    PaintBucket,
    Gradient,
    Text,
    Pen,
    DirectSelect,
    Shape,
    Hand,
    Zoom,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::Move => "Move",
            Tool::RectangularMarquee => "Rectangular Marquee",
            Tool::EllipticalMarquee => "Elliptical Marquee",
            Tool::Lasso => "Lasso",
            Tool::PolygonalLasso => "Polygonal Lasso",
            Tool::MagicWand => "Magic Wand",
            Tool::Crop => "Crop",
            Tool::Eyedropper => "Eyedropper",
            Tool::Brush => "Brush",
            Tool::Pencil => "Pencil",
            Tool::Eraser => "Eraser",
            Tool::CloneStamp => "Clone Stamp",
            Tool::Dodge => "Dodge",
            Tool::Burn => "Burn",
            Tool::Sponge => "Sponge",
            Tool::Blur => "Blur",
            Tool::Sharpen => "Sharpen",
            Tool::Smudge => "Smudge",
            Tool::HealingBrush => "Healing Brush",
            Tool::SpotHealing => "Spot Healing",
            Tool::HistoryBrush => "History Brush",
            Tool::PaintBucket => "Paint Bucket",
            Tool::Gradient => "Gradient",
            Tool::Text => "Horizontal Type",
            Tool::Pen => "Pen",
            Tool::DirectSelect => "Direct Selection",
            Tool::Shape => "Shape",
            Tool::Hand => "Hand",
            Tool::Zoom => "Zoom",
        }
    }

    /// Single-character glyph for the toolbar button.
    ///
    /// Purpose-drawn vector icons come later; until then these stand in
    /// and keep the column readable.
    pub fn glyph(self) -> &'static str {
        match self {
            Tool::Move => "✥",
            Tool::RectangularMarquee => "▭",
            Tool::EllipticalMarquee => "◯",
            Tool::Lasso => "◌",
            Tool::PolygonalLasso => "△",
            Tool::MagicWand => "✧",
            Tool::Crop => "⌗",
            Tool::Eyedropper => "⌇",
            Tool::Brush => "🖌",
            Tool::Pencil => "✎",
            Tool::Eraser => "▨",
            Tool::CloneStamp => "⎘",
            Tool::Dodge => "☀",
            Tool::Burn => "☁",
            Tool::Sponge => "◍",
            Tool::Blur => "💧",
            Tool::Sharpen => "◣",
            Tool::Smudge => "☞",
            Tool::HealingBrush => "⚕",
            Tool::SpotHealing => "✚",
            Tool::HistoryBrush => "↺",
            Tool::PaintBucket => "🪣",
            Tool::Gradient => "▤",
            Tool::Text => "T",
            Tool::Pen => "✒",
            Tool::DirectSelect => "◄",
            Tool::Shape => "▬",
            Tool::Hand => "✋",
            Tool::Zoom => "🔍",
        }
    }

    /// Whether the tool is implemented yet. Unimplemented tools still appear in
    /// the toolbar — greyed out — so the interface reads as complete and the
    /// gaps are honest rather than hidden.
    /// Looks a tool up by its display name, ignoring case and punctuation, so
    /// `clone stamp`, `CloneStamp` and the prefix `clone` all resolve. Used by
    /// the `--demo-tool` screenshot flag.
    pub fn from_name(name: &str) -> Option<Tool> {
        fn key(s: &str) -> String {
            s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
        }
        let want = key(name);
        if want.is_empty() {
            return None;
        }
        let all = || TOOL_GROUPS.iter().flat_map(|g| g.tools.iter().copied());
        all().find(|t| key(t.name()) == want).or_else(|| {
            let mut hits = all().filter(|t| key(t.name()).starts_with(&want));
            // Only accept a prefix when it is unambiguous.
            match (hits.next(), hits.next()) {
                (Some(t), None) => Some(t),
                _ => None,
            }
        })
    }

    pub fn is_implemented(self) -> bool {
        // Listed explicitly rather than by exclusion, so adding a tool to the
        // enum shows up as unimplemented until it really is.
        matches!(
            self,
            Tool::Move
                | Tool::RectangularMarquee
                | Tool::EllipticalMarquee
                | Tool::Lasso
                | Tool::PolygonalLasso
                | Tool::MagicWand
                | Tool::Crop
                | Tool::Eyedropper
                | Tool::Brush
                | Tool::Pencil
                | Tool::Eraser
                | Tool::CloneStamp
                | Tool::Dodge
                | Tool::Burn
                | Tool::Sponge
                | Tool::Blur
                | Tool::Sharpen
                | Tool::Smudge
                | Tool::HealingBrush
                | Tool::SpotHealing
                | Tool::HistoryBrush
                | Tool::PaintBucket
                | Tool::Gradient
                | Tool::Text
                | Tool::Pen
                | Tool::DirectSelect
                | Tool::Shape
                | Tool::Hand
                | Tool::Zoom
        )
    }

    /// Whether the tool creates or edits a selection, which the options bar
    /// uses to decide what controls to show.
    /// Tools that paint with the brush engine, and so answer to the brush
    /// size, hardness and opacity keys.
    pub fn uses_brush(self) -> bool {
        matches!(
            self,
            Tool::Brush
                | Tool::Pencil
                | Tool::Eraser
                | Tool::CloneStamp
                | Tool::Dodge
                | Tool::Burn
                | Tool::Sponge
                | Tool::Blur
                | Tool::Sharpen
                | Tool::Smudge
                | Tool::HealingBrush
                | Tool::SpotHealing
                | Tool::HistoryBrush
        )
    }

    /// Whether this tool repairs by taking texture from elsewhere and tone
    /// from where it lands. See [`cshop_core::heal`].
    pub fn heals(self) -> bool {
        matches!(self, Tool::HealingBrush | Tool::SpotHealing)
    }

    /// Tools that change the pixels they pass over rather than covering them.
    /// They read the brush's size, hardness and spacing but ignore its colour.
    pub fn retouches(self) -> Option<cshop_core::retouch::RetouchKind> {
        use cshop_core::retouch::RetouchKind;
        match self {
            Tool::Dodge => Some(RetouchKind::Dodge),
            Tool::Burn => Some(RetouchKind::Burn),
            Tool::Sponge => Some(RetouchKind::Sponge),
            _ => None,
        }
    }

    pub fn is_selection_tool(self) -> bool {
        // Only the tools that build a selection, which is what decides whether
        // the options bar shows the boolean modes and feather. Crop, the
        // bucket, the gradient and the clone stamp each have their own
        // options and must not land here.
        matches!(
            self,
            Tool::RectangularMarquee
                | Tool::EllipticalMarquee
                | Tool::Lasso
                | Tool::PolygonalLasso
                | Tool::MagicWand
        )
    }
}

/// One slot in the toolbar: a shortcut key and the tools it cycles through.
pub struct ToolGroup {
    pub key: egui::Key,
    pub label: char,
    pub tools: &'static [Tool],
}

/// The toolbar, top to bottom, with the conventional shortcut letters.
pub const TOOL_GROUPS: &[ToolGroup] = &[
    ToolGroup { key: egui::Key::V, label: 'V', tools: &[Tool::Move] },
    ToolGroup {
        key: egui::Key::M,
        label: 'M',
        tools: &[Tool::RectangularMarquee, Tool::EllipticalMarquee],
    },
    ToolGroup { key: egui::Key::L, label: 'L', tools: &[Tool::Lasso, Tool::PolygonalLasso] },
    ToolGroup { key: egui::Key::W, label: 'W', tools: &[Tool::MagicWand] },
    ToolGroup { key: egui::Key::C, label: 'C', tools: &[Tool::Crop] },
    ToolGroup { key: egui::Key::I, label: 'I', tools: &[Tool::Eyedropper] },
    ToolGroup { key: egui::Key::B, label: 'B', tools: &[Tool::Brush, Tool::Pencil] },
    ToolGroup {
        key: egui::Key::J,
        label: 'J',
        tools: &[Tool::SpotHealing, Tool::HealingBrush],
    },
    ToolGroup { key: egui::Key::S, label: 'S', tools: &[Tool::CloneStamp] },
    ToolGroup { key: egui::Key::E, label: 'E', tools: &[Tool::Eraser] },
    ToolGroup { key: egui::Key::Y, label: 'Y', tools: &[Tool::HistoryBrush] },
    ToolGroup {
        key: egui::Key::O,
        label: 'O',
        tools: &[Tool::Dodge, Tool::Burn, Tool::Sponge],
    },
    ToolGroup {
        key: egui::Key::R,
        label: 'R',
        tools: &[Tool::Blur, Tool::Sharpen, Tool::Smudge],
    },
    ToolGroup { key: egui::Key::G, label: 'G', tools: &[Tool::PaintBucket, Tool::Gradient] },
    ToolGroup { key: egui::Key::T, label: 'T', tools: &[Tool::Text] },
    ToolGroup { key: egui::Key::A, label: 'A', tools: &[Tool::DirectSelect] },
    ToolGroup { key: egui::Key::P, label: 'P', tools: &[Tool::Pen] },
    ToolGroup { key: egui::Key::U, label: 'U', tools: &[Tool::Shape] },
    ToolGroup { key: egui::Key::H, label: 'H', tools: &[Tool::Hand] },
    ToolGroup { key: egui::Key::Z, label: 'Z', tools: &[Tool::Zoom] },
];

/// The group containing `tool`, if any.
pub fn group_of(tool: Tool) -> Option<&'static ToolGroup> {
    TOOL_GROUPS.iter().find(|g| g.tools.contains(&tool))
}

/// Pressing a group's shortcut selects its first tool, or advances to the next
/// one when that group is already active — the usual shift-cycling made
/// simpler.
pub fn cycle(group: &ToolGroup, current: Tool) -> Tool {
    match group.tools.iter().position(|&t| t == current) {
        Some(i) => group.tools[(i + 1) % group.tools.len()],
        None => group.tools[0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_belongs_to_exactly_one_group() {
        let all = [
            Tool::Move,
            Tool::RectangularMarquee,
            Tool::EllipticalMarquee,
            Tool::Lasso,
            Tool::PolygonalLasso,
            Tool::MagicWand,
            Tool::Crop,
            Tool::Eyedropper,
            Tool::Brush,
            Tool::Pencil,
            Tool::Eraser,
            Tool::PaintBucket,
            Tool::Gradient,
            Tool::Text,
            Tool::DirectSelect,
            Tool::Pen,
            Tool::Shape,
            Tool::Hand,
            Tool::Zoom,
            Tool::CloneStamp,
        ];
        for t in all {
            let n = TOOL_GROUPS.iter().filter(|g| g.tools.contains(&t)).count();
            assert_eq!(n, 1, "{:?} appears in {n} groups", t);
        }
    }

    /// Every tool in the toolbar, for the exhaustiveness checks below.
    fn all_tools() -> Vec<Tool> {
        TOOL_GROUPS.iter().flat_map(|g| g.tools.iter().copied()).collect()
    }

    #[test]
    fn tools_resolve_from_their_names() {
        for tool in all_tools() {
            assert_eq!(Tool::from_name(tool.name()), Some(tool));
        }
        assert_eq!(Tool::from_name("clone stamp"), Some(Tool::CloneStamp));
        assert_eq!(Tool::from_name("clone"), Some(Tool::CloneStamp));
        assert_eq!(Tool::from_name("gradient"), Some(Tool::Gradient));
        assert_eq!(Tool::from_name("nonsense"), None);
        assert_eq!(Tool::from_name(""), None);
    }

    #[test]
    fn only_the_marquees_and_the_wand_count_as_selection_tools() {
        // This decides what the options bar shows. A stray entry here once put
        // the marquee's feather and boolean modes in front of the Gradient
        // tool, which has nothing to do with selections.
        let expected = [
            Tool::RectangularMarquee,
            Tool::EllipticalMarquee,
            Tool::Lasso,
            Tool::PolygonalLasso,
            Tool::MagicWand,
        ];
        for tool in all_tools() {
            assert_eq!(
                tool.is_selection_tool(),
                expected.contains(&tool),
                "{:?} is on the wrong side of is_selection_tool",
                tool
            );
        }
    }

    #[test]
    fn the_unimplemented_tools_are_the_ones_we_think() {
        // Keeps the greyed-out set honest: a tool that works should not look
        // broken, and one that does not should not look ready.
        let expected_missing: [Tool; 0] = [];
        for tool in all_tools() {
            assert_eq!(
                tool.is_implemented(),
                !expected_missing.contains(&tool),
                "{:?} disagrees with is_implemented",
                tool
            );
        }
    }

    #[test]
    fn shortcuts_are_unique() {
        let mut keys: Vec<_> = TOOL_GROUPS.iter().map(|g| g.label).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "two tool groups share a shortcut");
    }

    #[test]
    fn cycling_walks_a_group_and_wraps() {
        let g = group_of(Tool::Brush).unwrap();
        assert_eq!(cycle(g, Tool::Brush), Tool::Pencil);
        assert_eq!(cycle(g, Tool::Pencil), Tool::Brush);
        // Coming from another group selects the first entry.
        assert_eq!(cycle(g, Tool::Move), Tool::Brush);
    }

    #[test]
    fn single_tool_groups_stay_put() {
        let g = group_of(Tool::Move).unwrap();
        assert_eq!(cycle(g, Tool::Move), Tool::Move);
    }
}
