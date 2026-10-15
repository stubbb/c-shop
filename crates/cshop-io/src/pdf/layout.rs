//! Reading placed glyphs back as lines and words.
//!
//! Reading order comes from where glyphs land, not from the order of the
//! stream, because producers emit text in any order they like — PDFium, for
//! one, writes each word in its own text object under its own transform.
//! Glyphs are grouped by writing direction, then into lines by baseline, then
//! sorted along the line; a gap wider than a fraction of the em between
//! neighbours is a word break.

use super::content::Glyph;

/// Of the em: a wider gap between two glyphs is a space.
pub const WORD_GAP: f64 = 0.15;
/// Of the em: baselines closer than this share a line.
const LINE_TOLERANCE: f64 = 0.5;

/// A word: its glyphs, left to right, and where it starts and ends along
/// the line.
#[derive(Debug, Clone)]
pub struct Word {
    pub glyphs: Vec<usize>,
    pub start: f64,
    pub end: f64,
}

/// A line of words in one writing direction.
#[derive(Debug, Clone)]
pub struct Line {
    pub words: Vec<Word>,
    /// Distance across the page, in the line's own frame: down the page for
    /// upright text.
    pub baseline: f64,
    /// The largest em on the line.
    pub size: f64,
    /// 0, 90, 180 or 270 when within a few degrees of one; otherwise the
    /// rounded angle.
    pub direction: i32,
}

/// Snap a baseline angle to a right angle when it is within 5° of one.
pub fn direction(angle: f64) -> i32 {
    let snapped = (angle / 90.0).round() * 90.0;
    let off = ((angle - snapped + 180.0).rem_euclid(360.0) - 180.0).abs();
    let d = if off < 5.0 { snapped } else { angle.round() };
    (d as i32).rem_euclid(360)
}

/// (along, across) of a point, in a frame whose x runs along `direction`.
pub fn frame(x: f64, y: f64, direction: i32) -> (f64, f64) {
    let (sin, cos) = (direction as f64).to_radians().sin_cos();
    (x * cos + y * sin, -x * sin + y * cos)
}

/// Group glyphs into lines, upright text first and top to bottom.
pub fn lines(glyphs: &[Glyph]) -> Vec<Line> {
    let mut by_direction: Vec<(i32, Vec<usize>)> = Vec::new();
    for (i, g) in glyphs.iter().enumerate() {
        let d = direction(g.angle);
        match by_direction.iter_mut().find(|(k, _)| *k == d) {
            Some((_, v)) => v.push(i),
            None => by_direction.push((d, vec![i])),
        }
    }
    by_direction.sort_by_key(|(d, _)| (*d != 0, *d));
    let mut out = Vec::new();
    for (d, members) in by_direction {
        out.extend(lines_in_direction(glyphs, &members, d));
    }
    out
}

