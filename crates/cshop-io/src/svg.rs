//! SVG, in and out.
//!
//! # Why this is worth doing here
//!
//! The editor already has Bézier paths, a boolean-combining path type and a
//! rasteriser that works from a signed distance field. An SVG is a list of
//! Bézier paths with fills and strokes. Reading one is mostly a matter of
//! saying so in the right order; the work is in the parsing, not in the
//! drawing.
//!
//! # What is read
//!
//! Shapes and their geometry: `path`, `rect`, `circle`, `ellipse`, `line`,
//! `polyline`, `polygon`, nested in `g` elements that pass their attributes
//! down. Fills, strokes, widths and opacities. Transforms —
//! `translate`, `scale`, `rotate`, `matrix`, `skewX`, `skewY` — composed
//! through the nesting.
//!
//! # What is not, and why it says so
//!
//! Text, gradients, patterns, filters, clipping paths, masks, symbols and
//! `use`. Each is a substantial feature in its own right, and a reader that
//! quietly drops them produces a picture that is wrong in a way nobody can
//! see. So the reader counts what it skipped and hands the count back, for
//! the caller to say out loud.
//!
//! # The parser
//!
//! Written here rather than taken from a crate, and deliberately small: it
//! reads elements, attributes and nesting, and knows nothing about DTDs,
//! entities beyond the five predefined ones, or namespaces beyond ignoring
//! their prefixes. That is enough for the files a drawing program produces
//! and it is a few hundred lines rather than a dependency.

use crate::IoError;
use cshop_core::color::Rgba8;
use cshop_core::geom::Vec2;
use cshop_core::path::{Anchor, PathShape, SubPath};
use cshop_core::shape::{ShapeContent, ShapeKind, ShapeStyle, StrokeAlign};
use cshop_core::transform::Transform;

/// Bounds on what one file may ask for, so a hostile or generated SVG asks
/// for a lot rather than everything.
const MAX_SHAPES: usize = 20_000;
const MAX_DEPTH: usize = 64;

/// One shape from the file, in the document's coordinates.
#[derive(Debug, Clone)]
pub struct Shape {
    pub content: ShapeContent,
    /// Where the shape's box sits.
    pub offset: (i32, i32),
    /// What it was called in the file, when it said.
    pub name: Option<String>,
}

/// What came out of an SVG.
#[derive(Debug, Clone)]
pub struct Drawing {
    pub width: u32,
    pub height: u32,
    pub shapes: Vec<Shape>,
    /// Elements that were understood well enough to be recognised and not well
    /// enough to be drawn, by name. Reported rather than dropped, because a
    /// picture missing its text looks like a bug in this program.
    pub skipped: Vec<String>,
}

pub fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let text = String::from_utf8_lossy(head);
    text.contains("<svg") || text.trim_start().starts_with("<?xml")
}

/// Read an SVG into shapes.
pub fn read(bytes: &[u8]) -> Result<Drawing, IoError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| IoError::Decode("this SVG is not text".into()))?;
    let root = parse(text)?;
    if !root.name.eq_ignore_ascii_case("svg") {
        return Err(IoError::Decode(format!(
            "this file's outermost element is <{}>, not <svg>",
            root.name
        )));
    }

    // The size, and the mapping from the file's own coordinates onto it. A
    // viewBox is what lets an SVG say "these are my coordinates, draw them at
    // this size", and ignoring it puts everything in the wrong place at the
    // wrong scale.
    let width = root.attr("width").and_then(parse_length);
    let height = root.attr("height").and_then(parse_length);
    let view_box: Option<[f32; 4]> = root.attr("viewBox").and_then(|v| {
        let n: Vec<f32> = numbers(v);
        (n.len() == 4).then(|| [n[0], n[1], n[2], n[3]])
    });
    let (w, h) = match (width, height, view_box) {
        (Some(w), Some(h), _) => (w, h),
        (None, None, Some(vb)) => (vb[2], vb[3]),
        (Some(w), None, Some(vb)) if vb[2] > 0.0 => (w, w * vb[3] / vb[2]),
        (None, Some(h), Some(vb)) if vb[3] > 0.0 => (h * vb[2] / vb[3], h),
        _ => (512.0, 512.0),
    };
    let (w, h) = (w.clamp(1.0, 30_000.0), h.clamp(1.0, 30_000.0));
    let root_transform = match view_box {
        Some(vb) if vb[2] > 0.0 && vb[3] > 0.0 => Transform::translate(-vb[0], -vb[1])
            .then(Transform::scale(w / vb[2], h / vb[3])),
        _ => Transform::IDENTITY,
    };

    let mut out = Drawing {
        width: w.round() as u32,
        height: h.round() as u32,
        shapes: Vec::new(),
        skipped: Vec::new(),
    };
    let inherited = Inherited::default();
    walk(&root, root_transform, &inherited, &mut out, 0);
    Ok(out)
}

/// Attributes that pass down through nesting.
#[derive(Debug, Clone)]
struct Inherited {
    fill: Option<Rgba8>,
    stroke: Option<Rgba8>,
    stroke_width: f32,
    opacity: f32,
}

