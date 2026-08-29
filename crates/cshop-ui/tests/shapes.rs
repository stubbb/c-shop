//! Shape layers, driven through the real interface.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::shape::{ShapeKind, StrokeAlign};
use cshop_ui::app::CShopApp;
use cshop_ui::commands::Action;
use cshop_ui::input_harness::Harness;
use cshop_ui::tools::Tool;

fn ready() -> Option<Harness> {
    let mut h = Harness::new((1400, 820))?;
    h.app.open_document(Document::new("t", 300, 200, Background::White));
    h.settle(3);
    h.app.tool = Tool::Shape;
    Some(h)
}

fn active_shape(h: &Harness) -> Option<cshop_core::shape::ShapeContent> {
    let view = h.app.doc()?;
    let id = view.doc.active?;
    view.doc.tree.get(id)?.shape().map(|s| s.content().clone())
}

#[test]
fn dragging_creates_a_shape_layer() {
    let Some(mut h) = ready() else { return };
    let before = h.app.doc().unwrap().doc.tree.len();

    h.app.dispatch(Action::DrawShape {
        from: Vec2::new(20.0, 30.0),
        to: Vec2::new(120.0, 90.0),
        from_centre: false,
        constrain: false,
    });

    assert_eq!(h.app.doc().unwrap().doc.tree.len(), before + 1);
    let content = active_shape(&h).expect("a shape layer");
    assert_eq!(content.size, (100.0, 60.0));
    assert_eq!(h.app.doc().unwrap().history.labels(), vec!["Shape Layer"]);

    // It sits where it was drawn.
    let view = h.app.doc().unwrap();
    let layer = view.doc.tree.get(view.doc.active.unwrap()).unwrap();
    let anchor = layer.shape().unwrap().anchor();
    assert_eq!((layer.offset.0 + anchor.0, layer.offset.1 + anchor.1), (20, 30));
}

/// Shift squares the drag off; Alt grows it from where the drag started.
#[test]
fn shift_constrains_and_alt_draws_from_the_centre() {
    let (origin, size) = CShopApp::shape_rect(
        Vec2::new(10.0, 10.0),
        Vec2::new(90.0, 40.0),
        false,
        true,
    );
    assert_eq!(size, (80.0, 80.0), "Shift should square it off");
    assert_eq!(origin, Vec2::new(10.0, 10.0));

    // Dragging up-left with Shift keeps the direction.
    let (origin, size) =
        CShopApp::shape_rect(Vec2::new(90.0, 90.0), Vec2::new(40.0, 70.0), false, true);
    assert_eq!(size, (50.0, 50.0));
    assert_eq!(origin, Vec2::new(40.0, 40.0));

    // Alt centres the shape on the starting point.
    let (origin, size) =
        CShopApp::shape_rect(Vec2::new(50.0, 50.0), Vec2::new(80.0, 70.0), true, false);
    assert_eq!(size, (60.0, 40.0));
    assert_eq!(origin, Vec2::new(20.0, 30.0));
}

#[test]
fn a_shape_layer_actually_paints_pixels() {
    let Some(mut h) = ready() else { return };
    h.app.shape_kind = ShapeKind::Ellipse;
    h.app.shape_style.fill = Some(Rgba8::opaque(200, 40, 40));
    h.app.dispatch(Action::DrawShape {
        from: Vec2::new(20.0, 20.0),
        to: Vec2::new(120.0, 120.0),
        from_centre: false,
        constrain: false,
    });
    h.settle(2);

    let view = h.app.doc().unwrap();
    let layer = view.doc.tree.get(view.doc.active.unwrap()).unwrap();
    let px = layer.pixels().expect("a shape has a raster");
    let anchor = layer.shape().unwrap().anchor();
    let middle = px.get(anchor.0 + 50, anchor.1 + 50);
    assert_eq!(middle, Rgba8::opaque(200, 40, 40), "the ellipse should be filled");
}

/// The point of a vector layer: the shape can be changed after it is drawn.
#[test]
fn changing_the_options_re_renders_the_selected_shape() {
    let Some(mut h) = ready() else { return };
    h.app.shape_kind = ShapeKind::Rectangle { radius: 0.0 };
    h.app.dispatch(Action::DrawShape {
        from: Vec2::new(20.0, 20.0),
        to: Vec2::new(120.0, 80.0),
        from_centre: false,
        constrain: false,
    });
    let size = active_shape(&h).unwrap().size;

    // Switch it to an ellipse with a thick outside stroke.
    h.app.shape_kind = ShapeKind::Ellipse;
    h.app.shape_style.stroke = Some(Rgba8::BLACK);
    h.app.shape_style.stroke_width = 10.0;
    h.app.shape_style.stroke_align = StrokeAlign::Outside;
    h.app.refresh_shape_style();

    let content = active_shape(&h).expect("still a shape layer");
    assert!(matches!(content.kind, ShapeKind::Ellipse), "the kind should have changed");
    assert_eq!(content.size, size, "without resizing it");
    assert_eq!(h.app.doc().unwrap().history.labels(), vec!["Shape Layer", "Edit Shape"]);

    // And the change is undoable.
    h.app.dispatch(Action::Undo);
    assert!(matches!(active_shape(&h).unwrap().kind, ShapeKind::Rectangle { .. }));
}

