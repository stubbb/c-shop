//! A path used as a mask, kept as a path.
//!
//! The reason to have both kinds is that a painted mask is a picture of an
//! edge and a vector one is a description of it, so the two behave differently
//! when the document is resized. That is the test that matters.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::layer::LayerKind;
use cshop_core::pixels::PixelBuffer;
use cshop_core::resample::Resampling;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

fn app() -> Option<CShopApp> {
    let gpu = GpuContext::headless().ok()?;
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("t", 200, 200, Background::Transparent));
    let view = app.doc_mut()?;
    let id = view.doc.active?;
    view.doc.tree.get_mut(id).unwrap().kind =
        LayerKind::raster(PixelBuffer::filled(200, 200, Rgba8::opaque(200, 60, 60)));
    view.invalidate();
    Some(app)
}

/// A diamond drawn with the pen, ready to become a mask. Built directly
/// rather than clicked out, since what is being tested is what happens to the
/// path afterwards.
fn draw_diamond(app: &mut CShopApp) {
    app.tool = cshop_ui::tools::Tool::Pen;
    app.pen = Some(cshop_ui::app::PenDraft {
        anchors: [(100.0, 40.0), (160.0, 100.0), (100.0, 160.0), (40.0, 100.0)]
            .into_iter()
            // A corner: the handles sit on the anchor, which is what the
            // Pen produces for a click without a drag.
            .map(|(x, y)| cshop_core::path::Anchor::corner(Vec2::new(x, y)))
            .collect(),
        dragging: None,
        cursor: None,
    });
}

fn mask_of(app: &CShopApp) -> &cshop_core::layer::LayerMask {
    let view = app.doc().unwrap();
    view.doc.tree.get(view.doc.active.unwrap()).unwrap().mask.as_ref().unwrap()
}

/// How crisp an edge is: how many pixels along a row are neither fully hidden
/// nor fully revealed. A drawn edge stays two or three wide; a resampled one
/// spreads.
fn edge_width(mask: &cshop_core::mask::MaskBuffer, y: i32) -> usize {
    (0..mask.width() as i32)
        .filter(|&x| {
            let v = mask.get(x, y);
            v > 8 && v < 247
        })
        .count()
}

#[test]
fn a_path_becomes_a_mask_that_keeps_its_path() {
    let Some(mut app) = app() else { return };
    draw_diamond(&mut app);
    app.dispatch(Action::AddVectorMask { invert: false });

    let mask = mask_of(&app);
    assert!(mask.is_vector(), "it should have kept the path it was drawn from");
    assert_eq!(mask.data.get(100, 100), 255, "the middle of the diamond is revealed");
    assert_eq!(mask.data.get(5, 5), 0, "and the corner is hidden");
}

#[test]
fn inverting_hides_the_inside_instead() {
    let Some(mut app) = app() else { return };
    draw_diamond(&mut app);
    app.dispatch(Action::AddVectorMask { invert: true });
    let mask = mask_of(&app);
    assert_eq!(mask.data.get(100, 100), 0);
    assert_eq!(mask.data.get(5, 5), 255);
}

/// The whole point. Resize the document and a vector mask is drawn again;
/// a painted one is resampled and its edge softens.
#[test]
fn resizing_redraws_a_vector_mask_rather_than_resampling_it() {
    let Some(mut app) = app() else { return };
    draw_diamond(&mut app);
    app.dispatch(Action::AddVectorMask { invert: false });
    let before = edge_width(&mask_of(&app).data, 100);

    // Down to a fifth and back up, which is where resampling shows.
    app.dispatch(Action::ResizeImage { width: 40, height: 40, filter: Resampling::Bilinear });
    app.dispatch(Action::ResizeImage { width: 200, height: 200, filter: Resampling::Bilinear });

    let mask = mask_of(&app);
    assert!(mask.is_vector(), "still a path");
    let after = edge_width(&mask.data, 100);
    assert!(
        after <= before + 1,
        "a drawn edge should stay as crisp as it was: {before} became {after}"
    );
    // And it is still the same shape, in the right place.
    assert_eq!(mask.data.get(100, 100), 255);
    assert_eq!(mask.data.get(5, 5), 0);
}

/// The comparison that gives the number above its meaning.
#[test]
fn resizing_a_painted_mask_softens_it() {
    let Some(mut app) = app() else { return };
    draw_diamond(&mut app);
    app.dispatch(Action::AddVectorMask { invert: false });
    // Forget the path, keeping the pixels: now it is an ordinary mask.
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().mask.as_mut().unwrap().path = None;
    }
    let before = edge_width(&mask_of(&app).data, 100);

    app.dispatch(Action::ResizeImage { width: 40, height: 40, filter: Resampling::Bilinear });
    app.dispatch(Action::ResizeImage { width: 200, height: 200, filter: Resampling::Bilinear });

    let after = edge_width(&mask_of(&app).data, 100);
    assert!(
        after > before + 2,
        "a picture of an edge should have spread: {before} became {after}"
    );
}

#[test]
fn it_says_so_when_there_is_no_path_to_use() {
    let Some(mut app) = app() else { return };
    app.dispatch(Action::AddVectorMask { invert: false });
    let (msg, _) = app.toast.clone().expect("it should have said what to do");
    assert!(msg.contains("Pen"), "{msg}");
    assert!(mask_of_opt(&app).is_none(), "and made no mask");
}

fn mask_of_opt(app: &CShopApp) -> Option<&cshop_core::layer::LayerMask> {
    let view = app.doc()?;
    view.doc.tree.get(view.doc.active?)?.mask.as_ref()
}