impl Default for Inherited {
    fn default() -> Self {
        // SVG's own defaults: black fill, no stroke, a stroke width of one.
        Self { fill: Some(Rgba8::BLACK), stroke: None, stroke_width: 1.0, opacity: 1.0 }
    }
}

fn walk(node: &Node, parent: Transform, inherited: &Inherited, out: &mut Drawing, depth: usize) {
    if depth > MAX_DEPTH || out.shapes.len() >= MAX_SHAPES {
        return;
    }
    let here = match node.attr("transform").map(parse_transform) {
        Some(t) => t.then(parent),
        None => parent,
    };
    let style = resolve(node, inherited);

    match node.name.to_ascii_lowercase().as_str() {
        "svg" | "g" | "a" | "switch" => {
            for child in &node.children {
                walk(child, here, &style, out, depth + 1);
            }
            return;
        }
        // Recognised, and not drawn. Named so the caller can say which.
        "text" | "tspan" | "image" | "use" | "symbol" | "clippath" | "mask" | "filter"
        | "lineargradient" | "radialgradient" | "pattern" => {
            let name = node.name.to_ascii_lowercase();
            if !out.skipped.contains(&name) {
                out.skipped.push(name);
            }
            return;
        }
        "defs" | "title" | "desc" | "metadata" | "style" => return,
        _ => {}
    }

    let Some(path) = geometry(node) else { return };
    if path.is_empty() {
        return;
    }
    let placed = transform_path(&path, here);
    let Some(shape) = into_shape(placed, &style, node.attr("id")) else { return };
    out.shapes.push(shape);
}

/// A node's own fill, stroke and width, falling back to what it inherited.
fn resolve(node: &Node, inherited: &Inherited) -> Inherited {
    // `style="fill:red;stroke:none"` says the same things as the attributes
    // and wins over them, which is what the cascade does.
    let mut from_style: Vec<(String, String)> = Vec::new();
    if let Some(style) = node.attr("style") {
        for pair in style.split(';') {
            if let Some((k, v)) = pair.split_once(':') {
                from_style.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
            }
        }
    }
    let get = |key: &str| -> Option<&str> {
        from_style
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .or_else(|| node.attr(key))
    };

    let opacity = get("opacity").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0)
        * inherited.opacity;
    let alpha_of = |key: &str| {
        get(key).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0) * opacity
    };
    let fill = match get("fill") {
        Some(v) => parse_paint(v),
        None => inherited.fill,
    }
    .map(|c| with_alpha(c, alpha_of("fill-opacity")));
    let stroke = match get("stroke") {
        Some(v) => parse_paint(v),
        None => inherited.stroke,
    }
    .map(|c| with_alpha(c, alpha_of("stroke-opacity")));
    let stroke_width = get("stroke-width")
        .and_then(parse_length)
        .unwrap_or(inherited.stroke_width);

    Inherited { fill, stroke, stroke_width, opacity }
}

fn with_alpha(c: Rgba8, k: f32) -> Rgba8 {
    Rgba8::new(c.r, c.g, c.b, (c.a as f32 * k.clamp(0.0, 1.0)).round() as u8)
}

/// The element's geometry as a path, whatever kind of element it is.
fn geometry(node: &Node) -> Option<PathShape> {
    let number = |k: &str| node.attr(k).and_then(parse_length).unwrap_or(0.0);
    match node.name.to_ascii_lowercase().as_str() {
        "path" => parse_path_data(node.attr("d")?),
        "rect" => {
            let (x, y, w, h) = (number("x"), number("y"), number("width"), number("height"));
            if w <= 0.0 || h <= 0.0 {
                return None;
            }
            // Rounded corners: SVG lets one radius stand for both.
            let rx = node.attr("rx").and_then(parse_length);
            let ry = node.attr("ry").and_then(parse_length);
            let (rx, ry) = match (rx, ry) {
                (Some(a), Some(b)) => (a, b),
                (Some(a), None) => (a, a),
                (None, Some(b)) => (b, b),
                (None, None) => (0.0, 0.0),
            };
            let (rx, ry) = (rx.min(w / 2.0), ry.min(h / 2.0));
            Some(rounded_rect(x, y, w, h, rx, ry))
        }
        "circle" => {
            let r = number("r");
            (r > 0.0).then(|| ellipse(number("cx"), number("cy"), r, r))
        }
        "ellipse" => {
            let (rx, ry) = (number("rx"), number("ry"));
            (rx > 0.0 && ry > 0.0).then(|| ellipse(number("cx"), number("cy"), rx, ry))
        }
        "line" => Some(open_path(vec![
            Vec2::new(number("x1"), number("y1")),
            Vec2::new(number("x2"), number("y2")),
        ])),
        "polyline" | "polygon" => {
            let n = numbers(node.attr("points")?);
            let pts: Vec<Vec2> =
                n.chunks_exact(2).map(|c| Vec2::new(c[0], c[1])).collect();
            if pts.len() < 2 {
                return None;
            }
            let closed = node.name.eq_ignore_ascii_case("polygon");
            Some(PathShape::new(vec![SubPath {
                anchors: pts.into_iter().map(Anchor::corner).collect(),
                closed,
            }]))
        }
        _ => None,
    }
}

fn open_path(points: Vec<Vec2>) -> PathShape {
    PathShape::new(vec![SubPath {
        anchors: points.into_iter().map(Anchor::corner).collect(),
        closed: false,
    }])
}

