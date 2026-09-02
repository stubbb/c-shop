//! What the editor remembers between one run and the next.
//!
//! Until this existed, nothing was: not the window, not the tool, not the
//! brush, not which files had been opened. Every launch was identical to the
//! first, which is the sort of thing nobody notices on the first day and
//! everybody notices on the second.
//!
//! Written as JSON, using the reader and writer already in the tree for the
//! server, so this costs no dependency. Anything unreadable is ignored rather
//! than reported: a settings file is a convenience, and a convenience that can
//! stop the program starting is not one. A field that is missing or nonsense
//! falls back to the default, so an older file gains new settings and a newer
//! one loses nothing that this build does not understand.

use crate::tools::Tool;
use cshop_core::color::Rgba8;
use cshop_core::json::Json;
use cshop_core::paint::Brush;
use std::path::{Path, PathBuf};

/// How many opened files to remember.
const RECENT: usize = 12;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub window: Option<(u32, u32)>,
    pub tool: Tool,
    pub brush: Brush,
    /// Which drawn shape the brush stamps, if any. A tip defined from a
    /// selection is not remembered: it belongs to a document that may not be
    /// open next time, and a brush that silently stamps last week's leaf is
    /// worse than one that starts round.
    pub brush_shape: Option<cshop_core::tips::TipShape>,
    pub foreground: Rgba8,
    pub background: Rgba8,
    pub show_rulers: bool,
    pub show_guides: bool,
    pub show_grid: bool,
    pub snap: bool,
    pub grid_spacing: f32,
    pub show_panels: bool,
    /// Dodge, Burn and Sponge. Its `kind` is along for the ride: which of the
    /// three a stroke is comes from the selected tool, not from here.
    pub retouch: cshop_core::retouch::Retouch,
    /// Blur, Sharpen and Smudge.
    pub brush_filter_strength: f32,
    /// Shortcuts that have been changed, by command name. Only the changed
    /// ones, so a new build's defaults reach everyone who has not overridden
    /// them.
    pub shortcuts: Vec<(String, String)>,
    /// Most recently opened first.
    pub recent: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            window: None,
            tool: Tool::Brush,
            brush: Brush::default(),
            brush_shape: None,
            foreground: Rgba8::BLACK,
            background: Rgba8::WHITE,
            show_rulers: true,
            show_guides: true,
            show_grid: false,
            snap: true,
            grid_spacing: 32.0,
            show_panels: true,
            retouch: Default::default(),
            brush_filter_strength: 0.5,
            shortcuts: Vec::new(),
            recent: Vec::new(),
        }
    }
}

/// Where the file lives, by the usual rule for the platform.
pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("cshop").join("settings.json"))
}

impl Settings {
    /// Read them, or the defaults if there is nothing to read.
    pub fn load() -> Settings {
        let Some(path) = path() else { return Settings::default() };
        let Ok(text) = std::fs::read_to_string(&path) else { return Settings::default() };
        match cshop_core::json::parse(&text) {
            Ok(json) => Settings::from_json(&json),
            Err(e) => {
                log::warn!("ignoring unreadable settings at {}: {e}", path.display());
                Settings::default()
            }
        }
    }

