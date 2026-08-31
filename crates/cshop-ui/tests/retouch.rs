//! Dodge, Burn and Sponge, driven through the real stroke engine.
//!
//! The unit tests in `cshop_core::retouch` and `cshop_core::paint` check the arithmetic on one pixel.
//! These check that a stroke reaches the layer at all, lands where the brush
//! was and nowhere else, respects its tonal range on a real gradient, and
//! comes back off with one undo.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::layer::LayerKind;
use cshop_core::paint::PaintMode;
use cshop_core::pixels::PixelBuffer;
use cshop_core::retouch::{Retouch, RetouchKind, Tones};
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::app::StrokeFrom;
use cshop_ui::CShopApp;

/// A horizontal ramp from black to white, so a tonal range has somewhere to
/// act and somewhere to leave alone.
fn ramp(w: u32, h: u32) -> PixelBuffer {
    let mut px = PixelBuffer::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let v = (x as f32 / (w - 1) as f32 * 255.0).round() as u8;
            px.set(x, y, Rgba8::opaque(v, v, v));
        }
    }
    px
}

fn app_with(px: PixelBuffer) -> Option<CShopApp> {
    let gpu = GpuContext::headless().ok()?;
    let mut app = CShopApp::new(gpu);
    let (w, h) = (px.width(), px.height());
    app.open_document(Document::new("t", w, h, Background::Transparent));
    let view = app.doc_mut().unwrap();
    let id = view.doc.active.unwrap();
    view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(px);
    view.invalidate();
    Some(app)
}

fn pixels(app: &CShopApp) -> &PixelBuffer {
    let view = app.doc().unwrap();
    view.doc.tree.get(view.doc.active.unwrap()).unwrap().pixels().unwrap()
}

fn stroke(app: &mut CShopApp, at: (f32, f32), r: Retouch) {
    app.brush.size = 24.0;
    app.brush.hardness = 1.0;
    app.brush.opacity = 1.0;
    app.begin_stroke(Vec2::new(at.0, at.1), PaintMode::Retouch(r));
    app.end_stroke();
}

#[test]
fn dodging_lightens_where_the_brush_went_and_nowhere_else() {
    let Some(mut app) = app_with(ramp(128, 64)) else { return };
    let before = pixels(&app).clone();
    stroke(
        &mut app,
        (64.0, 32.0),
        Retouch { kind: RetouchKind::Dodge, range: Tones::Midtones, exposure: 0.8, soak: true },
    );

    let after = pixels(&app);
    assert!(after.get(64, 32).r > before.get(64, 32).r, "under the brush it lightens");
    assert_eq!(after.get(10, 32), before.get(10, 32), "well outside it, nothing moved");
    assert_eq!(after.get(64, 5), before.get(64, 5), "nor above it");
}

#[test]
fn burning_darkens() {
    let Some(mut app) = app_with(ramp(128, 64)) else { return };
    let before = pixels(&app).get(64, 32).r;
    stroke(
        &mut app,
        (64.0, 32.0),
        Retouch { kind: RetouchKind::Burn, range: Tones::Midtones, exposure: 0.8, soak: true },
    );
    assert!(pixels(&app).get(64, 32).r < before, "burning takes light away");
}

/// The whole point of the range control: a highlights stroke laid across the
/// dark end of a ramp should do nothing much, and the same stroke at the light
/// end should do a lot.
#[test]
fn the_range_decides_where_the_stroke_bites() {
    let Some(mut app) = app_with(ramp(256, 64)) else { return };
    let before = pixels(&app).clone();
    let hi = Retouch { kind: RetouchKind::Dodge, range: Tones::Highlights, exposure: 1.0, soak: true };
    stroke(&mut app, (20.0, 32.0), hi); // in the shadows
    stroke(&mut app, (230.0, 32.0), hi); // in the highlights

    let after = pixels(&app);
    let moved = |x: i32| after.get(x, 32).r as i32 - before.get(x, 32).r as i32;
    assert!(moved(20) <= 2, "a highlights stroke barely touches a shadow: {}", moved(20));
    assert!(moved(230) > 10, "but it moves a highlight: {}", moved(230));
}