/// How far a Bézier control point sits from its anchor to approximate a
/// quarter circle: the standard constant, good to about a part in a thousand.
const KAPPA: f32 = 0.552_284_8;

fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> PathShape {
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let anchors = vec![
        handles(Vec2::new(cx + rx, cy), Vec2::new(cx + rx, cy - ky), Vec2::new(cx + rx, cy + ky)),
        handles(Vec2::new(cx, cy + ry), Vec2::new(cx + kx, cy + ry), Vec2::new(cx - kx, cy + ry)),
        handles(Vec2::new(cx - rx, cy), Vec2::new(cx - rx, cy + ky), Vec2::new(cx - rx, cy - ky)),
        handles(Vec2::new(cx, cy - ry), Vec2::new(cx - kx, cy - ry), Vec2::new(cx + kx, cy - ry)),
    ];
    PathShape::new(vec![SubPath { anchors, closed: true }])
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, rx: f32, ry: f32) -> PathShape {
    if rx <= 0.0 || ry <= 0.0 {
        return PathShape::new(vec![SubPath {
            anchors: [
                Vec2::new(x, y),
                Vec2::new(x + w, y),
                Vec2::new(x + w, y + h),
                Vec2::new(x, y + h),
            ]
            .into_iter()
            .map(Anchor::corner)
            .collect(),
            closed: true,
        }]);
    }
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let (x1, y1) = (x + w, y + h);
    let anchors = vec![
        handles(Vec2::new(x + rx, y), Vec2::new(x + rx - kx, y), Vec2::new(x + rx, y)),
        handles(Vec2::new(x1 - rx, y), Vec2::new(x1 - rx, y), Vec2::new(x1 - rx + kx, y)),
        handles(Vec2::new(x1, y + ry), Vec2::new(x1, y + ry - ky), Vec2::new(x1, y + ry)),
        handles(Vec2::new(x1, y1 - ry), Vec2::new(x1, y1 - ry), Vec2::new(x1, y1 - ry + ky)),
        handles(Vec2::new(x1 - rx, y1), Vec2::new(x1 - rx + kx, y1), Vec2::new(x1 - rx, y1)),
        handles(Vec2::new(x + rx, y1), Vec2::new(x + rx, y1), Vec2::new(x + rx - kx, y1)),
        handles(Vec2::new(x, y1 - ry), Vec2::new(x, y1 - ry + ky), Vec2::new(x, y1 - ry)),
        handles(Vec2::new(x, y + ry), Vec2::new(x, y + ry), Vec2::new(x, y + ry - ky)),
    ];
    PathShape::new(vec![SubPath { anchors, closed: true }])
}

fn handles(at: Vec2, in_handle: Vec2, out_handle: Vec2) -> Anchor {
    Anchor { at, in_handle, out_handle }
}

fn transform_path(path: &PathShape, m: Transform) -> PathShape {
    let mut out = path.clone();
    for part in &mut out.parts {
        for sub in &mut part.subpaths {
            for a in &mut sub.anchors {
                a.at = m.apply(a.at);
                a.in_handle = m.apply(a.in_handle);
                a.out_handle = m.apply(a.out_handle);
            }
        }
    }
    out
}

/// A path in document coordinates, as a shape with its own box.
fn into_shape(path: PathShape, style: &Inherited, id: Option<&str>) -> Option<Shape> {
    let bounds = path.bounds()?;
    if !bounds.x0.is_finite() || (bounds.width() <= 0.0 && bounds.height() <= 0.0) {
        return None;
    }
    // The shape's geometry lives inside its box, so the box's corner becomes
    // the layer's offset and the path moves to meet it.
    let local = transform_path(&path, Transform::translate(-bounds.x0, -bounds.y0));
    let content = ShapeContent::new(
        ShapeKind::Path(local),
        (bounds.width().max(1.0), bounds.height().max(1.0)),
        ShapeStyle {
            fill: style.fill.filter(|c| c.a > 0),
            stroke: style.stroke.filter(|c| c.a > 0),
            stroke_width: style.stroke_width.max(0.0),
            // SVG strokes straddle the path, which is what centre means.
            stroke_align: StrokeAlign::Center,
            antialias: true,
        },
    );
    Some(Shape {
        content,
        offset: (bounds.x0.floor() as i32, bounds.y0.floor() as i32),
        name: id.map(|s| s.to_string()),
    })
}

// ---------------------------------------------------------------------------
// Path data
// ---------------------------------------------------------------------------