    /// Write them, quietly. A settings file that cannot be saved is worth a
    /// line in the log and nothing more.
    pub fn save(&self) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                log::warn!("could not make {}: {e}", dir.display());
                return;
            }
        }
        if let Err(e) = std::fs::write(&path, self.to_json().write()) {
            log::warn!("could not save settings to {}: {e}", path.display());
        }
    }

    /// Note a file as opened, most recent first and without repeats.
    pub fn remember(&mut self, file: &Path) {
        self.recent.retain(|p| p != file);
        self.recent.insert(0, file.to_path_buf());
        self.recent.truncate(RECENT);
    }

    pub fn to_json(&self) -> Json {
        let colour = |c: Rgba8| Json::String(format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b));
        let mut fields = vec![
            ("tool", Json::String(self.tool.name().to_string())),
            ("brush_size", Json::Number(self.brush.size as f64)),
            ("brush_hardness", Json::Number(self.brush.hardness as f64)),
            ("brush_opacity", Json::Number(self.brush.opacity as f64)),
            ("brush_flow", Json::Number(self.brush.flow as f64)),
            ("brush_spacing", Json::Number(self.brush.spacing as f64)),
            ("brush_scatter", Json::Number(self.brush.scatter.spread as f64)),
            ("brush_count", Json::Number(self.brush.scatter.count as f64)),
            ("brush_scale", Json::Number(self.brush.scatter.scale as f64)),
            ("brush_size_jitter", Json::Number(self.brush.scatter.size_jitter as f64)),
            ("brush_angle", Json::Number(self.brush.scatter.angle as f64)),
            ("brush_angle_follows", Json::Bool(self.brush.scatter.follow)),
            ("brush_pressure_size", Json::Bool(self.brush.pressure.size)),
            ("brush_pressure_flow", Json::Bool(self.brush.pressure.flow)),
            ("brush_pressure_opacity", Json::Bool(self.brush.pressure.opacity)),
            (
                "brush_shape",
                match self.brush_shape {
                    Some(shape) => Json::String(shape.name().to_string()),
                    None => Json::String("round".to_string()),
                },
            ),
            ("foreground", colour(self.foreground)),
            ("background", colour(self.background)),
            ("show_rulers", Json::Bool(self.show_rulers)),
            ("show_guides", Json::Bool(self.show_guides)),
            ("show_grid", Json::Bool(self.show_grid)),
            ("snap", Json::Bool(self.snap)),
            ("grid_spacing", Json::Number(self.grid_spacing as f64)),
            ("show_panels", Json::Bool(self.show_panels)),
            ("retouch_kind", Json::String(self.retouch.kind.name().to_string())),
            ("retouch_range", Json::String(self.retouch.range.name().to_string())),
            ("retouch_exposure", Json::Number(self.retouch.exposure as f64)),
            ("retouch_soak", Json::Bool(self.retouch.soak)),
            ("brush_filter_strength", Json::Number(self.brush_filter_strength as f64)),
            (
                "shortcuts",
                Json::Array(
                    self.shortcuts
                        .iter()
                        .map(|(name, chord)| {
                            Json::object(vec![
                                ("command", Json::String(name.clone())),
                                ("chord", Json::String(chord.clone())),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "recent",
                Json::Array(
                    self.recent
                        .iter()
                        .map(|p| Json::String(p.display().to_string()))
                        .collect(),
                ),
            ),
        ];
        if let Some((w, h)) = self.window {
            fields.push(("window_width", Json::Number(w as f64)));
            fields.push(("window_height", Json::Number(h as f64)));
        }
        Json::object(fields)
    }

    pub fn from_json(json: &Json) -> Settings {
        let mut s = Settings::default();
        let number = |k: &str| json.get(k).and_then(Json::as_f64);
        let flag = |k: &str, fallback: bool| json.get(k).and_then(Json::as_bool).unwrap_or(fallback);

        if let Some(name) = json.str_field("tool") {
            if let Some(tool) = crate::tools::TOOL_GROUPS
                .iter()
                .flat_map(|g| g.tools.iter().copied())
                .find(|t| t.name() == name)
            {
                s.tool = tool;
            }
        }
        if let Some(list) = json.get("shortcuts").and_then(Json::as_array) {
            s.shortcuts = list
                .iter()
                .filter_map(|e| {
                    Some((e.str_field("command")?.to_string(), e.str_field("chord")?.to_string()))
                })
                .take(512)
                .collect();
        }
        if let Some(name) = json.str_field("retouch_kind") {
            use cshop_core::retouch::RetouchKind::{Burn, Dodge, Sponge};
            if let Some(k) = [Dodge, Burn, Sponge].into_iter().find(|k| k.name() == name) {
                s.retouch.kind = k;
            }
        }
        if let Some(name) = json.str_field("retouch_range") {
            use cshop_core::retouch::Tones;
            if let Some(t) = [Tones::Shadows, Tones::Midtones, Tones::Highlights]
                .into_iter()
                .find(|t| t.name() == name)
            {
                s.retouch.range = t;
            }
        }
        if let Some(v) = number("retouch_exposure") {
            s.retouch.exposure = (v as f32).clamp(0.0, 1.0);
        }
        s.retouch.soak = flag("retouch_soak", s.retouch.soak);
        if let Some(v) = number("brush_filter_strength") {
            s.brush_filter_strength = (v as f32).clamp(0.0, 1.0);
        }
        if let Some(v) = number("brush_size") {
            s.brush.size = (v as f32).clamp(1.0, 2000.0);
        }
        if let Some(v) = number("brush_hardness") {
            s.brush.hardness = (v as f32).clamp(0.0, 1.0);
        }
        if let Some(v) = number("brush_opacity") {
            s.brush.opacity = (v as f32).clamp(0.0, 1.0);
        }
        if let Some(v) = number("brush_flow") {
            s.brush.flow = (v as f32).clamp(0.0, 1.0);
        }
        if let Some(v) = number("brush_spacing") {
            s.brush.spacing = (v as f32).clamp(0.01, 4.0);
        }
        // A settings file is editable, and a scatter count is a multiplier on
        // the cost of every dab, so these arrive through the same bounds the
        // sliders impose rather than straight into the brush.
        if let Some(v) = number("brush_scatter") {
            s.brush.scatter.spread = v as f32;
        }
        if let Some(v) = number("brush_count") {
            s.brush.scatter.count = v.max(0.0) as u32;
        }
        if let Some(v) = number("brush_scale") {
            s.brush.scatter.scale = v as f32;
        }
        if let Some(v) = number("brush_size_jitter") {
            s.brush.scatter.size_jitter = v as f32;
        }
        if let Some(v) = number("brush_angle") {
            s.brush.scatter.angle = v as f32;
        }
        s.brush.scatter.follow = flag("brush_angle_follows", s.brush.scatter.follow);
        s.brush.scatter = s.brush.scatter.sane();
        s.brush.pressure.size = flag("brush_pressure_size", s.brush.pressure.size);
        s.brush.pressure.flow = flag("brush_pressure_flow", s.brush.pressure.flow);
        s.brush.pressure.opacity = flag("brush_pressure_opacity", s.brush.pressure.opacity);
        if let Some(name) = json.str_field("brush_shape") {
            s.brush_shape =
                cshop_core::tips::TipShape::ALL.into_iter().find(|sh| sh.name() == name);
        }
        if let Some(c) = json.str_field("foreground").and_then(parse_colour) {
            s.foreground = c;
        }
        if let Some(c) = json.str_field("background").and_then(parse_colour) {
            s.background = c;
        }
        s.show_rulers = flag("show_rulers", s.show_rulers);
        s.show_guides = flag("show_guides", s.show_guides);
        s.show_grid = flag("show_grid", s.show_grid);
        s.snap = flag("snap", s.snap);
        s.show_panels = flag("show_panels", s.show_panels);
        if let Some(v) = number("grid_spacing") {
            s.grid_spacing = (v as f32).clamp(1.0, 4096.0);
        }
        if let (Some(w), Some(h)) = (number("window_width"), number("window_height")) {
            // A window larger than any plausible screen, or smaller than a
            // usable one, is not honoured: a bad value here would open the
            // editor somewhere it cannot be got at.
            let (w, h) = (w as u32, h as u32);
            if (320..=16384).contains(&w) && (240..=16384).contains(&h) {
                s.window = Some((w, h));
            }
        }
        if let Some(list) = json.get("recent").and_then(Json::as_array) {
            s.recent = list
                .iter()
                .filter_map(Json::as_str)
                .map(PathBuf::from)
                .take(RECENT)
                .collect();
        }
        s
    }
}

fn parse_colour(text: &str) -> Option<Rgba8> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some(Rgba8::opaque(byte(0)?, byte(2)?, byte(4)?))
}
