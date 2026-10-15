//! PDF, in and out.
//!
//! # Out
//!
//! A document goes out as one page holding the composite — see [`write`].
//!
//! # In: a page becomes layers
//!
//! Reading a PDF is not rendering one. The editor already has type, shapes and
//! pictures, and a PDF page is mostly those three things placed precisely, so
//! a page comes in as layers of them: every run of text a type layer set in an
//! installed face, every painted path a shape layer, every image a raster
//! layer — stacked in the order the page painted them, so what covers what is
//! kept. The result is a page that can be edited, not a picture of one.
//!
//! Getting there is a reader for the whole file format, written here as the
//! SVG reader is: the syntax ([`syntax`]), the file structure with its
//! cross-reference tables, object streams and repair of damaged files
//! (`file`), the stream filters (`filters`), fonts and their encodings
//! (`fonts`), a content-stream interpreter (`content`), images (`images`), the
//! grouping of placed glyphs into lines and words (`layout`), and finally the
//! layers (`import`).
//!
//! # What is not kept, and how it says so
//!
//! Glyph outlines: text is retyped in the nearest installed face, chosen to
//! keep line lengths (Liberation for Times, Arial and Courier is metric
//! compatible), and the report names every substitution. Gradients, pattern
//! fills, soft masks, dashes and non-rectangular clipping; JPEG 2000, JBIG2
//! and fax images; encrypted files. Each is counted and reported rather than
//! dropped quietly — a page missing its gradient looks like a bug in this
//! program unless something says otherwise.

mod content;
mod file;
mod filters;
mod fonts;
mod images;
mod import;
mod layout;
pub mod syntax;
mod write;

pub use write::write;

use crate::IoError;
use cshop_core::blend::BlendMode;
use cshop_core::color::Rgba8;
use cshop_core::layer::{FillStyle, Layer, LayerId, LayerKind};
use syntax::Object;

/// How text comes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextMode {
    /// A type layer for each run of a line: every line exactly where it was,
    /// at the length it was. Editing one does not reflow the paragraph.
    #[default]
    Lines,
    /// A wrapping text box for each paragraph, which reflows when edited.
    /// Line breaks fall where the face puts them, which with a metric
    /// compatible face is nearly always where they were.
    Paragraphs,
}

impl TextMode {
    pub fn name(self) -> &'static str {
        match self {
            TextMode::Lines => "A type layer per line",
            TextMode::Paragraphs => "A type layer per paragraph",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadOptions {
    /// Pixels per inch: how large a page becomes. A point is 1/72 inch.
    pub dpi: f32,
    pub text: TextMode,
    /// Put a white layer under each page, as the paper it was printed on.
    pub paper: bool,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self { dpi: 300.0, text: TextMode::Lines, paper: true }
    }
}

/// A layer that does not yet belong to a document: it gets its id from the
/// tree it goes into.
pub struct NewLayer {
    pub name: String,
    pub kind: LayerKind,
    pub offset: (i32, i32),
    pub visible: bool,
    pub opacity: f32,
    pub blend: BlendMode,
}

impl NewLayer {
    pub fn new(name: impl Into<String>, kind: LayerKind, offset: (i32, i32)) -> Self {
        Self { name: name.into(), kind, offset, visible: true, opacity: 1.0, blend: BlendMode::Normal }
    }

    pub fn into_layer(self, id: LayerId) -> Layer {
        let mut layer = Layer::new(id, self.name, self.kind);
        layer.offset = self.offset;
        layer.visible = self.visible;
        layer.opacity = self.opacity;
        layer.blend_mode = self.blend;
        layer
    }
}

/// One page's layers, bottom to top, and its size in pixels.
pub struct Page {
    pub number: usize,
    pub width: u32,
    pub height: u32,
    pub layers: Vec<NewLayer>,
}

pub struct Imported {
    pub pages: Vec<Page>,
    /// The document's title, when it gives one.
    pub title: Option<String>,
    /// What was substituted or left out, one sentence each.
    pub notes: Vec<String>,
}

/// The most pages one file may bring in.
pub const MAX_PAGES: usize = 500;

pub fn is_pdf(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(1024)].windows(5).any(|w| w == b"%PDF-")
}

fn open(bytes: &[u8]) -> Result<file::File, IoError> {
    file::File::open(bytes.to_vec()).map_err(|e| {
        if e.contains("encrypted") { IoError::Unsupported(e) } else { IoError::Malformed(e) }
    })
}

/// How many pages a file has, without reading them.
pub fn page_count(bytes: &[u8]) -> Result<usize, IoError> {
    Ok(open(bytes)?.pages().len())
}

/// Each page's size in points as it is shown — turned by its /Rotate —
/// without reading what is on it. What an import dialog needs to say how big
/// the result will be before anything is read.
pub fn page_sizes(bytes: &[u8]) -> Result<Vec<(f32, f32)>, IoError> {
    Ok(open(bytes)?
        .pages()
        .iter()
        .map(|p| {
            let (w, h) = (p.bbox[2] - p.bbox[0], p.bbox[3] - p.bbox[1]);
            if p.rotate % 180 == 90 { (h, w) } else { (w, h) }
        })
        .collect())
}

