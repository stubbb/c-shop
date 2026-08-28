//! Offscreen rendering of a single frame to a PNG.
//!
//! Exists for two reasons: it is the only way to see what the interface looks
//! like from a terminal session, and it exercises the entire stack — theme,
//! panels, compositor, present pass — without a window, a compositor or a
//! display server.

use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;
use std::path::{Path, PathBuf};

/// Render `frames` frames at `size` and write the last one to `out`.
///
/// More than one frame is usually needed: the first creates documents from the
/// queued actions, and only the second sees them composited and registered as
/// egui textures.
pub fn capture(
    out: &Path,
    size: (u32, u32),
    files: &[String],
    frames: u32,
    setup: impl Fn(&mut CShopApp),
    clicks: Vec<(f32, f32, egui::PointerButton)>,
    // Press at the first point, move to the second, and hold, so the capture
    // shows an interaction in progress rather than its result.
    drag: Option<(f32, f32, f32, f32)>,
) -> Result<(), Box<dyn std::error::Error>> {
    let gpu = GpuContext::headless()?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;

    let egui_ctx = egui::Context::default();
    cshop_ui::theme::apply(&egui_ctx);

    let mut renderer =
        egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions::default());

    let mut app = CShopApp::new(gpu.clone());
    for f in files {
        app.push(Action::OpenPath(PathBuf::from(f)));
    }
    setup(&mut app);

    let target = cshop_gpu::texture::GpuTexture::render_target(
        &gpu,
        "screenshot",
        size.0,
        size.1,
        format,
    );

    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [size.0, size.1],
        pixels_per_point: 1.0,
    };

    let frames = frames.max(1).max(clicks.len() as u32 * 3 + 12);
    for frame in 0..frames {
        // Advance time in real steps: egui fades modals and tooltips in over
        // about 80 ms, so a capture that does not let the clock run catches
        // them half transparent.
        // Right-click on a chosen frame, so a context menu can be captured.
        // Everything before it settles the interface; everything after lets the
        // menu finish animating open.
        let mut events = Vec::new();
        // Clicks are staged a few frames apart so a menu can be opened and
        // then an item inside it reached, which is what photographing a
        // submenu takes.
        for (i, (x, y, button)) in clicks.iter().enumerate() {
            let pos = egui::pos2(*x, *y);
            let press = 3 + i as u32 * 3;
            // egui hit-tests using the *previous* frame's widget geometry, so
            // the pointer has to sit at the position for a frame before the
            // press or the click lands on nothing.
            if frame + 1 >= press && frame <= press + 1 {
                events.push(egui::Event::PointerMoved(pos));
            }
            if frame == press || frame == press + 1 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: *button,
                    pressed: frame == press,
                    modifiers: Default::default(),
                });
            }
        }

        if let Some((x0, y0, x1, y1)) = drag {
            // Settle, press, then walk across so the drag is live when the
            // last frame is captured.
            let press = 4;
            if frame + 1 >= press && frame <= press {
                events.push(egui::Event::PointerMoved(egui::pos2(x0, y0)));
            }
            if frame == press {
                events.push(egui::Event::PointerButton {
                    pos: egui::pos2(x0, y0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                });
            }
            if frame > press {
                let t = ((frame - press) as f32 / 6.0).min(1.0);
                events.push(egui::Event::PointerMoved(egui::pos2(
                    x0 + (x1 - x0) * t,
                    y0 + (y1 - y0) * t,
                )));
            }
        }

        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(size.0 as f32, size.1 as f32),
            )),
            time: Some(frame as f64 / 60.0),
            events,
            ..Default::default()
        };

        let app_ref = &mut app;
        let renderer_ref = &mut renderer;
        let output = egui_ctx.run_ui(raw_input, |ui| app_ref.update(ui, renderer_ref));
        let primitives = egui_ctx.tessellate(output.shapes, 1.0);

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shot") });

        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &primitives, &screen);

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shot"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.11,
                            g: 0.11,
                            b: 0.11,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            renderer.render(&mut pass.forget_lifetime(), &primitives, &screen);
        }

        gpu.queue.submit(Some(encoder.finish()));
        gpu.wait();

        for id in &output.textures_delta.free {
            renderer.free_texture(id);
        }
    }

    let pixels = cshop_gpu::readback::read_srgb8(&gpu, &target);
    cshop_io::save(out, &pixels, 100)?;
    println!("wrote {} ({}x{})", out.display(), size.0, size.1);
    Ok(())
}


