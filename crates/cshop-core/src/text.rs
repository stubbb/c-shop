//! Type: layout and rasterisation.
//!
//! A text layer keeps its [`TextContent`] and a raster of it. Everything
//! downstream — the compositor, masks, blend modes, filters — sees only the
//! raster, so type needs no special case anywhere except where it is edited.
//! Re-rasterising happens whenever the content changes, which is cheap enough
//! at typing speed.
//!
//! # Where the text sits
//!
//! The raster covers the layout box plus a margin, and records where the
//! text's anchor — the click point — falls inside it. That keeps
//! [`crate::layer::Layer::offset`] the single source of position: moving the
//! layer needs no knowledge of type at all, and re-rasterising after an edit
//! just puts the new anchor back where the old one was.

use crate::color::Rgba8;
use crate::geom::IRect;
use crate::pixels::PixelBuffer;
use ab_glyph::{Font, FontVec, Glyph, GlyphId, PxScale, ScaleFont};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl TextAlign {
    pub fn name(self) -> &'static str {
        match self {
            TextAlign::Left => "Left",
            TextAlign::Center => "Center",
            TextAlign::Right => "Right",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    pub family: String,
    /// Cap height in pixels, as type size is always given.
    pub size: f32,
    pub color: Rgba8,
    pub bold: bool,
    pub italic: bool,
    pub align: TextAlign,
    /// Distance between baselines. `None` follows the font's own metrics,
    /// which is conventionally called auto leading.
    pub leading: Option<f32>,
    /// Letter spacing in thousandths of an em, the conventional unit.
    pub tracking: f32,
    pub antialias: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            family: String::new(),
            size: 48.0,
            color: Rgba8::BLACK,
            bold: false,
            italic: false,
            align: TextAlign::Left,
            leading: None,
            tracking: 0.0,
            antialias: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextContent {
    pub text: String,
    pub style: TextStyle,
    /// Width of the paragraph box. `None` is point text, which grows instead
    /// of wrapping and breaks only where the text itself does.
    pub wrap_width: Option<f32>,
}

impl TextContent {
    pub fn new(text: impl Into<String>, style: TextStyle) -> Self {
        Self { text: text.into(), style, wrap_width: None }
    }

    /// The name such a layer takes in the panel: type layers are named
    /// after their first line.
    pub fn layer_name(&self) -> String {
        let first = self.text.lines().next().unwrap_or("").trim();
        if first.is_empty() {
            "Type Layer".to_string()
        } else if first.chars().count() > 28 {
            let short: String = first.chars().take(28).collect();
            format!("{short}…")
        } else {
            first.to_string()
        }
    }
}

/// One positioned glyph within a laid-out line.
#[derive(Debug, Clone, Copy)]
pub struct PlacedGlyph {
    pub id: GlyphId,
    /// Pen position, relative to the layout box's top-left.
    pub x: f32,
    pub baseline: f32,
}

#[derive(Debug, Clone)]
pub struct Line {
    pub glyphs: Vec<PlacedGlyph>,
    pub width: f32,
    pub baseline: f32,
    /// Byte range of this line within the source text, so a caret can be
    /// mapped back to a character.
    pub range: std::ops::Range<usize>,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub lines: Vec<Line>,
    /// Width of the widest line, or the box width when wrapping.
    pub width: f32,
    pub height: f32,
    pub ascent: f32,
    pub line_height: f32,
}

/// The faux-italic slant, in x per y. About twelve degrees, which is what most
/// upright faces are slanted by when no true italic exists.
const FAUX_SLANT: f32 = 0.21;

fn scaled(font: &FontVec, size: f32) -> impl ScaleFont<&FontVec> {
    font.as_scaled(PxScale::from(size.max(1.0)))
}

/// Lay the text out, in a space whose origin is the layout box's top-left.
pub fn layout(content: &TextContent, font: &FontVec) -> Layout {
    let style = &content.style;
    let f = scaled(font, style.size);
    let ascent = f.ascent();
    let auto_leading = f.height() + f.line_gap();
    let line_height = style.leading.filter(|v| *v > 0.0).unwrap_or(auto_leading);
    // Tracking is per em, and the em is the type size.
    let tracking = style.tracking / 1000.0 * style.size;

    // Measure a run of text, returning its advance.
    let advance = |s: &str| -> f32 {
        let mut w = 0.0;
        let mut prev: Option<GlyphId> = None;
        for c in s.chars() {
            let id = f.glyph_id(c);
            if let Some(p) = prev {
                w += f.kern(p, id);
            }
            w += f.h_advance(id) + tracking;
            prev = Some(id);
        }
        w
    };

    // Split into display lines: hard breaks always, plus word wrapping when
    // there is a box to wrap inside.
    let mut rows: Vec<std::ops::Range<usize>> = Vec::new();
    let mut start = 0usize;
    for (i, c) in content.text.char_indices() {
        if c == '\n' {
            rows.push(start..i);
            start = i + c.len_utf8();
        }
    }
    rows.push(start..content.text.len());

    if let Some(max) = content.wrap_width.filter(|w| *w > 0.0) {
        rows = rows.into_iter().flat_map(|r| wrap_row(&content.text, r, max, &advance)).collect();
    }

    let mut lines = Vec::new();
    let mut widest: f32 = 0.0;
    for (i, range) in rows.iter().enumerate() {
        let text = &content.text[range.clone()];
        let width = advance(text.trim_end_matches(' '));
        widest = widest.max(width);
        lines.push(Line {
            glyphs: Vec::new(),
            width,
            baseline: ascent + i as f32 * line_height,
            range: range.clone(),
        });
    }

    // Alignment needs every width first, so glyphs are placed on a second pass.
    let box_width = content.wrap_width.filter(|w| *w > 0.0).unwrap_or(widest);
    for line in &mut lines {
        let mut pen = match style.align {
            TextAlign::Left => 0.0,
            TextAlign::Center => (box_width - line.width) / 2.0,
            TextAlign::Right => box_width - line.width,
        };
        let mut prev: Option<GlyphId> = None;
        for c in content.text[line.range.clone()].chars() {
            let id = f.glyph_id(c);
            if let Some(p) = prev {
                pen += f.kern(p, id);
            }
            if !c.is_whitespace() {
                line.glyphs.push(PlacedGlyph { id, x: pen, baseline: line.baseline });
            }
            pen += f.h_advance(id) + tracking;
            prev = Some(id);
        }
    }

    let height = if lines.is_empty() { line_height } else { lines.len() as f32 * line_height };
    Layout { lines, width: box_width.max(widest), height, ascent, line_height }
}

/// Break one paragraph into rows no wider than `max`.
///
/// Breaks at spaces; a single word longer than the box is broken mid-word
/// rather than overflowing, which is what a text box is for.
fn wrap_row(
    text: &str,
    range: std::ops::Range<usize>,
    max: f32,
    advance: &impl Fn(&str) -> f32,
) -> Vec<std::ops::Range<usize>> {
    let row = &text[range.clone()];
    if row.is_empty() || advance(row) <= max {
        return vec![range];
    }

    let mut out = Vec::new();
    let mut line_start = range.start;
    // The last space seen since `line_start`, as the place to break.
    let mut last_break: Option<usize> = None;

    for (i, c) in row.char_indices() {
        let abs = range.start + i;
        if c == ' ' {
            last_break = Some(abs);
        }
        if advance(&text[line_start..abs + c.len_utf8()]) > max && abs > line_start {
            let (end, next) = match last_break.filter(|b| *b > line_start) {
                Some(b) => (b, b + 1),
                // No space to break at, so break before the character that
                // did not fit.
                None => (abs, abs),
            };
            out.push(line_start..end);
            line_start = next;
            last_break = None;
        }
    }
    out.push(line_start..range.end);
    out
}

/// A rendered text layer.
pub struct Rasterized {
    pub pixels: PixelBuffer,
    /// Where the text's anchor falls inside `pixels`. For point text that is
    /// the start of the first baseline; for a box, its top-left corner.
    pub anchor: (i32, i32),
    /// Where the layout box's top-left falls inside `pixels`. Caret geometry
    /// is measured from there, so hit-testing needs it in document space.
    pub origin: (i32, i32),
}

/// Margin around the layout box, for antialiasing, italic overhang and glyphs
/// that reach past their advance.
fn margin(size: f32) -> i32 {
    (size * 0.4).ceil().max(4.0) as i32
}

/// Render the text. `None` when the family cannot be loaded or there is
/// nothing to draw.
pub fn rasterize(content: &TextContent, font: &FontVec, faux_italic: bool, faux_bold: bool) -> Option<Rasterized> {
    let style = &content.style;
    let laid = layout(content, font);
    let m = margin(style.size);

    // The buffer covers the layout box plus the margin. Anchoring to the box
    // rather than the ink keeps the layer still while the text is edited.
    let box_w = laid.width.ceil().max(1.0) as i32;
    let box_h = laid.height.ceil().max(1.0) as i32;
    // Alignment grows point text away from the anchor, so the box's left edge
    // moves relative to it.
    let left = match (content.wrap_width, style.align) {
        (Some(_), _) | (None, TextAlign::Left) => 0.0,
        (None, TextAlign::Center) => -laid.width / 2.0,
        (None, TextAlign::Right) => -laid.width,
    };
    // Point text is anchored on its first baseline, a box on its top-left.
    let top = if content.wrap_width.is_some() { 0.0 } else { -laid.ascent };

    let width = (box_w + 2 * m) as u32;
    let height = (box_h + 2 * m) as u32;
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return None;
    }
    let anchor = (m - left.floor() as i32, m - top.floor() as i32);

    // Accumulate coverage first, then colour it once. Overlapping glyphs take
    // the larger coverage rather than adding, or a script face would darken
    // wherever its letters join.
    let mut coverage = vec![0f32; (width * height) as usize];
    let f = scaled(font, style.size);
    let bold_smear = if faux_bold { (style.size / 26.0).max(0.7) } else { 0.0 };

    for line in &laid.lines {
        for placed in &line.glyphs {
            // The layout box's top-left sits at (m, m) in the buffer, and
            // glyph positions are relative to that box.
            let pen_x = m as f32 + placed.x;
            let baseline_px = m as f32 + placed.baseline;
            // Faux italic shears about the baseline, so the letter leans
            // without drifting off it.
            let slant = if faux_italic { FAUX_SLANT } else { 0.0 };

            for pass in 0..if bold_smear > 0.0 { 2 } else { 1 } {
                let dx = pass as f32 * bold_smear;
                let glyph: Glyph = placed.id.with_scale_and_position(
                    PxScale::from(style.size.max(1.0)),
                    ab_glyph::point(pen_x + dx, baseline_px),
                );
                let Some(outline) = f.font().outline_glyph(glyph) else { continue };
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, c| {
                    if c <= 0.0 {
                        return;
                    }
                    let py = bounds.min.y + gy as f32;
                    let px = bounds.min.x + gx as f32 + (baseline_px - py) * slant;
                    let (xi, yi) = (px.round() as i32, py.round() as i32);
                    if xi < 0 || yi < 0 || xi >= width as i32 || yi >= height as i32 {
                        return;
                    }
                    let slot = &mut coverage[(yi as u32 * width + xi as u32) as usize];
                    *slot = slot.max(c);
                });
            }
        }
    }

    let mut pixels = PixelBuffer::new(width, height);
    let colour = style.color;
    for (i, c) in coverage.iter().enumerate() {
        let mut a = c.clamp(0.0, 1.0);
        if !style.antialias {
            a = if a >= 0.5 { 1.0 } else { 0.0 };
        }
        if a <= 0.0 {
            continue;
        }
        let alpha = (a * colour.a as f32).round().clamp(0.0, 255.0) as u8;
        pixels.pixels_mut()[i] = Rgba8::new(colour.r, colour.g, colour.b, alpha);
    }

    Some(Rasterized { pixels, anchor, origin: (m, m) })
}