/// Parse a `d` attribute.
///
/// Every command, including the arcs, which are converted to cubics here
/// because everything downstream is cubic and an arc is the one SVG primitive
/// that is not.
fn parse_path_data(d: &str) -> Option<PathShape> {
    let mut lexer = PathLexer::new(d);
    let mut subpaths: Vec<SubPath> = Vec::new();
    let mut anchors: Vec<Anchor> = Vec::new();
    let mut at = Vec2::ZERO;
    let mut start = Vec2::ZERO;
    // The reflection point for smooth curves, and which command set it.
    let mut last_control: Option<Vec2> = None;
    let mut last_was_cubic = false;
    let mut command = ' ';

    let flush = |subpaths: &mut Vec<SubPath>, anchors: &mut Vec<Anchor>, closed: bool| {
        if anchors.len() >= 2 {
            subpaths.push(SubPath { anchors: std::mem::take(anchors), closed });
        } else {
            anchors.clear();
        }
    };

    loop {
        let next = lexer.command_or_number();
        match next {
            Token::End => break,
            Token::Command(c) => command = c,
            // A repeated argument list continues the previous command, except
            // that a repeated `moveto` becomes a `lineto` — the one place in
            // the grammar where the command silently changes.
            Token::Number(n) => {
                lexer.push_back(n);
                if command == 'M' {
                    command = 'L';
                } else if command == 'm' {
                    command = 'l';
                }
            }
        }

        let relative = command.is_ascii_lowercase();
        let base = if relative { at } else { Vec2::ZERO };
        match command.to_ascii_uppercase() {
            'M' => {
                flush(&mut subpaths, &mut anchors, false);
                at = Vec2::new(base.x + lexer.number()?, base.y + lexer.number()?);
                start = at;
                anchors.push(Anchor::corner(at));
                last_control = None;
                last_was_cubic = false;
            }
            'L' => {
                at = Vec2::new(base.x + lexer.number()?, base.y + lexer.number()?);
                anchors.push(Anchor::corner(at));
                last_control = None;
                last_was_cubic = false;
            }
            'H' => {
                at = Vec2::new(base.x + lexer.number()?, at.y);
                anchors.push(Anchor::corner(at));
                last_control = None;
                last_was_cubic = false;
            }
            'V' => {
                at = Vec2::new(at.x, base.y + lexer.number()?);
                anchors.push(Anchor::corner(at));
                last_control = None;
                last_was_cubic = false;
            }
            'C' | 'S' => {
                let c1 = if command.eq_ignore_ascii_case(&'S') {
                    // Smooth: the first control is the reflection of the last
                    // one, which is what makes the join smooth.
                    match (last_control, last_was_cubic) {
                        (Some(prev), true) => Vec2::new(2.0 * at.x - prev.x, 2.0 * at.y - prev.y),
                        _ => at,
                    }
                } else {
                    Vec2::new(base.x + lexer.number()?, base.y + lexer.number()?)
                };
                let c2 = Vec2::new(base.x + lexer.number()?, base.y + lexer.number()?);
                let to = Vec2::new(base.x + lexer.number()?, base.y + lexer.number()?);
                if let Some(last) = anchors.last_mut() {
                    last.out_handle = c1;
                }
                anchors.push(Anchor { at: to, in_handle: c2, out_handle: to });
                at = to;
                last_control = Some(c2);
                last_was_cubic = true;
            }
            'Q' | 'T' => {
                let q = if command.eq_ignore_ascii_case(&'T') {
                    match (last_control, last_was_cubic) {
                        (Some(prev), false) => Vec2::new(2.0 * at.x - prev.x, 2.0 * at.y - prev.y),
                        _ => at,
                    }
                } else {
                    Vec2::new(base.x + lexer.number()?, base.y + lexer.number()?)
                };
                let to = Vec2::new(base.x + lexer.number()?, base.y + lexer.number()?);
                // A quadratic is a cubic whose controls sit two thirds of the
                // way from each end toward the single control point.
                let c1 = Vec2::new(at.x + 2.0 / 3.0 * (q.x - at.x), at.y + 2.0 / 3.0 * (q.y - at.y));
                let c2 = Vec2::new(to.x + 2.0 / 3.0 * (q.x - to.x), to.y + 2.0 / 3.0 * (q.y - to.y));
                if let Some(last) = anchors.last_mut() {
                    last.out_handle = c1;
                }
                anchors.push(Anchor { at: to, in_handle: c2, out_handle: to });
                at = to;
                last_control = Some(q);
                last_was_cubic = false;
            }
            'A' => {
                let rx = lexer.number()?;
                let ry = lexer.number()?;
                let rotation = lexer.number()?;
                let large = lexer.flag()?;
                let sweep = lexer.flag()?;
                let to = Vec2::new(base.x + lexer.number()?, base.y + lexer.number()?);
                arc_to_cubics(at, to, rx, ry, rotation, large, sweep, &mut anchors);
                at = to;
                last_control = None;
                last_was_cubic = false;
            }
            'Z' => {
                flush(&mut subpaths, &mut anchors, true);
                at = start;
                anchors.push(Anchor::corner(at));
                last_control = None;
                last_was_cubic = false;
            }
            _ => return None,
        }
    }
    flush(&mut subpaths, &mut anchors, false);
    (!subpaths.is_empty()).then(|| PathShape::new(subpaths))
}

