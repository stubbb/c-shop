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
use std::path::{Component, Path, PathBuf};

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

    /// A whole-number option.
    ///
    /// Accepts a decimal and rounds it. Style arithmetic works in floats, so
    /// `radius={min*0.0045}` arrives here as `3.0465`; rejecting that would
    /// mean every integer option had to be written as a literal, which is
    /// exactly what makes a style stop scaling with the image.
    pub fn u32(&self, key: &str) -> Result<Option<u32>, String> {
        match self.opt(key) {
            None => Ok(None),
            Some(v) => match v.parse::<u32>() {
                Ok(n) => Ok(Some(n)),
                Err(_) => match v.parse::<f64>() {
                    Ok(n) if n.is_finite() && n >= 0.0 => Ok(Some(n.round() as u32)),
                    _ => Err(format!("{key}={v:?} is not a whole number")),
                },
            },
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

/// Keeping a script inside a directory.
///
/// The script language can open and write files. Run from a terminal that is
/// the point; served over a socket it is a filesystem primitive handed to
/// whoever can reach the port, so the server resolves every path a script
/// names through one of these instead of through [`resolve`].
///
/// Two checks, because either alone can be walked around. The lexical one
/// refuses `..` and absolute paths, which stops the obvious traversal. The
/// canonical one resolves the deepest part of the path that actually exists
/// and confirms it is still inside the root, which stops a symlink planted in
/// the workspace from pointing out of it. Only the pair is worth anything: the
/// lexical check cannot see a symlink, and the canonical check cannot see a
/// file that does not exist yet — which every export target is.
#[derive(Clone, Debug)]
pub struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    /// Take a directory as the root, creating it if it is not there.
    ///
    /// The root is canonicalised once, here, so that every later comparison is
    /// against a path with no symlinks or `.` left in it.
    pub fn new(root: &Path) -> Result<Sandbox, String> {
        std::fs::create_dir_all(root)
            .map_err(|e| format!("could not make the workspace {}: {e}", root.display()))?;
        let root = root
            .canonicalize()
            .map_err(|e| format!("could not resolve the workspace {}: {e}", root.display()))?;
        Ok(Sandbox { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a path a script gave, or say why it is refused.
    ///
    /// The messages name the rule rather than only refusing, because the
    /// caller is usually a program that can correct itself if it is told what
    /// the shape of an acceptable path is.
    pub fn resolve(&self, given: &str) -> Result<PathBuf, String> {
        if given.is_empty() {
            return Err("an empty path".to_string());
        }
        if given.starts_with('~') {
            return Err(format!(
                "{given:?} starts at a home directory; paths must be relative to the workspace"
            ));
        }

        let candidate = Path::new(given);
        if candidate.is_absolute() {
            return Err(format!(
                "{given:?} is absolute; paths must be relative to the workspace"
            ));
        }

        // Lexical: build the path a component at a time, refusing anything
        // that could climb. `.` is dropped; `..` is refused outright rather
        // than popped, so a path cannot climb out and back in through a
        // symlink on the way.
        let mut out = self.root.clone();
        for component in candidate.components() {
            match component {
                Component::Normal(part) => out.push(part),
                Component::CurDir => {}
                Component::ParentDir => {
                    return Err(format!("{given:?} contains \"..\", which is not allowed"))
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(format!("{given:?} is absolute; paths must be relative"))
                }
            }
        }

        // Canonical: the deepest ancestor that exists must still be inside the
        // root. A file that does not exist yet has no canonical form, so the
        // check climbs to the nearest directory that does.
        let mut existing = out.as_path();
        loop {
            match existing.parent() {
                _ if existing.exists() => break,
                Some(parent) => existing = parent,
                None => return Err(format!("{given:?} has no part that exists")),
            }
        }
        let real = existing
            .canonicalize()
            .map_err(|e| format!("could not resolve {}: {e}", existing.display()))?;
        if !real.starts_with(&self.root) {
            return Err(format!("{given:?} resolves outside the workspace"));
        }

        Ok(out)
    }

    /// The part of a path a caller should see, which is the part inside the
    /// workspace. Absolute paths on this machine are not the client's business.
    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root).unwrap_or(path).display().to_string()
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
    /// Set when the script is not being run by whoever owns the machine. Every
    /// path the script names is then resolved through it — see [`Sandbox`].
    sandbox: Option<Sandbox>,
    report: Report,
    /// How deep inside styles we are. A style is script, and script can apply
    /// a style, so a style that applies itself would otherwise not stop.
    depth: usize,
    /// Prefixes the steps a style runs, so a failure inside one says which.
    trail: Vec<String>,
    /// What `detect` last found, so `segment` can follow it without repeating
    /// the prompt.
    detected: Vec<cshop_ui::vision::Found>,
}

/// Run a script and return what happened.
///
/// `base` is the directory relative paths resolve against.
pub fn run(source: &str, base: &Path) -> Result<Report, String> {
    let gpu = GpuContext::headless().map_err(|e| format!("no GPU: {e}"))?;
    let mut runner = Runner::new(gpu, base.to_path_buf(), None);
    Ok(runner.run(source))
}

impl Runner {
    /// A runner that outlives one script.
    ///
    /// The one-shot [`run`] builds a GPU context per call, which is right for
    /// a command line and wrong for a server: the context costs far more than
    /// most scripts do, and a caller working on one picture over several calls
    /// needs the document to still be there on the next one.
    pub fn new(gpu: GpuContext, base: PathBuf, sandbox: Option<Sandbox>) -> Runner {
        Runner {
            app: CShopApp::new(gpu.clone()),
            gpu,
            base,
            sandbox,
            report: Report::default(),
            depth: 0,
            trail: Vec::new(),
            detected: Vec::new(),
        }
    }

    /// Run a script over whatever this runner already holds, and report.
    pub fn run(&mut self, source: &str) -> Report {
        self.report = Report::default();
        self.depth = 0;
        self.trail.clear();
        self.execute(source);
        self.finish()
    }

    /// Where a path a script named actually points.
    ///
    /// The only route from a script to the filesystem, so that confining one
    /// is a matter of setting a field rather than of remembering to check at
    /// each of the places that open a file.
    fn path(&self, given: &str) -> Result<PathBuf, String> {
        match &self.sandbox {
            Some(sandbox) => sandbox.resolve(given),
            None => Ok(resolve(&self.base, given)),
        }
    }

    /// A path as the caller should see it — relative, when there is a
    /// workspace, since absolute paths on this machine are not their business.
    fn shown(&self, path: &Path) -> String {
        match &self.sandbox {
            Some(sandbox) => sandbox.relative(path),
            None => path.display().to_string(),
        }
    }

    /// The current composite, as a PNG.
    pub fn composite_png(&mut self) -> Result<Vec<u8>, String> {
        let pixels = self.composite()?;
        cshop_io::encode(&pixels, cshop_io::ImageFormat::Png, 100)
            .map_err(|e| format!("could not encode a PNG: {e}"))
    }

    /// The current composite as a PNG, scaled so its longest side is `fit`.
    ///
    /// Scaled before encoding rather than after, so the cost is paid once and
    /// the bytes that come back are the bytes that were asked for.
    pub fn composite_png_fit(&mut self, fit: u32) -> Result<Vec<u8>, String> {
        let pixels = self.composite()?;
        let (w, h) = (pixels.width(), pixels.height());
        let longest = w.max(h);
        let pixels = if longest > fit && fit > 0 {
            let scale = fit as f32 / longest as f32;
            cshop_core::resample::resize(
                &pixels,
                ((w as f32 * scale).round() as u32).max(1),
                ((h as f32 * scale).round() as u32).max(1),
                cshop_core::resample::Resampling::Lanczos3,
            )
        } else {
            pixels
        };
        cshop_io::encode(&pixels, cshop_io::ImageFormat::Png, 100)
            .map_err(|e| format!("could not encode a PNG: {e}"))
    }

    /// The document's size, for a caller that wants it without a script.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.app.doc().map(|v| (v.doc.width, v.doc.height))
    }

    /// Name and size, for listing what a session is holding.
    pub fn document_summary(&self) -> Option<(String, u32, u32)> {
        self.app.doc().map(|v| (v.doc.name.clone(), v.doc.width, v.doc.height))
    }

    pub fn layer_count(&self) -> usize {
        self.app.doc().map(|v| v.doc.tree.len()).unwrap_or(0)
    }

    pub fn has_document(&self) -> bool {
        self.app.doc().is_some()
    }
}

impl Runner {
    fn execute(&mut self, source: &str) {
        for parsed in parse(source) {
            match parsed {
                Err((line, raw, why)) => {
                    let raw = if self.trail.is_empty() {
                        raw
                    } else {
                        format!("{}: {}", self.trail.join(" > "), raw)
                    };
                    self.report.steps.push(Step { line, command: raw, ok: false, note: why });
                }
                Ok(cmd) => {
                    let raw = if self.trail.is_empty() {
                        cmd.raw.clone()
                    } else {
                        format!("{}: {}", self.trail.join(" > "), cmd.raw)
                    };
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
    fn finish(&mut self) -> Report {
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
        std::mem::take(&mut self.report)
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
            "place" => self.cmd_place(cmd),
            "resize" => self.cmd_resize(cmd),
            "text" => self.cmd_text(cmd),
            "measure" => self.cmd_measure(cmd),
            "shape" => self.cmd_shape(cmd),
            "path" => self.cmd_path(cmd),
            "combine" => self.cmd_combine(cmd),
            "fill" => self.cmd_fill(cmd),
            "style" => self.cmd_style(cmd),
            "gradient" => self.cmd_gradient(cmd),
            "select" => self.cmd_select(cmd),
            "detect" => self.cmd_detect(cmd),
            "segment" => self.cmd_segment(cmd),
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
                 place, select, gradient, style, effect, filter, adjust, layer, set, move, \
                 order, info, export, save"
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
        let path = self.path(given)?;
        let doc = cshop_io::load_document(&path)
            .map_err(|e| format!("could not open {}: {e}", self.shown(&path)))?;
        let (w, h, n) = (doc.width, doc.height, doc.tree.len());
        let shown = self.shown(&path);
        self.app.open_document(doc);
        Ok(format!("opened {shown} ({w}x{h}, {n} layer{})", if n == 1 { "" } else { "s" }))
    }

    /// Bring an image in as a new layer above the active one.
    ///
    /// `open` replaces the document; this composites into it, which is what
    /// blending one picture over another needs — and what a style cannot do
    /// for itself, since a style that flattens has nothing left to blend with.
    ///
    /// With no path it re-places the file the document was opened from. That
    /// is what lets a style lay an original back over its own treatment
    /// without being told where the original lives.
    fn cmd_place(&mut self, cmd: &Command) -> Result<String, String> {
        self.need_doc()?;
        let path = match cmd.args.first() {
            Some(given) => self.path(given)?,
            None => self
                .app
                .doc()
                .and_then(|v| v.doc.path.clone())
                .ok_or("place needs a path; this document was not opened from a file")?,
        };
        let pixels = cshop_io::load(&path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let (w, h) = (pixels.width(), pixels.height());
        // Positioned where asked, or at the origin.
        let x = cmd.f32("x")?.unwrap_or(0.0) as i32;
        let y = cmd.f32("y")?.unwrap_or(0.0) as i32;

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Placed".into());
        let Some(view) = self.app.doc_mut() else { return Err("no document".into()) };
        let id = view.doc.tree.alloc_id();
        let mut layer = cshop_core::layer::Layer::raster(id, name.clone(), pixels);
        layer.offset = (x, y);
        let pos = view
            .doc
            .active
            .and_then(|a| view.doc.tree.position(a))
            .map(|p| cshop_core::tree::LayerPos { parent: p.parent, index: p.index + 1 })
            .unwrap_or(cshop_core::tree::LayerPos {
                parent: None,
                index: view.doc.tree.root().len(),
            });
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::AddLayer::new(layer, pos, "Place")),
        );
        view.mark_dirty(dirty);
        view.invalidate();
        Ok(format!("placed {name} ({w}x{h}) at ({x}, {y})"))
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

    /// A Bézier path, given as a run of points.
    ///
    /// `path "M 10 10 C 40 0 80 0 110 10 L 110 90 Z"` — a deliberately small
    /// subset of the usual path grammar: move, line, cubic, close. Enough to
    /// describe a shape without inventing a second one.
    fn cmd_path(&mut self, cmd: &Command) -> Result<String, String> {
        use cshop_core::path::PathShape;
        use cshop_core::shape::ShapeKind;
        self.need_doc()?;
        let data = cmd.args.first().ok_or("path needs its points, as \"M x y L x y ...\"")?;
        let subpaths = parse_path_data(data)?;
        if subpaths.is_empty() {
            return Err("that path has no points in it".into());
        }
        let shape = PathShape::new(subpaths);
        let open = ShapeKind::Path(shape.clone()).is_open();

        let mut style = cshop_core::shape::ShapeStyle {
            fill: cmd
                .color("fill")?
                .or(Some(Rgba8::BLACK))
                .filter(|_| cmd.opt("fill") != Some("none")),
            stroke: cmd.color("stroke")?,
            stroke_width: cmd.f32("stroke-width")?.unwrap_or(2.0),
            stroke_align: cshop_core::shape::StrokeAlign::Center,
            antialias: true,
        };
        if open {
            // Nothing to fill, so the colour has to go on the stroke.
            style.stroke = style.stroke.or(style.fill);
            style.fill = None;
        }
        let parts = shape.parts.len();
        let anchors = shape.anchors().count();
        self.app.add_path_layer(shape, style, "Path");
        Ok(format!(
            "drew a path of {anchors} anchor{} in {parts} contour group{}",
            if anchors == 1 { "" } else { "s" },
            if parts == 1 { "" } else { "s" }
        ))
    }

    /// Combine the shape layers named by index into one path.
    fn cmd_combine(&mut self, cmd: &Command) -> Result<String, String> {
        use cshop_core::path::BoolOp;
        self.need_doc()?;
        let name = cmd.args.first().map(|s| s.as_str()).unwrap_or("union");
        let op = BoolOp::all()
            .into_iter()
            .find(|o| o.name().eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                format!(
                    "no operation called {name:?}. There are: {}",
                    BoolOp::all().map(|o| o.name()).join(", ")
                )
            })?;

        // Which layers, by the index `info` reports. Defaults to all of them,
        // since combining every shape in a document is the common case in a
        // script that just drew them.
        let order = self
            .app
            .doc()
            .map(|v| v.doc.tree.iter_all())
            .unwrap_or_default();
        let chosen: Vec<_> = match cmd.opt("layers") {
            None => order
                .iter()
                .copied()
                .filter(|id| {
                    self.app.doc().and_then(|v| v.doc.tree.get(*id)).is_some_and(|l| l.shape().is_some())
                })
                .collect(),
            Some(list) => {
                let mut out = Vec::new();
                for part in list.split(',') {
                    let i: usize = part
                        .trim()
                        .parse()
                        .map_err(|_| format!("{part:?} is not a layer index"))?;
                    out.push(*order.get(i).ok_or_else(|| format!("no layer {i}"))?);
                }
                out
            }
        };
        if chosen.len() < 2 {
            return Err("combine needs two or more shape layers".into());
        }

        if let Some(view) = self.app.doc_mut() {
            view.doc.selected_layers = chosen.clone();
        }
        self.app.dispatch(Action::CombineShapes(op));
        Ok(format!("combined {} shapes with {}", chosen.len(), op.name()))
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

    /// Apply a named style, with its parameters overridden by the options
    /// given here.
    fn cmd_style(&mut self, cmd: &Command) -> Result<String, String> {
        const MAX_DEPTH: usize = 8;
        let name = cmd.args.first().ok_or("style needs a name")?.clone();
        if self.depth >= MAX_DEPTH {
            return Err(format!(
                "styles are nested {MAX_DEPTH} deep at {name:?}; is one applying itself?"
            ));
        }
        let (path, style) = find_style(&self.base, &name)?;

        // Defaults first, then whatever the caller passed, so an override
        // wins and a name the style does not declare is caught rather than
        // ignored.
        let mut values = style.params.clone();

        // The size of what we are working on, so a style can scale itself
        // rather than being written for one image. Bound underneath the
        // style's own parameters, so a style that wants to declare its own
        // `width` still can.
        if let Some(doc) = self.app.doc() {
            let (w, h) = (doc.doc.width as f32, doc.doc.height as f32);
            for (name, value) in [
                ("width", w),
                ("height", h),
                ("min", w.min(h)),
                ("max", w.max(h)),
                ("cx", w / 2.0),
                ("cy", h / 2.0),
            ] {
                if !values.iter().any(|(k, _)| k == name) {
                    values.push((name.to_string(), format!("{value}")));
                }
            }
        }

        for (k, v) in &cmd.opts {
            match values.iter_mut().find(|(name, _)| name == k) {
                Some(slot) => slot.1 = v.clone(),
                None => {
                    return Err(format!(
                        "{name:?} has no parameter {k:?}; it takes: {}",
                        if style.params.is_empty() {
                            "none".to_string()
                        } else {
                            style.params.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ")
                        }
                    ))
                }
            }
        }
        let body = substitute(&style.body, &values)?;

        // A style's own relative paths resolve against where it lives.
        let inner_base =
            path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| self.base.clone());
        let outer_base = std::mem::replace(&mut self.base, inner_base);
        self.depth += 1;
        self.trail.push(name.clone());
        self.execute(&body);
        self.trail.pop();
        self.depth -= 1;
        self.base = outer_base;

        let settings: Vec<String> =
            values.iter().map(|(k, v)| format!("{k}={v}")).collect();
        Ok(if settings.is_empty() {
            format!("applied style {name:?}")
        } else {
            format!("applied style {name:?} ({})", settings.join(" "))
        })
    }

    /// Resample the image.
    ///
    /// `resize 800 600` is exact; `resize fit=800` scales the longest side to
    /// 800 and keeps the proportions, which is what a script usually wants
    /// when it is handed a photograph of unknown size. `canvas` changes the
    /// frame instead of the picture, padding or cropping rather than scaling.
    fn cmd_resize(&mut self, cmd: &Command) -> Result<String, String> {
        use cshop_core::resample::Resampling;
        self.need_doc()?;
        let doc = self.app.doc().ok_or("no document")?;
        let (w0, h0) = (doc.doc.width, doc.doc.height);

        let (width, height) = if let Some(fit) = cmd.f32("fit")? {
            let scale = fit / w0.max(h0) as f32;
            (((w0 as f32 * scale).round() as u32).max(1), ((h0 as f32 * scale).round() as u32).max(1))
        } else if let Some(scale) = cmd.f32("scale")? {
            (((w0 as f32 * scale).round() as u32).max(1), ((h0 as f32 * scale).round() as u32).max(1))
        } else {
            let width = match cmd.args.first() {
                Some(_) => cmd.arg_f32(0, "width")? as u32,
                None => cmd.u32("width")?.ok_or("resize needs a width, or fit=, or scale=")?,
            };
            // Only one dimension given: keep the proportions.
            let height = match (cmd.args.get(1), cmd.u32("height")?) {
                (Some(_), _) => cmd.arg_f32(1, "height")? as u32,
                (None, Some(h)) => h,
                (None, None) => {
                    ((h0 as f32 * width as f32 / w0 as f32).round() as u32).max(1)
                }
            };
            (width.max(1), height.max(1))
        };

        let filter = match cmd.opt("filter").unwrap_or("lanczos") {
            "nearest" => Resampling::Nearest,
            "bilinear" | "linear" => Resampling::Bilinear,
            "bicubic" | "cubic" => Resampling::Bicubic,
            "lanczos" => Resampling::Lanczos3,
            other => return Err(format!("no resampling filter called {other:?}")),
        };

        if cmd.flag("canvas") {
            self.app.dispatch(Action::ResizeCanvas {
                width,
                height,
                anchor: cshop_ui::commands::Anchor::Center,
            });
        } else {
            self.app.dispatch(Action::ResizeImage { width, height, filter });
        }
        Ok(format!("resized {w0}x{h0} to {width}x{height}"))
    }

    /// The image the vision models should look at.
    ///
    /// Written out flattened, because the models take a file and the document
    /// may be a stack of layers that exists only in memory. Kept beside the
    /// mask so both are cleaned up together.
    fn vision_source(&mut self, dir: &Path) -> Result<(std::path::PathBuf, u32, u32), String> {
        let composite = self.composite()?;
        let (w, h) = (composite.width(), composite.height());
        std::fs::create_dir_all(dir).map_err(|e| format!("could not use {}: {e}", dir.display()))?;
        let path = dir.join("source.png");
        cshop_io::save(&path, &composite, 100)
            .map_err(|e| format!("could not write the image for the models: {e}"))?;
        Ok((path, w, h))
    }

    /// Somewhere to put the image and the mask while the models work.
    fn vision_dir(&self) -> std::path::PathBuf {
        cshop_ui::vision::scratch()
    }

    /// Find objects, and report what and where they are.
    ///
    /// The detector knows eighty kinds of thing and nothing else, so a picture
    /// of a mountain or a building comes back empty. That is not a failure of
    /// the picture — it is the list the model was trained on, and `segment`
    /// with a point works on anything.
    fn cmd_detect(&mut self, cmd: &Command) -> Result<String, String> {
        self.need_doc()?;
        let conf = cmd.f32("conf")?.unwrap_or(0.25);
        let classes = cmd.opt("class").unwrap_or("").to_string();
        let dir = self.vision_dir();
        let (image, _, _) = self.vision_source(&dir)?;

        let found = cshop_ui::vision::detect(&image, conf, &classes);
        let _ = std::fs::remove_dir_all(&dir);
        let found = found?;
        if found.is_empty() {
            let what = if classes.is_empty() {
                "nothing the detector knows".to_string()
            } else {
                format!("no {classes}")
            };
            self.report.facts.push(("detect".into(), what.clone()));
            return Ok(format!("found {what}"));
        }

        let mut lines = Vec::new();
        for f in &found {
            let line = format!(
                "{} {:.0}% at {:.0},{:.0} {:.0}x{:.0}",
                f.class,
                f.score * 100.0,
                f.box_[0],
                f.box_[1],
                f.width(),
                f.height()
            );
            self.report.facts.push((format!("detect {}", f.class), line.clone()));
            lines.push(line);
        }
        // Kept so `segment` with no prompt can use what was just found.
        self.detected = found;
        Ok(format!("found {}: {}", lines.len(), lines.join("; ")))
    }

    /// Cut something out, and leave it as the selection.
    ///
    /// The result is a selection rather than a new layer so that everything
    /// the editor already does with one — feather it, duplicate through it,
    /// fill it, invert it — applies without a second vocabulary.
    fn cmd_segment(&mut self, cmd: &Command) -> Result<String, String> {
        use cshop_ui::vision::Prompt;
        self.need_doc()?;
        let conf = cmd.f32("conf")?.unwrap_or(0.25);
        let feather = cmd.f32("feather")?.unwrap_or(0.0);

        let prompt = if let Some(name) = cmd.opt("class") {
            Prompt::Class(name.to_string())
        } else if let Some(b) = cmd.opt("box") {
            let v: Vec<f32> = b.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            if v.len() != 4 {
                return Err(format!("box={b:?} should be x0,y0,x1,y1"));
            }
            Prompt::Box([v[0], v[1], v[2], v[3]])
        } else if cmd.opt("point").is_some() || cmd.opt("not-point").is_some() {
            let points = |key: &str| -> Result<Vec<(f32, f32)>, String> {
                let Some(raw) = cmd.opt(key) else { return Ok(Vec::new()) };
                let mut out = Vec::new();
                // Several points as `point=x,y;x,y`, since one option cannot
                // repeat in this grammar.
                for part in raw.split(';').filter(|p| !p.trim().is_empty()) {
                    let v: Vec<f32> = part.split(',').filter_map(|p| p.trim().parse().ok()).collect();
                    if v.len() != 2 {
                        return Err(format!("{key}={part:?} should be x,y"));
                    }
                    out.push((v[0], v[1]));
                }
                Ok(out)
            };
            Prompt::Points(points("point")?, points("not-point")?)
        } else if let Some(found) = self.detected.first() {
            // Straight after `detect`, with nothing else said, segment what it
            // found — which is the whole point of running them in sequence.
            Prompt::Box(found.box_)
        } else {
            return Err(
                "segment needs class=, box=, point=, or a `detect` before it".to_string()
            );
        };

        let dir = self.vision_dir();
        let (image, w, h) = self.vision_source(&dir)?;
        let mask_path = dir.join("mask.png");
        let result = cshop_ui::vision::segment(&image, &prompt, &mask_path, conf);
        let _ = std::fs::remove_file(&image);
        let result = result.inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&dir);
        })?;

        let mask = cshop_io::load(&result.mask)
            .map_err(|e| format!("could not read the mask back: {e}"))?;
        let _ = std::fs::remove_dir_all(&dir);
        if mask.width() != w || mask.height() != h {
            return Err(format!(
                "the mask came back {}x{} for a {w}x{h} document",
                mask.width(),
                mask.height()
            ));
        }

        // The mask arrives as grey; its brightness is the coverage.
        let mut coverage = cshop_core::mask::MaskBuffer::hide_all(w, h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                coverage.set(x, y, mask.get(x, y).r);
            }
        }
        let mut selection = cshop_core::selection::Selection::from_mask(coverage);
        if feather > 0.0 {
            selection.feather(feather);
        }
        if selection.is_empty() {
            return Err("the segmenter returned an empty mask".into());
        }
        let bounds = selection.bounds();

        let Some(view) = self.app.doc_mut() else { return Err("no document".into()) };
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::SetSelection::new(Some(&selection), "Segment")),
        );
        view.mark_dirty(dirty);
        view.invalidate();

        let what = match (&result.detected, &prompt) {
            (Some(d), _) => format!("{} ({:.0}%)", d.class, d.score * 100.0),
            (None, Prompt::Class(c)) => c.clone(),
            _ => "the prompt".to_string(),
        };
        Ok(format!(
            "segmented {what}: {:.1}% of the image, {}x{} at {},{}, confidence {:.2}",
            result.coverage * 100.0,
            bounds.width(),
            bounds.height(),
            bounds.x0,
            bounds.y0,
            result.confidence
        ))
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
            Some("invert") | Some("inverse") => {
                self.app.dispatch(Action::InverseSelection);
                Ok("inverted the selection".into())
            }
            // Erase what is selected, which is how a background is removed
            // once the subject has been segmented.
            Some("clear") | Some("delete") => {
                self.app.dispatch(Action::ClearLayer);
                Ok("cleared the selection".into())
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
        let path = self.path(given)?;
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
            "crystallize" => Filter::Crystallize {
                size: cmd.u32("size")?.unwrap_or(12),
                seed: cmd.f32("seed")?.unwrap_or(1.0) as u64,
            },
            "emboss" => Filter::Emboss {
                angle: cmd.f32("angle")?.unwrap_or(135.0),
                height: cmd.f32("height")?.unwrap_or(3.0),
                amount: cmd.f32("amount")?.unwrap_or(1.0),
            },
            "solarize" => Filter::Solarize,
            "diffuse" => Filter::Diffuse {
                amount: cmd.u32("amount")?.unwrap_or(4),
                seed: cmd.f32("seed")?.unwrap_or(1.0) as u64,
            },
            "twirl" => Filter::Twirl { angle: cmd.f32("angle")?.unwrap_or(60.0) },
            "surface-blur" => Filter::SurfaceBlur {
                radius: r(8.0)?,
                threshold: cmd.f32("threshold")?.unwrap_or(0.1),
            },
            other => {
                return Err(format!(
                    "unknown filter {other:?}. Available: gaussian-blur, box-blur, motion-blur, \
                     surface-blur, sharpen, unsharp-mask, add-noise, high-pass, find-edges, \
                     median, mosaic, crystallize, emboss, solarize, diffuse, twirl"
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
            // The tonal three. Between them they are what most photographic
            // looks are actually made of.
            "levels" => {
                let channel = cshop_core::adjust::LevelsChannel {
                    input_black: cmd.f32("black")?.unwrap_or(0.0),
                    input_white: cmd.f32("white")?.unwrap_or(1.0),
                    gamma: cmd.f32("gamma")?.unwrap_or(1.0),
                    output_black: cmd.f32("out-black")?.unwrap_or(0.0),
                    output_white: cmd.f32("out-white")?.unwrap_or(1.0),
                };
                Adjustment::Levels { rgb: channel, channels: [Default::default(); 3] }
            }
            "gradient-map" => Adjustment::GradientMap {
                stops: vec![
                    cshop_core::fill::GradientStop {
                        position: 0.0,
                        color: cmd.color("from")?.unwrap_or(Rgba8::BLACK),
                    },
                    cshop_core::fill::GradientStop {
                        position: cmd.f32("midpoint")?.unwrap_or(0.5),
                        color: cmd.color("mid")?.unwrap_or_else(|| {
                            // No middle stop given: sit it halfway between the
                            // ends so the ramp is a plain two-colour one.
                            let a = cmd.color("from").ok().flatten().unwrap_or(Rgba8::BLACK);
                            let b = cmd.color("to").ok().flatten().unwrap_or(Rgba8::WHITE);
                            let mix = |x: u8, y: u8| ((x as u16 + y as u16) / 2) as u8;
                            Rgba8::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b), 255)
                        }),
                    },
                    cshop_core::fill::GradientStop {
                        position: 1.0,
                        color: cmd.color("to")?.unwrap_or(Rgba8::WHITE),
                    },
                ],
            },
            "photo-filter" => Adjustment::PhotoFilter {
                color: cmd.color("color")?.unwrap_or(Rgba8::opaque(236, 138, 0)),
                density: cmd.f32("density")?.unwrap_or(0.25),
                preserve_luminosity: !matches!(
                    cmd.opt("preserve-luminosity"),
                    Some("false" | "no" | "0" | "off")
                ),
            },
            other => {
                return Err(format!(
                    "unknown adjustment {other:?}. Available: brightness-contrast, levels, \
                     gradient-map, photo-filter, hue-saturation, vibrance, exposure, invert, \
                     posterize, threshold, black-and-white"
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
            // The selection-aware one: copies only what is selected onto a
            // new layer, which is how a segmented object is lifted out.
            "via-copy" | "from-selection" => Action::LayerViaCopy,
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

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

/// A named, parameterised script fragment.
///
/// A style is not a new kind of thing: it is the same commands, with holes in
/// it. That keeps one language to learn, lets a style be read and edited by
/// anyone who can read a script, and means a style can use anything the editor
/// can do the day it can do it.
///
/// ```text
/// # A bright pencil sketch.
/// param blur = 12
/// param contrast = 0.32
///
/// adjust black-and-white
/// layer duplicate
/// adjust invert
/// filter gaussian-blur radius={blur}
/// set blend="Color Dodge"
/// layer flatten
/// adjust brightness-contrast contrast={contrast}
/// ```
pub struct Style {
    /// Declared parameters and their defaults, in the order written.
    pub params: Vec<(String, String)>,
    pub body: String,
}

/// Split a style file into its parameters and its body.
pub fn parse_style(source: &str) -> Style {
    let mut params = Vec::new();
    let mut body = String::new();
    for line in source.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("param ") {
            if let Some((k, v)) = rest.split_once('=') {
                params.push((k.trim().to_string(), v.trim().to_string()));
                continue;
            }
        }
        body.push_str(line);
        body.push('\n');
    }
    Style { params, body }
}

/// Fill a style's holes.
///
/// An unknown `{name}` is an error rather than being left in place: a script
/// that silently drew `{blur}` pixels of blur would be worse than one that
/// stopped and said which name it did not know.
///
/// A hole that is a bare name is replaced with its value verbatim, so
/// `{blend}` can carry `Color Dodge` and not just a number. Anything else is
/// read as arithmetic over the parameters — `{min*0.01}`, `{width/2}`. That
/// is what lets a style be written once and applied to any size of image:
/// `width`, `height`, `min`, `max`, `cx` and `cy` are bound for it.
pub fn substitute(body: &str, values: &[(String, String)]) -> Result<String, String> {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            return Err("a { is never closed".into());
        };
        let key = &after[..close];
        match values.iter().find(|(k, _)| k == key) {
            // A bare name: verbatim, so a parameter can be a word.
            Some((_, value)) => out.push_str(value),
            None if key.chars().any(|c| "+-*/()".contains(c)) => {
                let value = evaluate(key, values)?;
                // Trim the float noise: 12 rather than 12.0000001.
                out.push_str(&format!("{}", (value * 1e6).round() / 1e6));
            }
            None => {
                return Err(format!(
                    "this style has no parameter {key:?}; it takes: {}",
                    if values.is_empty() {
                        "none".to_string()
                    } else {
                        values.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ")
                    }
                ));
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Arithmetic inside a `{...}` hole.
///
/// Deliberately tiny: numbers, parameters, `+ - * /`, and parentheses. It
/// exists so a style can scale itself to the image it is given, not so that
/// styles can become a programming language.
fn evaluate(expression: &str, values: &[(String, String)]) -> Result<f32, String> {
    let bytes: Vec<char> = expression.chars().filter(|c| !c.is_whitespace()).collect();
    let mut at = 0usize;
    let value = expr(&bytes, &mut at, values, expression)?;
    if at != bytes.len() {
        return Err(format!("could not read all of {expression:?} as arithmetic"));
    }
    Ok(value)
}

fn expr(c: &[char], at: &mut usize, v: &[(String, String)], whole: &str) -> Result<f32, String> {
    let mut left = term(c, at, v, whole)?;
    while let Some(&op @ ('+' | '-')) = c.get(*at) {
        *at += 1;
        let right = term(c, at, v, whole)?;
        left = if op == '+' { left + right } else { left - right };
    }
    Ok(left)
}

fn term(c: &[char], at: &mut usize, v: &[(String, String)], whole: &str) -> Result<f32, String> {
    let mut left = factor(c, at, v, whole)?;
    while let Some(&op @ ('*' | '/')) = c.get(*at) {
        *at += 1;
        let right = factor(c, at, v, whole)?;
        if op == '/' && right == 0.0 {
            return Err(format!("{whole:?} divides by zero"));
        }
        left = if op == '*' { left * right } else { left / right };
    }
    Ok(left)
}

fn factor(c: &[char], at: &mut usize, v: &[(String, String)], whole: &str) -> Result<f32, String> {
    match c.get(*at) {
        Some('-') => {
            *at += 1;
            Ok(-factor(c, at, v, whole)?)
        }
        Some('(') => {
            *at += 1;
            let inner = expr(c, at, v, whole)?;
            if c.get(*at) != Some(&')') {
                return Err(format!("{whole:?} has an unclosed ("));
            }
            *at += 1;
            Ok(inner)
        }
        Some(ch) if ch.is_ascii_digit() || *ch == '.' => {
            let start = *at;
            while matches!(c.get(*at), Some(d) if d.is_ascii_digit() || *d == '.') {
                *at += 1;
            }
            let text: String = c[start..*at].iter().collect();
            text.parse().map_err(|_| format!("{text:?} is not a number"))
        }
        Some(ch) if ch.is_alphabetic() || *ch == '_' => {
            let start = *at;
            // No hyphens: whitespace is already gone by here, so `a - b` and
            // `a-b` are the same text and subtraction has to win. A parameter
            // whose name does have a hyphen still resolves as a bare `{name}`,
            // which is the only place such a name is written anyway.
            while matches!(c.get(*at), Some(d) if d.is_alphanumeric() || *d == '_') {
                *at += 1;
            }
            let name: String = c[start..*at].iter().collect();
            let text = v
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, value)| value.as_str())
                .ok_or_else(|| format!("{whole:?} uses {name:?}, which is not a parameter"))?;
            text.parse()
                .map_err(|_| format!("{name:?} is {text:?}, which is not a number"))
        }
        _ => Err(format!("{whole:?} ends early")),
    }
}

/// Read a path from the small subset of the usual path grammar.
///
/// `M x y` moves to a new contour, `L x y` draws a line, `C x1 y1 x2 y2 x y` a
/// cubic, `Z` closes. Absolute coordinates only, and no shorthand: a script
/// writes these once and reads them back later, so the shortest spelling is
/// worth less than the one that is obvious.
pub fn parse_path_data(data: &str) -> Result<Vec<cshop_core::path::SubPath>, String> {
    use cshop_core::path::{Anchor, SubPath};

    let mut tokens = data.split([' ', ',', '\t', '\n']).filter(|t| !t.is_empty()).peekable();
    let mut subpaths: Vec<SubPath> = Vec::new();
    let mut current: Vec<Anchor> = Vec::new();
    let mut closed = false;

    let flush = |anchors: &mut Vec<Anchor>, closed: &mut bool, out: &mut Vec<SubPath>| {
        if anchors.len() >= 2 {
            out.push(SubPath { anchors: std::mem::take(anchors), closed: *closed });
        } else {
            anchors.clear();
        }
        *closed = false;
    };

    while let Some(token) = tokens.next() {
        let mut number = |what: &str| -> Result<f32, String> {
            tokens
                .next()
                .ok_or_else(|| format!("{what} is missing a number"))?
                .parse::<f32>()
                .map_err(|_| format!("{what} expected a number"))
        };
        match token {
            "M" | "m" => {
                let (x, y) = (number("M")?, number("M")?);
                flush(&mut current, &mut closed, &mut subpaths);
                current.push(Anchor::corner(Vec2::new(x, y)));
            }
            "L" | "l" => {
                let (x, y) = (number("L")?, number("L")?);
                if current.is_empty() {
                    return Err("a path has to start with M".into());
                }
                current.push(Anchor::corner(Vec2::new(x, y)));
            }
            "C" | "c" => {
                let v = [
                    number("C")?,
                    number("C")?,
                    number("C")?,
                    number("C")?,
                    number("C")?,
                    number("C")?,
                ];
                let Some(last) = current.last_mut() else {
                    return Err("a path has to start with M".into());
                };
                // The first control point belongs to the anchor being left,
                // the second to the one being arrived at.
                last.out_handle = Vec2::new(v[0], v[1]);
                let at = Vec2::new(v[4], v[5]);
                current.push(Anchor { at, in_handle: Vec2::new(v[2], v[3]), out_handle: at });
            }
            "Z" | "z" => {
                closed = true;
                flush(&mut current, &mut closed, &mut subpaths);
            }
            other => return Err(format!("{other:?} is not a path command; use M, L, C or Z")),
        }
    }
    flush(&mut current, &mut closed, &mut subpaths);
    Ok(subpaths)
}

/// Where styles are looked for, nearest first.
pub fn style_dirs(base: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![base.to_path_buf(), base.join("styles")];
    // Beside the binary, which is where an installed copy keeps them.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("styles"));
            // And out of a target/ directory, for a build tree.
            for up in [1, 2, 3] {
                if let Some(root) = dir.ancestors().nth(up) {
                    dirs.push(root.join("styles"));
                }
            }
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".config/cshop/styles"));
    }
    dirs
}

/// Find a style by name, and say where it looked when there is none.
pub fn find_style(base: &Path, name: &str) -> Result<(PathBuf, Style), String> {
    // A path wins over a name, so a one-off style needs no directory.
    let direct = resolve(base, name);
    let candidates: Vec<PathBuf> = std::iter::once(direct.clone())
        .chain(std::iter::once(direct.with_extension("style")))
        .chain(style_dirs(base).into_iter().map(|d| d.join(format!("{name}.style"))))
        .collect();

    for path in &candidates {
        if let Ok(source) = std::fs::read_to_string(path) {
            return Ok((path.clone(), parse_style(&source)));
        }
    }
    let mut available = style_dirs(base)
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "style") {
                p.file_stem().map(|n| n.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    // Several search paths can land on one directory — a build tree finds its
    // own `styles/` more than once — so the same style must not be listed
    // twice.
    available.sort();
    available.dedup();
    Err(if available.is_empty() {
        format!("no style called {name:?}, and no styles directory was found")
    } else {
        format!("no style called {name:?}. Available: {}", available.join(", "))
    })
}
