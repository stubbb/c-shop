//! Editing must not get slower because the canvas is bigger.
//!
//! A brush stroke covers the same few pixels whatever the document's size, so
//! it should cost about the same. Three things once made it cost the whole
//! canvas instead: the layer was copied to snapshot it, the thumbnail was
//! rebuilt by averaging every pixel, and a selection's extent was found by
//! scanning the entire mask. On a 10000x10000 document that was a second and a
//! half per stroke and a 150 ms stall on every mouse-down.
//!
//! These compare a large canvas against a small one rather than testing an
//! absolute time, so they measure the shape of the cost rather than the speed
//! of whatever machine is running them.

use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::paint::PaintMode;
use cshop_ui::input_harness::Harness;
use cshop_ui::tools::Tool;
use std::time::{Duration, Instant};

const SMALL: u32 = 512;
const LARGE: u32 = 8000;

/// Cost may grow with the canvas by at most this much before it counts as
/// scaling with it. Generous — the regressions this guards against were
/// fiftyfold and worse — so that a loaded machine does not fail the build.
const TOLERANCE: f64 = 12.0;

fn stroke_cost(h: &mut Harness, size: u32) -> Duration {
    h.app.open_document(Document::new("t", size, size, Background::White));
    h.settle(2);
    h.app.tool = Tool::Brush;

    let mid = Vec2::new(size as f32 / 2.0, size as f32 / 2.0);
    // Best of three: the first is warm-up, and the shortest is the one least
    // disturbed by whatever else the machine is doing.
    let mut best = Duration::from_secs(3600);
    for _ in 0..3 {
        let t = Instant::now();
        h.app.begin_stroke(mid, PaintMode::Paint);
        for i in 1..=8 {
            h.app.continue_stroke(Vec2::new(mid.x + i as f32 * 3.0, mid.y));
        }
        h.app.end_stroke();
        best = best.min(t.elapsed());
    }
    best
}

#[test]
fn a_stroke_costs_the_same_on_a_large_canvas_as_on_a_small_one() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    let small = stroke_cost(&mut h, SMALL);
    let large = stroke_cost(&mut h, LARGE);

    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-6);
    assert!(
        ratio < TOLERANCE,
        "a stroke cost {ratio:.1}x more on {LARGE}x{LARGE} than on {SMALL}x{SMALL} \
         ({small:?} against {large:?}); the stroke is scaling with the canvas again"
    );
}

/// The thumbnail is rebuilt every time its layer changes, so its cost has to
/// belong to the thumbnail rather than to the image behind it.
#[test]
fn a_thumbnail_costs_the_same_whatever_it_is_a_thumbnail_of() {
    use cshop_core::color::Rgba8;
    use cshop_core::pixels::PixelBuffer;

    let time = |side: u32| {
        let px = PixelBuffer::filled(side, side, Rgba8::opaque(90, 140, 200));
        let mut best = Duration::from_secs(3600);
        for _ in 0..3 {
            let t = Instant::now();
            let thumb = px.downscale(48, 48);
            std::hint::black_box(&thumb);
            best = best.min(t.elapsed());
        }
        best
    };

    let small = time(512);
    let large = time(8000);
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    assert!(
        ratio < TOLERANCE,
        "a 48-pixel thumbnail cost {ratio:.1}x more from 8000x8000 than from 512x512 \
         ({small:?} against {large:?}); downscale is reading the whole image again"
    );
}

/// Finding a marquee's extent must not mean reading the whole mask.
#[test]
fn a_marquee_knows_its_own_bounds_without_scanning_for_them() {
    use cshop_core::selection::{Rectf, Selection};

    let time = |side: u32| {
        let mut best = Duration::from_secs(3600);
        for _ in 0..3 {
            let t = Instant::now();
            // The same small rectangle either way, so any difference is the
            // cost of the canvas around it rather than of the selection.
            let s = Selection::from_rect(side, side, Rectf::from_points(
                Vec2::new(10.0, 10.0), Vec2::new(90.0, 90.0)), true);
            std::hint::black_box(&s);
            best = best.min(t.elapsed());
        }
        best
    };

    let small = time(512);
    let large = time(8000);
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    assert!(
        ratio < TOLERANCE,
        "an 80-pixel marquee cost {ratio:.1}x more on 8000x8000 than on 512x512 \
         ({small:?} against {large:?}); its bounds are being found by scanning"
    );
}