/// An elliptical arc as a run of cubic segments.
///
/// SVG states an arc by where it ends and which of the four possible arcs to
/// take; everything else has to be worked back out. The centre parameterisation
/// in the specification's appendix, then a cubic per quarter turn or less,
/// which is where the approximation is good.
#[allow(clippy::too_many_arguments)]
fn arc_to_cubics(
    from: Vec2,
    to: Vec2,
    rx: f32,
    ry: f32,
    rotation_deg: f32,
    large: bool,
    sweep: bool,
    anchors: &mut Vec<Anchor>,
) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-6 || ry < 1e-6 || (from.x - to.x).abs() + (from.y - to.y).abs() < 1e-9 {
        // A degenerate arc is a straight line, which is what the
        // specification says to draw.
        anchors.push(Anchor::corner(to));
        return;
    }
    let phi = rotation_deg.to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();

    let dx2 = (from.x - to.x) / 2.0;
    let dy2 = (from.y - to.y) / 2.0;
    let x1 = cos_phi * dx2 + sin_phi * dy2;
    let y1 = -sin_phi * dx2 + cos_phi * dy2;

    // Radii too small to reach: scaled up until they just can, as the
    // specification requires rather than failing.
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let k = lambda.sqrt();
        rx *= k;
        ry *= k;
    }

    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut factor = (num / den).max(0.0).sqrt();
    if large == sweep {
        factor = -factor;
    }
    let cx1 = factor * rx * y1 / ry;
    let cy1 = -factor * ry * x1 / rx;
    let cx = cos_phi * cx1 - sin_phi * cy1 + (from.x + to.x) / 2.0;
    let cy = sin_phi * cx1 + cos_phi * cy1 + (from.y + to.y) / 2.0;

    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let dot = ux * vx + uy * vy;
        let len = ((ux * ux + uy * uy) * (vx * vx + vy * vy)).sqrt();
        let mut a = (dot / len.max(1e-12)).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };
    let start = angle(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut sweep_angle = angle(
        (x1 - cx1) / rx,
        (y1 - cy1) / ry,
        (-x1 - cx1) / rx,
        (-y1 - cy1) / ry,
    );
    if !sweep && sweep_angle > 0.0 {
        sweep_angle -= std::f32::consts::TAU;
    } else if sweep && sweep_angle < 0.0 {
        sweep_angle += std::f32::consts::TAU;
    }

    let steps = ((sweep_angle.abs() / (std::f32::consts::FRAC_PI_2)).ceil() as usize).max(1);
    let step = sweep_angle / steps as f32;
    // The control-point distance for a cubic spanning `step` radians.
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |a: f32| {
        let (s, c) = a.sin_cos();
        Vec2::new(
            cx + rx * c * cos_phi - ry * s * sin_phi,
            cy + rx * c * sin_phi + ry * s * cos_phi,
        )
    };
    let derivative = |a: f32| {
        let (s, c) = a.sin_cos();
        Vec2::new(
            -rx * s * cos_phi - ry * c * sin_phi,
            -rx * s * sin_phi + ry * c * cos_phi,
        )
    };

    for i in 0..steps {
        let a0 = start + step * i as f32;
        let a1 = a0 + step;
        let (p0, p1) = (point(a0), point(a1));
        let (d0, d1) = (derivative(a0), derivative(a1));
        let c1 = Vec2::new(p0.x + k * d0.x, p0.y + k * d0.y);
        let c2 = Vec2::new(p1.x - k * d1.x, p1.y - k * d1.y);
        if let Some(last) = anchors.last_mut() {
            last.out_handle = c1;
        }
        anchors.push(Anchor { at: p1, in_handle: c2, out_handle: p1 });
    }
}

enum Token {
    Command(char),
    Number(f32),
    End,
}

/// A reader for path data, which is a grammar of its own: commands, numbers,
/// and separators that may be commas, spaces, or nothing at all.
struct PathLexer<'a> {
    bytes: &'a [u8],
    at: usize,
    pushed: Option<f32>,
}

impl<'a> PathLexer<'a> {
    fn new(s: &'a str) -> PathLexer<'a> {
        PathLexer { bytes: s.as_bytes(), at: 0, pushed: None }
    }

    fn skip(&mut self) {
        while self.at < self.bytes.len() {
            match self.bytes[self.at] {
                b' ' | b'\t' | b'\r' | b'\n' | b',' => self.at += 1,
                _ => break,
            }
        }
    }

    fn command_or_number(&mut self) -> Token {
        if self.pushed.is_some() {
            return Token::Number(self.pushed.take().expect("just checked"));
        }
        self.skip();
        let Some(&b) = self.bytes.get(self.at) else { return Token::End };
        if b.is_ascii_alphabetic() {
            self.at += 1;
            return Token::Command(b as char);
        }
        match self.read_number() {
            Some(n) => Token::Number(n),
            None => Token::End,
        }
    }

    fn push_back(&mut self, n: f32) {
        self.pushed = Some(n);
    }

    fn number(&mut self) -> Option<f32> {
        if let Some(n) = self.pushed.take() {
            return Some(n);
        }
        self.skip();
        self.read_number()
    }

    /// The two arc flags, which are single characters and may run straight
    /// into the number after them: `a1 1 0 118 9` is five arguments, not two.
    fn flag(&mut self) -> Option<bool> {
        if let Some(n) = self.pushed.take() {
            return Some(n != 0.0);
        }
        self.skip();
        let &b = self.bytes.get(self.at)?;
        if b == b'0' || b == b'1' {
            self.at += 1;
            return Some(b == b'1');
        }
        None
    }

    fn read_number(&mut self) -> Option<f32> {
        let start = self.at;
        if matches!(self.bytes.get(self.at), Some(b'+') | Some(b'-')) {
            self.at += 1;
        }
        let mut digits = false;
        while matches!(self.bytes.get(self.at), Some(b) if b.is_ascii_digit()) {
            self.at += 1;
            digits = true;
        }
        if self.bytes.get(self.at) == Some(&b'.') {
            self.at += 1;
            while matches!(self.bytes.get(self.at), Some(b) if b.is_ascii_digit()) {
                self.at += 1;
                digits = true;
            }
        }
        if !digits {
            self.at = start;
            return None;
        }
        if matches!(self.bytes.get(self.at), Some(b'e') | Some(b'E')) {
            let save = self.at;
            self.at += 1;
            if matches!(self.bytes.get(self.at), Some(b'+') | Some(b'-')) {
                self.at += 1;
            }
            if matches!(self.bytes.get(self.at), Some(b) if b.is_ascii_digit()) {
                while matches!(self.bytes.get(self.at), Some(b) if b.is_ascii_digit()) {
                    self.at += 1;
                }
            } else {
                self.at = save;
            }
        }
        std::str::from_utf8(&self.bytes[start..self.at]).ok()?.parse().ok()
    }
}

// ---------------------------------------------------------------------------
// Attributes
// ---------------------------------------------------------------------------

/// A length: a number, optionally with a unit.
///
/// Units other than pixels are converted at the conventional 96 dots to the
/// inch. Percentages are refused rather than guessed at, since what they are a
/// percentage *of* depends on context this does not have.
fn parse_length(s: &str) -> Option<f32> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic() || c == '%').unwrap_or(s.len());
    let value: f32 = s[..split].trim().parse().ok()?;
    let unit = s[split..].trim().to_ascii_lowercase();
    let scale = match unit.as_str() {
        "" | "px" => 1.0,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        _ => return None,
    };
    Some(value * scale)
}