#[test]
fn the_sponge_changes_colour_without_moving_the_tone() {
    let mut px = PixelBuffer::new(64, 64);
    for y in 0..64 {
        for x in 0..64 {
            px.set(x, y, Rgba8::opaque(200, 90, 90));
        }
    }
    let Some(mut app) = app_with(px) else { return };
    stroke(
        &mut app,
        (32.0, 32.0),
        Retouch { kind: RetouchKind::Sponge, range: Tones::Midtones, exposure: 0.9, soak: false },
    );
    let c = pixels(&app).get(32, 32);
    let (before, after) = (200i32 - 90, c.r as i32 - c.g as i32);
    assert!(after < before, "wrung out, the channels close up: {before} to {after}");
    // The tone is what the sponge must not move — measured with the same
    // weights the operation uses, which is the invariant it actually claims.
    let luma = |c: Rgba8| c.to_f32().luma() * 255.0;
    let drift = (luma(c) - luma(Rgba8::opaque(200, 90, 90))).abs();
    assert!(drift < 1.5, "the brightness should hold; it moved by {drift}");
}

#[test]
fn a_retouching_stroke_is_one_undo_step_named_after_itself() {
    let Some(mut app) = app_with(ramp(128, 64)) else { return };
    let before = pixels(&app).clone();
    stroke(
        &mut app,
        (64.0, 32.0),
        Retouch { kind: RetouchKind::Burn, range: Tones::Midtones, exposure: 0.8, soak: true },
    );
    assert_ne!(pixels(&app).get(64, 32), before.get(64, 32));

    let view = app.doc().unwrap();
    let named = view.history.undo_name().unwrap_or_default();
    assert_eq!(named, "Burn Tool", "the step should say which tool made it");

    app.dispatch(Action::Undo);
    assert_eq!(pixels(&app).pixels(), before.pixels(), "and one undo puts it all back");
}

/// A mask has no colour to shape. Doing nothing silently would look like the
/// tool was broken, so it says what it is refusing and why.
#[test]
fn retouching_a_mask_says_it_cannot() {
    let Some(mut app) = app_with(ramp(64, 64)) else { return };
    app.dispatch(Action::AddLayerMask { hide_all: false });
    stroke(
        &mut app,
        (32.0, 32.0),
        Retouch { kind: RetouchKind::Dodge, ..Default::default() },
    );
    let (msg, bad) = app.toast.clone().expect("it should have said something");
    assert!(bad && msg.contains("mask"), "{msg}");
}

// --- Blur, Sharpen and Smudge -------------------------------------------

/// A vertical edge, which is the only thing that tells these three apart:
/// blur softens it, sharpen steepens it, smudge drags it sideways.
fn edge(w: u32, h: u32) -> PixelBuffer {
    let mut px = PixelBuffer::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let v = if x < w as i32 / 2 { 40 } else { 215 };
            px.set(x, y, Rgba8::opaque(v, v, v));
        }
    }
    px
}

/// How steep the edge is at its middle — the measurement all three move.
fn slope(px: &PixelBuffer, y: i32) -> i32 {
    let mid = px.width() as i32 / 2;
    px.get(mid + 1, y).r as i32 - px.get(mid - 2, y).r as i32
}

#[test]
fn the_blur_brush_softens_the_edge_it_is_dragged_along() {
    let Some(mut app) = app_with(edge(64, 64)) else { return };
    let before = slope(pixels(&app), 32);
    app.tool = cshop_ui::tools::Tool::Blur;
    app.brush.size = 20.0;
    app.brush.hardness = 1.0;
    app.brush_filter_strength = 1.0;
    let filter = app.brush_filter().expect("the blur brush should have a filter");
    app.begin_stroke_from(Vec2::new(32.0, 16.0), PaintMode::Paint, StrokeFrom::Filter(filter));
    app.continue_stroke(Vec2::new(32.0, 48.0));
    app.end_stroke();

    let after = slope(pixels(&app), 32);
    assert!(after < before, "the edge should be gentler: {before} became {after}");
    // And only where the brush went.
    assert_eq!(slope(pixels(&app), 2), before, "the top of the edge is untouched");
}

