//! What a page drew, as layers.
//!
//! Text becomes type, set in an installed face chosen to stand in for the
//! PDF's own (metric-compatible where one exists — Liberation for Times, Arial
//! and Courier — so lines keep their length). Paths become shape layers and
//! images raster layers. Everything is stacked in the order the page painted
//! it.

use super::content::{self, Glyph, ImagePaint, PageContent, PathPaint, Seg};
use super::fonts::Description;
use super::layout::{self, Line};
use super::{NewLayer, ReadOptions, TextMode};
use ab_glyph::{Font as _, FontVec, PxScale, ScaleFont};
use cshop_core::blend::BlendMode;
use cshop_core::color::Rgba8;
use cshop_core::font::FontDb;
use cshop_core::geom::{IRect, Vec2};
use cshop_core::layer::{LayerKind, ShapeLayer, TextLayer};
use cshop_core::path::{Anchor, BoolOp, PathPart, PathShape, SubPath};
use cshop_core::resample::{self, Resampling};
use cshop_core::shape::{ShapeContent, ShapeKind, ShapeStyle, StrokeAlign};
use cshop_core::text::{TextAlign, TextContent, TextStyle};
use cshop_core::transform::Transform;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Of the em: a wider gap inside a line starts a new layer — a tab stop, or
/// the gutter between two columns.
const RUN_GAP: f64 = 0.9;
/// Shapes one page may make; the rest are counted and left out.
const MAX_SHAPES: usize = 5_000;
const MAX_IMAGES: usize = 500;

/// An installed face standing in for one of the PDF's fonts.
#[derive(Clone)]
struct Face {
    family: String,
    bold: bool,
    italic: bool,
    font: Arc<FontVec>,
    /// Font height over the em: what turns an em size into the size a type
    /// layer is given, which is a pixel height in ab_glyph's sense.
    height_per_em: f32,
}

impl Face {
    fn style_size(&self, em: f64) -> f32 {
        em as f32 * self.height_per_em
    }

    fn ascent(&self, size: f32) -> f32 {
        self.font.as_scaled(PxScale::from(size.max(1.0))).ascent()
    }

    fn space(&self, size: f32) -> f32 {
        let f = self.font.as_scaled(PxScale::from(size.max(1.0)));
        f.h_advance(f.glyph_id(' '))
    }
}

/// Accumulated across pages, and said once at the end.
#[derive(Default)]
pub struct Report {
    pub substituted: BTreeMap<String, String>,
    pub skipped: BTreeMap<String, usize>,
    pub notes: BTreeMap<&'static str, usize>,
}

impl Report {
    fn note(&mut self, what: &'static str) {
        *self.notes.entry(what).or_default() += 1;
    }

    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.substituted.is_empty() {
            // Words, not an arrow: the interface's own face has no arrow.
            let list: Vec<String> = self.substituted.iter().map(|(k, v)| format!("{k} as {v}")).collect();
            out.push(format!("Fonts set in the nearest installed face: {}", list.join(", ")));
        }
        // Counts after the description, so one reads as well as many.
        for (what, n) in &self.notes {
            let mut what = what.to_string();
            if let Some(first) = what.get(..1) {
                what = first.to_uppercase() + &what[1..];
            }
            out.push(format!("{what}: {n}"));
        }
        if !self.skipped.is_empty() {
            let list: Vec<String> = self.skipped.iter().map(|(k, n)| format!("{k} ({n})")).collect();
            out.push(format!("Not reproduced: {}", list.join(", ")));
        }
        out
    }
}