/// A document showing what the Paint Bucket, Gradient and Clone Stamp produce.
pub fn build_tools_demo(app: &mut CShopApp) {
    use cshop_core::color::Rgba8;
    use cshop_core::document::{Background, Document};
    use cshop_core::fill::{Gradient, GradientKind};
    use cshop_core::geom::{IRect, Vec2};
    use cshop_core::layer::Layer;
    use cshop_core::paint::PaintMode;
    use cshop_core::pixels::PixelBuffer;

    let (w, h) = (860u32, 520u32);
    let mut doc = Document::new("Tools.csd", w, h, Background::Color(Rgba8::opaque(26, 30, 40)));

    // A band of flat shapes for the bucket to fill.
    if let Some(id) = doc.active {
        if let Some(px) = doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
            for (i, colour) in [
                Rgba8::opaque(60, 70, 90),
                Rgba8::opaque(90, 60, 70),
                Rgba8::opaque(60, 90, 70),
            ]
            .into_iter()
            .enumerate()
            {
                let x = 40 + i as i32 * 180;
                px.fill_rect(IRect::at(x, 330, 150, 140), colour);
            }
        }
    }
    let base = doc.active.unwrap();

    let gradient_layer = doc.tree.alloc_id();
    doc.tree.push(
        Layer::raster(gradient_layer, "Gradient", PixelBuffer::new(w, h)),
        None,
    );
    doc.select(Some(gradient_layer));
    app.open_document(doc);

    // A radial gradient across the top.
    app.gradient = Gradient {
        kind: GradientKind::Radial,
        dither: true,
        ..Gradient::to_transparent(Rgba8::opaque(90, 210, 255))
    };
    app.gradient_drag = Some((Vec2::new(250.0, 150.0), Vec2::new(560.0, 300.0)));
    app.commit_gradient();

    // Fill two of the three swatches with the bucket.
    app.dispatch(cshop_ui::commands::Action::SelectLayer(base));
    app.bucket.antialias = true;
    app.foreground = Rgba8::opaque(240, 190, 60);
    app.bucket_fill_at(Vec2::new(110.0, 400.0));
    app.foreground = Rgba8::opaque(230, 90, 120);
    app.bucket_fill_at(Vec2::new(290.0, 400.0));

    // And clone the first swatch over the third.
    app.tool = cshop_ui::tools::Tool::CloneStamp;
    app.brush.size = 90.0;
    app.brush.hardness = 0.7;
    app.set_clone_anchor(Vec2::new(110.0, 400.0));
    app.begin_stroke_with(Vec2::new(470.0, 400.0), PaintMode::Paint, true);
    for x in (470..=560).step_by(6) {
        app.continue_stroke(Vec2::new(x as f32, 400.0));
    }
    app.end_stroke();

    app.tool = cshop_ui::tools::Tool::Gradient;
    app.foreground = Rgba8::opaque(240, 190, 60);
}

