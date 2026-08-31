//! A filter has to say roughly how far along it is, and stop when asked.
//!
//! Both of those are claims about code the filter does not contain: the count
//! comes from [`Filter::passes`], a hand-written table beside the dispatch,
//! and the stopping comes from a flag checked inside a helper. A table like
//! that drifts the first time somebody adds a second blur pass and does not
//! think of it — so it is compared against what actually happened rather than
//! trusted.

use cshop_core::color::Rgba8;
use cshop_core::filters::{Filter, FilterContext};
use cshop_core::pixels::PixelBuffer;
use cshop_core::progress::Progress;

const W: u32 = 96;
const H: u32 = 64;

fn subject() -> PixelBuffer {
    let mut px = PixelBuffer::new(W, H);
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            // Edges and a gradient, so nothing is a no-op on a flat field.
            let v = ((x * 3 + y * 2) % 256) as u8;
            let edge = if (x / 8 + y / 8) % 2 == 0 { 40 } else { 0 };
            px.set(x, y, Rgba8::opaque(v.saturating_add(edge), v, 255 - v));
        }
    }
    px
}

#[test]
fn every_filter_counts_out_what_it_claimed() {
    let src = subject();
    let ctx = FilterContext::default();
    let mut silent = Vec::new();
    let mut wrong = Vec::new();

    for filter in Filter::examples() {
        let p = Progress::new();
        let _ = filter.apply_reporting(&src, &ctx, &p);
        let (done, total) = (p.done(), p.total());

        if filter.passes() == 0 {
            // Declared as unmeasured, so it must not pretend otherwise.
            if done != 0 {
                wrong.push(format!(
                    "{} counted {done} while claiming to count nothing",
                    filter.name()
                ));
            }
            continue;
        }
        if done == 0 {
            silent.push(filter.name().to_string());
            continue;
        }
        // A pass either happens or it does not, so the only honest tolerance
        // is a fraction of one pass — here a tenth, for the odd filter that
        // skips a row or counts a cheap sweep alongside an expensive one.
        let slack = H as f64 / 10.0;
        if (done as f64 - total as f64).abs() > slack {
            wrong.push(format!(
                "{} counted {done} against a claimed {total} — {} pass(es) of {H} rows",
                filter.name(),
                filter.passes()
            ));
        }
    }

    assert!(
        silent.is_empty(),
        "these filters claim to report progress and never did: {}",
        silent.join(", ")
    );
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A filter told to stop before it starts must not do the work.
#[test]
fn cancelling_stops_a_filter_where_it_stands() {
    // The slowest of the lot, so the difference is not in the noise.
    let filter = Filter::SurfaceBlur { radius: 6.0, threshold: 0.15 };
    let big = {
        let mut px = PixelBuffer::new(700, 700);
        for y in 0..700 {
            for x in 0..700 {
                px.set(x, y, Rgba8::opaque((x % 256) as u8, (y % 256) as u8, 128));
            }
        }
        px
    };
    let ctx = FilterContext::default();

    let ran = Progress::new();
    let started = std::time::Instant::now();
    let _ = filter.apply_reporting(&big, &ctx, &ran);
    let whole = started.elapsed();

    let stopped = Progress::new();
    stopped.cancel();
    let started = std::time::Instant::now();
    let _ = filter.apply_reporting(&big, &ctx, &stopped);
    let cut_short = started.elapsed();

    assert_eq!(stopped.done(), 0, "a cancelled filter counted rows it should not have run");
    // Generously: the cancelled run still allocates the planes and converts
    // in and out, so it is not free — only far cheaper than doing the filter.
    assert!(
        cut_short.as_secs_f64() < whole.as_secs_f64() / 4.0,
        "cancelling saved almost nothing: {cut_short:?} against {whole:?}"
    );
}

/// Cancelling mid-flight is what actually happens, and the caller throws the
/// half-written picture away — so the only promise is that it stops early.
#[test]
fn cancelling_partway_leaves_the_rest_undone() {
    // Tall, so there are plenty of rows left when the flag goes up.
    let tall = {
        let src = subject();
        let mut px = PixelBuffer::new(W, H * 12);
        for band in 0..12 {
            px.paste(&src, 0, band * H as i32);
        }
        px
    };
    let ctx = FilterContext::default();
    let p = Progress::new();
    let watcher = p.clone();

    // Cancel from another thread once a few rows have gone by.
    let stop = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while watcher.done() < 4 && std::time::Instant::now() < deadline {
            std::hint::spin_loop();
        }
        watcher.cancel();
    });

    let filter = Filter::SurfaceBlur { radius: 8.0, threshold: 0.2 };
    let _ = filter.apply_reporting(&tall, &ctx, &p);
    stop.join().unwrap();

    assert!(p.cancelled());
    assert!(
        p.done() < p.total(),
        "a cancelled filter finished anyway: {} of {}",
        p.done(),
        p.total()
    );
}