/// Every number in a string, whatever separates them.
fn numbers(s: &str) -> Vec<f32> {
    let mut lexer = PathLexer::new(s);
    let mut out = Vec::new();
    while let Some(n) = lexer.number() {
        out.push(n);
    }
    out
}

/// A `fill` or `stroke` value. `None` means "do not paint this".
fn parse_paint(s: &str) -> Option<Rgba8> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("transparent") {
        return None;
    }
    // A reference to a gradient or pattern, which is not read. Something
    // visible is better than a hole; grey says "there was a paint here".
    if s.starts_with("url(") {
        return Some(Rgba8::opaque(128, 128, 128));
    }
    parse_colour(s)
}

fn parse_colour(s: &str) -> Option<Rgba8> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let n = |a: u8, b: u8| u8::from_str_radix(std::str::from_utf8(&[a, b]).ok()?, 16).ok();
        let b = hex.as_bytes();
        return match b.len() {
            3 => {
                let d = |c: u8| u8::from_str_radix(std::str::from_utf8(&[c, c]).ok()?, 16).ok();
                Some(Rgba8::opaque(d(b[0])?, d(b[1])?, d(b[2])?))
            }
            6 => Some(Rgba8::opaque(n(b[0], b[1])?, n(b[2], b[3])?, n(b[4], b[5])?)),
            8 => Some(Rgba8::new(
                n(b[0], b[1])?,
                n(b[2], b[3])?,
                n(b[4], b[5])?,
                n(b[6], b[7])?,
            )),
            _ => None,
        };
    }
    if let Some(rest) = s.strip_prefix("rgb(").and_then(|r| r.strip_suffix(')')) {
        let n = numbers(rest);
        if n.len() >= 3 {
            let c = |v: f32| v.clamp(0.0, 255.0) as u8;
            return Some(Rgba8::opaque(c(n[0]), c(n[1]), c(n[2])));
        }
    }
    if let Some(rest) = s.strip_prefix("rgba(").and_then(|r| r.strip_suffix(')')) {
        let n = numbers(rest);
        if n.len() >= 4 {
            let c = |v: f32| v.clamp(0.0, 255.0) as u8;
            return Some(Rgba8::new(c(n[0]), c(n[1]), c(n[2]), (n[3] * 255.0).clamp(0.0, 255.0) as u8));
        }
    }
    // The named colours worth having. The full list is a hundred and forty
    // names, most of which nothing produces.
    let named: &[(&str, [u8; 3])] = &[
        ("black", [0, 0, 0]),
        ("white", [255, 255, 255]),
        ("red", [255, 0, 0]),
        ("green", [0, 128, 0]),
        ("lime", [0, 255, 0]),
        ("blue", [0, 0, 255]),
        ("yellow", [255, 255, 0]),
        ("cyan", [0, 255, 255]),
        ("aqua", [0, 255, 255]),
        ("magenta", [255, 0, 255]),
        ("fuchsia", [255, 0, 255]),
        ("grey", [128, 128, 128]),
        ("gray", [128, 128, 128]),
        ("silver", [192, 192, 192]),
        ("maroon", [128, 0, 0]),
        ("olive", [128, 128, 0]),
        ("navy", [0, 0, 128]),
        ("teal", [0, 128, 128]),
        ("purple", [128, 0, 128]),
        ("orange", [255, 165, 0]),
        ("brown", [165, 42, 42]),
        ("pink", [255, 192, 203]),
    ];
    let lower = s.to_ascii_lowercase();
    named
        .iter()
        .find(|(n, _)| *n == lower)
        .map(|(_, c)| Rgba8::opaque(c[0], c[1], c[2]))
}