/// Widening an outside stroke grows the raster; the shape itself must not move.
#[test]
fn growing_the_stroke_does_not_move_the_shape() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::DrawShape {
        from: Vec2::new(40.0, 40.0),
        to: Vec2::new(140.0, 100.0),
        from_centre: false,
        constrain: false,
    });
    let corner = |h: &Harness| {
        let view = h.app.doc().unwrap();
        let layer = view.doc.tree.get(view.doc.active.unwrap()).unwrap();
        let a = layer.shape().unwrap().anchor();
        (layer.offset.0 + a.0, layer.offset.1 + a.1)
    };
    let before = corner(&h);

    h.app.shape_style.stroke = Some(Rgba8::BLACK);
    h.app.shape_style.stroke_width = 24.0;
    h.app.shape_style.stroke_align = StrokeAlign::Outside;
    h.app.refresh_shape_style();

    assert_eq!(corner(&h), before, "the shape's corner should not have moved");
}

#[test]
fn a_line_keeps_the_direction_it_was_dragged() {
    let Some(mut h) = ready() else { return };
    h.app.shape_kind = ShapeKind::Line { thickness: 4.0, from: (0.0, 0.0), to: (1.0, 1.0) };
    h.app.shape_style.stroke = Some(Rgba8::BLACK);

    // Drag up and to the right: the line should rise, not fall.
    h.app.dispatch(Action::DrawShape {
        from: Vec2::new(20.0, 120.0),
        to: Vec2::new(120.0, 20.0),
        from_centre: false,
        constrain: false,
    });
    let ShapeKind::Line { from, to, .. } = active_shape(&h).unwrap().kind else {
        panic!("expected a line");
    };
    assert!(from.1 > to.1, "the drag rose, so the line should too: {from:?} -> {to:?}");
}

#[test]
fn shapes_rasterise_and_come_back_on_undo() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::DrawShape {
        from: Vec2::new(20.0, 20.0),
        to: Vec2::new(90.0, 70.0),
        from_centre: false,
        constrain: false,
    });
    let id = h.app.doc().unwrap().doc.active.unwrap();
    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();

    h.app.dispatch(Action::RasterizeLayer);
    let view = h.app.doc().unwrap();
    let layer = view.doc.tree.get(id).unwrap();
    assert!(layer.shape().is_none(), "it should no longer be a shape");
    assert!(!layer.is_vector());
    assert_eq!(
        layer.pixels().unwrap().pixels(),
        before.pixels(),
        "rasterising must not change a single pixel"
    );
    assert_eq!(view.history.labels().last().map(String::as_str), Some("Rasterize Shape"));

    h.app.dispatch(Action::Undo);
    assert!(h.app.doc().unwrap().doc.tree.get(id).unwrap().shape().is_some());
}

#[test]
fn a_shape_cannot_be_painted_on_until_it_is_rasterised() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::DrawShape {
        from: Vec2::new(20.0, 20.0),
        to: Vec2::new(120.0, 120.0),
        from_centre: false,
        constrain: false,
    });
    let id = h.app.doc().unwrap().doc.active.unwrap();

    h.app.tool = Tool::Brush;
    h.app.begin_stroke_with(Vec2::new(60.0, 60.0), cshop_core::paint::PaintMode::Paint, false);
    h.app.end_stroke();
    assert!(
        h.app.doc().unwrap().doc.tree.get(id).unwrap().shape().is_some(),
        "painting must not have altered the shape layer"
    );

    // After rasterising it takes paint like anything else.
    h.app.dispatch(Action::RasterizeLayer);
    h.app.foreground = Rgba8::opaque(0, 200, 0);
    h.app.brush.size = 12.0;
    h.app.brush.hardness = 1.0;
    h.app.begin_stroke_with(Vec2::new(60.0, 60.0), cshop_core::paint::PaintMode::Paint, false);
    h.app.end_stroke();
    let view = h.app.doc().unwrap();
    let layer = view.doc.tree.get(id).unwrap();
    let px = layer.pixels().unwrap();
    assert_eq!(
        px.get(60 - layer.offset.0, 60 - layer.offset.1),
        Rgba8::opaque(0, 200, 0),
        "the rasterised shape should take paint"
    );
}

