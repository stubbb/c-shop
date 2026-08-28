//! Why `Adjustment::prepare` exists.
//!
//! The table-driven adjustments have to bake a 256-entry lookup table before
//! they can read a single entry from it. `Adjustment::apply` does that on
//! every call, so applying one per pixel bakes the table per pixel — which is
//! what made dragging a point in the Curves dialog freeze.
//!
//! Run with `cargo run --release -p cshop-core --example adjbench`.

use cshop_core::adjust::Adjustment;
use cshop_core::curve::Curve;
use cshop_core::pixels::PixelBuffer;
use std::time::Instant;

fn main() {
    let mut curves: [Curve; 4] = Default::default();
    curves[0] = Curve::new(vec![(0.0, 0.0), (0.35, 0.22), (0.7, 0.85), (1.0, 1.0)]);
    let adj = Adjustment::Curves { curves };

    for (label, w, h) in [("proxy 320x320", 320u32, 320u32), ("full 2MP", 1600, 1200)] {
        let mut buf = PixelBuffer::new(w, h);
        for (i, px) in buf.pixels_mut().iter_mut().enumerate() {
            let v = (i % 256) as u8;
            *px = cshop_core::color::Rgba8::new(v, v.wrapping_add(60), v.wrapping_add(120), 255);
        }

        let mut a = buf.clone();
        let t = Instant::now();
        for px in a.pixels_mut() {
            *px = adj.apply(px.to_f32()).to_u8();
        }
        let old = t.elapsed();

        let mut b = buf.clone();
        let t = Instant::now();
        adj.prepare().apply_buffer(b.pixels_mut());
        let new = t.elapsed();

        assert_eq!(a.pixels(), b.pixels(), "prepared path must match");
        println!(
            "{label:14}  per-pixel bake {old:>10.2?}   prepared {new:>10.2?}   {:.0}x",
            old.as_secs_f64() / new.as_secs_f64().max(1e-9)
        );
    }
}
