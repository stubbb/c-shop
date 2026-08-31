//! End-to-end tests for phase 5: filters applied through the application.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::filters::Filter;
use cshop_core::geom::IRect;
use cshop_core::layer::LayerKind;
use cshop_core::pixels::PixelBuffer;
use cshop_core::selection::{Rectf, Selection};
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

fn app_with(w: u32, h: u32) -> Option<CShopApp> {
    let gpu = match GpuContext::headless() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping: {e}");
            return None;
        }
    };
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("test", w, h, Background::Transparent));
    Some(app)
}

/// Half black, half white, so any blur shows immediately.
fn split(app: &mut CShopApp, w: u32, h: u32) {
    let view = app.doc_mut().unwrap();
    let id = view.doc.active.unwrap();
    let mut px = PixelBuffer::filled(w, h, Rgba8::BLACK);
    px.fill_rect(IRect::new(w as i32 / 2, 0, w as i32, h as i32), Rgba8::WHITE);
    view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(px);
    view.invalidate();
}

fn pixels(app: &CShopApp) -> &PixelBuffer {
    let view = app.doc().unwrap();
    view.doc.tree.get(view.doc.active.unwrap()).unwrap().pixels().unwrap()
}

#[test]
fn a_filter_changes_the_pixels_and_undoes() {
    let Some(mut app) = app_with(64, 64) else { return };
    split(&mut app, 64, 64);
    assert_eq!(pixels(&app).get(30, 32), Rgba8::BLACK);

    app.dispatch(Action::ApplyFilter(Box::new(Filter::GaussianBlur { radius: 9.0 })));
    let edge = pixels(&app).get(30, 32).r;
    assert!(edge > 10 && edge < 245, "the edge should have softened, got {edge}");
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Gaussian Blur"]);

    app.dispatch(Action::Undo);
    assert_eq!(pixels(&app).get(30, 32), Rgba8::BLACK, "undo restores the hard edge");
}

#[test]
fn a_filter_respects_the_selection() {
    let Some(mut app) = app_with(64, 64) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        let mut px = PixelBuffer::filled(64, 64, Rgba8::BLACK);
        // A grid of single white pixels, which a median filter would remove.
        for y in (4..64).step_by(8) {
            for x in (4..64).step_by(8) {
                px.set(x, y, Rgba8::WHITE);
            }
        }
        view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(px);
        view.invalidate();
    }

    let s = Selection::from_rect(64, 64, Rectf { x0: 0.0, y0: 0.0, x1: 32.0, y1: 64.0 }, false);
    app.dispatch(Action::SetSelection(Box::new(s), "Rectangular Marquee"));
    app.dispatch(Action::ApplyFilter(Box::new(Filter::Median { radius: 2 })));

    assert_eq!(pixels(&app).get(12, 12), Rgba8::BLACK, "specks removed inside the selection");
    assert_eq!(pixels(&app).get(44, 12), Rgba8::WHITE, "specks survive outside it");
}

#[test]
fn a_feathered_selection_fades_the_filter_in() {
    let Some(mut app) = app_with(128, 64) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().kind =
            LayerKind::raster(PixelBuffer::filled(128, 64, Rgba8::WHITE));
        view.invalidate();
    }

    let mut s =
        Selection::from_rect(128, 64, Rectf { x0: 20.0, y0: 0.0, x1: 108.0, y1: 64.0 }, false);
    s.feather(12.0);
    app.dispatch(Action::SetSelection(Box::new(s), "Rectangular Marquee"));
    app.dispatch(Action::ApplyFilter(Box::new(Filter::Solarize)));

    // Solarize turns white to black, so coverage reads directly as darkness.
    assert!(pixels(&app).get(64, 32).r < 20, "fully selected is fully filtered");
    assert_eq!(pixels(&app).get(2, 32), Rgba8::WHITE, "outside is untouched");
    let edge = pixels(&app).get(20, 32).r;
    assert!(edge > 20 && edge < 235, "the feathered edge should be partial, got {edge}");
}

#[test]
fn repeat_last_filter_reuses_the_settings() {
    let Some(mut app) = app_with(64, 64) else { return };
    split(&mut app, 64, 64);
    assert!(app.last_filter.is_none());

    app.dispatch(Action::RepeatLastFilter);
    assert!(app.doc().unwrap().history.labels().is_empty(), "nothing to repeat yet");

    app.dispatch(Action::ApplyFilter(Box::new(Filter::GaussianBlur { radius: 4.0 })));
    assert_eq!(app.last_filter, Some(Filter::GaussianBlur { radius: 4.0 }));

    app.dispatch(Action::RepeatLastFilter);
    assert_eq!(
        app.doc().unwrap().history.labels(),
        vec!["Gaussian Blur", "Gaussian Blur"],
        "the repeat should be its own history entry"
    );
}

