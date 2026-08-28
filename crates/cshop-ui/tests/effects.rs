//! Layer effects through the document and the undo stack.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::effects::*;
use cshop_core::geom::IRect;
use cshop_ui::commands::Action;
use cshop_ui::input_harness::Harness;

fn ready() -> Option<(Harness, cshop_core::layer::LayerId)> {
    let mut h = Harness::new((1400, 820))?;
    h.app.open_document(Document::new("t", 200, 200, Background::White));
    h.settle(2);
    h.app.dispatch(Action::NewLayer);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    // A square in the middle of the layer.
    if let Some(view) = h.app.doc_mut() {
        if let Some(px) = view.doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
            px.fill_rect(IRect::at(80, 80, 40, 40), Rgba8::opaque(200, 200, 200));
        }
        view.invalidate();
    }
    h.settle(2);
    Some((h, id))
}

fn shadow_style() -> LayerEffects {
    let mut fx = LayerEffects::new();
    fx.drop_shadow = Some(Shadow { distance: 10.0, size: 8.0, ..Default::default() });
    fx
}

#[test]
fn effects_enlarge_where_the_layer_draws() {
    let Some((mut h, id)) = ready() else { return };
    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().render_bounds();
    h.app.dispatch(Action::SetLayerEffects(id, Box::new(shadow_style())));

    let view = h.app.doc().unwrap();
    let layer = view.doc.tree.get(id).unwrap();
    assert_eq!(layer.bounds(), before, "the layer's own pixels have not moved");
    assert!(
        layer.render_bounds().width() > before.width(),
        "but it now draws further than it did"
    );
    assert!(layer.has_effects());
}

#[test]
fn the_composed_raster_holds_the_effect() {
    let Some((mut h, id)) = ready() else { return };
    h.app.dispatch(Action::SetLayerEffects(id, Box::new(shadow_style())));

    let view = h.app.doc().unwrap();
    let layer = view.doc.tree.get(id).unwrap();
    let (px, rect) = layer.render_with_effects().expect("a layer with effects composes");
    assert_eq!(
        rect,
        layer.render_bounds().intersect(&rect),
        "the composed rect should sit inside what the compositor was told"
    );
    assert!(px.width() > 40, "the raster grew to hold the shadow");
    // Something was drawn outside the original square.
    let outside = px.pixels().iter().filter(|p| p.a > 0).count();
    assert!(outside > 40 * 40, "the shadow should add coverage beyond the square");
}

#[test]
fn a_layer_without_effects_composes_nothing_extra() {
    let Some((h, id)) = ready() else { return };
    let view = h.app.doc().unwrap();
    let layer = view.doc.tree.get(id).unwrap();
    assert!(!layer.has_effects());
    assert!(layer.render_with_effects().is_none());
    assert_eq!(layer.render_bounds(), layer.bounds());
}

#[test]
fn a_layer_style_is_one_undo_step_and_can_be_cleared() {
    let Some((mut h, id)) = ready() else { return };
    h.app.dispatch(Action::SetLayerEffects(id, Box::new(shadow_style())));
    assert_eq!(
        h.app.doc().unwrap().history.labels().last().map(String::as_str),
        Some("Layer Style")
    );

    h.app.dispatch(Action::Undo);
    assert!(
        !h.app.doc().unwrap().doc.tree.get(id).unwrap().effects.any(),
        "undo should take the style off"
    );
    h.app.dispatch(Action::Redo);
    assert!(h.app.doc().unwrap().doc.tree.get(id).unwrap().effects.any());

    h.app.dispatch(Action::ClearLayerEffects(id));
    assert!(!h.app.doc().unwrap().doc.tree.get(id).unwrap().effects.any());
}

