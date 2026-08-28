//! Filter throughput, to decide what needs a proxy preview and what does not.

use cshop_core::color::Rgba8;
use cshop_core::filters::{Filter, FilterContext};
use cshop_core::pixels::PixelBuffer;
use std::time::Instant;

fn image(w: u32, h: u32) -> PixelBuffer {
    let mut px = PixelBuffer::new(w, h);
    for y in 0..h {
        for x in 0..w {
            px.set(
                x as i32,
                y as i32,
                Rgba8::opaque((x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8),
            );
        }
    }
    px
}

fn main() {
    let ctx = FilterContext::default();
    for (w, h) in [(1920u32, 1080u32), (6000, 4000)] {
        let src = image(w, h);
        let mp = w as f64 * h as f64 / 1e6;
        println!("--- {w}x{h} ({mp:.1} MP) ---");

        for filter in Filter::all_defaults() {
            let start = Instant::now();
            std::hint::black_box(filter.apply(&src, &ctx));
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            let flag = if ms > 500.0 { "  <-- slow" } else { "" };
            println!("{:<22} {ms:>9.1} ms{flag}", filter.name());
        }
        println!();
    }
}
