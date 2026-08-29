//! A command language for driving the editor without a window.
//!
//! Built for callers that cannot see and cannot click — an agent, a batch job,
//! a test. It exists because the alternative, a parallel API alongside the
//! interface, drifts: this drives the same [`CShopApp`] and the same actions
//! the buttons do, so anything the editor gains is reachable here the same day.
//!
//! # The loop it is designed for
//!
//! Intake, draw, analyse, return. A script goes in; the document is built; a
//! **report** comes out saying what actually happened — where every layer
//! landed, what each step did, and what failed and why — alongside the
//! rendered image. A caller that cannot see the canvas can still tell whether
//! the text fitted, whether the shadow fell off the edge, and what to change.
//!
//! Two rules follow from that:
//!
//! * **Nothing fails silently.** An unknown command, an unparseable value, an
//!   action that could not apply — each becomes a step marked failed with a
//!   reason, and the run carries on so one typo does not discard the rest.
//! * **Measurements are free.** `measure` reports the size of type without
//!   committing it, so a caller can place something before drawing it rather
//!   than rendering and guessing.
//!
//! # Shape
//!
//! One command per line, `#` for comments, positional arguments then
//! `key=value` options:
//!
//! ```text
//! open photo.jpg
//! text 60 120 "Hello" size=96 color=#ffffff bold
//! effect drop-shadow distance=10 size=12
//! export out.png
//! ```
//!
//! A bare word like `bold` is shorthand for `bold=true`. Nothing tells the
//! parser it is not a positional argument, so **bare flags go last**, after
//! the arguments a command counts.

use cshop_core::color::Rgba8;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

/// What one command did.
pub struct Step {
    pub line: usize,
    pub command: String,
    pub ok: bool,
    /// What happened, or why it did not. Written for a reader that cannot see
    /// the canvas.
    pub note: String,
}

/// One layer, as the report describes it.
pub struct LayerReport {
    pub index: usize,
    pub name: String,
    pub kind: String,
    pub bounds: [i32; 4],
    pub opacity: f32,
    pub blend: String,
    pub visible: bool,
    pub effects: Vec<String>,
}

#[derive(Default)]
pub struct Report {
    pub ok: bool,
    pub document: Option<(String, u32, u32)>,
    pub layers: Vec<LayerReport>,
    pub steps: Vec<Step>,
    pub outputs: Vec<String>,
    /// Free-form answers to `info` and `measure`, which are the whole point of
    /// those commands.
    pub facts: Vec<(String, String)>,
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

impl Report {
    /// The report as JSON.
    ///
    /// Hand-written rather than derived: it is a handful of shapes, and the
    /// output is something a reader parses by eye as often as by machine.
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        let _ = write!(s, "{{\n  \"ok\": {},\n", self.ok);
        if let Some((name, w, h)) = &self.document {
            let _ = writeln!(
                s,
                "  \"document\": {{ \"name\": \"{}\", \"width\": {w}, \"height\": {h} }},",
                json_escape(name)
            );
        } else {
            let _ = writeln!(s, "  \"document\": null,");
        }

        let _ = writeln!(s, "  \"layers\": [");
        for (i, l) in self.layers.iter().enumerate() {
            let fx: Vec<String> =
                l.effects.iter().map(|e| format!("\"{}\"", json_escape(e))).collect();
            let _ = writeln!(
                s,
                "    {{ \"index\": {}, \"name\": \"{}\", \"kind\": \"{}\", \"bounds\": [{}, {}, {}, {}], \"opacity\": {:.3}, \"blend\": \"{}\", \"visible\": {}, \"effects\": [{}] }}{}",
                l.index,
                json_escape(&l.name),
                l.kind,
                l.bounds[0], l.bounds[1], l.bounds[2], l.bounds[3],
                l.opacity,
                l.blend,
                l.visible,
                fx.join(", "),
                if i + 1 == self.layers.len() { "" } else { "," }
            );
        }
        let _ = write!(s, "  ],\n  \"facts\": {{\n");
        for (i, (k, v)) in self.facts.iter().enumerate() {
            let _ = writeln!(
                s,
                "    \"{}\": \"{}\"{}",
                json_escape(k),
                json_escape(v),
                if i + 1 == self.facts.len() { "" } else { "," }
            );
        }
        let _ = write!(s, "  }},\n  \"steps\": [\n");
        for (i, st) in self.steps.iter().enumerate() {
            let _ = writeln!(
                s,
                "    {{ \"line\": {}, \"command\": \"{}\", \"ok\": {}, \"note\": \"{}\" }}{}",
                st.line,
                json_escape(&st.command),
                st.ok,
                json_escape(&st.note),
                if i + 1 == self.steps.len() { "" } else { "," }
            );
        }
        let outs: Vec<String> =
            self.outputs.iter().map(|o| format!("\"{}\"", json_escape(o))).collect();
        let _ = write!(s, "  ],\n  \"outputs\": [{}]\n}}\n", outs.join(", "));
        s
    }

