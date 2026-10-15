//! Running a page's content stream (ISO 32000-1 §8–9): what is drawn, where,
//! and in what order.
//!
//! The interpreter starts with the CTM set to the page-to-document transform,
//! so everything it reports is already in document pixels, y down. Every mark
//! carries a sequence number: layers are stacked in the order the page painted
//! them, which is what keeps a white box drawn over some text above that text.

use super::file::File;
use super::fonts::Font;
use super::images;
use super::syntax::{Dict, Lexer, Object, Parser, Token};
use cshop_core::color::Rgba8;
use std::collections::HashMap;
use std::rc::Rc;

/// `[a b c d e f]`, PDF's row-vector convention: `p' = p × M`.
pub type Matrix = [f64; 6];

pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `m × n`, so `cm` is `CTM' = M × CTM`.
pub fn mult(m: &Matrix, n: &Matrix) -> Matrix {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

pub fn apply(m: &Matrix, x: f64, y: f64) -> (f64, f64) {
    (x * m[0] + y * m[2] + m[4], x * m[1] + y * m[3] + m[5])
}

/// Ligature glyphs read as their letters, so "file" set with an fi ligature
/// is found by a search for "file" and typed again by a face without one.
fn expand_ligatures(text: String) -> String {
    if !text.chars().any(|c| ('\u{FB00}'..='\u{FB06}').contains(&c)) {
        return text;
    }
    text.chars()
        .map(|c| match c {
            '\u{FB00}' => "ff".to_string(),
            '\u{FB01}' => "fi".to_string(),
            '\u{FB02}' => "fl".to_string(),
            '\u{FB03}' => "ffi".to_string(),
            '\u{FB04}' => "ffl".to_string(),
            '\u{FB05}' | '\u{FB06}' => "st".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// A glyph as placed, in document pixels.
#[derive(Debug, Clone)]
pub struct Glyph {
    pub text: String,
    /// Origin on the baseline.
    pub x: f64,
    pub y: f64,
    /// The glyph's own advance along the baseline, in pixels. Character and
    /// word spacing move the pen without widening the glyph, so a gap they
    /// open reads as a gap.
    pub advance: f64,
    /// The em, in pixels.
    pub size: f64,
    /// Baseline direction in degrees, clockwise on screen; 0 is upright.
    pub angle: f64,
    /// Drawn reflected, as through a mirror.
    pub mirrored: bool,
    /// Index into [`PageContent::fonts`].
    pub font: usize,
    pub color: Rgba8,
    /// Render mode 3: placed but not drawn, as OCR text over a scan is.
    pub invisible: bool,
    pub seq: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    Curve(f64, f64, f64, f64, f64, f64),
    Close,
}

/// A painted path, in document pixels.
#[derive(Debug, Clone)]
pub struct PathPaint {
    pub segs: Vec<Seg>,
    pub fill: Option<Rgba8>,
    pub even_odd: bool,
    pub stroke: Option<Rgba8>,
    pub stroke_width: f64,
    pub blend: Option<Vec<u8>>,
    pub clip: Option<[f64; 4]>,
    pub seq: usize,
}

/// A placed image: its pixels and where pixel space lands in the document.
pub struct ImagePaint {
    pub pixels: cshop_core::pixels::PixelBuffer,
    /// Image pixel (x, y), y down from the image's top, to document pixels.
    pub matrix: Matrix,
    pub opacity: f32,
    pub blend: Option<Vec<u8>>,
    pub clip: Option<[f64; 4]>,
    pub seq: usize,
}

/// What a page drew, and what it drew that could not be kept.
#[derive(Default)]
pub struct PageContent {
    pub glyphs: Vec<Glyph>,
    pub paths: Vec<PathPaint>,
    pub images: Vec<ImagePaint>,
    pub fonts: Vec<super::fonts::Description>,
    /// Things seen and not reproduced, by what they were, with a count.
    pub skipped: HashMap<&'static str, usize>,
}

impl PageContent {
    fn skip(&mut self, what: &'static str) {
        *self.skipped.entry(what).or_default() += 1;
    }
}

#[derive(Clone)]
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// A spot ink, shown as its tint of black.
    Separation,
    Indexed { base: Box<Space>, hival: usize, lookup: Rc<Vec<u8>> },
    Pattern,
}

impl Space {
    fn components(&self) -> usize {
        match self {
            Space::Gray | Space::Separation | Space::Indexed { .. } | Space::Pattern => 1,
            Space::Rgb => 3,
            Space::Cmyk => 4,
        }
    }

    fn initial(&self) -> Vec<f32> {
        match self {
            Space::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            Space::Separation => vec![1.0],
            other => vec![0.0; other.components()],
        }
    }

    /// The colour, or `None` for a pattern.
    fn rgb(&self, c: &[f32]) -> Option<[f32; 3]> {
        let at = |i: usize| c.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        Some(match self {
            Space::Gray => [at(0); 3],
            Space::Rgb => [at(0), at(1), at(2)],
            Space::Cmyk => {
                let k = 1.0 - at(3);
                [(1.0 - at(0)) * k, (1.0 - at(1)) * k, (1.0 - at(2)) * k]
            }
            Space::Separation => [1.0 - at(0); 3],
            Space::Indexed { base, hival, lookup } => {
                let i = (c.first().copied().unwrap_or(0.0).round().max(0.0) as usize).min(*hival);
                let n = base.components();
                let comps: Vec<f32> = (0..n).map(|k| *lookup.get(i * n + k).unwrap_or(&0) as f32 / 255.0).collect();
                return base.rgb(&comps);
            }
            Space::Pattern => return None,
        })
    }
}

fn to_rgba(rgb: [f32; 3], alpha: f32) -> Rgba8 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgba8::new(b(rgb[0]), b(rgb[1]), b(rgb[2]), b(alpha))
}

#[derive(Clone)]
struct State {
    ctm: Matrix,
    fill_space: Space,
    fill: Vec<f32>,
    stroke_space: Space,
    stroke: Vec<f32>,
    fill_alpha: f32,
    stroke_alpha: f32,
    blend: Option<Vec<u8>>,
    line_width: f64,
    dashed: bool,
    clip: Option<[f64; 4]>,
    font: Option<usize>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
    rise: f64,
    render: i64,
}

impl State {
    fn new(ctm: Matrix) -> State {
        State {
            ctm,
            fill_space: Space::Gray,
            fill: vec![0.0],
            stroke_space: Space::Gray,
            stroke: vec![0.0],
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            blend: None,
            line_width: 1.0,
            dashed: false,
            clip: None,
            font: None,
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            render: 0,
        }
    }

    fn fill_color(&self) -> Option<Rgba8> {
        self.fill_space.rgb(&self.fill).map(|c| to_rgba(c, self.fill_alpha))
    }

    fn stroke_color(&self) -> Option<Rgba8> {
        self.stroke_space.rgb(&self.stroke).map(|c| to_rgba(c, self.stroke_alpha))
    }
}

const MAX_FORM_DEPTH: usize = 12;
const MAX_MARKS: usize = 2_000_000;

pub struct Interpreter<'f> {
    file: &'f File,
    pub out: PageContent,
    /// Fonts already loaded, by the identity of their dictionary's object.
    fonts: HashMap<FontKey, usize>,
    loaded: Vec<Rc<Font>>,
    seq: usize,
    depth: usize,
    active_forms: Vec<*const ()>,
    // Per-stream state.
    resources: Dict,
    gs: State,
    stack: Vec<State>,
    tm: Matrix,
    tlm: Matrix,
    path: Vec<Seg>,
    pending_clip: Option<bool>,
}

#[derive(Hash, PartialEq, Eq, Clone)]
enum FontKey {
    Ref(u32),
    Inline(usize, Vec<u8>),
}

impl<'f> Interpreter<'f> {
    pub fn new(file: &'f File) -> Self {
        Interpreter {
            file,
            out: PageContent::default(),
            fonts: HashMap::new(),
            loaded: Vec::new(),
            seq: 0,
            depth: 0,
            active_forms: Vec::new(),
            resources: Dict::default(),
            gs: State::new(IDENTITY),
            stack: Vec::new(),
            tm: IDENTITY,
            tlm: IDENTITY,
            path: Vec::new(),
            pending_clip: None,
        }
    }

    fn next_seq(&mut self) -> usize {
        self.seq += 1;
        self.seq
    }

    /// Run a page's content under `ctm`.
    pub fn run_page(&mut self, content: &[u8], resources: &Dict, ctm: Matrix) {
        self.run(content, resources.clone(), State::new(ctm));
    }

    fn run(&mut self, content: &[u8], resources: Dict, gs: State) {
        let saved = (
            std::mem::replace(&mut self.resources, resources),
            std::mem::replace(&mut self.gs, gs),
            std::mem::take(&mut self.stack),
            self.tm,
            self.tlm,
            std::mem::take(&mut self.path),
        );
        self.tm = IDENTITY;
        self.tlm = IDENTITY;
        let mut parser = Parser::new(content, 0);
        let mut operands: Vec<Object> = Vec::new();
        while let Some(tok) = parser.lexer.next() {
            if self.out.glyphs.len() + self.out.paths.len() > MAX_MARKS {
                self.out.skip("marks beyond the limit of what one page may hold");
                break;
            }
            match tok {
                Token::Keyword(op) => match op.as_slice() {
                    b"true" => operands.push(Object::Bool(true)),
                    b"false" => operands.push(Object::Bool(false)),
                    b"null" => operands.push(Object::Null),
                    b"BI" => {
                        self.inline_image(&mut parser.lexer);
                        operands.clear();
                    }
                    _ => {
                        self.operator(&op, &operands);
                        operands.clear();
                    }
                },
                other => {
                    if let Ok(obj) = parser.build(other, 0) {
                        operands.push(obj);
                    }
                    if operands.len() > 64 {
                        operands.remove(0); // junk: keep only what an operator could want
                    }
                }
            }
        }
        (self.resources, self.gs, self.stack, self.tm, self.tlm, self.path) = saved;
    }

    fn num(ops: &[Object], from_end: usize) -> Option<f64> {
        ops.len().checked_sub(from_end).and_then(|i| ops[i].as_f64())
    }

    fn nums<const N: usize>(ops: &[Object]) -> Option<[f64; N]> {
        if ops.len() < N {
            return None;
        }
        let mut out = [0.0; N];
        for (i, o) in ops[ops.len() - N..].iter().enumerate() {
            out[i] = o.as_f64()?;
        }
        Some(out)
    }

    fn operator(&mut self, op: &[u8], ops: &[Object]) {
        match op {
            // Graphics state.
            b"q" => {
                if self.stack.len() < 256 {
                    self.stack.push(self.gs.clone());
                }
            }
            b"Q" => {
                if let Some(s) = self.stack.pop() {
                    self.gs = s;
                }
            }
            b"cm" => {
                if let Some(m) = Self::nums::<6>(ops) {
                    self.gs.ctm = mult(&m, &self.gs.ctm);
                }
            }
            b"w" => {
                if let Some(w) = Self::num(ops, 1) {
                    self.gs.line_width = w;
                }
            }
            b"d" => {
                self.gs.dashed = ops.iter().rev().nth(1).and_then(|a| a.as_array()).is_some_and(|a| !a.is_empty());
            }
            b"gs" => self.ext_gstate(ops),
            // Colour.
            b"g" | b"G" | b"rg" | b"RG" | b"k" | b"K" => {
                let (space, n) = match op {
                    b"g" | b"G" => (Space::Gray, 1),
                    b"rg" | b"RG" => (Space::Rgb, 3),
                    _ => (Space::Cmyk, 4),
                };
                if ops.len() < n {
                    return;
                }
                let comps: Vec<f32> = ops[ops.len() - n..].iter().map(|o| o.as_f32().unwrap_or(0.0)).collect();
                if op[0].is_ascii_lowercase() {
                    self.gs.fill_space = space;
                    self.gs.fill = comps;
                } else {
                    self.gs.stroke_space = space;
                    self.gs.stroke = comps;
                }
            }
            b"cs" | b"CS" => {
                let Some(space) = ops.last().map(|o| self.color_space(o)) else { return };
                let init = space.initial();
                if op == b"cs" {
                    self.gs.fill_space = space;
                    self.gs.fill = init;
                } else {
                    self.gs.stroke_space = space;
                    self.gs.stroke = init;
                }
            }
            b"sc" | b"scn" | b"SC" | b"SCN" => {
                let comps: Vec<f32> = ops.iter().filter_map(|o| o.as_f32()).collect();
                if comps.is_empty() {
                    return; // a pattern by name alone
                }
                if op[0] == b's' {
                    self.gs.fill = comps;
                } else {
                    self.gs.stroke = comps;
                }
            }
            // Paths.
            b"m" => {
                if let Some([x, y]) = Self::nums::<2>(ops) {
                    let (x, y) = apply(&self.gs.ctm, x, y);
                    self.path.push(Seg::Move(x, y));
                }
            }
            b"l" => {
                if let Some([x, y]) = Self::nums::<2>(ops) {
                    let (x, y) = apply(&self.gs.ctm, x, y);
                    self.path.push(Seg::Line(x, y));
                }
            }
            b"c" | b"v" | b"y" => {
                let current = self.current_point();
                let pts: Option<[f64; 6]> = match op {
                    b"c" => Self::nums::<6>(ops),
                    b"v" => Self::nums::<4>(ops).map(|[x2, y2, x3, y3]| {
                        // The first control point is the current point; it is
                        // already in document space, so mark it to skip the CTM.
                        [f64::NAN, f64::NAN, x2, y2, x3, y3]
                    }),
                    _ => Self::nums::<4>(ops).map(|[x1, y1, x3, y3]| [x1, y1, x3, y3, x3, y3]),
                };
                let Some(p) = pts else { return };
                let ctm = self.gs.ctm;
                let (x1, y1) = if p[0].is_nan() { current } else { apply(&ctm, p[0], p[1]) };
                let (x2, y2) = apply(&ctm, p[2], p[3]);
                let (x3, y3) = apply(&ctm, p[4], p[5]);
                self.path.push(Seg::Curve(x1, y1, x2, y2, x3, y3));
            }
            b"h" => self.path.push(Seg::Close),
            b"re" => {
                if let Some([x, y, w, h]) = Self::nums::<4>(ops) {
                    let ctm = self.gs.ctm;
                    let p = |a: f64, b: f64| apply(&ctm, a, b);
                    let (a, b, c, d) = (p(x, y), p(x + w, y), p(x + w, y + h), p(x, y + h));
                    self.path.extend([Seg::Move(a.0, a.1), Seg::Line(b.0, b.1), Seg::Line(c.0, c.1), Seg::Line(d.0, d.1), Seg::Close]);
                }
            }
            b"W" | b"W*" => self.pending_clip = Some(op == b"W*"),
            b"f" | b"F" | b"f*" => self.paint(true, op == b"f*", false, false),
            b"S" => self.paint(false, false, true, false),
            b"s" => self.paint(false, false, true, true),
            b"B" | b"B*" => self.paint(true, op == b"B*", true, false),
            b"b" | b"b*" => self.paint(true, op == b"b*", true, true),
            b"n" => self.paint(false, false, false, false),
            b"sh" => {
                self.out.skip("smooth shadings (gradients)");
            }
            // Text.
            b"BT" => {
                self.tm = IDENTITY;
                self.tlm = IDENTITY;
            }
            b"Tf" => {
                if let (Some(Object::Name(name)), Some(size)) = (ops.iter().rev().nth(1), Self::num(ops, 1)) {
                    self.gs.font = self.font(name);
                    self.gs.size = size;
                }
            }
            b"Tc" => self.gs.char_spacing = Self::num(ops, 1).unwrap_or(self.gs.char_spacing),
            b"Tw" => self.gs.word_spacing = Self::num(ops, 1).unwrap_or(self.gs.word_spacing),
            b"Tz" => self.gs.scale = Self::num(ops, 1).map_or(self.gs.scale, |v| v / 100.0),
            b"TL" => self.gs.leading = Self::num(ops, 1).unwrap_or(self.gs.leading),
            b"Ts" => self.gs.rise = Self::num(ops, 1).unwrap_or(self.gs.rise),
            b"Tr" => self.gs.render = ops.last().and_then(|o| o.as_int()).unwrap_or(self.gs.render),
            b"Td" => {
                if let Some([x, y]) = Self::nums::<2>(ops) {
                    self.move_text(x, y);
                }
            }
            b"TD" => {
                if let Some([x, y]) = Self::nums::<2>(ops) {
                    self.gs.leading = -y;
                    self.move_text(x, y);
                }
            }
            b"Tm" => {
                if let Some(m) = Self::nums::<6>(ops) {
                    self.tm = m;
                    self.tlm = m;
                }
            }
            b"T*" => self.move_text(0.0, -self.gs.leading),
            b"Tj" => {
                if let Some(Object::String(s)) = ops.last() {
                    self.show(s);
                }
            }
            b"'" => {
                self.move_text(0.0, -self.gs.leading);
                if let Some(Object::String(s)) = ops.last() {
                    self.show(s);
                }
            }
            b"\"" => {
                if let (Some(aw), Some(ac)) = (Self::num(ops, 3), Self::num(ops, 2)) {
                    self.gs.word_spacing = aw;
                    self.gs.char_spacing = ac;
                }
                self.move_text(0.0, -self.gs.leading);
                if let Some(Object::String(s)) = ops.last() {
                    self.show(s);
                }
            }
            b"TJ" => {
                let Some(Object::Array(items)) = ops.last() else { return };
                for item in items {
                    match item {
                        Object::String(s) => self.show(s),
                        other => {
                            if let Some(n) = other.as_f64() {
                                let tx = -n / 1000.0 * self.gs.size * self.gs.scale;
                                self.tm = [self.tm[0], self.tm[1], self.tm[2], self.tm[3], self.tm[4] + tx * self.tm[0], self.tm[5] + tx * self.tm[1]];
                            }
                        }
                    }
                }
            }
            // External objects.
            b"Do" => {
                if let Some(Object::Name(name)) = ops.last() {
                    self.xobject(name);
                }
            }
            _ => {}
        }
    }

    fn current_point(&self) -> (f64, f64) {
        let mut start = (0.0, 0.0);
        let mut at = (0.0, 0.0);
        for s in &self.path {
            match *s {
                Seg::Move(x, y) => {
                    start = (x, y);
                    at = start;
                }
                Seg::Line(x, y) | Seg::Curve(_, _, _, _, x, y) => at = (x, y),
                Seg::Close => at = start,
            }
        }
        at
    }

    fn paint(&mut self, fill: bool, even_odd: bool, stroke: bool, close: bool) {
        let mut segs = std::mem::take(&mut self.path);
        if let Some(clip_even_odd) = self.pending_clip.take() {
            let _ = clip_even_odd;
            let b = bounds(&segs);
            if !is_rectangle(&segs) {
                self.out.skip("non-rectangular clipping paths (clipped to their bounds)");
            }
            if let Some(b) = b {
                self.gs.clip = Some(match self.gs.clip {
                    Some(c) => [c[0].max(b[0]), c[1].max(b[1]), c[2].min(b[2]), c[3].min(b[3])],
                    None => b,
                });
            }
        }
        if !fill && !stroke || segs.is_empty() {
            return;
        }
        if close {
            segs.push(Seg::Close);
        }
        let fill_color = if fill {
            let c = self.gs.fill_color();
            if c.is_none() {
                self.out.skip("pattern fills");
            }
            c
        } else {
            None
        };
        let stroke_color = if stroke {
            let c = self.gs.stroke_color();
            if c.is_none() {
                self.out.skip("pattern strokes");
            }
            c
        } else {
            None
        };
        if fill_color.is_none() && stroke_color.is_none() {
            return;
        }
        // A line width of zero means the thinnest line the device can draw.
        let scale = ((self.gs.ctm[0] * self.gs.ctm[3] - self.gs.ctm[1] * self.gs.ctm[2]).abs()).sqrt();
        let stroke_width = (self.gs.line_width * scale).max(1.0);
        if stroke_color.is_some() && self.gs.dashed {
            self.out.skip("dashed lines (drawn solid)");
        }
        let seq = self.next_seq();
        self.out.paths.push(PathPaint {
            segs,
            fill: fill_color,
            even_odd,
            stroke: stroke_color,
            stroke_width,
            blend: self.gs.blend.clone(),
            clip: self.gs.clip,
            seq,
        });
    }

    fn ext_gstate(&mut self, ops: &[Object]) {
        let Some(Object::Name(name)) = ops.last() else { return };
        let states = self.file.get_in(&self.resources, b"ExtGState");
        let Some(d) = states.as_dict().map(|s| self.file.get_in(s, name)) else { return };
        let Some(d) = d.as_dict() else { return };
        let get = |k: &[u8]| self.file.get_in(d, k);
        if let Some(a) = get(b"ca").as_f32() {
            self.gs.fill_alpha = a.clamp(0.0, 1.0);
        }
        if let Some(a) = get(b"CA").as_f32() {
            self.gs.stroke_alpha = a.clamp(0.0, 1.0);
        }
        if let Some(w) = get(b"LW").as_f64() {
            self.gs.line_width = w;
        }
        match get(b"BM") {
            Object::Name(n) => self.gs.blend = (n != b"Normal" && n != b"Compatible").then_some(n),
            Object::Array(a) => self.gs.blend = a.first().and_then(|n| n.as_name()).filter(|n| *n != b"Normal").map(|n| n.to_vec()),
            _ => {}
        }
        match get(b"SMask") {
            Object::Null => {}
            Object::Name(n) if n == b"None" => {}
            _ => self.out.skip("soft masks"),
        }
    }

    fn color_space(&self, obj: &Object) -> Space {
        let resolved = match obj {
            Object::Name(n) => match n.as_slice() {
                b"DeviceGray" | b"G" | b"CalGray" => return Space::Gray,
                b"DeviceRGB" | b"RGB" | b"CalRGB" => return Space::Rgb,
                b"DeviceCMYK" | b"CMYK" => return Space::Cmyk,
                b"Pattern" => return Space::Pattern,
                _ => {
                    let spaces = self.file.get_in(&self.resources, b"ColorSpace");
                    match spaces.as_dict() {
                        Some(s) => self.file.get_in(s, n),
                        None => return Space::Gray,
                    }
                }
            },
            other => self.file.resolve(other),
        };
        space_from(self.file, &resolved, 0)
    }

    // ---- text -------------------------------------------------------------

    fn font(&mut self, name: &[u8]) -> Option<usize> {
        let fonts = self.file.get_in(&self.resources, b"Font");
        let entry = fonts.as_dict()?.get(name)?.clone();
        let key = match &entry {
            Object::Ref(n, _) => FontKey::Ref(*n),
            _ => FontKey::Inline(self.depth, name.to_vec()),
        };
        if let Some(&i) = self.fonts.get(&key) {
            return Some(i);
        }
        let dict = self.file.resolve(&entry);
        let font = Font::load(self.file, dict.as_dict()?);
        let i = self.loaded.len();
        self.out.fonts.push(font.description.clone());
        self.loaded.push(Rc::new(font));
        self.fonts.insert(key, i);
        Some(i)
    }

    fn move_text(&mut self, tx: f64, ty: f64) {
        self.tlm = mult(&[1.0, 0.0, 0.0, 1.0, tx, ty], &self.tlm);
        self.tm = self.tlm;
    }

    fn show(&mut self, string: &[u8]) {
        let Some(fi) = self.gs.font else { return };
        let font = self.loaded[fi].clone();
        let gs = &self.gs;
        let (size, scale) = (gs.size, gs.scale);
        let base: Matrix = [size * scale, 0.0, 0.0, size, 0.0, gs.rise];
        let color = match gs.render {
            1 | 5 => gs.stroke_color(),
            _ => gs.fill_color(),
        }
        .unwrap_or(Rgba8::BLACK);
        let invisible = gs.render == 3 || gs.render == 7;
        let (char_spacing, word_spacing, ctm) = (gs.char_spacing, gs.word_spacing, gs.ctm);
        let clip = gs.clip;
        for code in font.decode(string) {
            let tm_ctm = mult(&self.tm, &ctm);
            let trm = mult(&base, &tm_ctm);
            let glyph = code.width as f64 * size * scale;
            let tx = glyph + (char_spacing + if code.word_space { word_spacing } else { 0.0 }) * scale;
            let (x, y) = (trm[4], trm[5]);
            let outside = clip.is_some_and(|c| x < c[0] - size || x > c[2] + size || y < c[1] - size || y > c[3] + size);
            if !code.text.is_empty() && !outside {
                let seq = self.next_seq();
                self.out.glyphs.push(Glyph {
                    text: expand_ligatures(code.text),
                    x,
                    y,
                    advance: glyph * (tm_ctm[0].powi(2) + tm_ctm[1].powi(2)).sqrt(),
                    size: (trm[2].powi(2) + trm[3].powi(2)).sqrt(),
                    angle: trm[1].atan2(trm[0]).to_degrees(),
                    // Upright text in a y-down space has a negative determinant.
                    mirrored: trm[0] * trm[3] - trm[1] * trm[2] > 0.0,
                    font: fi,
                    color,
                    invisible,
                    seq,
                });
            }
            self.tm = [self.tm[0], self.tm[1], self.tm[2], self.tm[3], self.tm[4] + tx * self.tm[0], self.tm[5] + tx * self.tm[1]];
        }
    }

    // ---- images and forms -------------------------------------------------

    fn xobject(&mut self, name: &[u8]) {
        let xobjects = self.file.get_in(&self.resources, b"XObject");
        let Some(entry) = xobjects.as_dict().and_then(|x| x.get(name)).cloned() else { return };
        let Object::Stream(stream) = self.file.resolve(&entry) else { return };
        let subtype = self.file.get_in(&stream.dict, b"Subtype");
        if subtype.is_name(b"Image") {
            self.image(&stream.dict, &stream.raw, false);
        } else if subtype.is_name(b"Form") {
            let matrix = matrix_of(self.file, &stream.dict).unwrap_or(IDENTITY);
            self.form(&stream, matrix, false);
        }
    }

    /// Run a form XObject under `matrix` × CTM. It inherits the graphics state
    /// at the `Do` (§8.10.1); an annotation's appearance starts `fresh`.
    pub fn form(&mut self, stream: &Rc<super::syntax::Stream>, matrix: Matrix, fresh: bool) {
        let key = Rc::as_ptr(stream) as *const ();
        if self.depth >= MAX_FORM_DEPTH || self.active_forms.contains(&key) {
            return;
        }
        let Ok(data) = self.file.stream_data(&stream.dict, &stream.raw) else { return };
        let mut gs = if fresh { State::new(IDENTITY) } else { self.gs.clone() };
        gs.ctm = mult(&matrix, &gs.ctm);
        let resources = match self.file.get_in(&stream.dict, b"Resources") {
            Object::Dict(d) => d,
            _ => self.resources.clone(),
        };
        self.active_forms.push(key);
        self.depth += 1;
        self.run(&data, resources, gs);
        self.depth -= 1;
        self.active_forms.pop();
    }

    fn image(&mut self, dict: &Dict, raw: &[u8], inline: bool) {
        let fill = self.gs.fill_color().unwrap_or(Rgba8::BLACK);
        let spaces = self.file.get_in(&self.resources, b"ColorSpace");
        let lookup_space = |o: &Object| -> Option<images::Colors> {
            let resolved = match o {
                Object::Name(n) => match spaces.as_dict().map(|s| self.file.get_in(s, n)) {
                    Some(Object::Null) | None => o.clone(),
                    Some(other) => other,
                },
                other => self.file.resolve(other),
            };
            images::Colors::from_space(&space_from(self.file, &resolved, 0))
        };
        match images::decode(self.file, dict, raw, inline, fill, &lookup_space) {
            Ok(pixels) => {
                let (w, h) = (pixels.width() as f64, pixels.height() as f64);
                // Image space is the unit square, row 0 at its top.
                let unit: Matrix = [1.0 / w, 0.0, 0.0, -1.0 / h, 0.0, 1.0];
                let seq = self.next_seq();
                self.out.images.push(ImagePaint {
                    pixels,
                    matrix: mult(&unit, &self.gs.ctm),
                    opacity: self.gs.fill_alpha,
                    blend: self.gs.blend.clone(),
                    clip: self.gs.clip,
                    seq,
                });
            }
            Err(what) => self.out.skip(what),
        }
    }

    fn inline_image(&mut self, lexer: &mut Lexer) {
        // The dictionary, up to ID.
        let mut dict = Dict::default();
        let mut parser = Parser::new(lexer.data, lexer.pos);
        loop {
            match parser.lexer.next() {
                None => {
                    lexer.pos = lexer.data.len();
                    return;
                }
                Some(Token::Keyword(k)) if k == b"ID" => break,
                Some(Token::Name(key)) => {
                    let Some(t) = parser.lexer.next() else { break };
                    if let Ok(v) = parser.build(t, 0) {
                        dict.insert(&key, v);
                    }
                }
                Some(_) => {}
            }
        }
        let data = parser.lexer.data;
        let start = (parser.lexer.pos + 1).min(data.len());
        // The data ends at an EI with whitespace on both sides.
        let mut end = data.len();
        let mut i = start;
        while i + 2 <= data.len() {
            if &data[i..i + 2] == b"EI"
                && i > start
                && super::syntax::is_white(data[i - 1])
                && data.get(i + 2).is_none_or(|c| super::syntax::is_white(*c))
            {
                end = i - 1;
                break;
            }
            i += 1;
        }
        let raw = data[start..end.max(start)].to_vec();
        lexer.pos = (end + 3).min(data.len());
        self.image(&dict, &raw, true);
    }
}

fn space_from(file: &File, obj: &Object, depth: usize) -> Space {
    if depth > 8 {
        return Space::Gray;
    }
    match obj {
        Object::Name(n) => match n.as_slice() {
            b"DeviceRGB" | b"RGB" | b"CalRGB" => Space::Rgb,
            b"DeviceCMYK" | b"CMYK" => Space::Cmyk,
            b"Pattern" => Space::Pattern,
            _ => Space::Gray,
        },
        Object::Array(a) => {
            let kind = a.first().and_then(|k| k.as_name()).unwrap_or(b"");
            let arg = |i: usize| a.get(i).map(|o| file.resolve(o)).unwrap_or(Object::Null);
            match kind {
                b"ICCBased" => match arg(1).as_dict().and_then(|d| d.get(b"N")).and_then(|n| n.as_int()) {
                    Some(3) => Space::Rgb,
                    Some(4) => Space::Cmyk,
                    _ => Space::Gray,
                },
                b"CalRGB" | b"Lab" => Space::Rgb,
                b"CalGray" => Space::Gray,
                b"Indexed" | b"I" => {
                    let base = space_from(file, &arg(1), depth + 1);
                    let hival = arg(2).as_int().unwrap_or(0).clamp(0, 255) as usize;
                    let lookup = match arg(3) {
                        Object::String(s) => s,
                        Object::Stream(s) => file.stream_data(&s.dict, &s.raw).unwrap_or_default(),
                        _ => Vec::new(),
                    };
                    Space::Indexed { base: Box::new(base), hival, lookup: Rc::new(lookup) }
                }
                b"Separation" | b"DeviceN" => Space::Separation,
                b"Pattern" => Space::Pattern,
                _ => Space::Gray,
            }
        }
        _ => Space::Gray,
    }
}

impl images::Colors {
    fn from_space(space: &Space) -> Option<images::Colors> {
        Some(match space {
            Space::Gray => images::Colors::Gray,
            Space::Rgb => images::Colors::Rgb,
            Space::Cmyk => images::Colors::Cmyk,
            Space::Separation => images::Colors::Ink,
            Space::Indexed { base, hival, lookup } => {
                let n = base.components();
                let mut palette = Vec::with_capacity(hival + 1);
                for i in 0..=*hival {
                    let comps: Vec<f32> = (0..n).map(|k| *lookup.get(i * n + k).unwrap_or(&0) as f32 / 255.0).collect();
                    palette.push(to_rgba(base.rgb(&comps)?, 1.0));
                }
                images::Colors::Indexed(palette)
            }
            Space::Pattern => return None,
        })
    }
}

pub fn matrix_of(file: &File, dict: &Dict) -> Option<Matrix> {
    let m = file.get_in(dict, b"Matrix");
    let a = m.as_array()?;
    if a.len() != 6 {
        return None;
    }
    let mut out = [0.0; 6];
    for (i, v) in a.iter().enumerate() {
        out[i] = file.resolve(v).as_f64()?;
    }
    Some(out)
}

/// `[x0 y0 x1 y1]` of a path's points, control points included.
pub fn bounds(segs: &[Seg]) -> Option<[f64; 4]> {
    let mut b: Option<[f64; 4]> = None;
    let mut add = |x: f64, y: f64| {
        b = Some(match b {
            None => [x, y, x, y],
            Some(r) => [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)],
        });
    };
    for s in segs {
        match *s {
            Seg::Move(x, y) | Seg::Line(x, y) => add(x, y),
            Seg::Curve(a, b2, c, d, e, f) => {
                add(a, b2);
                add(c, d);
                add(e, f);
            }
            Seg::Close => {}
        }
    }
    b.filter(|r| r.iter().all(|v| v.is_finite()))
}

/// Whether a path is one axis-aligned rectangle.
pub fn is_rectangle(segs: &[Seg]) -> bool {
    let pts: Vec<(f64, f64)> = segs
        .iter()
        .filter_map(|s| match *s {
            Seg::Move(x, y) | Seg::Line(x, y) => Some((x, y)),
            _ => None,
        })
        .collect();
    let curves = segs.iter().any(|s| matches!(s, Seg::Curve(..)));
    let moves = segs.iter().filter(|s| matches!(s, Seg::Move(..))).count();
    if curves || moves != 1 || !(pts.len() == 4 || pts.len() == 5) {
        return false;
    }
    let eq = |a: f64, b: f64| (a - b).abs() < 0.01;
    let (p0, p1, p2, p3) = (pts[0], pts[1], pts[2], pts[3]);
    let horizontal_first = eq(p0.1, p1.1) && eq(p1.0, p2.0) && eq(p2.1, p3.1) && eq(p3.0, p0.0);
    let vertical_first = eq(p0.0, p1.0) && eq(p1.1, p2.1) && eq(p2.0, p3.0) && eq(p3.1, p0.1);
    horizontal_first || vertical_first
}