/// A document with a photo-like base under a stack of adjustment layers, with
/// a Curves layer selected so the Properties panel shows its editor.
pub fn build_adjustment_demo(app: &mut CShopApp) {
    use cshop_core::adjust::{Adjustment, LevelsChannel};
    use cshop_core::color::Rgba8;
    use cshop_core::curve::Curve;
    use cshop_core::document::{Background, Document};
    use cshop_core::layer::Layer;

    let (w, h) = (860u32, 560u32);
    let mut doc = Document::new("Adjustments.csd", w, h, Background::White);

    // Something with a real tonal range: a sky-to-ground gradient with a sun
    // and some banding, so the histogram and the adjustments have work to do.
    if let Some(id) = doc.active {
        if let Some(px) = doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
            for y in 0..h {
                let v = y as f32 / h as f32;
                for x in 0..w {
                    let u = x as f32 / w as f32;
                    let sky = (1.0 - v).powf(1.6);
                    let mut r = 40.0 + 150.0 * sky + 40.0 * u;
                    let mut g = 60.0 + 120.0 * sky;
                    let mut b = 90.0 + 150.0 * sky;
                    // A soft sun.
                    let d = ((u - 0.72).powi(2) * 2.4 + (v - 0.28).powi(2)).sqrt();
                    let glow = (1.0 - (d / 0.22).clamp(0.0, 1.0)).powf(2.0);
                    r += 190.0 * glow;
                    g += 150.0 * glow;
                    b += 60.0 * glow;
                    // Ground.
                    if v > 0.62 {
                        let t = ((v - 0.62) / 0.38).clamp(0.0, 1.0);
                        r = 30.0 + 60.0 * (1.0 - t);
                        g = 50.0 + 70.0 * (1.0 - t) + 20.0 * (u * 9.0).sin().abs();
                        b = 30.0 + 40.0 * (1.0 - t);
                    }
                    px.set(
                        x as i32,
                        y as i32,
                        Rgba8::opaque(
                            r.clamp(0.0, 255.0) as u8,
                            g.clamp(0.0, 255.0) as u8,
                            b.clamp(0.0, 255.0) as u8,
                        ),
                    );
                }
            }
        }
    }

    // A stack of adjustment layers, bottom to top.
    let levels = LevelsChannel {
        input_black: 0.06,
        input_white: 0.92,
        gamma: 1.15,
        ..Default::default()
    };
    let lv = doc.tree.alloc_id();
    doc.tree.push(
        Layer::adjustment(lv, Adjustment::Levels { rgb: levels, channels: [LevelsChannel::default(); 3] }),
        None,
    );

    let vb = doc.tree.alloc_id();
    doc.tree.push(
        Layer::adjustment(vb, Adjustment::Vibrance { vibrance: 0.55, saturation: 0.05 }),
        None,
    );

    let pf = doc.tree.alloc_id();
    let mut filter = Layer::adjustment(
        pf,
        Adjustment::PhotoFilter {
            color: Rgba8::opaque(236, 138, 0),
            density: 0.35,
            preserve_luminosity: true,
        },
    );
    filter.opacity = 0.8;
    doc.tree.push(filter, None);

    // The selected layer, so its editor is what the Properties panel shows.
    let mut curves: [Curve; 4] = Default::default();
    curves[0] = Curve::new(vec![(0.0, 0.02), (0.28, 0.22), (0.72, 0.80), (1.0, 0.99)]);
    curves[3] = Curve::new(vec![(0.0, 0.05), (1.0, 0.95)]);
    let cv = doc.tree.alloc_id();
    doc.tree.push(Layer::adjustment(cv, Adjustment::Curves { curves }), None);

    doc.select(Some(cv));
    app.open_document(doc);
}

