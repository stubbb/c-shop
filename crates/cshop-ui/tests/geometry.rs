//! Perspective crop and content-aware scale, through the application.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::layer::LayerKind;
use cshop_core::pixels::PixelBuffer;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

fn app_with(px: PixelBuffer) -> Option<CShopApp> {
    let gpu = GpuContext::headless().ok()?;
    let mut app = CShopApp::new(gpu);
    let (w, h) = (px.width(), px.height());
    app.open_document(Document::new("t", w, h, Background::Transparent));
    let view = app.doc_mut()?;
    let id = view.doc.active?;
    view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(px);
    view.invalidate();
    Some(app)
}

fn pixels(app: &CShopApp) -> &PixelBuffer {
    let view = app.doc().unwrap();
    view.doc.tree.get(view.doc.active.unwrap()).unwrap().pixels().unwrap()
}

/// Sky on the left, a dark bar on the right — the bar is the only thing worth
/// keeping.
fn sky_and_bar(w: u32, h: u32, bar: std::ops::Range<i32>) -> PixelBuffer {
    let mut px = PixelBuffer::filled(w, h, Rgba8::opaque(120, 140, 200));
    for y in 0..h as i32 {
        for x in bar.clone() {
            px.set(x, y, Rgba8::opaque(20, 20, 20));
        }
    }
    px
}

fn bar_width(px: &PixelBuffer, y: i32) -> i32 {
    (0..px.width() as i32).filter(|&x| px.get(x, y).r < 100).count() as i32
}