    /// A short human summary, for when the JSON is more than is wanted.
    pub fn summary(&self) -> String {
        let failed = self.steps.iter().filter(|s| !s.ok).count();
        let mut out = String::new();
        if let Some((name, w, h)) = &self.document {
            let _ = writeln!(out, "{name}: {w}x{h}, {} layers", self.layers.len());
        }
        for l in &self.layers {
            let _ = writeln!(
                out,
                "  [{}] {:<24} {:<7} at ({}, {}) {}x{}{}",
                l.index,
                l.name,
                l.kind,
                l.bounds[0],
                l.bounds[1],
                l.bounds[2],
                l.bounds[3],
                if l.effects.is_empty() {
                    String::new()
                } else {
                    format!("  fx: {}", l.effects.join(", "))
                }
            );
        }
        for (k, v) in &self.facts {
            let _ = writeln!(out, "  {k}: {v}");
        }
        for s in self.steps.iter().filter(|s| !s.ok) {
            let _ = writeln!(out, "  line {}: {} — {}", s.line, s.command, s.note);
        }
        let _ = writeln!(
            out,
            "{} step{} ran, {failed} failed",
            self.steps.len(),
            if self.steps.len() == 1 { "" } else { "s" }
        );
        out
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// One parsed line: a command, its positional arguments, and its options.
#[derive(Debug)]
pub struct Command {
    pub line: usize,
    pub raw: String,
    pub name: String,
    pub args: Vec<String>,
    pub opts: Vec<(String, String)>,
}

/// Split a line into words, keeping quoted runs together.
fn tokenise(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => match chars.next() {
                Some('n') => cur.push('\n'),
                Some('t') => cur.push('\t'),
                Some(other) => cur.push(other),
                None => return Err("a line ends in a backslash".into()),
            },
            c if c.is_whitespace() && !quoted => {
                if started || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if quoted {
        return Err("a quoted string is never closed".into());
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Parse a whole script. Bad lines become commands that report their own
/// failure, so parsing never aborts the run.
pub fn parse(source: &str) -> Vec<Result<Command, (usize, String, String)>> {
    let mut out = Vec::new();
    for (i, raw) in source.lines().enumerate() {
        let line = i + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let tokens = match tokenise(trimmed) {
            Ok(t) => t,
            Err(e) => {
                out.push(Err((line, trimmed.to_string(), e)));
                continue;
            }
        };
        if tokens.is_empty() {
            continue;
        }
        let name = tokens[0].to_ascii_lowercase();
        let mut args = Vec::new();
        let mut opts = Vec::new();
        for t in &tokens[1..] {
            // A bare `bold` is shorthand for `bold=true`, which reads better
            // in a script than the long form.
            match t.split_once('=') {
                Some((k, v)) if !k.is_empty() => {
                    opts.push((k.to_ascii_lowercase(), v.to_string()))
                }
                _ => args.push(t.clone()),
            }
        }
        out.push(Ok(Command { line, raw: trimmed.to_string(), name, args, opts }));
    }
    out
}

impl Command {
    pub fn opt(&self, key: &str) -> Option<&str> {
        self.opts.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn flag(&self, key: &str) -> bool {
        // Present as a bare word, or set to something truthy.
        self.args.iter().any(|a| a.eq_ignore_ascii_case(key))
            || matches!(self.opt(key), Some("true" | "yes" | "1" | "on"))
    }

    pub fn f32(&self, key: &str) -> Result<Option<f32>, String> {
        match self.opt(key) {
            None => Ok(None),
            Some(v) => v
                .parse()
                .map(Some)
                .map_err(|_| format!("{key}={v:?} is not a number")),
        }
    }

    pub fn u32(&self, key: &str) -> Result<Option<u32>, String> {
        match self.opt(key) {
            None => Ok(None),
            Some(v) => v
                .parse()
                .map(Some)
                .map_err(|_| format!("{key}={v:?} is not a whole number")),
        }
    }

    pub fn color(&self, key: &str) -> Result<Option<Rgba8>, String> {
        match self.opt(key) {
            None => Ok(None),
            Some(v) => parse_color(v).map(Some),
        }
    }

    /// A positional argument as a number.
    pub fn arg_f32(&self, i: usize, what: &str) -> Result<f32, String> {
        let v = self.args.get(i).ok_or_else(|| format!("missing {what}"))?;
        v.parse().map_err(|_| format!("{what} {v:?} is not a number"))
    }
}

/// `#rgb`, `#rrggbb`, `#rrggbbaa`, or one of a few names.
pub fn parse_color(s: &str) -> Result<Rgba8, String> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix('#') {
        let n = |from: usize, len: usize| -> Result<u8, String> {
            let part = hex
                .get(from..from + len)
                .ok_or_else(|| format!("{s:?} is not a colour"))?;
            let v = u8::from_str_radix(part, 16).map_err(|_| format!("{s:?} is not a colour"))?;
            // A single hex digit means both nibbles, so #f00 is #ff0000.
            Ok(if len == 1 { v * 17 } else { v })
        };
        return match hex.len() {
            3 => Ok(Rgba8::new(n(0, 1)?, n(1, 1)?, n(2, 1)?, 255)),
            4 => Ok(Rgba8::new(n(0, 1)?, n(1, 1)?, n(2, 1)?, n(3, 1)?)),
            6 => Ok(Rgba8::new(n(0, 2)?, n(2, 2)?, n(4, 2)?, 255)),
            8 => Ok(Rgba8::new(n(0, 2)?, n(2, 2)?, n(4, 2)?, n(6, 2)?)),
            _ => Err(format!("{s:?} is not a colour; use #rgb, #rrggbb or #rrggbbaa")),
        };
    }
    Ok(match t.to_ascii_lowercase().as_str() {
        "black" => Rgba8::BLACK,
        "white" => Rgba8::WHITE,
        "red" => Rgba8::opaque(220, 40, 40),
        "green" => Rgba8::opaque(40, 170, 90),
        "blue" => Rgba8::opaque(50, 110, 230),
        "yellow" => Rgba8::opaque(245, 200, 60),
        "orange" => Rgba8::opaque(240, 140, 40),
        "purple" => Rgba8::opaque(150, 80, 200),
        "grey" | "gray" => Rgba8::opaque(128, 128, 128),
        "transparent" | "none" => Rgba8::TRANSPARENT,
        _ => return Err(format!("{s:?} is not a colour; use #rrggbb or a basic name")),
    })
}

/// Resolve a path in the script against the directory it came from, so a
/// script is portable, expanding a leading `~` because that is how a path
/// gets written by hand.
pub fn resolve(base: &Path, given: &str) -> PathBuf {
    if let Some(rest) = given.strip_prefix("~/").or_else(|| given.strip_prefix("~")) {
        if let Some(home) = std::env::var_os("HOME") {
            // `~` alone is the home directory; `~/x` is a path inside it.
            return PathBuf::from(home).join(rest.trim_start_matches('/'));
        }
    }
    let p = Path::new(given);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

use cshop_core::document::{Background, Document};
use cshop_core::effects::*;
use cshop_core::geom::{IRect, Vec2};
use cshop_core::pixels::PixelBuffer;
use cshop_gpu::context::GpuContext;
use cshop_ui::app::CShopApp;
use cshop_ui::commands::Action;

pub struct Runner {
    app: CShopApp,
    gpu: GpuContext,
    base: PathBuf,
    report: Report,
}

/// Run a script and return what happened.
///
/// `base` is the directory relative paths resolve against.
pub fn run(source: &str, base: &Path) -> Result<Report, String> {
    let gpu = GpuContext::headless().map_err(|e| format!("no GPU: {e}"))?;
    let mut runner = Runner {
        app: CShopApp::new(gpu.clone()),
        gpu,
        base: base.to_path_buf(),
        report: Report::default(),
    };
    runner.execute(source);
    Ok(runner.finish())
}

impl Runner {
    fn execute(&mut self, source: &str) {
        for parsed in parse(source) {
            match parsed {
                Err((line, raw, why)) => {
                    self.report.steps.push(Step { line, command: raw, ok: false, note: why });
                }
                Ok(cmd) => {
                    let raw = cmd.raw.clone();
                    let line = cmd.line;
                    match self.step(&cmd) {
                        Ok(note) => {
                            self.report.steps.push(Step { line, command: raw, ok: true, note })
                        }
                        Err(why) => {
                            self.report.steps.push(Step { line, command: raw, ok: false, note: why })
                        }
                    }
                }
            }
        }
    }

    /// Describe the document as it now stands.
    fn finish(mut self) -> Report {
        self.report.ok = self.report.steps.iter().all(|s| s.ok);
        if let Some(view) = self.app.doc() {
            self.report.document =
                Some((view.doc.name.clone(), view.doc.width, view.doc.height));
            // Bottom-first, which is the order the layers composite in.
            for (i, id) in view.doc.tree.iter_all().into_iter().enumerate() {
                let Some(l) = view.doc.tree.get(id) else { continue };
                let b = l.render_bounds();
                self.report.layers.push(LayerReport {
                    index: i,
                    name: l.name.clone(),
                    kind: l.kind.type_name().to_string(),
                    bounds: [b.x0, b.y0, b.width() as i32, b.height() as i32],
                    opacity: l.opacity,
                    blend: l.blend_mode.name().to_string(),
                    visible: l.visible,
                    effects: l.effects.active_names().iter().map(|s| s.to_string()).collect(),
                });
            }
        }
        self.report
    }

    fn need_doc(&self) -> Result<(), String> {
        if self.app.doc().is_none() {
            return Err("there is no document; use `new` or `open` first".into());
        }
        Ok(())
    }

    /// The composite, read back from the GPU.
    fn composite(&mut self) -> Result<PixelBuffer, String> {
        let i = self.app.active.ok_or("there is no document")?;
        let gpu = self.gpu.clone();
        Ok(self.app.render_composite(&gpu, i))
    }

    fn step(&mut self, cmd: &Command) -> Result<String, String> {
        match cmd.name.as_str() {
            "new" => self.cmd_new(cmd),
            "open" => self.cmd_open(cmd),
            "text" => self.cmd_text(cmd),
            "measure" => self.cmd_measure(cmd),
            "shape" => self.cmd_shape(cmd),
            "fill" => self.cmd_fill(cmd),
            "gradient" => self.cmd_gradient(cmd),
            "select" => self.cmd_select(cmd),
            "effect" => self.cmd_effect(cmd),
            "filter" => self.cmd_filter(cmd),
            "adjust" => self.cmd_adjust(cmd),
            "layer" => self.cmd_layer(cmd),
            "set" => self.cmd_set(cmd),
            "move" => self.cmd_move(cmd),
            "order" => self.cmd_order(cmd),
            "info" => self.cmd_info(cmd),
            "export" | "save" => self.cmd_write(cmd),
            other => Err(format!(
                "unknown command {other:?}. Available: new, open, text, measure, shape, fill, \
                 select, gradient, effect, filter, adjust, layer, set, move, order, info, \
                 export, save"
            )),
        }
    }

    fn cmd_new(&mut self, cmd: &Command) -> Result<String, String> {
        let w = cmd.arg_f32(0, "width")? as u32;
        let h = cmd.arg_f32(1, "height")? as u32;
        if w == 0 || h == 0 || w > 30_000 || h > 30_000 {
            return Err(format!("{w}x{h} is not a usable canvas size"));
        }
        let background = match cmd.opt("background").unwrap_or("white") {
            "white" => Background::White,
            "transparent" => Background::Transparent,
            other => Background::Color(parse_color(other)?),
        };
        self.app.open_document(Document::new("Untitled", w, h, background));
        Ok(format!("new {w}x{h} document"))
    }

    fn cmd_open(&mut self, cmd: &Command) -> Result<String, String> {
        let given = cmd.args.first().ok_or("open needs a path")?;
        let path = resolve(&self.base, given);
        let doc = cshop_io::load_document(&path)
            .map_err(|e| format!("could not open {}: {e}", path.display()))?;
        let (w, h, n) = (doc.width, doc.height, doc.tree.len());
        self.app.open_document(doc);
        Ok(format!("opened {} ({w}x{h}, {n} layer{})", path.display(), if n == 1 { "" } else { "s" }))
    }

    /// Build the type style a `text` or `measure` command asks for.
    fn text_style(&self, cmd: &Command) -> Result<cshop_core::text::TextStyle, String> {
        use cshop_core::text::{TextAlign, TextStyle};
        let db = cshop_core::font::FontDb::global();
        let family = match cmd.opt("family") {
            Some(f) => {
                // Say so rather than silently drawing in something else.
                if db.family(f).is_none() {
                    return Err(format!(
                        "no font family {f:?} is installed; try one of: {}",
                        db.families()
                            .iter()
                            .take(6)
                            .map(|x| x.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                f.to_string()
            }
            None => db.default_family(),
        };
        Ok(TextStyle {
            family,
            size: cmd.f32("size")?.unwrap_or(48.0),
            color: cmd.color("color")?.unwrap_or(Rgba8::BLACK),
            bold: cmd.flag("bold"),
            italic: cmd.flag("italic"),
            align: match cmd.opt("align").unwrap_or("left") {
                "center" | "centre" => TextAlign::Center,
                "right" => TextAlign::Right,
                _ => TextAlign::Left,
            },
            leading: cmd.f32("leading")?.filter(|v| *v > 0.0),
            tracking: cmd.f32("tracking")?.unwrap_or(0.0),
            antialias: !matches!(cmd.opt("antialias"), Some("false" | "no" | "0" | "off")),
        })
    }

    fn cmd_text(&mut self, cmd: &Command) -> Result<String, String> {
        self.need_doc()?;
        let x = cmd.arg_f32(0, "x")?;
        let y = cmd.arg_f32(1, "y")?;
        let content = cmd.args.get(2).ok_or("text needs a string to draw")?.clone();
        let style = self.text_style(cmd)?;
        let wrap = cmd.f32("wrap")?.filter(|v| *v > 0.0);

        // Type draws in the foreground colour, so `color=` has to set that
        // too or the style's colour is overridden the moment the layer starts.
        self.app.foreground = style.color;
        self.app.text_style = style;
        self.app.tool = cshop_ui::tools::Tool::Text;
        self.app.dispatch(Action::BeginText { at: Vec2::new(x, y), wrap });
        for ch in content.chars() {
            self.app.dispatch(Action::TextInput(
                cshop_ui::text_tool::TextInput::Insert(ch.to_string()),
            ));
        }
        self.app.dispatch(Action::CommitText);

        // Report where it actually landed, which is what a caller placing
        // things by number needs to know.
        let placed = self
            .app
            .doc()
            .and_then(|v| v.doc.active.and_then(|id| v.doc.tree.get(id)))
            .map(|l| l.bounds())
            .unwrap_or(IRect::EMPTY);
        if placed.is_empty() {
            return Err("the text drew nothing; is the family installed?".into());
        }
        Ok(format!(
            "drew {content:?} at ({}, {}), {}x{}",
            placed.x0,
            placed.y0,
            placed.width(),
            placed.height()
        ))
    }

    fn cmd_measure(&mut self, cmd: &Command) -> Result<String, String> {
        let what = cmd.args.first().map(|s| s.as_str()).unwrap_or("");
        if what != "text" {
            return Err("measure takes: measure text \"...\" [size=..] [family=..]".into());
        }
        let content = cmd.args.get(1).ok_or("measure text needs a string")?;
        let style = self.text_style(cmd)?;
        let c = cshop_core::text::TextContent {
            text: content.clone(),
            style,
            wrap_width: cmd.f32("wrap")?.filter(|v| *v > 0.0),
        };
        let r = cshop_core::text::measure(&c).ok_or("that font could not be loaded")?;
        let fact = format!("{}x{} (offset {}, {})", r.width(), r.height(), r.x0, r.y0);
        self.report.facts.push((format!("measure {content:?}"), fact.clone()));
        Ok(format!("{content:?} measures {fact}"))
    }

    fn cmd_shape(&mut self, cmd: &Command) -> Result<String, String> {
        use cshop_core::shape::{ShapeKind, ShapeStyle, StrokeAlign};
        self.need_doc()?;
        let kind_name = cmd.args.first().map(|s| s.as_str()).unwrap_or("rect");
        let x = cmd.arg_f32(1, "x")?;
        let y = cmd.arg_f32(2, "y")?;
        let w = cmd.arg_f32(3, "width")?;
        let h = cmd.arg_f32(4, "height")?;

        let kind = match kind_name {
            "rect" | "rectangle" => ShapeKind::Rectangle { radius: cmd.f32("radius")?.unwrap_or(0.0) },
            "ellipse" | "circle" => ShapeKind::Ellipse,
            "polygon" => ShapeKind::Polygon {
                sides: cmd.u32("sides")?.unwrap_or(6),
                star: false,
                inner: 0.5,
            },
            "star" => ShapeKind::Polygon {
                sides: cmd.u32("sides")?.unwrap_or(5),
                star: true,
                inner: cmd.f32("inner")?.unwrap_or(0.45),
            },
            "line" => ShapeKind::Line {
                thickness: cmd.f32("thickness")?.unwrap_or(3.0),
                from: (0.0, 0.0),
                to: (1.0, 1.0),
            },
            other => {
                return Err(format!(
                    "unknown shape {other:?}; use rect, ellipse, polygon, star or line"
                ))
            }
        };
        self.app.shape_kind = kind;
        self.app.shape_style = ShapeStyle {
            fill: cmd.color("fill")?.or(Some(Rgba8::BLACK)).filter(|_| cmd.opt("fill") != Some("none")),
            stroke: cmd.color("stroke")?,
            stroke_width: cmd.f32("stroke-width")?.unwrap_or(2.0),
            stroke_align: match cmd.opt("stroke-align").unwrap_or("center") {
                "inside" => StrokeAlign::Inside,
                "outside" => StrokeAlign::Outside,
                _ => StrokeAlign::Center,
            },
            antialias: true,
        };
        self.app.tool = cshop_ui::tools::Tool::Shape;
        self.app.dispatch(Action::DrawShape {
            from: Vec2::new(x, y),
            to: Vec2::new(x + w, y + h),
            from_centre: false,
            constrain: false,
        });
        Ok(format!("drew a {kind_name} at ({x}, {y}), {w}x{h}"))
    }

    fn cmd_fill(&mut self, cmd: &Command) -> Result<String, String> {
        self.need_doc()?;
        let c = match cmd.args.first() {
            Some(v) => parse_color(v)?,
            None => cmd.color("color")?.ok_or("fill needs a colour")?,
        };
        self.app.foreground = c;
        self.app.dispatch(Action::fill_foreground(cmd.flag("preserve-transparency")));
        Ok(format!("filled with #{:02x}{:02x}{:02x}", c.r, c.g, c.b))
    }

    /// Lay a gradient across the layer, from one point to another.
    ///
    /// Colours carry their alpha, so `from=#00000000 to=#000000cc` is a wash
    /// that fades out — which is what decorative shading usually wants, and
    /// what a solid ramp cannot give.
    fn cmd_gradient(&mut self, cmd: &Command) -> Result<String, String> {
        use cshop_core::fill::{Gradient, GradientKind, GradientStop};
        self.need_doc()?;
        let x1 = cmd.arg_f32(0, "start x")?;
        let y1 = cmd.arg_f32(1, "start y")?;
        let x2 = cmd.arg_f32(2, "end x")?;
        let y2 = cmd.arg_f32(3, "end y")?;

        let from = cmd.color("from")?.unwrap_or(Rgba8::BLACK);
        let to = cmd.color("to")?.unwrap_or(Rgba8::TRANSPARENT);
        let kind = match cmd.opt("style").unwrap_or("linear") {
            "linear" => GradientKind::Linear,
            "radial" => GradientKind::Radial,
            "angle" => GradientKind::Angle,
            "reflected" => GradientKind::Reflected,
            "diamond" => GradientKind::Diamond,
            other => return Err(format!("gradient style {other:?}")),
        };
        let mode = match cmd.opt("blend") {
            Some(want) => cshop_core::blend::BlendMode::all()
                .find(|m| m.name().eq_ignore_ascii_case(want))
                .ok_or_else(|| format!("no blend mode called {want:?}"))?,
            None => cshop_core::blend::BlendMode::Normal,
        };

        self.app.gradient = Gradient {
            stops: vec![
                GradientStop { position: 0.0, color: from },
                GradientStop { position: 1.0, color: to },
            ],
            kind,
            reverse: cmd.flag("reverse"),
            opacity: cmd.f32("opacity")?.unwrap_or(1.0),
            mode,
            dither: !matches!(cmd.opt("dither"), Some("false" | "no" | "0" | "off")),
        };
        self.app.gradient_drag = Some((Vec2::new(x1, y1), Vec2::new(x2, y2)));
        self.app.commit_gradient();
        Ok(format!("laid a {} gradient from ({x1}, {y1}) to ({x2}, {y2})", cmd.opt("style").unwrap_or("linear")))
    }

    fn cmd_select(&mut self, cmd: &Command) -> Result<String, String> {
        use cshop_core::selection::{Rectf, Selection};
        self.need_doc()?;
        match cmd.args.first().map(|s| s.as_str()) {
            Some("all") | None => {
                self.app.dispatch(Action::SelectAll);
                Ok("selected everything".into())
            }
            Some("none") => {
                self.app.dispatch(Action::Deselect);
                Ok("deselected".into())
            }
            Some(_) => {
                let x = cmd.arg_f32(0, "x")?;
                let y = cmd.arg_f32(1, "y")?;
                let w = cmd.arg_f32(2, "width")?;
                let h = cmd.arg_f32(3, "height")?;
                let (dw, dh) = self
                    .app
                    .doc()
                    .map(|v| (v.doc.width, v.doc.height))
                    .ok_or("no document")?;
                let mut sel = Selection::from_rect(
                    dw,
                    dh,
                    Rectf::from_points(Vec2::new(x, y), Vec2::new(x + w, y + h)),
                    true,
                );
                if let Some(f) = cmd.f32("feather")? {
                    if f > 0.0 {
                        sel.feather(f);
                    }
                }
                self.app.dispatch(Action::SetSelection(Box::new(sel), "Rectangular Marquee"));
                Ok(format!("selected ({x}, {y}) {w}x{h}"))
            }
        }
    }

    fn cmd_write(&mut self, cmd: &Command) -> Result<String, String> {
        self.need_doc()?;
        let given = cmd.args.first().ok_or("export needs a path")?;
        let path = resolve(&self.base, given);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let composite = self.composite()?;
        let doc = self.app.doc().ok_or("no document")?.doc.clone();
        cshop_io::save_document(&path, &doc, &composite)
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
        self.report.outputs.push(path.display().to_string());
        Ok(format!("wrote {}", path.display()))
    }

    fn cmd_info(&mut self, _cmd: &Command) -> Result<String, String> {
        self.need_doc()?;
        let view = self.app.doc().ok_or("no document")?;
        let n = view.doc.tree.len();
        let fact = format!("{}x{}, {n} layers", view.doc.width, view.doc.height);
        self.report.facts.push(("document".into(), fact.clone()));
        Ok(fact)
    }
}

impl Runner {
    fn cmd_effect(&mut self, cmd: &Command) -> Result<String, String> {
        self.need_doc()?;
        let name = cmd.args.first().map(|s| s.as_str()).unwrap_or("");
        let id = self
            .app
            .doc()
            .and_then(|v| v.doc.active)
            .ok_or("there is no active layer to style")?;
        let mut fx = self
            .app
            .doc()
            .and_then(|v| v.doc.tree.get(id))
            .map(|l| l.effects)
            .unwrap_or_default();
        if !fx.enabled {
            fx = LayerEffects { enabled: true, ..LayerEffects::new() };
        }

        let opacity = cmd.f32("opacity")?;
        let color = cmd.color("color")?;
        let size = cmd.f32("size")?;
        let mode = match cmd.opt("blend") {
            Some(want) => Some(
                cshop_core::blend::BlendMode::all()
                    .find(|m| m.name().eq_ignore_ascii_case(want))
                    .ok_or_else(|| format!("no blend mode called {want:?}"))?,
            ),
            None => None,
        };

        match name {
            "drop-shadow" | "inner-shadow" => {
                let mut s = if name == "drop-shadow" {
                    fx.drop_shadow.unwrap_or_default()
                } else {
                    fx.inner_shadow.unwrap_or(Shadow { distance: 5.0, size: 5.0, ..Default::default() })
                };
                if let Some(v) = color { s.color = v; }
                if let Some(v) = opacity { s.opacity = v; }
                if let Some(v) = size { s.size = v; }
                if let Some(v) = cmd.f32("distance")? { s.distance = v; }
                if let Some(v) = cmd.f32("angle")? { s.angle = v; s.use_global_light = false; }
                if let Some(v) = cmd.f32("spread")? { s.spread = v; }
                if let Some(v) = cmd.f32("choke")? { s.spread = v; }
                if let Some(m) = mode { s.mode = m; }
                if name == "drop-shadow" { fx.drop_shadow = Some(s) } else { fx.inner_shadow = Some(s) }
            }
            "outer-glow" | "inner-glow" => {
                let mut g = if name == "outer-glow" {
                    fx.outer_glow.unwrap_or_default()
                } else {
                    fx.inner_glow.unwrap_or_default()
                };
                if let Some(v) = color { g.color = v; }
                if let Some(v) = opacity { g.opacity = v; }
                if let Some(v) = size { g.size = v; }
                if let Some(v) = cmd.f32("spread")?.or(cmd.f32("choke")?) { g.spread = v; }
                if cmd.opt("source") == Some("center") { g.source = GlowSource::Center; }
                if let Some(m) = mode { g.mode = m; }
                if name == "outer-glow" { fx.outer_glow = Some(g) } else { fx.inner_glow = Some(g) }
            }
            "stroke" => {
                let mut s = fx.stroke.unwrap_or_default();
                if let Some(v) = color { s.color = v; }
                if let Some(v) = opacity { s.opacity = v; }
                if let Some(v) = size { s.size = v; }
                if let Some(p) = cmd.opt("position") {
                    s.position = match p {
                        "inside" => StrokePosition::Inside,
                        "center" | "centre" => StrokePosition::Center,
                        "outside" => StrokePosition::Outside,
                        other => return Err(format!("stroke position {other:?}")),
                    };
                }
                if let Some(m) = mode { s.mode = m; }
                fx.stroke = Some(s);
            }
            "color-overlay" => {
                let mut o = fx.color_overlay.unwrap_or_default();
                if let Some(v) = color { o.color = v; }
                if let Some(v) = opacity { o.opacity = v; }
                if let Some(m) = mode { o.mode = m; }
                fx.color_overlay = Some(o);
            }
            "gradient-overlay" => {
                let mut g = fx.gradient_overlay.unwrap_or_default();
                if let Some(v) = cmd.color("from")? { g.from = v; }
                if let Some(v) = cmd.color("to")? { g.to = v; }
                if let Some(v) = opacity { g.opacity = v; }
                if let Some(v) = cmd.f32("angle")? { g.angle = v; }
                if let Some(v) = cmd.f32("scale")? { g.scale = v; }
                g.reverse = cmd.flag("reverse") || g.reverse;
                if let Some(k) = cmd.opt("style") {
                    use cshop_core::fill::GradientKind as K;
                    g.kind = match k {
                        "linear" => K::Linear,
                        "radial" => K::Radial,
                        "angle" => K::Angle,
                        "reflected" => K::Reflected,
                        "diamond" => K::Diamond,
                        other => return Err(format!("gradient style {other:?}")),
                    };
                }
                if let Some(m) = mode { g.mode = m; }
                fx.gradient_overlay = Some(g);
            }
            "pattern-overlay" => {
                let mut o = fx.pattern_overlay.unwrap_or_default();
                if let Some(v) = color { o.color = v; }
                if let Some(v) = cmd.color("background")? { o.background = v; }
                if let Some(v) = opacity { o.opacity = v; }
                if let Some(v) = cmd.f32("scale")? { o.scale = v; }
                if let Some(v) = cmd.f32("angle")? { o.angle = v; }
                if let Some(k) = cmd.opt("pattern") {
                    o.kind = PatternKind::ALL
                        .into_iter()
                        .find(|p| p.name().eq_ignore_ascii_case(k) || p.name().replace(' ', "-").eq_ignore_ascii_case(k))
                        .ok_or_else(|| format!("no pattern called {k:?}"))?;
                }
                if let Some(m) = mode { o.mode = m; }
                fx.pattern_overlay = Some(o);
            }
            "satin" => {
                let mut s = fx.satin.unwrap_or_default();
                if let Some(v) = color { s.color = v; }
                if let Some(v) = opacity { s.opacity = v; }
                if let Some(v) = size { s.size = v; }
                if let Some(v) = cmd.f32("distance")? { s.distance = v; }
                if let Some(v) = cmd.f32("angle")? { s.angle = v; }
                if let Some(m) = mode { s.mode = m; }
                fx.satin = Some(s);
            }
            "bevel" | "emboss" => {
                let mut b = fx.bevel.unwrap_or_default();
                if let Some(v) = size { b.size = v; }
                if let Some(v) = cmd.f32("depth")? { b.depth = v; }
                if let Some(v) = cmd.f32("soften")? { b.soften = v; }
                if let Some(v) = cmd.f32("angle")? { b.angle = v; b.use_global_light = false; }
                if let Some(v) = cmd.f32("altitude")? { b.altitude = v; }
                if let Some(st) = cmd.opt("style") {
                    b.style = match st {
                        "inner" => BevelStyle::Inner,
                        "outer" => BevelStyle::Outer,
                        "emboss" => BevelStyle::Emboss,
                        "pillow" => BevelStyle::Pillow,
                        other => return Err(format!("bevel style {other:?}")),
                    };
                } else if name == "emboss" {
                    b.style = BevelStyle::Emboss;
                }
                fx.bevel = Some(b);
            }
            "none" | "clear" => {
                self.app.dispatch(Action::ClearLayerEffects(id));
                return Ok("cleared the layer's effects".into());
            }
            other => {
                return Err(format!(
                    "unknown effect {other:?}. Available: drop-shadow, inner-shadow, outer-glow, \
                     inner-glow, bevel, emboss, satin, color-overlay, gradient-overlay, \
                     pattern-overlay, stroke, none"
                ))
            }
        }

        self.app.dispatch(Action::SetLayerEffects(id, Box::new(fx)));
        Ok(format!("applied {name}"))
    }

    fn cmd_filter(&mut self, cmd: &Command) -> Result<String, String> {
        use cshop_core::filters::Filter;
        self.need_doc()?;
        let name = cmd.args.first().map(|s| s.as_str()).unwrap_or("");
        let r = |d: f32| -> Result<f32, String> { Ok(cmd.f32("radius")?.unwrap_or(d)) };
        let filter = match name {
            "gaussian-blur" | "blur" => Filter::GaussianBlur { radius: r(4.0)? },
            "box-blur" => Filter::BoxBlur { radius: r(4.0)? },
            "motion-blur" => Filter::MotionBlur {
                angle: cmd.f32("angle")?.unwrap_or(0.0),
                distance: cmd.f32("distance")?.unwrap_or(20.0),
            },
            "sharpen" => Filter::Sharpen { amount: cmd.f32("amount")?.unwrap_or(1.0) },
            "unsharp-mask" => Filter::UnsharpMask {
                amount: cmd.f32("amount")?.unwrap_or(1.0),
                radius: r(2.0)?,
                threshold: cmd.f32("threshold")?.unwrap_or(0.0),
            },
            "add-noise" => Filter::AddNoise {
                amount: cmd.f32("amount")?.unwrap_or(0.1),
                monochromatic: cmd.flag("monochromatic"),
                gaussian: true,
                seed: 1,
            },
            "high-pass" => Filter::HighPass { radius: r(4.0)? },
            "find-edges" => Filter::FindEdges,
            "median" => Filter::Median { radius: cmd.u32("radius")?.unwrap_or(2) },
            "mosaic" => Filter::Mosaic { size: cmd.u32("size")?.unwrap_or(10) },
            other => {
                return Err(format!(
                    "unknown filter {other:?}. Available: gaussian-blur, box-blur, motion-blur, \
                     sharpen, unsharp-mask, add-noise, high-pass, find-edges, median, mosaic"
                ))
            }
        };
        self.app.dispatch(Action::ApplyFilter(Box::new(filter)));
        Ok(format!("applied {name}"))
    }

    fn cmd_adjust(&mut self, cmd: &Command) -> Result<String, String> {
        use cshop_core::adjust::Adjustment;
        self.need_doc()?;
        let name = cmd.args.first().map(|s| s.as_str()).unwrap_or("");
        let adjustment = match name {
            "brightness-contrast" => Adjustment::BrightnessContrast {
                brightness: cmd.f32("brightness")?.unwrap_or(0.0),
                contrast: cmd.f32("contrast")?.unwrap_or(0.0),
            },
            "hue-saturation" => Adjustment::HueSaturation {
                hue: cmd.f32("hue")?.unwrap_or(0.0),
                saturation: cmd.f32("saturation")?.unwrap_or(0.0),
                lightness: cmd.f32("lightness")?.unwrap_or(0.0),
                colorize: cmd.flag("colorize"),
            },
            "vibrance" => Adjustment::Vibrance {
                vibrance: cmd.f32("vibrance")?.unwrap_or(0.0),
                saturation: cmd.f32("saturation")?.unwrap_or(0.0),
            },
            "exposure" => Adjustment::Exposure {
                exposure: cmd.f32("exposure")?.unwrap_or(0.0),
                offset: cmd.f32("offset")?.unwrap_or(0.0),
                gamma: cmd.f32("gamma")?.unwrap_or(1.0),
            },
            "invert" => Adjustment::Invert,
            "posterize" => Adjustment::Posterize { levels: cmd.u32("levels")?.unwrap_or(8) },
            "threshold" => Adjustment::Threshold { level: cmd.f32("level")?.unwrap_or(0.5) },
            "black-and-white" => Adjustment::BlackAndWhite {
                weights: [0.4, 0.6, 0.4, 0.6, 0.2, 0.8],
                tint: cmd.color("tint")?,
            },
            other => {
                return Err(format!(
                    "unknown adjustment {other:?}. Available: brightness-contrast, \
                     hue-saturation, vibrance, exposure, invert, posterize, threshold, \
                     black-and-white"
                ))
            }
        };
        // As a layer when asked, so it stays editable; otherwise baked in.
        if cmd.flag("as-layer") {
            self.app.dispatch(Action::AddAdjustmentLayer(Box::new(adjustment)));
            Ok(format!("added a {name} adjustment layer"))
        } else {
            self.app.dispatch(Action::ApplyAdjustment(Box::new(adjustment)));
            Ok(format!("applied {name}"))
        }
    }

    fn cmd_layer(&mut self, cmd: &Command) -> Result<String, String> {
        self.need_doc()?;
        let what = cmd.args.first().map(|s| s.as_str()).unwrap_or("");
        let action = match what {
            "new" => Action::NewLayer,
            "group" => Action::NewGroup,
            "duplicate" => Action::DuplicateLayer,
            "delete" => Action::DeleteLayer,
            "merge-down" => Action::MergeDown,
            "flatten" => Action::FlattenImage,
            "rasterize" => Action::RasterizeLayer,
            "select" => {
                let n = cmd.arg_f32(1, "layer index")? as usize;
                let id = self
                    .app
                    .doc()
                    .and_then(|v| v.doc.tree.iter_all().get(n).copied())
                    .ok_or_else(|| format!("there is no layer {n}"))?;
                Action::SelectLayer(id)
            }
            other => {
                return Err(format!(
                    "unknown layer command {other:?}. Available: new, group, duplicate, delete, \
                     merge-down, flatten, rasterize, select <index>"
                ))
            }
        };
        self.app.dispatch(action);
        Ok(format!("layer {what}"))
    }

    fn cmd_set(&mut self, cmd: &Command) -> Result<String, String> {
        use cshop_core::history::LayerProperty;
        self.need_doc()?;
        let id = self.app.doc().and_then(|v| v.doc.active).ok_or("no active layer")?;
        let mut done = Vec::new();
        if let Some(v) = cmd.f32("opacity")? {
            self.app.dispatch(Action::SetLayerProperty(id, LayerProperty::Opacity(v.clamp(0.0, 1.0))));
            done.push(format!("opacity {v}"));
        }
        if let Some(v) = cmd.f32("fill-opacity")? {
            self.app
                .dispatch(Action::SetLayerProperty(id, LayerProperty::FillOpacity(v.clamp(0.0, 1.0))));
            done.push(format!("fill opacity {v}"));
        }
        if let Some(v) = cmd.opt("name") {
            self.app.dispatch(Action::SetLayerProperty(id, LayerProperty::Name(v.to_string())));
            done.push(format!("name {v:?}"));
        }
        if let Some(want) = cmd.opt("blend") {
            let mode = cshop_core::blend::BlendMode::all()
                .find(|m| m.name().eq_ignore_ascii_case(want))
                .ok_or_else(|| format!("no blend mode called {want:?}"))?;
            self.app.dispatch(Action::SetLayerProperty(id, LayerProperty::Blend(mode)));
            done.push(format!("blend {}", mode.name()));
        }
        if done.is_empty() {
            return Err("set takes opacity=, fill-opacity=, name= or blend=".into());
        }
        Ok(format!("set {}", done.join(", ")))
    }

    fn cmd_move(&mut self, cmd: &Command) -> Result<String, String> {
        self.need_doc()?;
        let dx = cmd.arg_f32(0, "dx")? as i32;
        let dy = cmd.arg_f32(1, "dy")? as i32;
        self.app.dispatch(Action::NudgeLayer(dx, dy));
        Ok(format!("moved by ({dx}, {dy})"))
    }

    fn cmd_order(&mut self, cmd: &Command) -> Result<String, String> {
        self.need_doc()?;
        let by = match cmd.args.first().map(|s| s.as_str()).unwrap_or("") {
            "top" => i32::MAX,
            "bottom" => i32::MIN,
            "up" => 1,
            "down" => -1,
            other => return Err(format!("order takes top, bottom, up or down, not {other:?}")),
        };
        self.app.dispatch(Action::ReorderActiveLayer(by));
        Ok("reordered".into())
    }
}