/// Dragging a slider in the dialog must not fill the history with one entry
/// per frame.
#[test]
fn successive_style_changes_merge_into_one_entry() {
    let Some((mut h, id)) = ready() else { return };
    for size in [4.0, 6.0, 8.0, 10.0, 12.0] {
        let mut fx = LayerEffects::new();
        fx.drop_shadow = Some(Shadow { size, ..Default::default() });
        h.app.dispatch(Action::SetLayerEffects(id, Box::new(fx)));
    }
    let labels = h.app.doc().unwrap().history.labels();
    assert_eq!(
        labels.iter().filter(|l| *l == "Layer Style").count(),
        1,
        "five changes should collapse into one entry, got {labels:?}"
    );
    // And undoing that one restores the layer to having no style at all.
    h.app.dispatch(Action::Undo);
    assert!(!h.app.doc().unwrap().doc.tree.get(id).unwrap().effects.any());
}

/// With effects, fill opacity has already been applied while composing, so the
/// compositor must not apply it a second time.
#[test]
fn fill_opacity_is_not_applied_twice() {
    let Some((mut h, id)) = ready() else { return };
    h.app.dispatch(Action::SetLayerProperty(
        id,
        cshop_core::history::LayerProperty::FillOpacity(0.5),
    ));
    let plain = h.app.doc().unwrap().doc.tree.get(id).unwrap().effective_alpha();
    assert!((plain - 0.5).abs() < 1e-6, "without effects, fill opacity scales the layer");

    h.app.dispatch(Action::SetLayerEffects(id, Box::new(shadow_style())));
    let styled = h.app.doc().unwrap().doc.tree.get(id).unwrap().effective_alpha();
    assert!(
        (styled - 1.0).abs() < 1e-6,
        "with effects it is applied while composing instead, got {styled}"
    );
}

#[test]
fn the_panel_lists_the_effects_that_are_on() {
    let mut fx = LayerEffects::new();
    fx.drop_shadow = Some(Shadow::default());
    fx.stroke = Some(Stroke::default());
    fx.bevel = Some(Bevel::default());
    // Listed top-first, the order they stack in.
    assert_eq!(fx.active_names(), vec!["Stroke", "Bevel & Emboss", "Drop Shadow"]);

    fx.enabled = false;
    assert!(!fx.any(), "the whole set can be switched off without losing it");
    assert_eq!(fx.active_names().len(), 3, "and the settings are still there");
}

/// Effects need pixels, so the kinds that have none should say so rather than
/// silently doing nothing.
#[test]
fn a_group_cannot_take_effects() {
    let Some((mut h, _)) = ready() else { return };
    h.app.dispatch(Action::NewGroup);
    h.settle(2);
    h.app.dispatch(Action::ShowLayerStyle);
    assert!(!h.app.dialog.is_open(), "the dialog should not open on a group");
    assert!(
        h.app.toast.as_ref().is_some_and(|(m, _)| m.contains("pixels")),
        "and should say why, got {:?}",
        h.app.toast
    );
}

#[test]
fn effects_survive_a_document_round_trip_through_the_compositor() {
    let Some((mut h, id)) = ready() else { return };
    h.app.dispatch(Action::SetLayerEffects(id, Box::new(shadow_style())));
    h.settle(3);
    // Rendering a frame with an effect layer must not panic or lose the layer.
    let view = h.app.doc().unwrap();
    assert!(view.doc.tree.get(id).unwrap().has_effects());
    assert_eq!(view.doc.tree.len(), 2);
}

/// A blurred shadow spreads an edit well beyond the pixels that changed, so
/// the region to recomposite has to grow with it.
#[test]
fn editing_a_styled_layer_dirties_the_area_the_effect_reaches() {
    let Some((mut h, id)) = ready() else { return };
    let mut fx = LayerEffects::new();
    fx.drop_shadow = Some(Shadow { distance: 20.0, size: 20.0, ..Default::default() });
    h.app.dispatch(Action::SetLayerEffects(id, Box::new(fx)));
    h.settle(2);

    let reach = cshop_core::effects::padding(
        &h.app.doc().unwrap().doc.tree.get(id).unwrap().effects,
    );
    assert!(reach >= 40, "this style reaches {reach} pixels");

    let small = IRect::at(90, 90, 4, 4);
    if let Some(view) = h.app.doc_mut() {
        view.mark_dirty(cshop_core::document::Dirty::pixels(id, small));
        assert!(
            view.pending_rect().width() >= small.width() + 2 * reach as u32,
            "a small edit must still repaint everything the shadow covers"
        );
    }
}
