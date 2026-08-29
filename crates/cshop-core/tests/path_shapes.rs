//! Paths, and the boolean operations over them.
//!
//! The outline conversion is checked by rendering: a rectangle turned into
//! Bézier contours and filled as a path must come out looking like the
//! rectangle the distance field draws directly. That compares the new code
//! against the old rather than against my arithmetic.

use cshop_core::color::Rgba8;
use cshop_core::geom::Vec2;
use cshop_core::path::{BoolOp, PathPart, PathShape, SubPath};
use cshop_core::pixels::PixelBuffer;
use cshop_core::shape::{outline, rasterize, ShapeContent, ShapeKind, ShapeStyle};

fn solid() -> ShapeStyle {
    ShapeStyle { fill: Some(Rgba8::BLACK), stroke: None, antialias: true, ..Default::default() }
}

fn render(kind: ShapeKind, size: (f32, f32)) -> PixelBuffer {
    rasterize(&ShapeContent::new(kind, size, solid())).expect("should render").pixels
}

/// Share of pixels whose coverage differs by more than a hair.
fn disagreement(a: &PixelBuffer, b: &PixelBuffer) -> f64 {
    assert_eq!((a.width(), a.height()), (b.width(), b.height()));
    let mut bad = 0usize;
    for (p, q) in a.pixels().iter().zip(b.pixels()) {
        if (p.a as i32 - q.a as i32).abs() > 24 {
            bad += 1;
        }
    }
    bad as f64 / a.pixels().len() as f64
}

#[test]
fn a_shape_converted_to_contours_renders_as_itself() {
    let size = (120.0, 90.0);
    let kinds = [
        ("rectangle", ShapeKind::Rectangle { radius: 0.0 }),
        ("rounded rectangle", ShapeKind::Rectangle { radius: 18.0 }),
        ("ellipse", ShapeKind::Ellipse),
        ("hexagon", ShapeKind::Polygon { sides: 6, star: false, inner: 0.5 }),
        ("star", ShapeKind::Polygon { sides: 5, star: true, inner: 0.45 }),
    ];
    for (name, kind) in kinds {
        let native = render(kind.clone(), size);
        let as_path = render(ShapeKind::Path(PathShape::new(outline(&kind, size))), size);
        let differs = disagreement(&native, &as_path);
        assert!(
            differs < 0.01,
            "{name}: {:.1}% of pixels differ between the shape and its own outline",
            differs * 100.0
        );
    }
}

/// The operations have to hold on rendered coverage, not only on distances.
#[test]
fn the_boolean_operations_render_what_they_mean() {
    let size = (140.0, 100.0);
    let left = PathPart::new(outline(&ShapeKind::Ellipse, (100.0, 100.0)));
    let right_subs: Vec<SubPath> = outline(&ShapeKind::Ellipse, (100.0, 100.0))
        .iter()
        .map(|s| s.translate(Vec2::new(40.0, 0.0)))
        .collect();

    // Sample points: in the left lobe only, in both, in the right only.
    let probe = |px: &PixelBuffer, at: (i32, i32)| px.get(at.0, at.1).a > 128;
    // The raster is inset by its margin, so sample well inside each region.
    let only_left = (25, 52);
    let both = (72, 52);
    let only_right = (120, 52);

    for (op, want) in [
        (BoolOp::Union, [true, true, true]),
        (BoolOp::Intersect, [false, true, false]),
        (BoolOp::Subtract, [true, false, false]),
        (BoolOp::Exclude, [true, false, true]),
    ] {
        let shape = PathShape {
            parts: vec![left.clone(), PathPart { subpaths: right_subs.clone(), op }],
        };
        let px = render(ShapeKind::Path(shape), size);
        for (at, expected) in [only_left, both, only_right].iter().zip(want) {
            assert_eq!(probe(&px, *at), expected, "{} at {at:?}", op.name());
        }
    }
}

/// An unclosed path is a stroke; it must not quietly fill itself in.
#[test]
fn an_open_path_is_stroked_and_not_filled() {
    use cshop_core::path::Anchor;
    let sub = SubPath::open(vec![
        Anchor::corner(Vec2::new(10.0, 10.0)),
        Anchor::corner(Vec2::new(90.0, 10.0)),
        Anchor::corner(Vec2::new(90.0, 70.0)),
    ]);
    let kind = ShapeKind::Path(PathShape::new(vec![sub]));
    assert!(kind.is_open(), "nothing in it closes");

    let content = ShapeContent::new(
        kind,
        (100.0, 80.0),
        ShapeStyle {
            fill: Some(Rgba8::BLACK),
            stroke: Some(Rgba8::BLACK),
            stroke_width: 4.0,
            ..Default::default()
        },
    );
    let px = rasterize(&content).expect("should render").pixels;
    let m = 6; // comfortably inside the margin
    // On the line: painted. Inside the corner it almost encloses: not.
    assert!(px.get(50 + m, 10 + m).a > 128, "the stroke should be drawn");
    assert!(px.get(50 + m, 45 + m).a < 32, "an open path has no interior to fill");
}