/// Shapes carry their own raster, so every layer feature should work on them
/// with no special case.
#[test]
fn shapes_behave_like_any_other_layer() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::DrawShape {
        from: Vec2::new(20.0, 20.0),
        to: Vec2::new(120.0, 90.0),
        from_centre: false,
        constrain: false,
    });
    let id = h.app.doc().unwrap().doc.active.unwrap();

    h.app.dispatch(Action::SetLayerProperty(
        id,
        cshop_core::history::LayerProperty::Opacity(0.4),
    ));
    h.app.dispatch(Action::SetLayerProperty(
        id,
        cshop_core::history::LayerProperty::Blend(cshop_core::blend::BlendMode::Screen),
    ));
    h.app.dispatch(Action::AddLayerMask { hide_all: false });
    h.app.dispatch(Action::NudgeLayer(7, -3));

    let view = h.app.doc().unwrap();
    let layer = view.doc.tree.get(id).unwrap();
    assert_eq!(layer.opacity, 0.4);
    assert_eq!(layer.blend_mode, cshop_core::blend::BlendMode::Screen);
    assert!(layer.mask.is_some(), "a shape should take a layer mask");
    assert!(layer.shape().is_some(), "and still be a shape afterwards");
}

#[test]
fn dragging_on_the_canvas_draws_a_shape() {
    let Some(mut h) = ready() else { return };
    let before = h.app.doc().unwrap().doc.tree.len();
    let a = h.doc_to_screen(40.0, 40.0).expect("a visible canvas");
    let b = h.doc_to_screen(140.0, 110.0).unwrap();
    h.drag(a, b, 8);
    h.settle(2);

    assert_eq!(h.app.doc().unwrap().doc.tree.len(), before + 1);
    let content = active_shape(&h).expect("a shape layer from the drag");
    assert!(content.size.0 > 80.0 && content.size.1 > 50.0, "got {:?}", content.size);
}

#[test]
fn a_click_without_a_drag_makes_nothing() {
    let Some(mut h) = ready() else { return };
    let before = h.app.doc().unwrap().doc.tree.len();
    h.app.dispatch(Action::DrawShape {
        from: Vec2::new(50.0, 50.0),
        to: Vec2::new(50.0, 50.0),
        from_centre: false,
        constrain: false,
    });
    assert_eq!(h.app.doc().unwrap().doc.tree.len(), before, "a zero-size drag is not a shape");
}

/// Transforming resamples pixels, so a shape stops being a shape. The tool
/// says so when it starts; this pins the behaviour rather than leaving it to
/// be discovered.
#[test]
fn transforming_a_shape_turns_it_into_pixels() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::DrawShape {
        from: Vec2::new(20.0, 20.0),
        to: Vec2::new(120.0, 90.0),
        from_centre: false,
        constrain: false,
    });
    let id = h.app.doc().unwrap().doc.active.unwrap();

    h.app.dispatch(Action::BeginTransform);
    assert!(
        h.app.toast.as_ref().is_some_and(|(m, _)| m.contains("rasterises")),
        "the user should be told, got {:?}",
        h.app.toast
    );
    h.app.dispatch(Action::TransformPreset(cshop_ui::commands::TransformPreset::FlipHorizontal));
    h.app.dispatch(Action::CommitTransform);

    let view = h.app.doc().unwrap();
    assert!(
        view.doc.tree.get(id).unwrap().shape().is_none(),
        "the transformed layer should now be pixels"
    );
    assert!(view.doc.tree.get(id).unwrap().pixels().is_some());
}

/// Selecting a shape must load its settings into the tool, or the first
/// option touched would stamp the tool's fill and stroke over the layer's.
#[test]
fn selecting_a_shape_adopts_its_settings() {
    let Some(mut h) = ready() else { return };
    h.app.shape_kind = ShapeKind::Polygon { sides: 7, star: false, inner: 0.5 };
    h.app.shape_style.fill = Some(Rgba8::opaque(10, 20, 30));
    h.app.shape_style.stroke = Some(Rgba8::opaque(200, 100, 50));
    h.app.shape_style.stroke_width = 9.0;
    h.app.dispatch(Action::DrawShape {
        from: Vec2::new(20.0, 20.0),
        to: Vec2::new(120.0, 120.0),
        from_centre: false,
        constrain: false,
    });
    let drawn = active_shape(&h).unwrap();

    // Draw a second, quite different shape, then go back to the first.
    let first = h.app.doc().unwrap().doc.active.unwrap();
    h.app.shape_kind = ShapeKind::Ellipse;
    h.app.shape_style.fill = Some(Rgba8::WHITE);
    h.app.shape_style.stroke = None;
    h.app.dispatch(Action::DrawShape {
        from: Vec2::new(150.0, 20.0),
        to: Vec2::new(250.0, 120.0),
        from_centre: false,
        constrain: false,
    });

    h.app.dispatch(Action::SelectLayer(first));
    h.app.sync_shape_options();
    assert_eq!(h.app.shape_style.fill, drawn.style.fill, "the tool should adopt the fill");
    assert_eq!(h.app.shape_style.stroke, drawn.style.stroke);
    assert_eq!(h.app.shape_style.stroke_width, 9.0);
    assert!(matches!(h.app.shape_kind, ShapeKind::Polygon { sides: 7, .. }));

    // And nothing was written to the layer just by selecting it.
    assert_eq!(active_shape(&h).unwrap(), drawn);
    assert_eq!(h.app.doc().unwrap().history.labels(), vec!["Shape Layer", "Shape Layer"]);
}