/// Wait for the worker thread to finish; a script would do the same.
fn settle_carve(app: &mut CShopApp) {
    let ctx = egui::Context::default();
    let started = std::time::Instant::now();
    while app.carve_progress().is_some() {
        app.poll_carve(&ctx);
        if started.elapsed() > std::time::Duration::from_secs(60) {
            panic!("the carve never finished");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn content_aware_scale_takes_the_space_and_leaves_the_subject() {
    let Some(mut app) = app_with(sky_and_bar(120, 40, 90..100)) else { return };
    assert_eq!(bar_width(pixels(&app), 20), 10);

    app.dispatch(Action::ContentAwareScale {
        width: 80,
        height: 40,
        protect_selection: false,
    });
    settle_carve(&mut app);

    assert_eq!(app.doc().unwrap().doc.width, 80);
    assert_eq!(bar_width(pixels(&app), 20), 10, "the bar keeps its width");
}

#[test]
fn a_content_aware_scale_undoes() {
    let Some(mut app) = app_with(sky_and_bar(100, 30, 70..80)) else { return };
    let before = pixels(&app).clone();
    app.dispatch(Action::ContentAwareScale {
        width: 70,
        height: 30,
        protect_selection: false,
    });
    settle_carve(&mut app);
    assert_eq!(app.doc().unwrap().doc.width, 70);
    assert_eq!(
        app.doc().unwrap().history.undo_name().unwrap_or_default(),
        "Content-Aware Scale"
    );

    app.dispatch(Action::Undo);
    assert_eq!(app.doc().unwrap().doc.width, 100);
    assert_eq!(pixels(&app).pixels(), before.pixels(), "every pixel back");
}

/// The selection is how the energy is overruled, which is where the
/// segmentation work pays off.
#[test]
fn the_selection_protects_what_it_covers() {
    let Some(mut app) = app_with(sky_and_bar(120, 40, 90..100)) else { return };
    {
        let view = app.doc_mut().unwrap();
        let mut m = cshop_core::mask::MaskBuffer::hide_all(120, 40);
        for y in 0..40 {
            for x in 0..60 {
                m.set(x, y, 255);
            }
        }
        view.doc.set_selection(Some(cshop_core::selection::Selection::from_mask(m)));
    }
    app.dispatch(Action::ContentAwareScale {
        width: 90,
        height: 40,
        protect_selection: true,
    });
    settle_carve(&mut app);
    // The left half was protected, so the seams had to come out of the bar.
    assert!(
        bar_width(pixels(&app), 20) < 10,
        "the seams had nowhere else to go: {}",
        bar_width(pixels(&app), 20)
    );
}

/// A photographed rectangle, straightened by dragging four corners onto it.
#[test]
fn a_perspective_crop_straightens_and_undoes() {
    let quad = [
        Vec2::new(30.0, 20.0),
        Vec2::new(130.0, 20.0),
        Vec2::new(150.0, 110.0),
        Vec2::new(10.0, 110.0),
    ];
    let mut px = PixelBuffer::filled(160, 120, Rgba8::opaque(30, 30, 30));
    // Fill the quad, so there is something to straighten.
    let board = cshop_core::geom::IRect::new(0, 0, 100, 100);
    let m = cshop_core::transform::Transform::from_quad(board, quad).unwrap();
    let inverse = m.invert().unwrap();
    for y in 0..120 {
        for x in 0..160 {
            let p = inverse.apply(Vec2::new(x as f32 + 0.5, y as f32 + 0.5));
            if (0.0..100.0).contains(&p.x) && (0.0..100.0).contains(&p.y) {
                px.set(x, y, Rgba8::opaque(220, 220, 220));
            }
        }
    }
    let Some(mut app) = app_with(px.clone()) else { return };

    app.tool = cshop_ui::tools::Tool::Crop;
    let mut crop = cshop_ui::transform_tool::ActiveCrop::new(
        cshop_core::geom::IRect::new(0, 0, 160, 120),
    );
    crop.set_perspective(true);
    crop.corners = Some(quad);
    app.crop = Some(crop);
    app.dispatch(Action::CommitCrop);

    let (w, h) = (app.doc().unwrap().doc.width, app.doc().unwrap().doc.height);
    assert!((w as i32 - 120).abs() < 8 && (h as i32 - 90).abs() < 8, "{w}x{h}");
    // The straightened board fills the frame, so its corners are pale.
    let out = pixels(&app);
    for (x, y) in [(3, 3), (w as i32 - 4, 3), (3, h as i32 - 4)] {
        assert!(out.get(x, y).r > 150, "({x}, {y}) should be inside the board");
    }

    app.dispatch(Action::Undo);
    assert_eq!(app.doc().unwrap().doc.width, 160);
    assert_eq!(pixels(&app).pixels(), px.pixels());
}

#[test]
fn a_quad_that_encloses_nothing_is_refused() {
    let Some(mut app) = app_with(sky_and_bar(60, 40, 40..50)) else { return };
    let mut crop = cshop_ui::transform_tool::ActiveCrop::new(
        cshop_core::geom::IRect::new(0, 0, 60, 40),
    );
    crop.set_perspective(true);
    crop.corners = Some([
        Vec2::new(0.0, 10.0),
        Vec2::new(20.0, 10.0),
        Vec2::new(40.0, 10.0),
        Vec2::new(60.0, 10.0),
    ]);
    app.crop = Some(crop);
    app.dispatch(Action::CommitCrop);
    assert_eq!(app.doc().unwrap().doc.width, 60, "nothing happened");
    let (msg, bad) = app.toast.clone().expect("and it said why");
    assert!(bad && msg.contains("corners"), "{msg}");
}

// --- Warp and puppet warp -------------------------------------------------

fn stripes(w: u32, h: u32) -> PixelBuffer {
    let mut px = PixelBuffer::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let on = (y / 8) % 2 == 0;
            px.set(x, y, if on { Rgba8::WHITE } else { Rgba8::opaque(40, 40, 40) });
        }
    }
    px
}

#[test]
fn a_warp_bends_the_middle_and_holds_the_corners() {
    let Some(mut app) = app_with(stripes(80, 80)) else { return };
    app.dispatch(Action::BeginWarp { puppet: false });
    let active = app.warp.as_ref().expect("a warp should have started");
    assert_eq!(active.warp.to.len(), 16, "a four-by-four mesh");

    // Pull one interior point down.
    {
        let w = &mut app.warp.as_mut().unwrap().warp;
        // Row 1, column 1 of the mesh — an interior point.
        w.to[5].y += 20.0;
    }
    app.refresh_warp();

    // The layer grew or moved to hold the bend.
    let view = app.doc().unwrap();
    let layer = view.doc.tree.get(view.doc.active.unwrap()).unwrap();
    let px = layer.pixels().unwrap();
    assert!(px.height() >= 80, "the bend needs room: {}", px.height());

    app.dispatch(Action::CommitWarp);
    assert!(app.warp.is_none());
    assert_eq!(app.doc().unwrap().history.undo_name().unwrap_or_default(), "Warp");
}

#[test]
fn a_puppet_warp_starts_pinned_at_the_corners() {
    let Some(mut app) = app_with(stripes(60, 60)) else { return };
    app.dispatch(Action::BeginWarp { puppet: true });
    let active = app.warp.as_ref().unwrap();
    assert!(active.puppet);
    assert_eq!(active.warp.to.len(), 4, "the corners, so a pin moves a part and not the lot");
    assert!(active.warp.rigid, "and rigidly, which is what keeps a shape a shape");
}

#[test]
fn cancelling_a_warp_puts_the_layer_back() {
    let Some(mut app) = app_with(stripes(60, 60)) else { return };
    let before = pixels(&app).clone();
    app.dispatch(Action::BeginWarp { puppet: true });
    {
        let w = &mut app.warp.as_mut().unwrap().warp;
        w.pin(Vec2::new(30.0, 30.0));
        w.to[4] = Vec2::new(45.0, 30.0);
    }
    app.refresh_warp();
    assert_ne!(pixels(&app).pixels(), before.pixels(), "the preview changed the layer");

    app.dispatch(Action::CancelWarp);
    assert!(app.warp.is_none());
    assert_eq!(pixels(&app).pixels(), before.pixels(), "and cancelling put it all back");
    assert!(app.doc().unwrap().history.labels().is_empty(), "with nothing in the history");
}

#[test]
fn a_warp_that_moved_nothing_is_not_an_undo_step() {
    let Some(mut app) = app_with(stripes(60, 60)) else { return };
    let before = pixels(&app).clone();
    app.dispatch(Action::BeginWarp { puppet: false });
    app.dispatch(Action::CommitWarp);
    assert!(app.doc().unwrap().history.labels().is_empty());
    assert_eq!(pixels(&app).pixels(), before.pixels(), "and it was not resampled either");
}

#[test]
fn warping_undoes() {
    let Some(mut app) = app_with(stripes(60, 60)) else { return };
    let before = pixels(&app).clone();
    app.dispatch(Action::BeginWarp { puppet: true });
    {
        let w = &mut app.warp.as_mut().unwrap().warp;
        w.pin(Vec2::new(30.0, 30.0));
        w.to[4] = Vec2::new(30.0, 48.0);
    }
    app.refresh_warp();
    app.dispatch(Action::CommitWarp);
    assert_eq!(app.doc().unwrap().history.undo_name().unwrap_or_default(), "Puppet Warp");

    app.dispatch(Action::Undo);
    assert_eq!(pixels(&app).pixels(), before.pixels());
}

// --- Aligning frames ------------------------------------------------------

/// A scene with plenty of distinct corners, which is what alignment needs and
/// a flat gradient is not.
fn cluttered(w: u32, h: u32, seed: u32) -> PixelBuffer {
    let mut px = PixelBuffer::filled(w, h, Rgba8::opaque(80, 90, 110));
    let mut s = seed | 1;
    let mut next = || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (s >> 8) as f32 / 16_777_216.0
    };
    for _ in 0..150 {
        let x = (next() * (w as f32 - 30.0)) as i32 + 8;
        let y = (next() * (h as f32 - 30.0)) as i32 + 8;
        let side = 4 + (next() * 10.0) as i32;
        let v = (next() * 200.0) as u8 + 30;
        for dy in 0..side {
            for dx in 0..side {
                px.set(x + dx, y + dy, Rgba8::opaque(v, v / 2 + 40, 255 - v));
            }
        }
    }
    px
}

