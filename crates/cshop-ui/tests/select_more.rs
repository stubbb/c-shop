//! Selecting by colour, refining an edge, and paths both ways — through the
//! application.

use cshop_core::color::Rgba8;
use cshop_core::color_range::{ColorRange, Pick};
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::layer::LayerKind;
use cshop_core::pixels::PixelBuffer;
use cshop_core::refine::RefineEdge;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

/// Two red patches with a blue one between them — one colour, two regions,
/// which is the case the wand cannot do in one go.
fn spotted() -> PixelBuffer {
    let mut px = PixelBuffer::filled(60, 20, Rgba8::opaque(30, 30, 200));
    for y in 4..16 {
        for x in 4..16 {
            px.set(x, y, Rgba8::opaque(200, 40, 40));
        }
        for x in 44..56 {
            px.set(x, y, Rgba8::opaque(200, 40, 40));
        }
    }
    px
}

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

fn coverage(app: &CShopApp, x: i32, y: i32) -> u8 {
    app.doc().unwrap().doc.selection.as_ref().map_or(0, |s| s.coverage(x, y))
}

#[test]
fn colour_range_selects_every_region_of_one_colour_at_once() {
    let Some(mut app) = app_with(spotted()) else { return };
    app.dispatch(Action::ApplyColorRange(Box::new(ColorRange {
        pick: Pick::Sampled(vec![Rgba8::opaque(200, 40, 40)]),
        fuzziness: 0.1,
        invert: false,
    })));

    assert_eq!(coverage(&app, 8, 10), 255, "the first patch");
    assert_eq!(coverage(&app, 50, 10), 255, "and the second, which is not joined to it");
    assert_eq!(coverage(&app, 30, 10), 0, "and not the blue between them");
}

#[test]
fn a_tonal_band_selects_by_brightness() {
    let mut px = PixelBuffer::new(3, 1);
    px.set(0, 0, Rgba8::opaque(8, 8, 8));
    px.set(1, 0, Rgba8::opaque(128, 128, 128));
    px.set(2, 0, Rgba8::opaque(248, 248, 248));
    let Some(mut app) = app_with(px) else { return };

    app.dispatch(Action::ApplyColorRange(Box::new(ColorRange {
        pick: Pick::Highlights,
        fuzziness: 0.3,
        invert: false,
    })));
    assert!(coverage(&app, 2, 0) > 200 && coverage(&app, 0, 0) < 20);
}

#[test]
fn a_colour_range_selection_undoes() {
    let Some(mut app) = app_with(spotted()) else { return };
    app.dispatch(Action::SelectAll);
    app.dispatch(Action::ApplyColorRange(Box::new(ColorRange {
        pick: Pick::Sampled(vec![Rgba8::opaque(200, 40, 40)]),
        fuzziness: 0.1,
        invert: false,
    })));
    assert_eq!(coverage(&app, 30, 10), 0);
    assert_eq!(app.doc().unwrap().history.undo_name().unwrap_or_default(), "Colour Range");
    app.dispatch(Action::Undo);
    assert_eq!(coverage(&app, 30, 10), 255, "back to everything selected");
}

/// A hard edge in the picture, and a selection whose edge is a few pixels off
/// it — refining should move it onto the picture's.
#[test]
fn refining_moves_the_selection_edge_onto_the_picture() {
    let mut px = PixelBuffer::new(64, 24);
    for y in 0..24 {
        for x in 0..64 {
            let v = if x < 32 { 230 } else { 25 };
            px.set(x, y, Rgba8::opaque(v, v, v));
        }
    }
    let Some(mut app) = app_with(px) else { return };
    {
        let view = app.doc_mut().unwrap();
        let mut m = cshop_core::mask::MaskBuffer::hide_all(64, 24);
        for y in 0..24 {
            for x in 0..29 {
                m.set(x, y, 255);
            }
        }
        view.doc.set_selection(Some(cshop_core::selection::Selection::from_mask(m)));
    }
    // Where the selection crosses half coverage, which is where its edge is.
    let crossing = |app: &CShopApp| -> f32 {
        for x in 0..63 {
            let (a, b) = (coverage(app, x, 12) as f32, coverage(app, x + 1, 12) as f32);
            if a >= 128.0 && b < 128.0 {
                return x as f32 + (a - 128.0) / (a - b).max(1e-4);
            }
        }
        -1.0
    };
    let before = crossing(&app);
    assert!((before - 28.0).abs() < 2.0, "the selection stops short of the edge: {before}");

    // The radius has to reach the edge to find it; three pixels out wants
    // more than a three-pixel window, as `cshop_core::refine` sets out.
    app.dispatch(Action::ApplyRefineEdge(Box::new(RefineEdge {
        radius: 16.0,
        ..Default::default()
    })));
    let after = crossing(&app);
    assert!(
        (after - 31.0).abs() < 1.5,
        "and after refining it sits on the picture's edge at 31: {after}"
    );
    assert!(coverage(&app, 40, 12) < 40, "without spilling past it");
}

#[test]
fn refining_says_so_when_there_is_nothing_to_refine() {
    let Some(mut app) = app_with(spotted()) else { return };
    app.dispatch(Action::ShowRefineEdge);
    let (msg, _) = app.toast.clone().expect("it should have said what to do");
    assert!(msg.contains("selection"), "{msg}");
}

#[test]
fn a_drawn_path_becomes_a_selection() {
    let Some(mut app) = app_with(spotted()) else { return };
    app.tool = cshop_ui::tools::Tool::Pen;
    app.pen = Some(cshop_ui::app::PenDraft {
        anchors: [(10.0, 4.0), (30.0, 4.0), (30.0, 16.0), (10.0, 16.0)]
            .into_iter()
            .map(|(x, y)| cshop_core::path::Anchor::corner(Vec2::new(x, y)))
            .collect(),
        dragging: None,
        cursor: None,
    });
    app.dispatch(Action::SelectionFromPath);

    assert_eq!(coverage(&app, 20, 10), 255, "inside the path");
    assert_eq!(coverage(&app, 40, 10), 0, "and outside it");
    assert!(app.pen.is_none(), "the draft became the selection");
}

#[test]
fn a_selection_becomes_a_path_layer() {
    let Some(mut app) = app_with(spotted()) else { return };
    {
        let view = app.doc_mut().unwrap();
        let mut m = cshop_core::mask::MaskBuffer::hide_all(60, 20);
        for y in 4..16 {
            for x in 4..16 {
                m.set(x, y, 255);
            }
        }
        view.doc.set_selection(Some(cshop_core::selection::Selection::from_mask(m)));
    }
    let before = app.doc().unwrap().doc.tree.len();
    app.dispatch(Action::PathFromSelection);

    let view = app.doc().unwrap();
    assert_eq!(view.doc.tree.len(), before + 1, "a path layer was added");
    let layer = view.doc.tree.get(view.doc.active.unwrap()).unwrap();
    let shape = layer.shape().expect("it should be a shape layer");
    match &shape.content().kind {
        cshop_core::shape::ShapeKind::Path(p) => {
            assert_eq!(p.parts[0].subpaths[0].anchors.len(), 4, "a square traces to four");
        }
        other => panic!("expected a path, got {other:?}"),
    }
}

#[test]
fn tracing_says_so_when_nothing_is_selected() {
    let Some(mut app) = app_with(spotted()) else { return };
    app.dispatch(Action::PathFromSelection);
    let (msg, _) = app.toast.clone().expect("it should have said something");
    assert!(msg.contains("selected"), "{msg}");
}