/// A `transform` attribute: any number of operations, applied right to left as
/// the specification says.
fn parse_transform(s: &str) -> Transform {
    let mut out = Transform::IDENTITY;
    let mut rest = s.trim();
    let mut ops: Vec<Transform> = Vec::new();
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim().to_ascii_lowercase();
        let Some(close) = rest[open..].find(')') else { break };
        let args = numbers(&rest[open + 1..open + close]);
        rest = &rest[open + close + 1..];
        let t = match (name.as_str(), args.len()) {
            ("translate", 1) => Transform::translate(args[0], 0.0),
            ("translate", _) if args.len() >= 2 => Transform::translate(args[0], args[1]),
            ("scale", 1) => Transform::scale(args[0], args[0]),
            ("scale", _) if args.len() >= 2 => Transform::scale(args[0], args[1]),
            ("rotate", 1) => Transform::rotate(args[0].to_radians()),
            ("rotate", _) if args.len() >= 3 => Transform::about(
                Vec2::new(args[1], args[2]),
                Transform::rotate(args[0].to_radians()),
            ),
            ("skewx", _) if !args.is_empty() => Transform::skew(args[0].to_radians().tan(), 0.0),
            ("skewy", _) if !args.is_empty() => Transform::skew(0.0, args[0].to_radians().tan()),
            ("matrix", _) if args.len() >= 6 => Transform {
                // SVG lists a matrix down its columns: a b c d e f is
                // [[a, c, e], [b, d, f]].
                m: [
                    [args[0], args[2], args[4]],
                    [args[1], args[3], args[5]],
                    [0.0, 0.0, 1.0],
                ],
            },
            _ => continue,
        };
        ops.push(t);
    }
    // Right to left: the last written is applied first.
    for t in ops.into_iter().rev() {
        out = out.then(t);
    }
    out
}

// ---------------------------------------------------------------------------
// The XML reader
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
}

