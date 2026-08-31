//! The half of the worker machinery that only the window uses.
//!
//! Everything else that drives this application — scripts, the other tests,
//! the screenshot generator — runs jobs where it starts them, so none of them
//! ever exercises the thread, the progress counter or the cancel flag. These
//! turn workers on deliberately and check the three things that mode has to
//! get right: the frame keeps being drawn, the work can be stopped, and an
//! answer worked out against pixels that have since changed is not written
//! back over them.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::filters::Filter;
use cshop_core::geom::Vec2;
use cshop_core::layer::LayerKind;
use cshop_core::paint::PaintMode;
use cshop_core::pixels::PixelBuffer;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;
use std::time::{Duration, Instant};

/// Big enough that the filter below takes a visible fraction of a second, and
/// small enough that a test suite is not held up by it.
const SIDE: u32 = 900;

/// Slow on purpose: a surface blur is the most expensive thing here.
fn slow() -> Filter {
    Filter::SurfaceBlur { radius: 7.0, threshold: 0.2 }
}

fn noisy(side: u32) -> PixelBuffer {
    let mut px = PixelBuffer::new(side, side);
    let mut s: u32 = 0x5EED;
    for y in 0..side as i32 {
        for x in 0..side as i32 {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = ((s >> 20) & 0x3f) as u8;
            px.set(x, y, Rgba8::opaque(80 + n, 120, 200 - n));
        }
    }
    px
}

fn worker_app(side: u32) -> Option<CShopApp> {
    let gpu = GpuContext::headless()
        .inspect_err(|e| eprintln!("skipping worker tests: {e}"))
        .ok()?;
    let mut app = CShopApp::new(gpu).with_workers();
    app.open_document(Document::new("test", side, side, Background::Transparent));
    let view = app.doc_mut().unwrap();
    let id = view.doc.active.unwrap();
    view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(noisy(side));
    view.invalidate();
    Some(app)
}

fn pixels(app: &CShopApp) -> &PixelBuffer {
    let view = app.doc().unwrap();
    view.doc.tree.get(view.doc.active.unwrap()).unwrap().pixels().unwrap()
}

/// Collect jobs until nothing is left, or give up.
fn settle(app: &mut CShopApp) -> bool {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        app.collect_jobs();
        if !app.jobs.any() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

#[test]
fn a_filter_on_a_worker_reports_its_way_to_the_answer() {
    let Some(mut app) = worker_app(SIDE) else { return };
    let before = pixels(&app).get(40, 40);

    app.dispatch(Action::ApplyFilter(Box::new(slow())));
    assert!(app.jobs.any(), "the filter should be outstanding, not already done");

    // Watch it move rather than asserting on one reading: what matters is
    // that the number is real, and a single sample cannot tell.
    let mut seen: Vec<f32> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        for job in app.jobs.running() {
            if let Some(f) = job.progress.fraction() {
                seen.push(f);
            }
        }
        app.collect_jobs();
        if !app.jobs.any() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    assert!(!app.jobs.any(), "the filter never finished");
    assert_ne!(pixels(&app).get(40, 40), before, "the filter did not land");
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Surface Blur"]);
    assert!(
        seen.iter().any(|&f| f > 0.0),
        "the bar never moved off zero in {} readings",
        seen.len()
    );
    assert!(
        seen.windows(2).all(|w| w[1] >= w[0]),
        "progress went backwards: {seen:?}"
    );
}

/// The claim the whole exercise rests on: a long filter does not stop the
/// window being drawn.
///
/// Measured against the filter's own duration rather than against a number of
/// milliseconds, so the test says the same thing on a fast machine as on a
/// slow one: whatever the work costs, one frame during it must cost a small
/// fraction of that.
#[test]
fn the_window_keeps_drawing_while_a_filter_runs() {
    use cshop_ui::input_harness::Harness;
    // Bigger and blurrier than the rest, so the work lasts long enough for a
    // frame drawn during it to be distinguishable from the whole of it.
    const BIG: u32 = 1400;
    let heavy = Filter::SurfaceBlur { radius: 14.0, threshold: 0.25 };

    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.jobs.run_here(false);
    h.app.open_document(Document::new("test", BIG, BIG, Background::Transparent));
    {
        let view = h.app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(noisy(BIG));
        view.invalidate();
    }
    h.settle(2);

    let started = Instant::now();
    h.app.dispatch(Action::ApplyFilter(Box::new(heavy)));
    assert!(h.app.jobs.any(), "the filter should still be running");

    let frame = Instant::now();
    h.settle(1);
    let busy = frame.elapsed();

    let deadline = Instant::now() + Duration::from_secs(120);
    while h.app.jobs.any() && Instant::now() < deadline {
        h.settle(1);
    }
    let whole = started.elapsed();
    assert!(!h.app.jobs.any(), "the filter never finished");

    assert!(
        busy * 4 < whole,
        "a frame took {busy:?} of the filter's {whole:?} — the work is being \
         done on the drawing thread"
    );
}

#[test]
fn cancelling_a_filter_leaves_the_picture_alone() {
    let Some(mut app) = worker_app(SIDE) else { return };
    let before = pixels(&app).clone();

    app.dispatch(Action::ApplyFilter(Box::new(slow())));
    let running = app.jobs.running();
    assert_eq!(running.len(), 1);
    running[0].progress.cancel();

    assert!(settle(&mut app), "the cancelled filter was never collected");
    assert_eq!(
        pixels(&app).get(40, 40),
        before.get(40, 40),
        "a cancelled filter still changed the picture"
    );
    assert!(
        app.doc().unwrap().history.labels().is_empty(),
        "a cancelled filter should leave nothing to undo"
    );
}

/// A filter reads a region, takes seconds over it, and writes it back. If the
/// region changed meanwhile, writing it back would quietly undo whatever
/// changed it — so it refuses instead.
#[test]
fn a_filter_will_not_write_over_what_changed_while_it_ran() {
    let Some(mut app) = worker_app(SIDE) else { return };
    app.dispatch(Action::ApplyFilter(Box::new(slow())));
    assert!(app.jobs.any());

    // Paint on the layer the filter is working on.
    app.tool = cshop_ui::tools::Tool::Brush;
    app.brush.size = 40.0;
    app.foreground = Rgba8::opaque(255, 0, 0);
    app.begin_stroke(Vec2::new(60.0, 60.0), PaintMode::Paint);
    app.continue_stroke(Vec2::new(90.0, 60.0));
    app.end_stroke();
    let painted = pixels(&app).get(70, 60);
    assert_eq!(painted.r, 255, "the stroke should have gone down");

    assert!(settle(&mut app), "the filter was never collected");
    assert_eq!(
        pixels(&app).get(70, 60),
        painted,
        "the filter wrote over a stroke made while it was running"
    );
    assert!(
        app.doc().unwrap().history.labels().iter().all(|l| l != "Surface Blur"),
        "the stale filter was applied anyway: {:?}",
        app.doc().unwrap().history.labels()
    );
}