/// Layers for one page, bottom to top.
pub fn page_layers(content: PageContent, size: (u32, u32), options: &ReadOptions, report: &mut Report) -> Vec<NewLayer> {
    let mut layers: Vec<(usize, NewLayer)> = Vec::new();
    for (what, n) in &content.skipped {
        *report.skipped.entry(what.to_string()).or_default() += n;
    }
    let faces = choose_faces(&content.fonts, report);
    let bounds = IRect::at(0, 0, size.0, size.1);

    // Text.
    let unmapped = content.glyphs.iter().filter(|g| g.text.contains(super::fonts::REPLACEMENT)).count();
    if unmapped > 0 {
        *report.skipped.entry("characters with no Unicode mapping (shown as \u{FFFD})".into()).or_default() += unmapped;
    }
    let lines = layout::lines(&content.glyphs);
    let mut text = TextBuilder { glyphs: &content.glyphs, faces: &faces, report, out: Vec::new() };
    match options.text {
        TextMode::Lines => {
            for line in &lines {
                text.line(line);
            }
        }
        TextMode::Paragraphs => text.paragraphs(&lines),
    }
    layers.extend(text.out);

    // Shapes.
    let mut shapes = 0;
    for path in &content.paths {
        if shapes >= MAX_SHAPES {
            *report.skipped.entry(format!("shapes past the first {MAX_SHAPES} on a page")).or_default() += 1;
            continue;
        }
        for layer in shape_layers(path) {
            shapes += 1;
            layers.push((path.seq, layer));
        }
    }

    // Images.
    for (i, image) in content.images.into_iter().enumerate() {
        if i >= MAX_IMAGES {
            *report.skipped.entry(format!("images past the first {MAX_IMAGES} on a page")).or_default() += 1;
            continue;
        }
        let seq = image.seq;
        if let Some(layer) = image_layer(image, bounds) {
            layers.push((seq, layer));
        }
    }

    layers.sort_by_key(|(seq, _)| *seq);
    layers.into_iter().map(|(_, l)| l).collect()
}

// ---------------------------------------------------------------------------
// Faces
// ---------------------------------------------------------------------------

const SERIF: &[&str] = &["Times New Roman", "Liberation Serif", "Tinos", "Nimbus Roman", "Nimbus Roman No9 L", "FreeSerif", "DejaVu Serif"];
const SANS: &[&str] = &["Arial", "Liberation Sans", "Arimo", "Nimbus Sans", "Helvetica", "FreeSans", "DejaVu Sans"];
const MONO: &[&str] = &["Courier New", "Liberation Mono", "Cousine", "Nimbus Mono PS", "FreeMono", "DejaVu Sans Mono"];

fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// "TimesNewRomanPS-BoldMT" → ("Times New Roman", "BoldMT").
fn split_name(name: &str) -> (String, String) {
    let (base, style) = match name.find(['-', ',']) {
        Some(i) => (&name[..i], &name[i + 1..]),
        None => (name, ""),
    };
    let mut base = base.to_string();
    for suffix in ["PSMT", "MT", "PS", "Std", "Pro", "LT"] {
        if base.len() > suffix.len() + 2 && base.ends_with(suffix) {
            base.truncate(base.len() - suffix.len());
            break;
        }
    }
    // Words from CamelCase.
    let mut words = String::new();
    let chars: Vec<char> = base.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() && (chars[i - 1].is_lowercase() || chars.get(i + 1).is_some_and(|n| n.is_lowercase()) && chars[i - 1].is_uppercase()) {
            words.push(' ');
        }
        words.push(c);
    }
    (words, style.to_string())
}

fn choose_faces(fonts: &[Description], report: &mut Report) -> Vec<Option<Face>> {
    let db = FontDb::global();
    fonts.iter().map(|d| choose_face(db, d, report)).collect()
}

fn choose_face(db: &FontDb, d: &Description, report: &mut Report) -> Option<Face> {
    let (base, style) = split_name(&d.name);
    let lower = format!("{} {}", d.name, style).to_lowercase();
    let bold = ["bold", "black", "heavy", "semibold", "demi"].iter().any(|w| lower.contains(w))
        || d.weight.is_some_and(|w| w >= 600.0)
        || d.flags & (1 << 18) != 0;
    let italic = ["italic", "oblique"].iter().any(|w| lower.contains(w)) || d.italic_angle.abs() > 0.5 || d.flags & 64 != 0;

    let installed = |want: &str| -> Option<String> {
        let w = squash(want);
        if w.is_empty() {
            return None;
        }
        db.families().iter().find(|f| squash(&f.name) == w).map(|f| f.name.clone())
    };
    let key = squash(&base);
    let fallback: &[&str] = if key.contains("courier") || key.contains("mono") || d.flags & 1 != 0 {
        MONO
    } else if key.contains("times") || key.contains("roman") || key.contains("georgia") || key.contains("garamond") || key.contains("cambria") {
        SERIF
    } else if key.contains("arial") || key.contains("helvetica") || key.contains("calibri") || key.contains("verdana") {
        SANS
    } else if d.flags & 2 != 0 {
        SERIF
    } else {
        SANS
    };
    let family = d
        .family
        .as_deref()
        .and_then(installed)
        .or_else(|| installed(&base))
        .or_else(|| fallback.iter().find_map(|f| installed(f)))
        .or_else(|| Some(db.default_family()).filter(|f| !f.is_empty()))?;
    let font = db.load(&family, bold, italic)?;
    let upem = font.units_per_em().unwrap_or(1000.0).max(1.0);
    let height_per_em = (font.height_unscaled() / upem).clamp(0.5, 3.0);
    if squash(&family) != key && !d.name.is_empty() {
        report.substituted.insert(d.name.clone(), family.clone());
    }
    Some(Face { family, bold, italic, font, height_per_em })
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// A word of a run, and where the page put it.
#[derive(Clone)]
struct RunWord {
    text: String,
    start: f64,
    end: f64,
    origin: (f64, f64),
}

/// A stretch of a line set in one style, which becomes one type layer.
#[derive(Clone)]
struct Run {
    text: String,
    words: Vec<RunWord>,
    face: usize,
    em: f64,
    color: Rgba8,
    invisible: bool,
    start: f64,
    end: f64,
    /// The first glyph's origin, in document pixels.
    origin: (f64, f64),
    baseline: f64,
    seq: usize,
    direction: i32,
    mirrored: bool,
}

struct TextBuilder<'a> {
    glyphs: &'a [Glyph],
    faces: &'a [Option<Face>],
    report: &'a mut Report,
    out: Vec<(usize, NewLayer)>,
}