impl Node {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

/// Read the document's outermost element and everything under it.
fn parse(text: &str) -> Result<Node, IoError> {
    let bytes = text.as_bytes();
    let mut at = 0usize;
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;

    while at < bytes.len() {
        let Some(open) = find(bytes, at, b'<') else { break };
        at = open + 1;
        // Declarations, comments and processing instructions, none of which
        // carry geometry.
        if bytes[at..].starts_with(b"!--") {
            at = find_seq(bytes, at, b"-->").map(|i| i + 3).unwrap_or(bytes.len());
            continue;
        }
        if matches!(bytes.get(at), Some(b'?') | Some(b'!')) {
            at = find(bytes, at, b'>').map(|i| i + 1).unwrap_or(bytes.len());
            continue;
        }
        let closing = bytes.get(at) == Some(&b'/');
        if closing {
            at += 1;
        }
        let Some(end) = find(bytes, at, b'>') else { break };
        let inner = &text[at..end];
        at = end + 1;

        if closing {
            if let Some(node) = stack.pop() {
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            continue;
        }

        let self_closing = inner.trim_end().ends_with('/');
        let inner = inner.trim_end().trim_end_matches('/');
        let mut node = Node::default();
        let (name, attrs) = match inner.find(|c: char| c.is_whitespace()) {
            Some(i) => (&inner[..i], &inner[i..]),
            None => (inner, ""),
        };
        // Namespace prefixes are dropped: `svg:path` is a path.
        node.name = name.rsplit(':').next().unwrap_or(name).to_string();
        node.attrs = parse_attrs(attrs);

        if self_closing {
            match stack.last_mut() {
                Some(parent) => parent.children.push(node),
                None => root = Some(node),
            }
        } else {
            if stack.len() > MAX_DEPTH {
                return Err(IoError::Malformed("this SVG nests too deeply".into()));
            }
            stack.push(node);
        }
    }

    // An unclosed tree is still worth what was read, which is what a truncated
    // file usually is.
    while let Some(node) = stack.pop() {
        match stack.last_mut() {
            Some(parent) => parent.children.push(node),
            None => root = Some(node),
        }
    }
    root.ok_or_else(|| IoError::Decode("this file has no elements in it".into()))
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut at = 0usize;
    while at < b.len() {
        while at < b.len() && (b[at] as char).is_whitespace() {
            at += 1;
        }
        let start = at;
        while at < b.len() && b[at] != b'=' && !(b[at] as char).is_whitespace() {
            at += 1;
        }
        if at >= b.len() || start == at {
            break;
        }
        let key = s[start..at].rsplit(':').next().unwrap_or(&s[start..at]).to_string();
        while at < b.len() && (b[at] as char).is_whitespace() {
            at += 1;
        }
        if b.get(at) != Some(&b'=') {
            continue;
        }
        at += 1;
        while at < b.len() && (b[at] as char).is_whitespace() {
            at += 1;
        }
        let quote = match b.get(at) {
            Some(&q @ (b'"' | b'\'')) => {
                at += 1;
                q
            }
            _ => b' ',
        };
        let vstart = at;
        while at < b.len() && b[at] != quote && !(quote == b' ' && (b[at] as char).is_whitespace()) {
            at += 1;
        }
        let value = unescape(&s[vstart..at]);
        if at < b.len() {
            at += 1;
        }
        out.push((key.to_ascii_lowercase(), value));
    }
    out
}

/// The five predefined entities. Numeric ones too, since they turn up in
/// generated files.
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let replacement = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => name
                .strip_prefix('#')
                .and_then(|n| match n.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => n.parse().ok(),
                })
                .and_then(char::from_u32),
        };
        match replacement {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn find(b: &[u8], from: usize, needle: u8) -> Option<usize> {
    b.get(from..)?.iter().position(|&c| c == needle).map(|i| i + from)
}

fn find_seq(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    b.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Write a document as SVG.
///
/// Shape layers go out as paths, which is what they are — the geometry
/// survives, and what comes back from a round trip is editable rather than a
/// picture of what was editable. Everything else is a raster, and a raster in
/// an SVG is a PNG embedded in it: not vector, and better than absent, since
/// the alternative is a file missing most of the document.
pub fn write(
    doc: &cshop_core::document::Document,
    composite: &cshop_core::pixels::PixelBuffer,
) -> Result<Vec<u8>, IoError> {
    let mut out = String::new();
    out.push_str(&format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" \
         xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
         width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\">\n",
        doc.width, doc.height, doc.width, doc.height
    ));

    // Anything that is not a shape has to go out as pixels, and one embedded
    // PNG of the whole composite is both smaller and more faithful than one
    // per raster layer — the layers' blend modes and effects are in it.
    let has_raster = doc.tree.iter_all().into_iter().any(|id| {
        doc.tree.get(id).is_some_and(|l| l.visible && l.shape().is_none() && l.pixels().is_some())
    });
    if has_raster {
        let png = crate::encode(composite, crate::ImageFormat::Png, 92)?;
        out.push_str(&format!(
            "  <image x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" \
             xlink:href=\"data:image/png;base64,{}\"/>\n",
            doc.width,
            doc.height,
            base64(&png)
        ));
    }

    for id in doc.tree.iter_all() {
        let Some(layer) = doc.tree.get(id) else { continue };
        if !layer.visible {
            continue;
        }
        let Some(shape) = layer.shape() else { continue };
        let ShapeKind::Path(path) = &shape.content().kind else {
            // Rectangles, ellipses and polygons all have exact SVG spellings,
            // but they are stored here as a kind plus a box rather than as
            // geometry; the shared way out is the raster above.
            continue;
        };
        let placed = transform_path(
            path,
            Transform::translate(layer.offset.0 as f32, layer.offset.1 as f32),
        );
        let style = shape.content().style;
        out.push_str("  <path d=\"");
        out.push_str(&path_data(&placed));
        out.push_str("\" ");
        out.push_str(&paint("fill", style.fill));
        out.push(' ');
        out.push_str(&paint("stroke", style.stroke));
        if style.stroke.is_some() {
            out.push_str(&format!(" stroke-width=\"{}\"", trim(style.stroke_width)));
        }
        if layer.opacity < 1.0 {
            out.push_str(&format!(" opacity=\"{}\"", trim(layer.opacity)));
        }
        out.push_str(&format!(" id=\"{}\"/>\n", escape(&layer.name)));
    }

    out.push_str("</svg>\n");
    Ok(out.into_bytes())
}

fn paint(key: &str, colour: Option<Rgba8>) -> String {
    match colour {
        None => format!("{key}=\"none\""),
        Some(c) if c.a == 255 => {
            format!("{key}=\"#{:02x}{:02x}{:02x}\"", c.r, c.g, c.b)
        }
        Some(c) => format!(
            "{key}=\"#{:02x}{:02x}{:02x}\" {key}-opacity=\"{}\"",
            c.r,
            c.g,
            c.b,
            trim(c.a as f32 / 255.0)
        ),
    }
}

/// A path as a `d` attribute. Every segment is written as a cubic, since every
/// segment here is one.
fn path_data(path: &PathShape) -> String {
    let mut out = String::new();
    for part in &path.parts {
        for sub in &part.subpaths {
            let Some(first) = sub.anchors.first() else { continue };
            out.push_str(&format!("M{} {}", trim(first.at.x), trim(first.at.y)));
            let n = sub.anchors.len();
            let last = if sub.closed { n } else { n - 1 };
            for i in 0..last {
                let a = &sub.anchors[i];
                let b = &sub.anchors[(i + 1) % n];
                out.push_str(&format!(
                    "C{} {} {} {} {} {}",
                    trim(a.out_handle.x),
                    trim(a.out_handle.y),
                    trim(b.in_handle.x),
                    trim(b.in_handle.y),
                    trim(b.at.x),
                    trim(b.at.y)
                ));
            }
            if sub.closed {
                out.push('Z');
            }
        }
    }
    out
}

/// A number without the trailing zeros that would triple the file's size.
fn trim(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.to_string()
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        let digit = |shift: u32| ALPHABET[(n >> shift & 63) as usize] as char;
        out.push(digit(18));
        out.push(digit(12));
        out.push(if chunk.len() > 1 { digit(6) } else { '=' });
        out.push(if chunk.len() > 2 { digit(0) } else { '=' });
    }
    out
}