/// Assemble a document showing off the selection tools: an active elliptical
/// selection with marching ants, and a layer mask made from a selection.
pub fn build_selection_demo(app: &mut CShopApp) {
    use cshop_core::color::Rgba8;
    use cshop_core::document::{Background, Document, EditTarget};
    use cshop_core::geom::IRect;
    use cshop_core::layer::{Layer, LayerMask};
    use cshop_core::pixels::PixelBuffer;
    use cshop_core::selection::{Rectf, Selection};

    let (w, h) = (820u32, 560u32);
    let mut doc = Document::new("Selections.csd", w, h, Background::Color(Rgba8::opaque(32, 36, 46)));

    // A checkerboard of colour so the selection edges are easy to read.
    if let Some(id) = doc.active {
        if let Some(px) = doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
            for y in 0..h {
                for x in 0..w {
                    let cell = ((x / 40) + (y / 40)) % 2 == 0;
                    let t = x as f32 / w as f32;
                    let c = if cell {
                        Rgba8::opaque((40.0 + 150.0 * t) as u8, 70, (200.0 - 90.0 * t) as u8)
                    } else {
                        Rgba8::opaque(28, 34, 44)
                    };
                    px.set(x as i32, y as i32, c);
                }
            }
        }
    }

    // A masked layer: a solid band revealed only through a selection-shaped
    // mask, so the Layers panel shows both plates.
    let band_id = doc.tree.alloc_id();
    let mut band = Layer::raster(
        band_id,
        "Masked band",
        PixelBuffer::filled(w, 150, Rgba8::opaque(250, 200, 90)),
    );
    band.offset = (0, 360);
    let mut mask_data = cshop_core::mask::MaskBuffer::hide_all(w, 150);
    // A soft wedge, which is what a feathered selection turned into a mask
    // looks like.
    for y in 0..150i32 {
        for x in 0..w {
            let t = 1.0 - (x as f32 / w as f32);
            mask_data.set(x as i32, y, (t.clamp(0.0, 1.0) * 255.0) as u8);
        }
    }
    band.mask = Some(LayerMask { data: mask_data, offset: (0, 360), enabled: true, linked: true });
    doc.tree.push(band, None);

    // A second layer with hard edges, to show a plain thumbnail beside it.
    let dot_id = doc.tree.alloc_id();
    let mut dots = PixelBuffer::new(300, 300);
    for y in 0..300i32 {
        for x in 0..300i32 {
            let (dx, dy) = ((x - 150) as f32, (y - 150) as f32);
            if (dx * dx + dy * dy).sqrt() < 140.0 {
                dots.set(x, y, Rgba8::new(90, 220, 255, 200));
            }
        }
    }
    let mut dot_layer = Layer::raster(dot_id, "Disc", dots);
    dot_layer.offset = (60, 40);
    dot_layer.blend_mode = cshop_core::blend::BlendMode::Screen;
    doc.tree.push(dot_layer, None);
    let _ = IRect::from_size(w, h);

    // An active elliptical selection, so the marching ants are visible.
    let mut selection =
        Selection::from_ellipse(w, h, Rectf { x0: 330.0, y0: 120.0, x1: 700.0, y1: 400.0 }, true);
    let notch = Selection::from_rect(w, h, Rectf { x0: 560.0, y0: 240.0, x1: 780.0, y1: 330.0 }, true);
    // Subtract a rectangle so the outline has a non-trivial shape.
    selection.combine(&notch, cshop_core::selection::SelectionMode::Subtract);
    doc.set_selection(Some(selection));

    doc.select(Some(band_id));
    doc.edit_target = EditTarget::Mask;
    app.open_document(doc);
}