/// Sharpening is an unsharp mask, and what an unsharp mask does to a soft
/// edge is overshoot it: the dark side dips darker just before the rise and
/// the light side rises higher just after. That — not the slope at the centre,
/// which a symmetric kernel leaves exactly where it found it — is the
/// measurement that says it worked.
#[test]
fn the_sharpen_brush_overshoots_a_soft_edge() {
    // A logistic edge: soft, and curved on both shoulders.
    let mut px = PixelBuffer::new(64, 64);
    for y in 0..64 {
        for x in 0..64i32 {
            // Soft, but on the scale the brush's own radius works at — a
            // sharpen radius much smaller than the edge it is applied to does
            // nothing anyone can see, and rightly so.
            let t = 1.0 / (1.0 + (-((x - 32) as f32) / 1.5).exp());
            let v = (40.0 + t * 175.0) as u8;
            px.set(x, y, Rgba8::opaque(v, v, v));
        }
    }
    let Some(mut app) = app_with(px) else { return };
    let before = pixels(&app).clone();

    app.tool = cshop_ui::tools::Tool::Sharpen;
    // Small enough that the top and bottom rows are genuinely outside it.
    app.brush.size = 20.0;
    app.brush.hardness = 1.0;
    app.brush_filter_strength = 1.0;
    let filter = app.brush_filter().unwrap();
    app.begin_stroke_from(Vec2::new(32.0, 16.0), PaintMode::Paint, StrokeFrom::Filter(filter));
    app.continue_stroke(Vec2::new(32.0, 48.0));
    app.end_stroke();

    // The dip and the lift are local, so look for them rather than guessing
    // where they land: somewhere on the dark side a pixel got darker, and
    // somewhere on the light side one got lighter.
    let after = pixels(&app);
    let moved = |xs: std::ops::Range<i32>| -> (i32, i32) {
        xs.map(|x| after.get(x, 32).r as i32 - before.get(x, 32).r as i32)
            .fold((0, 0), |(lo, hi), d| (lo.min(d), hi.max(d)))
    };
    let (dark_dip, _) = moved(16..32);
    let (_, light_lift) = moved(32..48);
    assert!(dark_dip < -2, "the dark side should dip; the most it fell was {dark_dip}");
    assert!(light_lift > 2, "the light side should lift; the most it rose was {light_lift}");

    // Above the stroke, nothing at all: the brush is 16 across and starts at
    // y = 16, so row 2 is well clear of it.
    for x in 0..64 {
        assert_eq!(after.get(x, 2), before.get(x, 2), "row 2 is outside the stroke");
    }
}

#[test]
fn the_smudge_brush_drags_and_undoes_in_one_step() {
    let Some(mut app) = app_with(edge(96, 64)) else { return };
    let before = pixels(&app).clone();
    app.brush.size = 18.0;
    app.brush.hardness = 1.0;
    app.brush.flow = 0.4;
    app.brush.opacity = 1.0;

    // From the light side back into the dark one.
    app.begin_smudge(Vec2::new(60.0, 32.0), 0.8);
    for x in (30..=58).rev() {
        app.continue_stroke(Vec2::new(x as f32, 32.0));
    }
    app.end_stroke();

    let after = pixels(&app);
    assert!(
        after.get(40, 32).r > before.get(40, 32).r + 5,
        "light should have been dragged into the dark side: {} to {}",
        before.get(40, 32).r,
        after.get(40, 32).r
    );
    assert_eq!(after.get(5, 5), before.get(5, 5), "and nowhere near the corner");

    let view = app.doc().unwrap();
    assert_eq!(view.history.undo_name().unwrap_or_default(), "Smudge Tool");
    app.dispatch(Action::Undo);
    assert_eq!(pixels(&app).pixels(), before.pixels(), "one undo puts the whole drag back");
}