#[test]
fn a_filter_with_no_settings_skips_the_dialog() {
    let Some(mut app) = app_with(32, 32) else { return };
    split(&mut app, 32, 32);

    app.dispatch(Action::ShowFilterDialog(Box::new(Filter::Solarize)));
    assert!(!app.dialog.is_open(), "Solarize has nothing to configure");
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Solarize"]);
}

#[test]
fn a_filter_with_settings_opens_a_dialog() {
    let Some(mut app) = app_with(32, 32) else { return };
    split(&mut app, 32, 32);

    app.dispatch(Action::ShowFilterDialog(Box::new(Filter::GaussianBlur { radius: 5.0 })));
    assert!(app.dialog.is_open());
    assert!(app.doc().unwrap().history.labels().is_empty(), "nothing applied until OK");
}

#[test]
fn a_pixel_locked_layer_refuses_filters() {
    let Some(mut app) = app_with(32, 32) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        let l = view.doc.tree.get_mut(id).unwrap();
        l.kind = LayerKind::raster(PixelBuffer::filled(32, 32, Rgba8::WHITE));
        l.locks.pixels = true;
        view.invalidate();
    }
    app.dispatch(Action::ApplyFilter(Box::new(Filter::Solarize)));
    assert_eq!(pixels(&app).get(16, 16), Rgba8::WHITE, "the lock should hold");
    assert!(app.doc().unwrap().history.labels().is_empty());
}

#[test]
fn a_background_layer_still_accepts_filters() {
    // The Background layer is locked against *moving*, not against
    // painting or filtering, and so is ours.
    let gpu = match GpuContext::headless() {
        Ok(g) => g,
        Err(_) => return,
    };
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("test", 32, 32, Background::White));
    app.dispatch(Action::ApplyFilter(Box::new(Filter::Solarize)));
    assert_eq!(pixels(&app).get(16, 16), Rgba8::BLACK, "white solarises to black");
}

#[test]
fn a_generative_filter_fills_the_layer() {
    let Some(mut app) = app_with(48, 48) else { return };
    app.foreground = Rgba8::opaque(255, 0, 0);
    app.background = Rgba8::opaque(0, 0, 255);
    app.dispatch(Action::ApplyFilter(Box::new(Filter::Clouds {
        scale: 20.0,
        seed: 3,
        difference: false,
    })));

    // Clouds interpolate between the two colours, so green never appears.
    for (x, y) in [(4, 4), (24, 24), (40, 40)] {
        let c = pixels(&app).get(x, y);
        assert_eq!(c.a, 255, "clouds fill the layer");
        assert_eq!(c.g, 0, "and stay on the red-to-blue ramp");
    }
}

#[test]
fn filtering_only_touches_the_selection_bounds() {
    // The command should rewrite the selection's rect, not the whole layer.
    let Some(mut app) = app_with(200, 200) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().kind =
            LayerKind::raster(PixelBuffer::filled(200, 200, Rgba8::WHITE));
        view.invalidate();
    }
    let s = Selection::from_rect(200, 200, Rectf { x0: 10.0, y0: 10.0, x1: 30.0, y1: 30.0 }, false);
    app.dispatch(Action::SetSelection(Box::new(s), "Rectangular Marquee"));
    app.dispatch(Action::ApplyFilter(Box::new(Filter::Solarize)));

    assert!(pixels(&app).get(20, 20).r < 20, "inside was filtered");
    assert_eq!(pixels(&app).get(100, 100), Rgba8::WHITE, "the rest is untouched");
}

#[test]
fn filter_actions_on_an_empty_workspace_do_not_panic() {
    let gpu = match GpuContext::headless() {
        Ok(g) => g,
        Err(_) => return,
    };
    let mut app = CShopApp::new(gpu);
    for action in [
        Action::RepeatLastFilter,
        Action::ShowFilterDialog(Box::new(Filter::GaussianBlur { radius: 3.0 })),
        Action::ApplyFilter(Box::new(Filter::Solarize)),
    ] {
        app.dispatch(action);
    }
    assert!(app.docs.is_empty());
}

#[test]
fn every_filter_can_be_applied_through_the_app() {
    let Some(mut app) = app_with(48, 48) else { return };
    split(&mut app, 48, 48);
    // The whole menu, in sequence, on the same layer — the closest thing to a
    // user going down the list.
    for filter in Filter::all_defaults() {
        app.dispatch(Action::ApplyFilter(Box::new(filter.clone())));
        let view = app.doc().unwrap();
        assert_eq!(
            (view.doc.width, view.doc.height),
            (48, 48),
            "{} changed the document size",
            filter.name()
        );
    }
    assert_eq!(app.doc().unwrap().history.labels().len(), Filter::all_defaults().len());
}