/// Assemble a layered document that exercises the compositor visibly: blend
/// modes, group nesting, opacity, a clipping mask and a layer mask.
///
/// Used by `--screenshot --demo` to show the editor doing real work.
pub fn build_demo(app: &mut CShopApp) {
    use cshop_core::blend::BlendMode;
    use cshop_core::color::Rgba8;
    use cshop_core::document::{Background, Document};
    use cshop_core::layer::{Layer, LayerMask};
    use cshop_core::mask::MaskBuffer;
    use cshop_core::pixels::PixelBuffer;

    let (w, h) = (900u32, 600u32);
    let mut doc = Document::new("Demo.csd", w, h, Background::Color(Rgba8::opaque(24, 28, 38)));

    // A soft diagonal gradient as the base.
    if let Some(id) = doc.active {
        if let Some(px) = doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
            for y in 0..h {
                for x in 0..w {
                    let t = (x as f32 / w as f32 + y as f32 / h as f32) * 0.5;
                    px.set(
                        x as i32,
                        y as i32,
                        Rgba8::opaque(
                            (20.0 + 60.0 * t) as u8,
                            (28.0 + 90.0 * t) as u8,
                            (54.0 + 150.0 * t) as u8,
                        ),
                    );
                }
            }
        }
    }

    // Three overlapping discs, each in a different blend mode.
    let discs = [
        ("Cyan · Screen", Rgba8::opaque(0, 190, 255), (300, 250), BlendMode::Screen, 1.0),
        ("Magenta · Overlay", Rgba8::opaque(255, 40, 170), (470, 250), BlendMode::Overlay, 1.0),
        ("Amber · Linear Dodge", Rgba8::opaque(255, 190, 40), (385, 380), BlendMode::LinearDodge, 0.85),
    ];
    for (name, color, (cx, cy), mode, opacity) in discs {
        let r = 140i32;
        let size = (r * 2) as u32;
        let mut px = PixelBuffer::new(size, size);
        for y in 0..size as i32 {
            for x in 0..size as i32 {
                let (dx, dy) = ((x - r) as f32, (y - r) as f32);
                let d = (dx * dx + dy * dy).sqrt();
                // Feather the edge so the blend modes show a gradient.
                let a = (1.0 - (d / r as f32).clamp(0.0, 1.0)).powf(0.7);
                if a > 0.0 {
                    px.set(x, y, Rgba8::new(color.r, color.g, color.b, (a * 255.0) as u8));
                }
            }
        }
        let id = doc.tree.alloc_id();
        let mut layer = Layer::raster(id, name, px);
        layer.offset = (cx - r, cy - r);
        layer.blend_mode = mode;
        layer.opacity = opacity;
        doc.tree.push(layer, None);
    }

    // A group holding a masked band and a clipped highlight.
    let group_id = doc.tree.alloc_id();
    let mut group = Layer::group(group_id, "Accents");
    group.blend_mode = BlendMode::PassThrough;
    doc.tree.push(group, None);

    // A wide band, masked to fade out toward the right.
    let band_h = 90u32;
    let mut band = PixelBuffer::filled(w, band_h, Rgba8::opaque(240, 245, 255));
    for y in 0..band_h {
        for x in 0..w {
            let edge = 1.0 - ((y as f32 / band_h as f32) * 2.0 - 1.0).abs();
            band.set(x as i32, y as i32, Rgba8::new(240, 245, 255, (edge * 255.0) as u8));
        }
    }
    let band_id = doc.tree.alloc_id();
    let mut band_layer = Layer::raster(band_id, "Band · masked", band);
    band_layer.offset = (0, 470);
    band_layer.opacity = 0.9;
    let mut mask = MaskBuffer::hide_all(w, band_h);
    for y in 0..band_h {
        for x in 0..w {
            let t = 1.0 - x as f32 / w as f32;
            mask.set(x as i32, y as i32, (t.clamp(0.0, 1.0) * 255.0) as u8);
        }
    }
    band_layer.mask = Some(LayerMask { data: mask, offset: (0, 470), enabled: true, linked: true });
    doc.tree.push(band_layer, Some(group_id));

    // Clipped to the band above it, so it only shows where the band is.
    let stripe_id = doc.tree.alloc_id();
    let mut stripe = PixelBuffer::new(w, band_h);
    for y in 0..band_h {
        for x in 0..w {
            if (x / 26) % 2 == 0 {
                stripe.set(x as i32, y as i32, Rgba8::opaque(255, 90, 60));
            }
        }
    }
    let mut stripe_layer = Layer::raster(stripe_id, "Stripes · clipped", stripe);
    stripe_layer.offset = (0, 470);
    stripe_layer.clipping = true;
    stripe_layer.blend_mode = BlendMode::Multiply;
    doc.tree.push(stripe_layer, Some(group_id));

    doc.select(Some(group_id));
    app.open_document(doc);
}