impl TextBuilder<'_> {
    /// The style a word is set in: that of most of its glyphs.
    fn word_style(&self, glyphs: &[usize]) -> (usize, u32, Rgba8, bool) {
        let mut counts: Vec<((usize, u32, Rgba8, bool), usize)> = Vec::new();
        for &i in glyphs {
            let g = &self.glyphs[i];
            // Sizes within 2% are one size.
            let key = (g.font, (g.size.max(0.01).ln() / 0.02).round() as i64 as u32, g.color, g.invisible);
            match counts.iter_mut().find(|(k, _)| *k == key) {
                Some((_, n)) => *n += 1,
                None => counts.push((key, 1)),
            }
        }
        counts.iter().max_by_key(|(_, n)| *n).map(|(k, _)| *k).unwrap_or((0, 0, Rgba8::BLACK, false))
    }

    fn runs(&self, line: &Line) -> Vec<Run> {
        let mut runs: Vec<Run> = Vec::new();
        let mut last_key = None;
        let mut last_end = f64::NEG_INFINITY;
        for word in &line.words {
            let key = self.word_style(&word.glyphs);
            let em = word.glyphs.iter().map(|&i| self.glyphs[i].size).fold(0.0, f64::max);
            let text: String = word.glyphs.iter().map(|&i| self.glyphs[i].text.as_str()).collect();
            let first = &self.glyphs[word.glyphs[0]];
            let run_word = RunWord { text: text.clone(), start: word.start, end: word.end, origin: (first.x, first.y) };
            let same_face = |a: usize, b: usize| {
                a == b
                    || match (&self.faces.get(a).cloned().flatten(), &self.faces.get(b).cloned().flatten()) {
                        (Some(x), Some(y)) => x.family == y.family && x.bold == y.bold && x.italic == y.italic,
                        _ => false,
                    }
            };
            let joins = match (&last_key, runs.last()) {
                (Some((f, s, c, inv)), Some(_)) => {
                    same_face(*f, key.0) && *s == key.1 && *c == key.2 && *inv == key.3 && word.start - last_end <= RUN_GAP * em
                }
                _ => false,
            };
            if joins {
                let run = runs.last_mut().expect("joins needs a run");
                run.text.push(' ');
                run.text.push_str(&text);
                run.words.push(run_word);
                run.end = word.end;
                run.em = run.em.max(em);
                run.seq = run.seq.min(word.glyphs.iter().map(|&i| self.glyphs[i].seq).min().unwrap_or(usize::MAX));
            } else {
                let (_, across) = layout::frame(first.x, first.y, line.direction);
                runs.push(Run {
                    text,
                    words: vec![run_word],
                    face: key.0,
                    em,
                    color: key.2,
                    invisible: key.3,
                    start: word.start,
                    end: word.end,
                    origin: (first.x, first.y),
                    baseline: across,
                    seq: word.glyphs.iter().map(|&i| self.glyphs[i].seq).min().unwrap_or(0),
                    direction: line.direction,
                    mirrored: first.mirrored,
                });
            }
            last_key = Some(key);
            last_end = word.end;
        }
        runs
    }

    fn line(&mut self, line: &Line) {
        for run in self.runs(line) {
            for piece in self.pieces(&run) {
                self.point_text(&piece);
            }
        }
    }

    /// A run cut where one layer cannot keep its words in place.
    ///
    /// Tracking stretches every letter, and a justified line stretched only
    /// its spaces, so stretching a whole line to its length leaves the words
    /// in its middle off where the page put them. A run is kept whole while
    /// every word stays within a fraction of an em of its place, and cut
    /// before the word that would not. On a justified page that is a layer
    /// or two more for the loosest lines, and none for the rest.
    fn pieces(&self, run: &Run) -> Vec<Run> {
        let Some(face) = self.face(run.face) else { return vec![run.clone()] };
        if run.words.len() < 2 {
            return vec![run.clone()];
        }
        let style = self.style(face, run.em, run.color);
        let width = |t: &str| cshop_core::text::layout(&TextContent::new(t, style.clone()), &face.font).width as f64;
        // A quarter of an em is a space: no word ends up further than a
        // space from where the page put it.
        let tolerance = 0.25 * run.em;
        // The worst distance between where this face, tracked to the span's
        // length, puts a word and where the page put it.
        let error = |words: &[RunWord]| -> f64 {
            let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
            let joined = texts.join(" ");
            let n = joined.chars().count() as f64;
            let target = words[words.len() - 1].end - words[0].start;
            let per_char = if n > 1.0 { (target - width(&joined)) / n } else { 0.0 };
            let mut worst: f64 = 0.0;
            for k in 1..words.len() {
                let upto = texts[..=k].join(" ");
                let at = width(&upto) - width(texts[k]);
                let chars_before = texts[..k].join(" ").chars().count() as f64 + 1.0;
                let placed = at + per_char * chars_before;
                worst = worst.max((placed - (words[k].start - words[0].start)).abs());
            }
            worst
        };
        let mut out = Vec::new();
        let mut i = 0;
        while i < run.words.len() {
            let mut j = i + 1;
            while j < run.words.len() && error(&run.words[i..=j]) <= tolerance {
                j += 1;
            }
            let words = run.words[i..j].to_vec();
            let mut piece = run.clone();
            piece.text = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
            piece.start = words[0].start;
            piece.end = words[words.len() - 1].end;
            piece.origin = words[0].origin;
            piece.words = words;
            out.push(piece);
            i = j;
        }
        out
    }

    fn face(&self, i: usize) -> Option<&Face> {
        self.faces.get(i).and_then(|f| f.as_ref())
    }

    fn style(&self, face: &Face, em: f64, color: Rgba8) -> TextStyle {
        TextStyle {
            family: face.family.clone(),
            size: face.style_size(em),
            color,
            bold: face.bold,
            italic: face.italic,
            align: TextAlign::Left,
            leading: None,
            tracking: 0.0,
            antialias: true,
        }
    }

    /// One run as point text, stretched by tracking to the length it has on
    /// the page — which also takes up the extra space of a justified line.
    fn point_text(&mut self, run: &Run) {
        let Some(face) = self.face(run.face).cloned() else {
            self.report.note("runs of text with no face to set them in");
            return;
        };
        if run.mirrored {
            *self.report.skipped.entry("mirrored text".into()).or_default() += 1;
            return;
        }
        let mut content = TextContent::new(run.text.clone(), self.style(&face, run.em, run.color));
        let natural = cshop_core::text::layout(&content, &face.font).lines.first().map_or(0.0, |l| l.width);
        let n = run.text.trim_end().chars().count();
        let target = (run.end - run.start) as f32;
        if natural > 0.0 && n > 1 && target > 0.0 {
            let per_char = (target - natural) / n as f32;
            let tracking = per_char / content.style.size * 1000.0;
            if tracking.abs() >= 0.5 {
                content.style.tracking = tracking.clamp(-150.0, 300.0);
            }
        }
        let name = content.layer_name();
        let Some(layer) = TextLayer::new(content) else {
            self.report.note("runs of text that could not be set");
            return;
        };
        let anchor = layer.anchor();
        let new = if run.direction == 0 {
            NewLayer::new(
                name,
                LayerKind::Text(Box::new(layer)),
                (run.origin.0.round() as i32 - anchor.0, run.baseline.round() as i32 - anchor.1),
            )
        } else {
            // A type layer has no angle, so turned text is set upright and
            // turned as pixels.
            self.report.note("runs of turned text drawn as pixels rather than type");
            let raster = layer.raster().clone();
            let turn = Transform::rotate((run.direction as f32).to_radians())
                .then(Transform::translate(run.origin.0 as f32, run.origin.1 as f32));
            let Some((px, offset)) = resample::transform(&raster, (-anchor.0, -anchor.1), turn, Resampling::Bilinear, None) else {
                return;
            };
            NewLayer::new(name, LayerKind::raster(px), offset)
        };
        let mut new = new;
        new.visible = !run.invisible;
        self.out.push((run.seq, new));
    }

    /// Lines gathered into paragraphs, each a wrapping text box. Lines laid
    /// out in columns — a signature block, a table — stay as they are.
    fn paragraphs(&mut self, lines: &[Line]) {
        let upright: Vec<&Line> = lines.iter().filter(|l| l.direction == 0).collect();
        let columnar = |i: usize| -> bool {
            let gaps = |l: &Line| -> Vec<f64> {
                l.words.windows(2).filter(|w| w[1].start - w[0].end > 3.0 * l.size).map(|w| w[1].start).collect()
            };
            let mine = gaps(upright[i]);
            if mine.is_empty() {
                return false;
            }
            let near = |j: usize| {
                upright.get(j).is_some_and(|o| {
                    (o.baseline - upright[i].baseline).abs() < 3.0 * upright[i].size
                        && gaps(o).iter().any(|x| mine.iter().any(|m| (m - x).abs() < upright[i].size))
                })
            };
            (i > 0 && near(i - 1)) || near(i + 1)
        };

        // Paragraphs as runs of line indices.
        let mut paras: Vec<Vec<usize>> = Vec::new();
        for i in 0..upright.len() {
            let line = upright[i];
            if columnar(i) {
                paras.push(vec![i]);
                paras.push(Vec::new()); // nothing joins a columnar line
                continue;
            }
            let joins = paras.last().filter(|p| !p.is_empty()).is_some_and(|p| {
                let first = upright[p[0]];
                let last = upright[*p.last().expect("not empty")];
                let em = first.size;
                let gap = line.baseline - last.baseline;
                let pitch_ok = p.len() < 2 || (gap - (last.baseline - upright[p[p.len() - 2]].baseline)).abs() <= 0.2 * em;
                let left = p.iter().skip(1).map(|&k| upright[k].words[0].start).fold(f64::INFINITY, f64::min);
                let x = line.words[0].start;
                let aligned = if p.len() >= 2 {
                    (x - left).abs() <= em
                } else {
                    let first_x = first.words[0].start;
                    (x - first_x).abs() <= em || (x < first_x && first_x - x <= 8.0 * em)
                };
                (line.size - em).abs() <= 0.1 * em && gap > 0.0 && gap <= 1.7 * em && pitch_ok && aligned
            });
            if joins {
                paras.last_mut().expect("checked").push(i);
            } else {
                paras.push(vec![i]);
            }
        }

        for p in paras.into_iter().filter(|p| !p.is_empty()) {
            if p.len() == 1 {
                self.line(upright[p[0]]);
            } else {
                let lines: Vec<&Line> = p.iter().map(|&i| upright[i]).collect();
                self.text_box(&lines);
            }
        }
        for line in lines.iter().filter(|l| l.direction != 0) {
            self.line(line);
        }
    }

    fn text_box(&mut self, lines: &[&Line]) {
        let runs: Vec<Vec<Run>> = lines.iter().map(|l| self.runs(l)).collect();
        // The paragraph's style is the one most of its characters are in.
        let mut weight: Vec<((usize, u32, Rgba8), usize)> = Vec::new();
        let mut styles = 0;
        for r in runs.iter().flatten() {
            let key = (r.face, (r.em * 10.0).round() as u32, r.color);
            match weight.iter_mut().find(|(k, _)| *k == key) {
                Some((_, n)) => *n += r.text.len(),
                None => {
                    styles += 1;
                    weight.push((key, r.text.len()));
                }
            }
        }
        let Some(&((face_i, _, color), _)) = weight.iter().max_by_key(|(_, n)| *n) else { return };
        let Some(face) = self.face(face_i).cloned() else {
            self.report.note("paragraphs with no face to set them in");
            return;
        };
        if styles > 1 {
            self.report.note("paragraphs that mixed styles, set in their main one");
        }
        let em = runs.iter().flatten().filter(|r| r.face == face_i).map(|r| r.em).fold(0.0, f64::max);
        let mut style = self.style(&face, em, color);
        let space = face.space(style.size).max(0.5) as f64;

        let left = lines.iter().map(|l| l.words[0].start).fold(f64::INFINITY, f64::min);
        // Each line's text as it goes into the box.
        let mut line_texts: Vec<String> = Vec::new();
        for (k, line_runs) in runs.iter().enumerate() {
            let mut t = String::new();
            if k == 0 {
                // A first line that starts further in keeps its indent.
                let indent = ((lines[0].words[0].start - left) / space).round().max(0.0) as usize;
                t.push_str(&" ".repeat(indent));
            }
            for (j, r) in line_runs.iter().enumerate() {
                if j > 0 {
                    // A tab-like gap becomes as many spaces as fill it.
                    let gap = r.start - line_runs[j - 1].end;
                    t.push_str(&" ".repeat(((gap / space).round() as usize).clamp(1, 64)));
                }
                t.push_str(&r.text);
            }
            line_texts.push(t);
        }
        let pitches: Vec<f64> = lines.windows(2).map(|w| w[1].baseline - w[0].baseline).collect();
        let mut sorted = pitches.clone();
        sorted.sort_by(f64::total_cmp);
        style.leading = Some(sorted[sorted.len() / 2] as f32);
        let fit = wrap_fit(&line_texts, &style, &face.font);
        style.tracking = fit.tracking;
        if !fit.hard_breaks.is_empty() {
            self.report.note("paragraphs with a line break kept by hand, where the face would have moved a word");
        }
        let wrap = fit.width;
        let mut text = String::new();
        for (i, t) in line_texts.iter().enumerate() {
            if i > 0 {
                let prev = &line_texts[i - 1];
                if fit.hard_breaks.contains(&(i - 1)) {
                    text.push('\n');
                } else if !prev.ends_with('-') {
                    // A line ending in a hyphen ran on into the next word.
                    text.push(' ');
                }
            }
            text.push_str(t);
        }
        let ascent = face.ascent(style.size) as f64;
        let content = TextContent { text, style, wrap_width: Some(wrap as f32) };
        let name = content.layer_name();
        let seq = runs.iter().flatten().map(|r| r.seq).min().unwrap_or(0);
        let Some(layer) = TextLayer::new(content) else {
            self.report.note("paragraphs that could not be set");
            return;
        };
        let anchor = layer.anchor();
        let top = lines[0].baseline - ascent;
        let mut new = NewLayer::new(
            name,
            LayerKind::Text(Box::new(layer)),
            (left.round() as i32 - anchor.0, top.round() as i32 - anchor.1),
        );
        new.visible = !runs.iter().flatten().all(|r| r.invisible);
        self.out.push((seq, new));
    }
}