fn lines_in_direction(glyphs: &[Glyph], members: &[usize], direction: i32) -> Vec<Line> {
    struct Placed {
        across: f64,
        along: f64,
        index: usize,
    }
    let mut placed: Vec<Placed> = members
        .iter()
        .map(|&i| {
            let (along, across) = frame(glyphs[i].x, glyphs[i].y, direction);
            Placed { across, along, index: i }
        })
        .collect();
    placed.sort_by(|a, b| a.across.total_cmp(&b.across).then(a.along.total_cmp(&b.along)));

    // Rows: [baseline, size, members]. Only the last few rows can still take
    // a glyph, since they are visited in order down the page.
    let mut rows: Vec<(f64, f64, Vec<Placed>)> = Vec::new();
    for p in placed {
        let size = glyphs[p.index].size;
        let n = rows.len();
        let hit = rows[n.saturating_sub(3)..]
            .iter()
            .rposition(|r| (r.0 - p.across).abs() <= LINE_TOLERANCE * r.1.max(size).max(1e-6))
            .map(|k| n.saturating_sub(3) + k);
        match hit {
            Some(k) => {
                let row = &mut rows[k];
                if size > row.1 {
                    // The biggest text on a line sets its baseline.
                    row.0 = p.across;
                    row.1 = size;
                }
                row.2.push(p);
            }
            None => rows.push((p.across, size, vec![p])),
        }
    }

    let mut lines = Vec::new();
    for (baseline, size, mut row) in rows {
        row.sort_by(|a, b| a.along.total_cmp(&b.along));
        let mut words: Vec<Word> = Vec::new();
        let mut current: Vec<usize> = Vec::new();
        let mut start = 0.0;
        let mut prev: Option<(&Placed, f64)> = None; // and where it ended
        for p in &row {
            let g = &glyphs[p.index];
            let advance = g.advance;
            if let Some((q, _)) = prev {
                let h = &glyphs[q.index];
                // Overprinted to fake bold: the same glyph a hair to the side.
                if h.text == g.text && (p.along - q.along).abs() < 0.1 * g.size && (p.across - q.across).abs() < 0.1 * g.size {
                    continue;
                }
            }
            if g.text.chars().all(char::is_whitespace) {
                if !current.is_empty() {
                    let end = prev.map_or(start, |(_, e)| e);
                    words.push(Word { glyphs: std::mem::take(&mut current), start, end });
                }
            } else {
                if let Some((q, end)) = prev {
                    let em = g.size.max(glyphs[q.index].size);
                    if !current.is_empty() && p.along - end > WORD_GAP * em {
                        words.push(Word { glyphs: std::mem::take(&mut current), start, end });
                    }
                }
                if current.is_empty() {
                    start = p.along;
                }
                current.push(p.index);
            }
            prev = Some((p, p.along + advance));
        }
        if !current.is_empty() {
            let end = prev.map_or(start, |(_, e)| e);
            words.push(Word { glyphs: current, start, end });
        }
        if !words.is_empty() {
            lines.push(Line { words, baseline, size, direction });
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use cshop_core::color::Rgba8;

    fn glyph(text: &str, x: f64, y: f64, size: f64) -> Glyph {
        Glyph {
            text: text.into(),
            x,
            y,
            advance: 0.6 * size,
            size,
            angle: 0.0,
            mirrored: false,
            font: 0,
            color: Rgba8::BLACK,
            invisible: false,
            seq: 0,
        }
    }

    fn text(lines: &[Line], glyphs: &[Glyph]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.words
                    .iter()
                    .map(|w| w.glyphs.iter().map(|&i| glyphs[i].text.as_str()).collect::<String>())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    #[test]
    fn lines_run_top_to_bottom_and_words_split_on_gaps() {
        // y grows down the page; the second line is given first.
        let mut g = Vec::new();
        for (i, c) in "two".chars().enumerate() {
            g.push(glyph(&c.to_string(), 10.0 + 6.0 * i as f64, 40.0, 10.0));
        }
        for (i, c) in "one".chars().enumerate() {
            g.push(glyph(&c.to_string(), 10.0 + 6.0 * i as f64, 20.0, 10.0));
        }
        // "far" 4 px after "one" ends (28): a 0.4 em gap, so a new word.
        for (i, c) in "far".chars().enumerate() {
            g.push(glyph(&c.to_string(), 32.0 + 6.0 * i as f64, 21.0, 10.0));
        }
        let l = lines(&g);
        assert_eq!(text(&l, &g), ["one far", "two"]);
    }

    #[test]
    fn overprinted_bold_reads_once() {
        let mut g = Vec::new();
        for dx in [0.0, 0.3] {
            for (i, c) in "Bold".chars().enumerate() {
                g.push(glyph(&c.to_string(), 10.0 + dx + 6.0 * i as f64, 20.0, 10.0));
            }
        }
        assert_eq!(text(&lines(&g), &g), ["Bold"]);
    }

    #[test]
    fn directions_snap() {
        assert_eq!(direction(0.4), 0);
        assert_eq!(direction(-89.0), 270);
        assert_eq!(direction(90.0), 90);
        assert_eq!(direction(30.0), 30);
    }
}