/// A page of type, for looking at the Type tool.
pub fn build_text_demo(app: &mut CShopApp) {
    use cshop_core::color::Rgba8;
    use cshop_core::document::{Background, Document};
    use cshop_core::geom::Vec2;
    use cshop_core::text::TextAlign;
    use cshop_ui::commands::Action;

    let doc = Document::new("Type.csd", 860, 520, Background::Color(Rgba8::opaque(250, 248, 244)));
    app.open_document(doc);
    app.tool = cshop_ui::tools::Tool::Text;

    let write = |app: &mut CShopApp, at: Vec2, wrap: Option<f32>, text: &str, size: f32, colour: Rgba8, align: TextAlign, bold: bool, italic: bool| {
        app.foreground = colour;
        app.text_style.size = size;
        app.text_style.align = align;
        app.text_style.bold = bold;
        app.text_style.italic = italic;
        app.dispatch(Action::BeginText { at, wrap });
        for c in text.chars() {
            app.dispatch(Action::TextInput(cshop_ui::text_tool::TextInput::Insert(c.to_string())));
        }
        app.dispatch(Action::CommitText);
    };

    write(app, Vec2::new(48.0, 96.0), None, "Type in C-Shop", 54.0, Rgba8::opaque(20, 24, 34), TextAlign::Left, true, false);
    write(app, Vec2::new(48.0, 140.0), None, "Live, re-editable text layers", 24.0, Rgba8::opaque(90, 100, 120), TextAlign::Left, false, true);
    write(
        app,
        Vec2::new(48.0, 190.0),
        Some(430.0),
        "A paragraph box wraps between words at the width it was dragged out to. Leading, tracking and alignment all apply, and the layer stays editable afterwards.",
        18.0,
        Rgba8::opaque(40, 46, 60),
        TextAlign::Left,
        false,
        false,
    );
    write(app, Vec2::new(820.0, 200.0), None, "right aligned", 22.0, Rgba8::opaque(200, 60, 70), TextAlign::Right, false, false);
    write(app, Vec2::new(820.0, 240.0), None, "and centred", 22.0, Rgba8::opaque(40, 130, 90), TextAlign::Right, false, false);

    // Leave one layer mid-edit, so the caret and the outline are visible.
    app.foreground = Rgba8::opaque(20, 24, 34);
    app.text_style.size = 34.0;
    app.text_style.align = TextAlign::Left;
    app.text_style.bold = false;
    app.text_style.italic = false;
    app.dispatch(Action::BeginText { at: Vec2::new(48.0, 400.0), wrap: None });
    for c in "editing right now".chars() {
        app.dispatch(Action::TextInput(cshop_ui::text_tool::TextInput::Insert(c.to_string())));
    }
}