/// How a paragraph's box is set so that this face, wrapping on its own,
/// breaks it where the page did.
struct Fit {
    /// In thousandths of an em.
    tracking: f32,
    width: f64,
    /// Lines after which the break is kept by hand, because no width puts it
    /// there.
    hard_breaks: Vec<usize>,
}

/// Every line must fit, and the next line's first word must not fit after it:
/// an interval of widths for each line. Where they all overlap, any width in
/// the overlap reproduces the page. A face that kerns where the page did not
/// shortens some lines more than others, which can leave no overlap; tracking
/// adds the same amount to every character, so a few thousandths of an em of
/// it — too little to see — usually evens that out, and the smallest that
/// works is used. When none does, the box is made wide enough for every line
/// and a line break is kept by hand wherever a word would otherwise come up.
fn wrap_fit(line_texts: &[String], style: &TextStyle, font: &FontVec) -> Fit {
    let measure = |t: &str| {
        let w = cshop_core::text::layout(&TextContent::new(t, style.clone()), font).width as f64;
        (w, t.trim_end_matches(' ').chars().count() as f64)
    };
    let fits: Vec<(f64, f64)> = line_texts.iter().map(|t| measure(t)).collect();
    // How wide the box would have to be for the next word to come up, by the
    // line it would come up onto.
    let pulls: Vec<(usize, f64, f64)> = line_texts
        .windows(2)
        .enumerate()
        .filter_map(|(i, w)| match w[1].split_whitespace().next() {
            Some(next) if !w[0].ends_with('-') => {
                let (width, n) = measure(&format!("{} {next}", w[0]));
                Some((i, width, n))
            }
            _ => None,
        })
        .collect();
    let per_mille = style.size as f64 / 1000.0;
    for step in 0..=80 {
        for t in [step as f64 * 0.25, -(step as f64) * 0.25] {
            let px = t * per_mille;
            let lo = fits.iter().map(|(w, n)| w + px * n).fold(0.0, f64::max);
            let hi = pulls.iter().map(|(_, w, n)| w + px * n).fold(f64::INFINITY, f64::min);
            if lo < hi {
                let width = if hi.is_finite() { lo + ((hi - lo) / 2.0).min(2.0) } else { lo + 1.0 };
                return Fit { tracking: t as f32, width, hard_breaks: Vec::new() };
            }
            if step == 0 {
                break;
            }
        }
    }
    let width = fits.iter().map(|(w, _)| *w).fold(0.0, f64::max) + 1.0;
    let hard_breaks = pulls.iter().filter(|(_, w, _)| *w <= width).map(|(i, _, _)| *i).collect();
    Fit { tracking: 0.0, width, hard_breaks }
}