fn shifted(px: &PixelBuffer, dx: i32, dy: i32) -> PixelBuffer {
    let mut out = PixelBuffer::new(px.width(), px.height());
    out.paste(px, dx, dy);
    out
}

fn add_layer(app: &mut CShopApp, name: &str, px: PixelBuffer) -> cshop_core::layer::LayerId {
    let view = app.doc_mut().unwrap();
    let id = view.doc.tree.alloc_id();
    view.doc.tree.push(
        cshop_core::layer::Layer::new(id, name, LayerKind::raster(px)),
        None,
    );
    view.invalidate();
    id
}

#[test]
fn a_shifted_layer_is_moved_back_onto_the_one_below() {
    let base = cluttered(320, 240, 7);
    let Some(mut app) = app_with(base.clone()) else { return };
    let id = add_layer(&mut app, "Frame 2", shifted(&base, 14, -9));

    app.dispatch(Action::AlignLayers {
        motion: cshop_core::align::Motion::Translation,
    });

    let view = app.doc().unwrap();
    let layer = view.doc.tree.get(id).unwrap();
    // Shifted by (14, -9), so aligning has to put it back by about (-14, 9).
    assert!(
        (layer.offset.0 + 14).abs() <= 2 && (layer.offset.1 - 9).abs() <= 2,
        "it landed at {:?}, which is not back where it came from",
        layer.offset
    );
    assert_eq!(view.history.undo_name().unwrap_or_default(), "Align Layers");
}

