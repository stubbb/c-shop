//! Straightening a photographed rectangle.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::history::{History, PerspectiveCrop};
use cshop_core::layer::LayerKind;
use cshop_core::pixels::PixelBuffer;
use cshop_core::resample::Resampling;

/// A checkerboard drawn *into* a quadrilateral, as a camera would see a
/// chequered board photographed at an angle.
fn photographed(w: u32, h: u32, quad: [Vec2; 4], squares: i32) -> PixelBuffer {
    let mut px = PixelBuffer::filled(w, h, Rgba8::opaque(20, 20, 20));
    let board = cshop_core::geom::IRect::new(0, 0, squares, squares);
    let to_quad = cshop_core::transform::Transform::from_quad(board, quad).unwrap();
    let inverse = to_quad.invert().unwrap();
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let p = inverse.apply(Vec2::new(x as f32 + 0.5, y as f32 + 0.5));
            if p.x < 0.0 || p.y < 0.0 || p.x >= squares as f32 || p.y >= squares as f32 {
                continue;
            }
            let on = (p.x as i32 + p.y as i32) % 2 == 0;
            px.set(x, y, if on { Rgba8::WHITE } else { Rgba8::opaque(40, 40, 40) });
        }
    }
    px
}

fn doc_with(px: PixelBuffer) -> Document {
    let (w, h) = (px.width(), px.height());
    let mut doc = Document::new("t", w, h, Background::Transparent);
    let id = doc.tree.iter_all()[0];
    doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(px);
    doc
}

/// How square the result is: compare the checker along the top row against the
/// bottom one. In the photograph the far edge is compressed; straightened,
/// the two should agree.
fn squares_across(px: &PixelBuffer, y: i32) -> usize {
    let mut flips = 0;
    let mut last = px.get(1, y).r > 128;
    for x in 2..px.width() as i32 - 1 {
        let now = px.get(x, y).r > 128;
        if now != last {
            flips += 1;
            last = now;
        }
    }
    flips
}

/// The corners of the board as the camera saw it: the far edge shorter than
/// the near one, which is what perspective does.
fn skewed() -> [Vec2; 4] {
    [
        Vec2::new(60.0, 30.0),
        Vec2::new(196.0, 30.0),
        Vec2::new(240.0, 170.0),
        Vec2::new(16.0, 170.0),
    ]
}

#[test]
fn straightening_makes_the_far_edge_match_the_near_one() {
    let quad = skewed();
    let px = photographed(256, 200, quad, 8);
    // In the photograph, the top of the board spans fewer pixels than the
    // bottom, so a straight run across it crosses the same squares in less
    // room. The straightened one should not.
    let before_top = squares_across(&px, 40);
    let before_bottom = squares_across(&px, 160);
    assert_eq!(before_top, before_bottom, "the same squares are crossed either way");

    let mut doc = doc_with(px);
    let mut history = History::new("Open");
    let edit = PerspectiveCrop::new(quad, Resampling::Bilinear).expect("a valid quad");
    let (w, h) = edit.size();
    history.apply(&mut doc, Box::new(edit));

    assert_eq!((doc.width, doc.height), (w, h), "the document is the straightened size");
    let out = doc.tree.get(doc.tree.iter_all()[0]).unwrap().pixels().unwrap();

    // The board now fills the canvas, so its squares should be evenly spaced:
    // measure where the flips are near the top and near the bottom and check
    // they line up.
    let flips_at = |y: i32| {
        let mut xs = Vec::new();
        let mut last = out.get(1, y).r > 128;
        for x in 2..out.width() as i32 - 1 {
            let now = out.get(x, y).r > 128;
            if now != last {
                xs.push(x);
                last = now;
            }
        }
        xs
    };
    let (top, bottom) = (flips_at(h as i32 / 8), flips_at(h as i32 * 7 / 8));
    assert_eq!(top.len(), bottom.len(), "the same number of squares at both ends");
    assert!(!top.is_empty(), "and there are some");
    for (a, b) in top.iter().zip(&bottom) {
        assert!(
            (a - b).abs() <= 3,
            "a straightened board has its edges in the same place at both ends: {a} against {b}"
        );
    }
}

#[test]
fn the_size_comes_from_the_quad_and_not_the_canvas() {
    let quad = skewed();
    let edit = PerspectiveCrop::new(quad, Resampling::Bilinear).unwrap();
    let (w, h) = edit.size();
    // Top edge 136, bottom 224, so about 180 across; sides about 147.
    assert!((w as i32 - 180).abs() < 6, "width {w}");
    assert!((h as i32 - 147).abs() < 6, "height {h}");
}

#[test]
fn straightening_undoes_to_exactly_what_was_there() {
    let quad = skewed();
    let px = photographed(256, 200, quad, 8);
    let before = px.clone();
    let mut doc = doc_with(px);
    let mut history = History::new("Open");
    history.apply(&mut doc, Box::new(PerspectiveCrop::new(quad, Resampling::Bilinear).unwrap()));
    assert_ne!((doc.width, doc.height), (256, 200));

    history.undo(&mut doc);
    assert_eq!((doc.width, doc.height), (256, 200));
    let back = doc.tree.get(doc.tree.iter_all()[0]).unwrap().pixels().unwrap();
    assert_eq!(
        back.pixels(),
        before.pixels(),
        "a projective warp drops samples, so undo has to hold the original rather than \
         try to invert it"
    );
}

#[test]
fn four_points_in_a_line_are_refused_rather_than_producing_nothing() {
    let flat = [
        Vec2::new(0.0, 10.0),
        Vec2::new(50.0, 10.0),
        Vec2::new(100.0, 10.0),
        Vec2::new(150.0, 10.0),
    ];
    assert!(PerspectiveCrop::new(flat, Resampling::Bilinear).is_none());
}