// ---------------------------------------------------------------------------
// Shapes and images
// ---------------------------------------------------------------------------

fn v(x: f64, y: f64) -> Vec2 {
    Vec2::new(x as f32, y as f32)
}

/// Contours from path segments, each with whether it was closed.
fn contours(segs: &[Seg]) -> Vec<SubPath> {
    let mut out: Vec<SubPath> = Vec::new();
    let mut current: Vec<Anchor> = Vec::new();
    let mut closed = false;
    let flush = |current: &mut Vec<Anchor>, closed: &mut bool, out: &mut Vec<SubPath>| {
        if current.len() >= 2 {
            out.push(SubPath { anchors: std::mem::take(current), closed: *closed });
        } else {
            current.clear();
        }
        *closed = false;
    };
    for s in segs {
        match *s {
            Seg::Move(x, y) => {
                flush(&mut current, &mut closed, &mut out);
                current.push(Anchor::corner(v(x, y)));
            }
            Seg::Line(x, y) => current.push(Anchor::corner(v(x, y))),
            Seg::Curve(x1, y1, x2, y2, x3, y3) => {
                if let Some(last) = current.last_mut() {
                    last.out_handle = v(x1, y1);
                    current.push(Anchor { at: v(x3, y3), in_handle: v(x2, y2), out_handle: v(x3, y3) });
                }
            }
            Seg::Close => {
                // The closing point usually repeats the first; fold it in.
                if current.len() >= 2 {
                    let (first, last) = (current[0], current[current.len() - 1]);
                    if (first.at.x - last.at.x).abs() < 0.01 && (first.at.y - last.at.y).abs() < 0.01 {
                        current[0].in_handle = last.in_handle;
                        current.pop();
                    }
                }
                closed = true;
                flush(&mut current, &mut closed, &mut out);
            }
        }
    }
    flush(&mut current, &mut closed, &mut out);
    out
}

