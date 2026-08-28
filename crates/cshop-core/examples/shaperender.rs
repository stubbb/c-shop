//! Render sample shapes to raw RGBA, so the rasteriser can be looked at.
use cshop_core::color::Rgba8;
use cshop_core::pixels::PixelBuffer;
use cshop_core::shape::*;

fn main() {
    let mut sheet = PixelBuffer::filled(900, 480, Rgba8::new(0, 0, 0, 0));
    let blue = Rgba8::opaque(60, 130, 230);
    let dark = Rgba8::opaque(20, 30, 50);
    let red = Rgba8::opaque(220, 70, 60);

    let solid = ShapeStyle { fill: Some(blue), stroke: None, ..Default::default() };
    let outlined = ShapeStyle {
        fill: Some(blue),
        stroke: Some(dark),
        stroke_width: 6.0,
        stroke_align: StrokeAlign::Center,
        antialias: true,
    };
    let hollow = ShapeStyle {
        fill: None,
        stroke: Some(red),
        stroke_width: 5.0,
        stroke_align: StrokeAlign::Inside,
        antialias: true,
    };

    let cases: Vec<(ShapeContent, i32, i32)> = vec![
        (ShapeContent::new(ShapeKind::Rectangle { radius: 0.0 }, (160.0, 110.0), solid), 20, 20),
        (ShapeContent::new(ShapeKind::Rectangle { radius: 28.0 }, (160.0, 110.0), outlined), 210, 20),
        (ShapeContent::new(ShapeKind::Ellipse, (160.0, 110.0), outlined), 400, 20),
        (ShapeContent::new(ShapeKind::Ellipse, (160.0, 110.0), hollow), 590, 20),
        (ShapeContent::new(ShapeKind::Polygon { sides: 6, star: false, inner: 0.5 }, (150.0, 150.0), outlined), 20, 170),
        (ShapeContent::new(ShapeKind::Polygon { sides: 3, star: false, inner: 0.5 }, (150.0, 150.0), solid), 200, 170),
        (ShapeContent::new(ShapeKind::Polygon { sides: 5, star: true, inner: 0.42 }, (150.0, 150.0), outlined), 380, 170),
        (ShapeContent::new(ShapeKind::Polygon { sides: 8, star: true, inner: 0.7 }, (150.0, 150.0), hollow), 560, 170),
        (ShapeContent::new(ShapeKind::Line { thickness: 7.0, from: (0.0, 0.0), to: (1.0, 1.0) }, (150.0, 100.0), ShapeStyle { fill: None, stroke: Some(dark), ..Default::default() }), 20, 350),
        (ShapeContent::new(ShapeKind::Line { thickness: 3.0, from: (0.0, 1.0), to: (1.0, 0.0) }, (150.0, 100.0), ShapeStyle { fill: None, stroke: Some(red), ..Default::default() }), 200, 350),
        // Aliased, and outside vs inside strokes for comparison.
        (ShapeContent::new(ShapeKind::Ellipse, (120.0, 100.0), ShapeStyle { antialias: false, ..solid }), 380, 350),
        (ShapeContent::new(ShapeKind::Rectangle { radius: 10.0 }, (120.0, 100.0), ShapeStyle { stroke_align: StrokeAlign::Outside, ..outlined }), 540, 350),
        (ShapeContent::new(ShapeKind::Rectangle { radius: 10.0 }, (120.0, 100.0), ShapeStyle { stroke_align: StrokeAlign::Inside, ..outlined }), 710, 350),
    ];

    for (content, x, y) in &cases {
        match rasterize(content) {
            Some(r) => {
                println!("{:20} {}x{} anchor {:?}", content.kind.name(), r.pixels.width(), r.pixels.height(), r.anchor);
                sheet.paste(&r.pixels, *x, *y);
            }
            None => println!("{:20} FAILED", content.kind.name()),
        }
    }

    let path = std::env::args().nth(1).unwrap_or_else(|| "shapes.raw".into());
    std::fs::write(&path, sheet.as_bytes()).unwrap();
    println!("wrote {path} {}x{}", sheet.width(), sheet.height());
}
