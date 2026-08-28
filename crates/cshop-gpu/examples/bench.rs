//! Compositor throughput on realistic documents.
//!
//! Measures what actually governs the feel of the editor: how long a full
//! recomposite takes at various document sizes and layer counts, and how much
//! cheaper a brush-sized dirty rectangle is than the whole canvas.

use cshop_core::blend::BlendMode;
use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Dirty, Document};
use cshop_core::geom::IRect;
use cshop_core::layer::Layer;
use cshop_core::pixels::PixelBuffer;
use cshop_gpu::compositor::Compositor;
use cshop_gpu::context::GpuContext;
use cshop_gpu::layers::LayerTextures;
use cshop_gpu::texture::GpuTexture;
use std::time::Instant;

fn build(width: u32, height: u32, layers: usize) -> Document {
    let mut doc = Document::new("bench", width, height, Background::White);
    let modes = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::Luminosity,
        BlendMode::ColorDodge,
        BlendMode::Difference,
    ];
    for i in 0..layers {
        let id = doc.tree.alloc_id();
        let shade = (40 + (i * 23) % 180) as u8;
        let px = PixelBuffer::filled(width, height, Rgba8::new(shade, 255 - shade, shade / 2, 200));
        let mut layer = Layer::raster(id, format!("L{i}"), px);
        layer.blend_mode = modes[i % modes.len()];
        layer.opacity = 0.85;
        doc.tree.push(layer, None);
    }
    doc
}

fn human(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
}

fn time(label: &str, iters: u32, mut f: impl FnMut()) -> f64 {
    // One untimed pass so shader compilation and allocation do not count.
    f();
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0 / iters as f64;
    println!("{label:<44} {ms:>8.2} ms   {:>7.1} fps", 1000.0 / ms);
    ms
}

fn main() {
    let ctx = match GpuContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("no GPU: {e}");
            return;
        }
    };
    println!("adapter: {}\n", ctx.adapter_name());

    let mut compositor = Compositor::new(&ctx);

    for (w, h, layers) in [
        (1920, 1080, 5),
        (1920, 1080, 20),
        (3840, 2160, 10),
        (6000, 4000, 10),
        (6000, 4000, 30),
    ] {
        // Full-resolution layers are the memory ceiling today; skip rather
        // than push the GPU into an allocation failure.
        let doc_probe = build(w, h, layers);
        let needed = LayerTextures::required_bytes(&doc_probe);
        drop(doc_probe);
        if needed > ctx.texture_budget() {
            println!(
                "{w}x{h}, {layers} layers: skipped — {} of layer textures exceeds the ~{} budget\n",
                human(needed),
                human(ctx.texture_budget())
            );
            continue;
        }
        let doc = build(w, h, layers);
        let mut cache = LayerTextures::new();
        cache.sync(&ctx, &doc, &Dirty::NONE);
        let dest = GpuTexture::render_target(&ctx, "dest", w, h, ctx.work_format());

        let mp = w as f64 * h as f64 / 1e6;
        time(
            &format!("{w}x{h} ({mp:.1} MP), {layers} layers, full"),
            10,
            || {
                compositor.composite(&ctx, &doc, &cache, &dest, doc.bounds());
                ctx.wait();
            },
        );

        // A brush dab only dirties its own bounding box.
        let dab = IRect::at(w as i32 / 2, h as i32 / 2, 256, 256);
        time(
            &format!("{w}x{h} ({mp:.1} MP), {layers} layers, 256px dab"),
            50,
            || {
                compositor.composite(&ctx, &doc, &cache, &dest, dab);
                ctx.wait();
            },
        );
        println!();
    }
}