/// Load the face a style asks for and render it, synthesising bold or italic
/// when the family has no such face.
pub fn render(content: &TextContent) -> Option<Rasterized> {
    let db = crate::font::FontDb::global();
    let style = &content.style;
    let font: Arc<FontVec> = db.load(&style.family, style.bold, style.italic)?;
    let exact = db.has_exact(&style.family, style.bold, style.italic);
    let faux_italic = style.italic && !exact;
    let faux_bold = style.bold && !exact;
    rasterize(content, &font, faux_italic, faux_bold)
}

/// Bounds of the rendered text relative to its anchor, without rendering it.
pub fn measure(content: &TextContent) -> Option<IRect> {
    let db = crate::font::FontDb::global();
    let font = db.load(&content.style.family, content.style.bold, content.style.italic)?;
    let laid = layout(content, &font);
    let left = match (content.wrap_width, content.style.align) {
        (Some(_), _) | (None, TextAlign::Left) => 0.0,
        (None, TextAlign::Center) => -laid.width / 2.0,
        (None, TextAlign::Right) => -laid.width,
    };
    let top = if content.wrap_width.is_some() { 0.0 } else { -laid.ascent };
    Some(IRect::at(
        left.floor() as i32,
        top.floor() as i32,
        laid.width.ceil().max(1.0) as u32,
        laid.height.ceil().max(1.0) as u32,
    ))
}