/// The page-to-document transform: default user space to pixels, y down,
/// the crop box's corner at the origin and the page turned by its /Rotate.
fn page_matrix(bbox: [f32; 4], rotate: u32, scale: f64) -> content::Matrix {
    let [x0, y0, x1, y1] = bbox.map(|v| v as f64);
    let s = scale;
    match rotate {
        90 => [0.0, s, s, 0.0, -y0 * s, -x0 * s],
        180 => [-s, 0.0, 0.0, s, x1 * s, -y0 * s],
        270 => [0.0, -s, -s, 0.0, y1 * s, x1 * s],
        _ => [s, 0.0, 0.0, -s, -x0 * s, y1 * s],
    }
}

/// Read every page as layers.
pub fn read(bytes: &[u8], options: &ReadOptions) -> Result<Imported, IoError> {
    let file = open(bytes)?;
    let pages = file.pages();
    if pages.is_empty() {
        return Err(IoError::Decode("this PDF has no pages".into()));
    }
    let mut report = import::Report::default();
    let mut out = Vec::new();
    for (i, page) in pages.iter().enumerate().take(MAX_PAGES) {
        let (pw, ph) = (page.bbox[2] - page.bbox[0], page.bbox[3] - page.bbox[1]);
        let (pw, ph) = if page.rotate % 180 == 90 { (ph, pw) } else { (pw, ph) };
        // Points to pixels, kept inside what a document may be.
        let mut scale = options.dpi.clamp(18.0, 2400.0) / 72.0;
        let longest = pw.max(ph) * scale;
        if longest > 30_000.0 {
            scale *= 30_000.0 / longest;
        }
        let (w, h) = ((pw * scale).round().max(1.0) as u32, (ph * scale).round().max(1.0) as u32);
        let ctm = page_matrix(page.bbox, page.rotate, scale as f64);

        let mut interp = content::Interpreter::new(&file);
        interp.run_page(&file.contents(page), &page.resources, ctm);
        annotations(&file, page, &mut interp, ctm);

        let mut layers = Vec::new();
        if options.paper {
            layers.push(NewLayer::new("Paper", LayerKind::Fill(FillStyle::Solid(Rgba8::WHITE)), (0, 0)));
        }
        layers.extend(import::page_layers(interp.out, (w, h), options, &mut report));
        out.push(Page { number: i + 1, width: w, height: h, layers });
    }
    let mut notes = report.lines();
    if pages.len() > MAX_PAGES {
        notes.push(format!("Only the first {MAX_PAGES} of {} pages were read", pages.len()));
    }
    if file.repaired {
        notes.insert(0, "The file's index was damaged, so its objects were found by scanning it".into());
    }
    Ok(Imported { pages: out, title: file.info(b"Title"), notes })
}

/// Text drawn by annotation appearances: filled-in form fields, mostly
/// (§12.5.5). An appearance's box is fitted to the annotation's rectangle.
fn annotations(file: &file::File, page: &file::Page, interp: &mut content::Interpreter, ctm: content::Matrix) {
    let annots = file.get_in(&page.dict, b"Annots");
    for annot in annots.as_array().unwrap_or(&[]) {
        let annot = file.resolve(annot);
        let Some(a) = annot.as_dict() else { continue };
        let flags = file.get_in(a, b"F").as_int().unwrap_or(0);
        if flags & (2 | 32) != 0 || file.get_in(a, b"Subtype").is_name(b"Popup") {
            continue; // Hidden, or NoView
        }
        let normal = match file.get_in(a, b"AP").as_dict().map(|ap| file.get_in(ap, b"N")) {
            Some(Object::Stream(s)) => s,
            Some(Object::Dict(states)) => match file.get_in(a, b"AS") {
                Object::Name(state) => match file.get_in(&states, &state) {
                    Object::Stream(s) => s,
                    _ => continue,
                },
                _ => continue,
            },
            _ => continue,
        };
        let rect = |o: Object| -> Option<[f64; 4]> {
            let v: Vec<f64> = o.as_array()?.iter().filter_map(|x| file.resolve(x).as_f64()).collect();
            (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
        };
        let (Some(r), Some(bbox)) = (rect(file.get_in(a, b"Rect")), rect(file.get_in(&normal.dict, b"BBox"))) else {
            continue;
        };
        let matrix = content::matrix_of(file, &normal.dict).unwrap_or(content::IDENTITY);
        let corners = [(bbox[0], bbox[1]), (bbox[2], bbox[1]), (bbox[0], bbox[3]), (bbox[2], bbox[3])]
            .map(|(x, y)| content::apply(&matrix, x, y));
        let tx0 = corners.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
        let ty0 = corners.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let tx1 = corners.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
        let ty1 = corners.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
        if tx1 - tx0 <= 0.0 || ty1 - ty0 <= 0.0 {
            continue;
        }
        let (sx, sy) = ((r[2] - r[0]) / (tx1 - tx0), (r[3] - r[1]) / (ty1 - ty0));
        let fit = [sx, 0.0, 0.0, sy, r[0] - tx0 * sx, r[1] - ty0 * sy];
        interp.form(&normal, content::mult(&content::mult(&matrix, &fit), &ctm), true);
    }
}
