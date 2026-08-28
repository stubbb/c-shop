//! Vector shapes.
//!
//! A shape layer keeps its geometry and style and a raster of them, exactly as
//! a type layer does, so everything downstream — the compositor, masks, blend
//! modes, filters — needs no knowledge of vectors at all. Re-rendering happens
//! whenever the shape or its style changes.
//!
//! # Why signed distance rather than scanlines
//!
//! Every shape here has a cheap distance function, and one distance gives both
//! the fill and the stroke: the fill is where the distance is negative, the
//! stroke is a band around zero. That makes antialiasing a clamp rather than a
//! coverage integral, keeps the two perfectly registered with each other, and
//! means an inside, centred or outside stroke differ only in where the band is
//! centred.

use crate::color::Rgba8;
use crate::geom::Vec2;
use crate::pixels::PixelBuffer;
use rayon::prelude::*;

/// Which side of the outline a stroke sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StrokeAlign {
    Inside,
    #[default]
    Center,
    Outside,
}

impl StrokeAlign {
    pub fn name(self) -> &'static str {
        match self {
            StrokeAlign::Inside => "Inside",
            StrokeAlign::Center => "Center",
            StrokeAlign::Outside => "Outside",
        }
    }

    /// Distance the band's centre sits from the outline, as a multiple of the
    /// stroke width.
    fn offset(self, width: f32) -> f32 {
        match self {
            StrokeAlign::Inside => -width / 2.0,
            StrokeAlign::Center => 0.0,
            StrokeAlign::Outside => width / 2.0,
        }
    }

    /// How far the stroke reaches outside the shape's own box.
    fn overhang(self, width: f32) -> f32 {
        match self {
            StrokeAlign::Inside => 0.0,
            StrokeAlign::Center => width / 2.0,
            StrokeAlign::Outside => width,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShapeKind {
    /// A rectangle, rounded when `radius` is above zero.
    Rectangle { radius: f32 },
    Ellipse,
    /// A regular polygon, or a star when `star` is set — `inner` is then the
    /// inner radius as a fraction of the outer one.
    Polygon { sides: u32, star: bool, inner: f32 },
    /// A straight line of the given thickness, between two points given as
    /// fractions of the box so any drag direction is representable.
    Line { thickness: f32, from: (f32, f32), to: (f32, f32) },
}

impl ShapeKind {
    pub fn name(self) -> &'static str {
        match self {
            ShapeKind::Rectangle { radius } if radius > 0.0 => "Rounded Rectangle",
            ShapeKind::Rectangle { .. } => "Rectangle",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Polygon { star: true, .. } => "Star",
            ShapeKind::Polygon { .. } => "Polygon",
            ShapeKind::Line { .. } => "Line",
        }
    }

    /// A line has no interior, so it is drawn from its stroke colour and
    /// ignores the fill.
    pub fn is_open(self) -> bool {
        matches!(self, ShapeKind::Line { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeStyle {
    pub fill: Option<Rgba8>,
    pub stroke: Option<Rgba8>,
    pub stroke_width: f32,
    pub stroke_align: StrokeAlign,
    pub antialias: bool,
}

impl Default for ShapeStyle {
    fn default() -> Self {
        Self {
            fill: Some(Rgba8::BLACK),
            stroke: None,
            stroke_width: 2.0,
            stroke_align: StrokeAlign::Center,
            antialias: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeContent {
    pub kind: ShapeKind,
    /// The shape's box, in pixels. Geometry is defined inside it, so moving
    /// the layer needs no knowledge of the shape.
    pub size: (f32, f32),
    pub style: ShapeStyle,
}

impl ShapeContent {
    pub fn new(kind: ShapeKind, size: (f32, f32), style: ShapeStyle) -> Self {
        Self { kind, size, style }
    }

    pub fn layer_name(&self) -> String {
        self.kind.name().to_string()
    }

    /// How far the drawing reaches outside the box, in pixels.
    fn margin(&self) -> f32 {
        let stroke = match self.style.stroke {
            Some(_) => self.style.stroke_align.overhang(self.style.stroke_width.max(0.0)),
            None => 0.0,
        };
        // A line's thickness is its own, and reaches both ways from the path.
        let line = match self.kind {
            ShapeKind::Line { thickness, .. } => thickness / 2.0,
            _ => 0.0,
        };
        // Two extra pixels so antialiasing is never clipped.
        stroke + line + 2.0
    }

    /// Vertices of a polygon or star, in box-local coordinates.
    fn polygon_points(&self, sides: u32, star: bool, inner: f32) -> Vec<Vec2> {
        let (w, h) = (self.size.0.max(1.0), self.size.1.max(1.0));
        let (cx, cy) = (w / 2.0, h / 2.0);
        let n = sides.clamp(3, 64);
        let count = if star { n * 2 } else { n };
        let inner = inner.clamp(0.05, 1.0);
        (0..count)
            .map(|i| {
                // Start at the top, as every drawing program's polygon does.
                let a = -std::f32::consts::FRAC_PI_2
                    + i as f32 * std::f32::consts::TAU / count as f32;
                let r = if star && i % 2 == 1 { inner } else { 1.0 };
                Vec2::new(cx + a.cos() * cx * r, cy + a.sin() * cy * r)
            })
            .collect()
    }
}

/// Distance from `p` to a segment.
fn sd_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let pa = Vec2::new(p.x - a.x, p.y - a.y);
    let ba = Vec2::new(b.x - a.x, b.y - a.y);
    let denom = ba.x * ba.x + ba.y * ba.y;
    let t = if denom <= 1e-9 { 0.0 } else { ((pa.x * ba.x + pa.y * ba.y) / denom).clamp(0.0, 1.0) };
    let d = Vec2::new(pa.x - ba.x * t, pa.y - ba.y * t);
    (d.x * d.x + d.y * d.y).sqrt()
}

/// Signed distance to a closed polygon; negative inside.
fn sd_polygon(p: Vec2, verts: &[Vec2]) -> f32 {
    let n = verts.len();
    if n < 3 {
        return f32::MAX;
    }
    let mut dist = f32::MAX;
    let mut inside = false;
    for i in 0..n {
        let j = (i + n - 1) % n;
        let (vi, vj) = (verts[i], verts[j]);
        dist = dist.min(sd_segment(p, vi, vj));
        // Crossing test: an odd number of edges to the right means inside.
        if (vi.y > p.y) != (vj.y > p.y) {
            let x = vi.x + (p.y - vi.y) / (vj.y - vi.y) * (vj.x - vi.x);
            if p.x < x {
                inside = !inside;
            }
        }
    }
    if inside {
        -dist
    } else {
        dist
    }
}

/// Signed distance to the shape's outline, in box-local coordinates.
fn distance(content: &ShapeContent, polygon: &[Vec2], p: Vec2) -> f32 {
    let (w, h) = (content.size.0.max(1.0), content.size.1.max(1.0));
    let (cx, cy) = (w / 2.0, h / 2.0);
    match content.kind {
        ShapeKind::Rectangle { radius } => {
            let r = radius.max(0.0).min(cx.min(cy));
            // Rounded box: shrink the half-extents by the radius, then take
            // the distance to that smaller box and subtract the radius back.
            let qx = (p.x - cx).abs() - (cx - r);
            let qy = (p.y - cy).abs() - (cy - r);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            outside + qx.max(qy).min(0.0) - r
        }
        ShapeKind::Ellipse => {
            // An exact ellipse distance needs iteration; this approximation is
            // correct at the outline and close enough either side of it for
            // antialiasing and a stroke a few pixels wide.
            let (ax, ay) = (cx.max(1e-3), cy.max(1e-3));
            let (dx, dy) = (p.x - cx, p.y - cy);
            let k1 = ((dx / ax).powi(2) + (dy / ay).powi(2)).sqrt();
            let k2 = ((dx / (ax * ax)).powi(2) + (dy / (ay * ay)).powi(2)).sqrt();
            if k2 <= 1e-9 {
                -ax.min(ay)
            } else {
                k1 * (k1 - 1.0) / k2
            }
        }
        ShapeKind::Polygon { .. } => sd_polygon(p, polygon),
        ShapeKind::Line { thickness, from, to } => {
            let a = Vec2::new(from.0 * w, from.1 * h);
            let b = Vec2::new(to.0 * w, to.1 * h);
            sd_segment(p, a, b) - thickness.max(0.1) / 2.0
        }
    }
}

/// A rendered shape.
pub struct Rasterized {
    pub pixels: PixelBuffer,
    /// Where the shape's box's top-left falls inside `pixels`. Kept so that
    /// widening a stroke grows the raster without moving the shape.
    pub anchor: (i32, i32),
}

/// Coverage from a signed distance: one pixel of feather across the outline.
#[inline]
fn coverage(d: f32, antialias: bool) -> f32 {
    if antialias {
        (0.5 - d).clamp(0.0, 1.0)
    } else if d <= 0.0 {
        1.0
    } else {
        0.0
    }
}

/// Render the shape. `None` when it would be degenerate.
pub fn rasterize(content: &ShapeContent) -> Option<Rasterized> {
    let m = content.margin().ceil().max(1.0) as i32;
    let w = (content.size.0.ceil().max(1.0) as i32 + 2 * m) as u32;
    let h = (content.size.1.ceil().max(1.0) as i32 + 2 * m) as u32;
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return None;
    }

    let polygon = match content.kind {
        ShapeKind::Polygon { sides, star, inner } => content.polygon_points(sides, star, inner),
        _ => Vec::new(),
    };
    let style = content.style;
    // A line has no interior to fill; its own thickness is the whole shape.
    let fill = if content.kind.is_open() { None } else { style.fill };
    let stroke = style.stroke.filter(|_| style.stroke_width > 0.0);
    let line_colour = if content.kind.is_open() {
        style.stroke.or(style.fill)
    } else {
        None
    };
    if fill.is_none() && stroke.is_none() && line_colour.is_none() {
        return None;
    }
    let band = style.stroke_align.offset(style.stroke_width);
    let half = style.stroke_width / 2.0;

    let mut pixels = PixelBuffer::new(w, h);
    pixels
        .pixels_mut()
        .par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, slot) in row.iter_mut().enumerate() {
                // Sample at the pixel's centre.
                let p = Vec2::new(x as f32 - m as f32 + 0.5, y as f32 - m as f32 + 0.5);
                let d = distance(content, &polygon, p);

                let mut out = crate::color::Rgba::new(0.0, 0.0, 0.0, 0.0);
                if let Some(c) = line_colour {
                    let a = coverage(d, style.antialias) * (c.a as f32 / 255.0);
                    out = over(out, c, a);
                }
                if let Some(c) = fill {
                    let a = coverage(d, style.antialias) * (c.a as f32 / 255.0);
                    out = over(out, c, a);
                }
                if let Some(c) = stroke {
                    // The band is |d - offset| <= half, which is one more
                    // distance comparison rather than a second shape.
                    let a = coverage((d - band).abs() - half, style.antialias)
                        * (c.a as f32 / 255.0);
                    out = over(out, c, a);
                }
                if out.a > 0.0 {
                    *slot = out.to_u8();
                }
            }
        });

    Some(Rasterized { pixels, anchor: (m, m) })
}

/// Source-over of a solid colour at `alpha` onto a straight-alpha accumulator.
#[inline]
fn over(dst: crate::color::Rgba, src: Rgba8, alpha: f32) -> crate::color::Rgba {
    if alpha <= 0.0 {
        return dst;
    }
    let s = src.to_f32();
    let a = alpha.clamp(0.0, 1.0);
    let out_a = a + dst.a * (1.0 - a);
    if out_a <= 0.0 {
        return crate::color::Rgba::new(0.0, 0.0, 0.0, 0.0);
    }
    // Premultiply, add, then unpremultiply — the accumulator holds straight
    // alpha so the buffer can be written without a second pass.
    let mix = |sc: f32, dc: f32| (sc * a + dc * dst.a * (1.0 - a)) / out_a;
    crate::color::Rgba::new(mix(s.r, dst.r), mix(s.g, dst.g), mix(s.b, dst.b), out_a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(r: &Rasterized, x: i32, y: i32) -> u8 {
        r.pixels.get(x, y).a
    }

    /// The anchor is the box's corner, so a shape sits where it was drawn no
    /// matter how far its stroke reaches outside.
    #[test]
    fn the_anchor_marks_the_box_whatever_the_stroke_does() {
        let base = ShapeContent::new(
            ShapeKind::Rectangle { radius: 0.0 },
            (40.0, 30.0),
            ShapeStyle { fill: Some(Rgba8::BLACK), stroke: None, ..Default::default() },
        );
        let thin = rasterize(&base).unwrap();

        let mut fat = base;
        fat.style.stroke = Some(Rgba8::WHITE);
        fat.style.stroke_width = 20.0;
        fat.style.stroke_align = StrokeAlign::Outside;
        let fat = rasterize(&fat).unwrap();

        assert!(fat.pixels.width() > thin.pixels.width(), "an outside stroke grows the raster");
        // The box's corner is opaque in both, at each one's own anchor.
        assert!(alpha_at(&thin, thin.anchor.0 + 2, thin.anchor.1 + 2) > 200);
        assert!(alpha_at(&fat, fat.anchor.0 + 2, fat.anchor.1 + 2) > 200);
    }

    #[test]
    fn a_filled_rectangle_covers_its_box_and_nothing_else() {
        let c = ShapeContent::new(
            ShapeKind::Rectangle { radius: 0.0 },
            (30.0, 20.0),
            ShapeStyle { fill: Some(Rgba8::BLACK), stroke: None, ..Default::default() },
        );
        let r = rasterize(&c).unwrap();
        let (ax, ay) = r.anchor;
        assert_eq!(alpha_at(&r, ax + 15, ay + 10), 255, "the middle should be solid");
        assert_eq!(alpha_at(&r, ax - 2, ay + 10), 0, "and outside the box, empty");
        assert_eq!(alpha_at(&r, ax + 32, ay + 10), 0);
    }

    /// A rounded rectangle must actually lose its corners, or the radius is
    /// doing nothing.
    #[test]
    fn rounding_cuts_the_corners() {
        let square = ShapeContent::new(
            ShapeKind::Rectangle { radius: 0.0 },
            (60.0, 60.0),
            ShapeStyle { fill: Some(Rgba8::BLACK), stroke: None, ..Default::default() },
        );
        let round = ShapeContent { kind: ShapeKind::Rectangle { radius: 20.0 }, ..square };
        let (a, b) = (rasterize(&square).unwrap(), rasterize(&round).unwrap());
        assert_eq!(alpha_at(&a, a.anchor.0 + 1, a.anchor.1 + 1), 255);
        assert_eq!(alpha_at(&b, b.anchor.0 + 1, b.anchor.1 + 1), 0, "the corner should be cut away");
        assert_eq!(alpha_at(&b, b.anchor.0 + 30, b.anchor.1 + 30), 255, "but the middle stays");
    }

    #[test]
    fn an_ellipse_fills_its_middle_and_misses_its_corners() {
        let c = ShapeContent::new(
            ShapeKind::Ellipse,
            (80.0, 50.0),
            ShapeStyle { fill: Some(Rgba8::BLACK), stroke: None, ..Default::default() },
        );
        let r = rasterize(&c).unwrap();
        let (ax, ay) = r.anchor;
        assert_eq!(alpha_at(&r, ax + 40, ay + 25), 255);
        assert_eq!(alpha_at(&r, ax + 1, ay + 1), 0, "a corner is outside the ellipse");
        // And it reaches the edge midpoints, which a smaller shape would not.
        assert!(alpha_at(&r, ax + 40, ay + 1) > 128, "the top should touch the box");
    }

    /// Where the stroke sits is the only difference between the alignments, so
    /// each should paint on its own side of the outline.
    #[test]
    fn stroke_alignment_puts_the_band_on_the_right_side() {
        let make = |align| {
            let c = ShapeContent::new(
                ShapeKind::Rectangle { radius: 0.0 },
                (60.0, 60.0),
                ShapeStyle {
                    fill: None,
                    stroke: Some(Rgba8::BLACK),
                    stroke_width: 8.0,
                    stroke_align: align,
                    antialias: false,
                },
            );
            rasterize(&c).unwrap()
        };

        // Just inside the box's left edge.
        let inside = make(StrokeAlign::Inside);
        assert_eq!(alpha_at(&inside, inside.anchor.0 + 2, inside.anchor.1 + 30), 255);
        assert_eq!(alpha_at(&inside, inside.anchor.0 - 3, inside.anchor.1 + 30), 0);

        // Just outside it.
        let outside = make(StrokeAlign::Outside);
        assert_eq!(alpha_at(&outside, outside.anchor.0 - 3, outside.anchor.1 + 30), 255);
        assert_eq!(alpha_at(&outside, outside.anchor.0 + 3, outside.anchor.1 + 30), 0);

        // Centred straddles it.
        let centre = make(StrokeAlign::Center);
        assert_eq!(alpha_at(&centre, centre.anchor.0 - 2, centre.anchor.1 + 30), 255);
        assert_eq!(alpha_at(&centre, centre.anchor.0 + 2, centre.anchor.1 + 30), 255);
    }

    #[test]
    fn antialiasing_softens_the_edge_and_turning_it_off_does_not() {
        let soft = ShapeContent::new(
            ShapeKind::Ellipse,
            (60.0, 60.0),
            ShapeStyle { fill: Some(Rgba8::BLACK), stroke: None, ..Default::default() },
        );
        let hard = ShapeContent {
            style: ShapeStyle { antialias: false, ..soft.style },
            ..soft
        };
        let partial = |c: &ShapeContent| {
            rasterize(c).unwrap().pixels.pixels().iter().filter(|p| p.a > 0 && p.a < 255).count()
        };
        assert!(partial(&soft) > 50, "an antialiased edge has partial pixels");
        assert_eq!(partial(&hard), 0, "an aliased one has none");
    }

    /// A star has to actually indent, or it is just a polygon.
    #[test]
    fn a_star_is_narrower_between_its_points() {
        let c = ShapeContent::new(
            ShapeKind::Polygon { sides: 5, star: true, inner: 0.4 },
            (100.0, 100.0),
            ShapeStyle { fill: Some(Rgba8::BLACK), stroke: None, antialias: false, ..Default::default() },
        );
        let star = rasterize(&c).unwrap();
        let plain = rasterize(&ShapeContent {
            kind: ShapeKind::Polygon { sides: 5, star: false, inner: 0.4 },
            ..c
        })
        .unwrap();
        let ink = |r: &Rasterized| r.pixels.pixels().iter().filter(|p| p.a > 128).count();
        assert!(ink(&star) < ink(&plain), "a star covers less than the pentagon around it");
        // The very top is a point on both.
        assert!(alpha_at(&star, star.anchor.0 + 50, star.anchor.1 + 2) > 0);
    }

    #[test]
    fn a_line_follows_its_endpoints() {
        let down = ShapeContent::new(
            ShapeKind::Line { thickness: 6.0, from: (0.0, 0.0), to: (1.0, 1.0) },
            (60.0, 60.0),
            ShapeStyle { fill: None, stroke: Some(Rgba8::BLACK), ..Default::default() },
        );
        let up = ShapeContent {
            kind: ShapeKind::Line { thickness: 6.0, from: (0.0, 1.0), to: (1.0, 0.0) },
            ..down
        };
        let (d, u) = (rasterize(&down).unwrap(), rasterize(&up).unwrap());
        // Each diagonal is drawn on, and the other one is not.
        assert!(alpha_at(&d, d.anchor.0 + 10, d.anchor.1 + 10) > 128);
        assert_eq!(alpha_at(&d, d.anchor.0 + 10, d.anchor.1 + 50), 0);
        assert!(alpha_at(&u, u.anchor.0 + 10, u.anchor.1 + 50) > 128);
        assert_eq!(alpha_at(&u, u.anchor.0 + 10, u.anchor.1 + 10), 0);
    }

    #[test]
    fn a_shape_with_nothing_to_paint_renders_nothing() {
        let c = ShapeContent::new(
            ShapeKind::Rectangle { radius: 0.0 },
            (20.0, 20.0),
            ShapeStyle { fill: None, stroke: None, ..Default::default() },
        );
        assert!(rasterize(&c).is_none());
    }

    /// Semi-transparent fill under a semi-transparent stroke must composite,
    /// not overwrite: the band should end up more opaque than either alone.
    #[test]
    fn a_translucent_stroke_composites_over_the_fill() {
        let c = ShapeContent::new(
            ShapeKind::Rectangle { radius: 0.0 },
            (40.0, 40.0),
            ShapeStyle {
                fill: Some(Rgba8::new(255, 0, 0, 128)),
                stroke: Some(Rgba8::new(0, 0, 255, 128)),
                stroke_width: 6.0,
                stroke_align: StrokeAlign::Inside,
                antialias: false,
            },
        );
        let r = rasterize(&c).unwrap();
        let (ax, ay) = r.anchor;
        let middle = r.pixels.get(ax + 20, ay + 20);
        let edge = r.pixels.get(ax + 2, ay + 20);
        assert_eq!(middle.a, 128, "the fill alone keeps its own alpha");
        assert!(edge.a > middle.a, "stroke over fill is more opaque than either: {edge:?}");
        assert!(edge.b > edge.r, "and takes the stroke's colour");
    }
}
