//! Selection performance on realistic documents.
//!
//! The operations that matter for feel are the ones that run on a gesture:
//! building a selection, tracing its outline for the marching ants, and the
//! modify operations that run from a menu.

use cshop_core::color::Rgba8;
use cshop_core::geom::Vec2;
use cshop_core::pixels::PixelBuffer;
use cshop_core::selection::{Rectf, Selection};
use cshop_core::wand::{magic_wand, WandOptions};
use std::time::Instant;

fn time(label: &str, iters: u32, mut f: impl FnMut()) {
    f();
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0 / iters as f64;
    println!("{label:<48} {ms:>8.2} ms");
}

/// A photo-like image: smooth gradients plus noise, which is the worst case for
/// the wand because the matched region has a long, ragged boundary.
fn photo(w: u32, h: u32) -> PixelBuffer {
    let mut px = PixelBuffer::new(w, h);
    let mut seed = 0x2545F491_4F6CDD1Du64;
    for y in 0..h {
        for x in 0..w {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let n = (seed >> 56) as u8 / 8;
            let r = (x * 200 / w) as u8;
            let g = (y * 200 / h) as u8;
            px.set(x as i32, y as i32, Rgba8::opaque(r.saturating_add(n), g, 128));
        }
    }
    px
}

fn main() {
    for (w, h) in [(1920u32, 1080u32), (6000, 4000)] {
        let mp = w as f64 * h as f64 / 1e6;
        println!("--- {w}x{h} ({mp:.1} MP) ---");

        let rect = Rectf { x0: w as f32 * 0.2, y0: h as f32 * 0.2, x1: w as f32 * 0.8, y1: h as f32 * 0.8 };
        time("rectangular marquee", 20, || {
            std::hint::black_box(Selection::from_rect(w, h, rect, true));
        });
        time("elliptical marquee (4x4 supersampled)", 10, || {
            std::hint::black_box(Selection::from_ellipse(w, h, rect, true));
        });

        // A lasso traced with a few hundred points, as a real drag produces.
        let points: Vec<Vec2> = (0..400)
            .map(|i| {
                let t = i as f32 / 400.0 * std::f32::consts::TAU;
                let r = 0.3 + 0.08 * (t * 7.0).sin();
                Vec2::new(
                    w as f32 * (0.5 + r * t.cos()),
                    h as f32 * (0.5 + r * t.sin()),
                )
            })
            .collect();
        time("lasso, 400 points", 10, || {
            std::hint::black_box(Selection::from_polygon(w, h, &points, true));
        });

        let mut s = Selection::from_ellipse(w, h, rect, true);
        time("trace outline (marching ants)", 20, || {
            let mut c = s.clone();
            std::hint::black_box(c.contours().len());
        });
        time("feather 10 px", 10, || {
            let mut c = s.clone();
            c.feather(10.0);
        });
        time("expand 8 px (distance transform)", 5, || {
            let mut c = s.clone();
            c.expand(8);
        });
        time("compress for undo", 20, || {
            std::hint::black_box(s.compress().memory_bytes());
        });
        s.invalidate();

        // The wand's ragged boundary is the worst case for outline tracing.
        let img = photo(w, h);
        let opts = WandOptions { tolerance: 40, contiguous: true, antialias: false };
        time("magic wand on noisy photo", 5, || {
            std::hint::black_box(magic_wand(&img, (w / 2) as i32, (h / 2) as i32, opts));
        });
        let mut wand_sel = magic_wand(&img, (w / 2) as i32, (h / 2) as i32, opts);
        let loops = wand_sel.contours().len();
        time("trace wand outline", 5, || {
            let mut c = wand_sel.clone();
            std::hint::black_box(c.contours().len());
        });
        println!("  (wand selection traced into {loops} loops)");
        println!();
    }
}