/// A page of shapes, for looking at the Shape tool.
pub fn build_shape_demo(app: &mut CShopApp) {
    use cshop_core::color::Rgba8;
    use cshop_core::document::{Background, Document};
    use cshop_core::geom::Vec2;
    use cshop_core::shape::{ShapeKind, ShapeStyle, StrokeAlign};
    use cshop_ui::commands::Action;

    let doc = Document::new("Shapes.csd", 860, 520, Background::Color(Rgba8::opaque(250, 249, 246)));
    app.open_document(doc);
    app.tool = cshop_ui::tools::Tool::Shape;

    let blue = Rgba8::opaque(56, 122, 223);
    let ink = Rgba8::opaque(22, 30, 48);
    let red = Rgba8::opaque(214, 68, 62);
    let green = Rgba8::opaque(38, 150, 108);

    let draw = |app: &mut CShopApp, kind: ShapeKind, style: ShapeStyle, a: (f32, f32), b: (f32, f32)| {
        app.shape_kind = kind;
        app.shape_style = style;
        app.dispatch(Action::DrawShape {
            from: Vec2::new(a.0, a.1),
            to: Vec2::new(b.0, b.1),
            from_centre: false,
            constrain: false,
        });
    };

    let solid = |c| ShapeStyle { fill: Some(c), stroke: None, ..Default::default() };
    let outlined = |f, s| ShapeStyle {
        fill: Some(f),
        stroke: Some(s),
        stroke_width: 6.0,
        stroke_align: StrokeAlign::Center,
        antialias: true,
    };
    let hollow = |s| ShapeStyle {
        fill: None,
        stroke: Some(s),
        stroke_width: 5.0,
        stroke_align: StrokeAlign::Inside,
        antialias: true,
    };

    draw(app, ShapeKind::Rectangle { radius: 0.0 }, solid(blue), (50.0, 60.0), (210.0, 170.0));
    draw(app, ShapeKind::Rectangle { radius: 26.0 }, outlined(blue, ink), (240.0, 60.0), (400.0, 170.0));
    draw(app, ShapeKind::Ellipse, outlined(green, ink), (430.0, 60.0), (590.0, 170.0));
    draw(app, ShapeKind::Ellipse, hollow(red), (620.0, 60.0), (780.0, 170.0));
    draw(app, ShapeKind::Polygon { sides: 6, star: false, inner: 0.5 }, outlined(blue, ink), (50.0, 210.0), (190.0, 350.0));
    draw(app, ShapeKind::Polygon { sides: 3, star: false, inner: 0.5 }, solid(green), (220.0, 210.0), (360.0, 350.0));
    draw(app, ShapeKind::Polygon { sides: 5, star: true, inner: 0.42 }, outlined(Rgba8::opaque(240, 190, 60), ink), (390.0, 210.0), (530.0, 350.0));
    draw(app, ShapeKind::Polygon { sides: 8, star: true, inner: 0.7 }, hollow(red), (560.0, 210.0), (700.0, 350.0));
    draw(app, ShapeKind::Line { thickness: 8.0, from: (0.0, 0.0), to: (1.0, 1.0) }, hollow(ink), (60.0, 390.0), (220.0, 470.0));
    draw(app, ShapeKind::Line { thickness: 3.0, from: (0.0, 0.0), to: (1.0, 1.0) }, hollow(red), (250.0, 470.0), (410.0, 390.0));
    // Outside vs inside stroke, side by side.
    draw(app, ShapeKind::Rectangle { radius: 10.0 }, ShapeStyle { stroke_align: StrokeAlign::Outside, ..outlined(blue, ink) }, (440.0, 390.0), (580.0, 470.0));
    draw(app, ShapeKind::Rectangle { radius: 10.0 }, ShapeStyle { stroke_align: StrokeAlign::Inside, ..outlined(blue, ink) }, (610.0, 390.0), (750.0, 470.0));
}