/// Two photographs of different things must be refused with a reason, not
/// aligned to whatever the arithmetic happened to produce.
#[test]
fn unrelated_layers_are_refused_and_named() {
    let Some(mut app) = app_with(cluttered(320, 240, 7)) else { return };
    add_layer(&mut app, "Something else", cluttered(320, 240, 999));

    app.dispatch(Action::AlignLayers {
        motion: cshop_core::align::Motion::Similarity,
    });
    let (msg, bad) = app.toast.clone().expect("it should have said something");
    assert!(bad, "and refused: {msg}");
    assert!(msg.contains("Something else"), "naming the layer: {msg}");
}

#[test]
fn stacking_averages_the_frames_into_a_new_layer() {
    let clean = cluttered(200, 160, 3);
    let mut noisy = Vec::new();
    let mut s = 12345u32;
    let mut next = || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (s >> 8) as f32 / 16_777_216.0
    };
    for _ in 0..4 {
        let mut f = clean.clone();
        for y in 0..160 {
            for x in 0..200 {
                let c = clean.get(x, y);
                let n =
                    |v: u8, r: f32| (v as f32 + (r - 0.5) * 80.0).clamp(0.0, 255.0) as u8;
                f.set(
                    x,
                    y,
                    Rgba8::new(n(c.r, next()), n(c.g, next()), n(c.b, next()), c.a),
                );
            }
        }
        noisy.push(f);
    }

    let Some(mut app) = app_with(noisy[0].clone()) else { return };
    for (i, f) in noisy.iter().skip(1).enumerate() {
        add_layer(&mut app, &format!("Frame {}", i + 2), f.clone());
    }
    let before = app.doc().unwrap().doc.tree.len();

    app.dispatch(Action::StackLayers);

    let view = app.doc().unwrap();
    assert_eq!(view.doc.tree.len(), before + 1, "a stacked layer was added");
    let stacked = view
        .doc
        .tree
        .iter_all()
        .into_iter()
        .filter_map(|id| view.doc.tree.get(id))
        .find(|l| l.name.starts_with("Stacked"))
        .expect("named for what it is");

    let error_of = |px: &PixelBuffer| {
        let mut total = 0.0f64;
        for y in 0..160 {
            for x in 0..200 {
                total += (px.get(x, y).r as f64 - clean.get(x, y).r as f64).powi(2);
            }
        }
        (total / (200.0 * 160.0)).sqrt()
    };
    let one = error_of(&noisy[0]);
    let many = error_of(stacked.pixels().unwrap());
    assert!(many < one * 0.7, "stacking should have cut the noise: {one:.1} to {many:.1}");
}

#[test]
fn aligning_needs_something_to_align() {
    let Some(mut app) = app_with(cluttered(120, 100, 5)) else { return };
    app.dispatch(Action::AlignLayers {
        motion: cshop_core::align::Motion::Similarity,
    });
    let (msg, bad) = app.toast.clone().expect("it should have said so");
    assert!(bad && msg.contains("two layers"), "{msg}");
}