/// A blur brush that read the live layer would chase its own output and smear
/// along the direction of travel. It reads a frozen copy instead, so a single
/// stroke blurs once however slowly the pointer crosses.
#[test]
fn a_blur_stroke_does_not_chase_its_own_output() {
    let Some(mut app) = app_with(edge(64, 64)) else { return };
    app.tool = cshop_ui::tools::Tool::Blur;
    app.brush.size = 20.0;
    app.brush.hardness = 1.0;
    app.brush_filter_strength = 1.0;
    let filter = app.brush_filter().unwrap();

    // The same stroke, once in two steps and once in twenty.
    app.begin_stroke_from(Vec2::new(32.0, 10.0), PaintMode::Paint, StrokeFrom::Filter(filter));
    app.continue_stroke(Vec2::new(32.0, 54.0));
    app.end_stroke();
    let quick = pixels(&app).clone();

    app.dispatch(Action::Undo);
    app.begin_stroke_from(Vec2::new(32.0, 10.0), PaintMode::Paint, StrokeFrom::Filter(filter));
    for i in 1..=20 {
        app.continue_stroke(Vec2::new(32.0, 10.0 + i as f32 * 2.2));
    }
    app.end_stroke();
    let slow = pixels(&app);

    let worst = (0..64)
        .flat_map(|y| (0..64).map(move |x| (x, y)))
        .map(|(x, y)| (slow.get(x, y).r as i32 - quick.get(x, y).r as i32).abs())
        .max()
        .unwrap();
    assert!(worst <= 2, "how fast the pointer moved should not matter; worst was {worst}");
}

// --- Healing -------------------------------------------------------------

/// A brightness gradient with fine texture, and a dark mark on it — the case
/// the clone stamp gets wrong.
fn blemished(w: u32, h: u32, spot: (i32, i32), r: i32) -> PixelBuffer {
    let mut px = PixelBuffer::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let base = 60.0 + y as f32 / h as f32 * 140.0;
            let n = ((x * 7 + y * 13) % 11) as f32 - 5.0;
            let v = (base + n * 3.0).clamp(0.0, 255.0) as u8;
            px.set(x, y, Rgba8::opaque(v, v, v));
        }
    }
    for y in -r..=r {
        for x in -r..=r {
            if x * x + y * y <= r * r {
                px.set(spot.0 + x, spot.1 + y, Rgba8::opaque(20, 20, 20));
            }
        }
    }
    px
}

/// How far the repaired patch sits from the brightness of the picture just
/// above and below it.
fn tone_error(px: &PixelBuffer, at: (i32, i32), r: i32) -> f32 {
    let mean = |cx: i32, cy: i32| {
        let (mut sum, mut n) = (0.0, 0.0);
        for y in -(r - 2)..=(r - 2) {
            for x in -(r - 2)..=(r - 2) {
                sum += px.get(cx + x, cy + y).r as f32;
                n += 1.0;
            }
        }
        sum / n
    };
    (mean(at.0, at.1) - (mean(at.0, at.1 - r * 2) + mean(at.0, at.1 + r * 2)) / 2.0).abs()
}

#[test]
fn the_healing_brush_repairs_a_mark_on_a_gradient() {
    let (spot, r) = ((64, 90), 9);
    let Some(mut app) = app_with(blemished(128, 180, spot, r)) else { return };
    let before = tone_error(pixels(&app), spot, r);
    assert!(before > 50.0, "the mark should be obvious to begin with: {before:.1}");

    app.tool = cshop_ui::tools::Tool::HealingBrush;
    app.brush.size = (r * 2 + 6) as f32;
    app.brush.hardness = 1.0;
    app.brush.opacity = 1.0;
    // A source well up the gradient, which is where cloning goes wrong.
    app.set_clone_anchor(Vec2::new(spot.0 as f32, spot.1 as f32 - 40.0));
    app.begin_stroke_from(
        Vec2::new(spot.0 as f32, spot.1 as f32),
        PaintMode::Paint,
        StrokeFrom::Heal,
    );
    app.end_stroke();

    let after = tone_error(pixels(&app), spot, r);
    assert!(after < 6.0, "the repair should sit on the surrounding tone: {after:.1}");
    let view = app.doc().unwrap();
    assert_eq!(view.history.undo_name().unwrap_or_default(), "Healing Brush");
}