// ---------------------------------------------------------------------------
// Caret geometry
// ---------------------------------------------------------------------------

/// Where a caret sits, in the same space [`layout`] works in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caret {
    pub x: f32,
    /// Top of the caret bar.
    pub top: f32,
    pub height: f32,
    /// Index of the display line the caret is on.
    pub line: usize,
}

/// The line a byte index belongs to.
///
/// A byte at a wrap point belongs to the end of the earlier line, which is
/// where someone who has just typed the last word of it expects to be.
fn line_of(laid: &Layout, byte: usize) -> usize {
    for (i, line) in laid.lines.iter().enumerate() {
        if byte <= line.range.end {
            return i;
        }
    }
    laid.lines.len().saturating_sub(1)
}

/// Advance of a run, in the same units the layout uses.
fn run_width<F: Font, S: ScaleFont<F>>(f: &S, s: &str, tracking: f32) -> f32 {
    let mut w = 0.0;
    let mut prev: Option<GlyphId> = None;
    for c in s.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            w += f.kern(p, id);
        }
        w += f.h_advance(id) + tracking;
        prev = Some(id);
    }
    w
}

pub fn caret_at(content: &TextContent, font: &FontVec, byte: usize) -> Caret {
    let laid = layout(content, font);
    let f = scaled(font, content.style.size);
    let tracking = content.style.tracking / 1000.0 * content.style.size;
    let byte = byte.min(content.text.len());
    let i = line_of(&laid, byte);
    let line = &laid.lines[i.min(laid.lines.len().saturating_sub(1))];

    let start = line.range.start.min(content.text.len());
    let end = byte.clamp(start, line.range.end.min(content.text.len()));
    let run = &content.text[start..end];
    let box_width = content.wrap_width.filter(|w| *w > 0.0).unwrap_or(laid.width);
    let indent = match content.style.align {
        TextAlign::Left => 0.0,
        TextAlign::Center => (box_width - line.width) / 2.0,
        TextAlign::Right => box_width - line.width,
    };

    Caret {
        x: indent + run_width(&f, run, tracking),
        top: line.baseline - laid.ascent,
        height: laid.line_height,
        line: i,
    }
}