fn blend_mode(name: &Option<Vec<u8>>) -> BlendMode {
    let Some(name) = name else { return BlendMode::Normal };
    match name.as_slice() {
        b"Multiply" => BlendMode::Multiply,
        b"Screen" => BlendMode::Screen,
        b"Overlay" => BlendMode::Overlay,
        b"Darken" => BlendMode::Darken,
        b"Lighten" => BlendMode::Lighten,
        b"ColorDodge" => BlendMode::ColorDodge,
        b"ColorBurn" => BlendMode::ColorBurn,
        b"HardLight" => BlendMode::HardLight,
        b"SoftLight" => BlendMode::SoftLight,
        b"Difference" => BlendMode::Difference,
        b"Exclusion" => BlendMode::Exclusion,
        b"Hue" => BlendMode::Hue,
        b"Saturation" => BlendMode::Saturation,
        b"Color" => BlendMode::Color,
        b"Luminosity" => BlendMode::Luminosity,
        _ => BlendMode::Normal,
    }
}

fn shape_layers(path: &PathPaint) -> Vec<NewLayer> {
    let Some(b) = content::bounds(&path.segs) else { return Vec::new() };
    if let Some(c) = path.clip {
        let pad = path.stroke_width;
        if b[2] + pad < c[0] || b[0] - pad > c[2] || b[3] + pad < c[1] || b[1] - pad > c[3] {
            return Vec::new(); // wholly clipped away
        }
    }
    let subpaths = contours(&path.segs);
    if subpaths.is_empty() {
        return Vec::new();
    }
    let blend = blend_mode(&path.blend);
    let all_closed = subpaths.iter().all(|s| s.closed);
    let style = |fill: Option<Rgba8>, stroke: Option<Rgba8>| ShapeStyle {
        fill,
        stroke,
        stroke_width: path.stroke_width as f32,
        // PDF strokes straddle the path, which is what centre means.
        stroke_align: StrokeAlign::Center,
        antialias: true,
    };

    // Fill and stroke together need every contour closed; an open one is
    // filled as if closed and stroked open, which takes two layers.
    let mut jobs: Vec<(Option<Rgba8>, Option<Rgba8>, bool)> = Vec::new();
    match (path.fill, path.stroke) {
        (Some(f), Some(s)) if !all_closed => {
            jobs.push((Some(f), None, true));
            jobs.push((None, Some(s), false));
        }
        (f, s) => jobs.push((f, s, f.is_some())),
    }

    let mut out = Vec::new();
    for (fill, stroke, close_all) in jobs {
        // An axis-aligned rectangle stays one: editable as a rectangle, and
        // far cheaper to draw than a path.
        let (kind, origin, size) = if content::is_rectangle(&path.segs) && (fill.is_some() || all_closed) {
            let (w, h) = ((b[2] - b[0]) as f32, (b[3] - b[1]) as f32);
            if w < 0.5 && h < 0.5 {
                continue;
            }
            (ShapeKind::Rectangle { radius: 0.0 }, (b[0], b[1]), (w.max(1.0), h.max(1.0)))
        } else {
            let (ox, oy) = (b[0].floor(), b[1].floor());
            let local: Vec<SubPath> = subpaths
                .iter()
                .map(|s| SubPath {
                    anchors: s
                        .anchors
                        .iter()
                        .map(|a| Anchor {
                            at: Vec2::new(a.at.x - ox as f32, a.at.y - oy as f32),
                            in_handle: Vec2::new(a.in_handle.x - ox as f32, a.in_handle.y - oy as f32),
                            out_handle: Vec2::new(a.out_handle.x - ox as f32, a.out_handle.y - oy as f32),
                        })
                        .collect(),
                    closed: s.closed || close_all,
                })
                .collect();
            // Even-odd: each contour exclusive-ors the ones before, which is
            // what an even count of crossings means. Non-zero is one operand.
            let shape = if path.even_odd && fill.is_some() && local.len() > 1 {
                PathShape {
                    parts: local.into_iter().map(|s| PathPart { subpaths: vec![s], op: BoolOp::Exclude }).collect(),
                }
            } else {
                PathShape::new(local)
            };
            let size = (((b[2] - ox).ceil() as f32).max(1.0), ((b[3] - oy).ceil() as f32).max(1.0));
            (ShapeKind::Path(shape), (ox, oy), size)
        };
        let content = ShapeContent::new(kind, size, style(fill, stroke));
        let name = content.layer_name();
        let Some(layer) = ShapeLayer::new(content) else { continue };
        let anchor = layer.anchor();
        let mut new = NewLayer::new(
            name,
            LayerKind::Shape(Box::new(layer)),
            (origin.0.round() as i32 - anchor.0, origin.1.round() as i32 - anchor.1),
        );
        new.blend = blend;
        out.push(new);
    }
    out
}

fn image_layer(image: ImagePaint, page: IRect) -> Option<NewLayer> {
    let m = image.matrix;
    let t = Transform { m: [[m[0] as f32, m[2] as f32, m[4] as f32], [m[1] as f32, m[3] as f32, m[5] as f32], [0.0, 0.0, 1.0]] };
    let mut clip = page;
    if let Some(c) = image.clip {
        let (x0, y0) = (c[0].floor().max(0.0) as i32, c[1].floor().max(0.0) as i32);
        let (x1, y1) = (c[2].ceil() as i32, c[3].ceil() as i32);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        clip = IRect::at(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32);
    }
    let (px, offset) = resample::transform(&image.pixels, (0, 0), t, Resampling::Bilinear, Some(clip))?;
    if px.width() == 0 || px.height() == 0 {
        return None;
    }
    let mut new = NewLayer::new("Image", LayerKind::raster(px), offset);
    new.opacity = image.opacity;
    new.blend = blend_mode(&image.blend);
    Some(new)
}