#[test]
fn spot_healing_needs_no_source() {
    let (spot, r) = ((64, 90), 9);
    let Some(mut app) = app_with(blemished(128, 180, spot, r)) else { return };
    let before = pixels(&app).clone();

    app.tool = cshop_ui::tools::Tool::SpotHealing;
    app.brush.size = (r * 2 + 6) as f32;
    app.brush.hardness = 1.0;
    app.brush.opacity = 1.0;
    // Deliberately no anchor set.
    app.begin_stroke_from(
        Vec2::new(spot.0 as f32, spot.1 as f32),
        PaintMode::Paint,
        StrokeFrom::HealSpot,
    );
    app.end_stroke();

    let after = tone_error(pixels(&app), spot, r);
    assert!(after < 8.0, "it should have found a source and used it: {after:.1}");
    assert_eq!(
        pixels(&app).get(5, 5),
        before.get(5, 5),
        "and left the rest of the picture alone"
    );
}

/// The healing brush without a source has to say so. Doing nothing would be
/// indistinguishable from a broken tool.
#[test]
fn the_healing_brush_asks_for_a_source() {
    let Some(mut app) = app_with(blemished(128, 180, (64, 90), 9)) else { return };
    let before = pixels(&app).clone();
    app.tool = cshop_ui::tools::Tool::HealingBrush;
    app.begin_stroke_from(Vec2::new(64.0, 90.0), PaintMode::Paint, StrokeFrom::Heal);
    app.end_stroke();
    assert_eq!(pixels(&app).pixels(), before.pixels(), "nothing should have happened");
    let (msg, _) = app.toast.clone().expect("but it should have said why");
    assert!(msg.contains("Alt-click"), "{msg}");
}

// --- The History Brush ---------------------------------------------------

/// Painting a region back to how it was, while everything else stays as it is
/// now. Undo would take the whole document back; this is the local version.
#[test]
fn the_history_brush_paints_one_region_back() {
    let Some(mut app) = app_with(ramp(128, 128)) else { return };
    let original = pixels(&app).clone();

    // Mark the document as it was opened, then change the whole layer.
    app.dispatch(Action::SetHistorySource(0));
    app.dispatch(Action::ApplyAdjustment(Box::new(
        cshop_core::adjust::Adjustment::Invert,
    )));
    let inverted = pixels(&app).clone();
    assert_ne!(inverted.get(64, 64), original.get(64, 64), "the layer should have changed");

    // Paint one spot back.
    app.tool = cshop_ui::tools::Tool::HistoryBrush;
    app.brush.size = 24.0;
    app.brush.hardness = 1.0;
    app.brush.opacity = 1.0;
    app.begin_stroke_from(Vec2::new(64.0, 64.0), PaintMode::Paint, StrokeFrom::History);
    app.end_stroke();

    let after = pixels(&app);
    assert_eq!(after.get(64, 64), original.get(64, 64), "under the brush it is as it was");
    assert_eq!(after.get(5, 5), inverted.get(5, 5), "away from it, the change stands");

    let view = app.doc().unwrap();
    assert_eq!(view.history.undo_name().unwrap_or_default(), "History Brush");
}

/// Marking a state has to leave the document exactly where it was: it walks
/// the history there to take a copy, and walks back.
#[test]
fn marking_a_state_does_not_move_the_document() {
    let Some(mut app) = app_with(ramp(64, 64)) else { return };
    app.dispatch(Action::ApplyAdjustment(Box::new(cshop_core::adjust::Adjustment::Invert)));
    let now = pixels(&app).clone();
    let cursor = app.doc().unwrap().history.cursor();

    app.dispatch(Action::SetHistorySource(0));

    assert_eq!(pixels(&app).pixels(), now.pixels(), "the pixels should not have moved");
    assert_eq!(app.doc().unwrap().history.cursor(), cursor, "nor the history's place in itself");
}

#[test]
fn the_history_brush_says_when_nothing_is_marked() {
    let Some(mut app) = app_with(ramp(64, 64)) else { return };
    let before = pixels(&app).clone();
    app.tool = cshop_ui::tools::Tool::HistoryBrush;
    app.begin_stroke_from(Vec2::new(32.0, 32.0), PaintMode::Paint, StrokeFrom::History);
    app.end_stroke();
    assert_eq!(pixels(&app).pixels(), before.pixels());
    let (msg, _) = app.toast.clone().expect("it should have said what to do");
    assert!(msg.contains("History panel"), "{msg}");
}