/// The byte index nearest a point in layout space, for clicking into text.
pub fn byte_at(content: &TextContent, font: &FontVec, x: f32, y: f32) -> usize {
    let laid = layout(content, font);
    if laid.lines.is_empty() {
        return 0;
    }
    let f = scaled(font, content.style.size);
    let tracking = content.style.tracking / 1000.0 * content.style.size;

    let i = ((y / laid.line_height.max(1.0)).floor() as isize)
        .clamp(0, laid.lines.len() as isize - 1) as usize;
    let line = &laid.lines[i];
    let box_width = content.wrap_width.filter(|w| *w > 0.0).unwrap_or(laid.width);
    let indent = match content.style.align {
        TextAlign::Left => 0.0,
        TextAlign::Center => (box_width - line.width) / 2.0,
        TextAlign::Right => box_width - line.width,
    };

    // Walk the line, stopping at the character whose midpoint the click passed.
    let mut pen = indent;
    let mut prev: Option<GlyphId> = None;
    for (off, c) in content.text[line.range.clone()].char_indices() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            pen += f.kern(p, id);
        }
        let adv = f.h_advance(id) + tracking;
        if x < pen + adv / 2.0 {
            return line.range.start + off;
        }
        pen += adv;
        prev = Some(id);
    }
    line.range.end
}

/// Move a caret one display line, keeping roughly the same x.
pub fn caret_line_step(content: &TextContent, font: &FontVec, byte: usize, down: bool) -> usize {
    let laid = layout(content, font);
    if laid.lines.is_empty() {
        return byte;
    }
    let here = caret_at(content, font, byte);
    let target = here.line as isize + if down { 1 } else { -1 };
    if target < 0 || target >= laid.lines.len() as isize {
        // Off the top or bottom: go to the very start or end, as editors do.
        return if down { content.text.len() } else { 0 };
    }
    let y = target as f32 * laid.line_height + laid.line_height / 2.0;
    byte_at(content, font, here.x, y)
}

/// Start and end of the display line a byte is on, for Home and End.
pub fn line_bounds(content: &TextContent, font: &FontVec, byte: usize) -> (usize, usize) {
    let laid = layout(content, font);
    if laid.lines.is_empty() {
        return (0, 0);
    }
    let line = &laid.lines[line_of(&laid, byte.min(content.text.len()))];
    (line.range.start, line.range.end)
}