/// Layer effects on real layers, for looking at the Layer Style dialog.
pub fn build_effects_demo(app: &mut CShopApp) {
    use cshop_core::color::Rgba8;
    use cshop_core::document::{Background, Document};
    use cshop_core::effects::*;
    use cshop_core::geom::Vec2;
    use cshop_core::shape::{ShapeKind, ShapeStyle, StrokeAlign};
    use cshop_ui::commands::Action;

    let doc = Document::new("Effects.csd", 860, 520, Background::Color(Rgba8::opaque(96, 104, 116)));
    app.open_document(doc);

    // A row of shapes, each carrying a different style.
    let draw = |app: &mut CShopApp, kind: ShapeKind, fill: Rgba8, a: (f32, f32), b: (f32, f32)| {
        app.tool = cshop_ui::tools::Tool::Shape;
        app.shape_kind = kind;
        app.shape_style =
            ShapeStyle { fill: Some(fill), stroke: None, stroke_align: StrokeAlign::Center, ..Default::default() };
        app.dispatch(Action::DrawShape {
            from: Vec2::new(a.0, a.1),
            to: Vec2::new(b.0, b.1),
            from_centre: false,
            constrain: false,
        });
        app.doc().and_then(|v| v.doc.active).unwrap()
    };

    let style = |app: &mut CShopApp, id, fx: LayerEffects| {
        app.dispatch(Action::SetLayerEffects(id, Box::new(fx)));
    };

    let grey = Rgba8::opaque(206, 210, 216);

    let id = draw(app, ShapeKind::Rectangle { radius: 16.0 }, grey, (40.0, 50.0), (200.0, 160.0));
    let mut fx = LayerEffects::new();
    fx.drop_shadow = Some(Shadow { distance: 12.0, size: 12.0, ..Default::default() });
    fx.bevel = Some(Bevel { size: 10.0, depth: 1.3, soften: 2.0, ..Default::default() });
    style(app, id, fx);

    let id = draw(app, ShapeKind::Ellipse, grey, (240.0, 50.0), (400.0, 160.0));
    let mut fx = LayerEffects::new();
    fx.outer_glow = Some(Glow { size: 22.0, color: Rgba8::opaque(120, 220, 255), ..Default::default() });
    fx.inner_shadow = Some(Shadow { distance: 6.0, size: 10.0, ..Default::default() });
    style(app, id, fx);

    let id = draw(
        app,
        ShapeKind::Polygon { sides: 5, star: true, inner: 0.45 },
        grey,
        (440.0, 40.0),
        (600.0, 175.0),
    );
    let mut fx = LayerEffects::new();
    fx.color_overlay = Some(ColorOverlay { color: Rgba8::opaque(250, 190, 60), ..Default::default() });
    fx.stroke = Some(Stroke { size: 4.0, color: Rgba8::opaque(60, 40, 10), ..Default::default() });
    fx.drop_shadow = Some(Shadow { distance: 8.0, size: 8.0, ..Default::default() });
    style(app, id, fx);

    let id = draw(app, ShapeKind::Rectangle { radius: 8.0 }, grey, (640.0, 50.0), (800.0, 160.0));
    let mut fx = LayerEffects::new();
    fx.bevel = Some(Bevel { style: BevelStyle::Pillow, size: 12.0, depth: 1.5, ..Default::default() });
    fx.satin = Some(Satin::default());
    style(app, id, fx);

    // Type with a full style, and a stroke-only layer beside it.
    app.tool = cshop_ui::tools::Tool::Text;
    app.foreground = Rgba8::opaque(230, 234, 240);
    app.text_style.size = 76.0;
    app.text_style.bold = true;
    app.dispatch(Action::BeginText { at: Vec2::new(50.0, 300.0), wrap: None });
    for c in "Effects".chars() {
        app.dispatch(Action::TextInput(cshop_ui::text_tool::TextInput::Insert(c.to_string())));
    }
    app.dispatch(Action::CommitText);
    let id = app.doc().and_then(|v| v.doc.active).unwrap();
    let mut fx = LayerEffects::new();
    fx.drop_shadow = Some(Shadow { distance: 6.0, size: 6.0, ..Default::default() });
    fx.bevel = Some(Bevel { size: 6.0, depth: 1.2, soften: 1.0, ..Default::default() });
    fx.inner_glow = Some(Glow { size: 10.0, color: Rgba8::opaque(255, 220, 150), ..Default::default() });
    style(app, id, fx);

    app.dispatch(Action::BeginText { at: Vec2::new(50.0, 430.0), wrap: None });
    for c in "stroke only".chars() {
        app.dispatch(Action::TextInput(cshop_ui::text_tool::TextInput::Insert(c.to_string())));
    }
    app.dispatch(Action::CommitText);
    let id = app.doc().and_then(|v| v.doc.active).unwrap();
    let mut fx = LayerEffects::new();
    fx.stroke = Some(Stroke { size: 2.5, color: Rgba8::opaque(255, 255, 255), ..Default::default() });
    fx.outer_glow = Some(Glow { size: 14.0, opacity: 0.9, ..Default::default() });
    style(app, id, fx);
    // Fill opacity to zero: the layer's pixels go, the effects stay.
    app.dispatch(Action::SetLayerProperty(
        id,
        cshop_core::history::LayerProperty::FillOpacity(0.0),
    ));
}
