//! Turning a layer into a mask, a mask into a selection, and depth into either.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::layer::{Layer, LayerKind};
use cshop_core::pixels::PixelBuffer;
use cshop_ui::commands::Action;
use cshop_ui::input_harness::Harness;

/// Two layers: a picture, and a grey ramp above it to be read as a mask.
fn stacked(h: &mut Harness) -> (cshop_core::layer::LayerId, cshop_core::layer::LayerId) {
    let doc = Document::new("t", 64, 32, Background::Color(Rgba8::opaque(200, 80, 60)));
    h.app.open_document(doc);
    h.settle(2);
    let below = h.app.doc().unwrap().doc.active.unwrap();

    let mut ramp = PixelBuffer::new(64, 32);
    for y in 0..32i32 {
        for x in 0..64i32 {
            let v = (x * 255 / 63) as u8;
            ramp.set(x, y, Rgba8::opaque(v, v, v));
        }
    }
    let view = h.app.doc_mut().unwrap();
    let id = view.doc.tree.alloc_id();
    view.doc.tree.push(Layer::new(id, "Ramp", LayerKind::Raster(ramp)), None);
    view.doc.active = Some(id);
    view.doc.selected_layers = vec![id];
    h.settle(2);
    (below, id)
}

#[test]
fn a_layer_becomes_a_mask_on_the_one_below_and_is_consumed() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    let (below, ramp) = stacked(&mut h);
    assert_eq!(h.app.doc().unwrap().doc.tree.len(), 2);

    h.app.push(Action::LayerToMask);
    h.settle(2);

    let doc = &h.app.doc().unwrap().doc;
    assert_eq!(doc.tree.len(), 1, "the layer is consumed, not left showing twice");
    assert!(doc.tree.get(ramp).is_none(), "and it is the ramp that went");
    let mask = doc.tree.get(below).and_then(|l| l.mask.as_ref()).expect("a mask below");
    assert!(mask.data.get(1, 16) < 20, "the dark end hides");
    assert!(mask.data.get(62, 16) > 235, "the bright end reveals");
    assert_eq!(doc.active, Some(below), "and the layer it landed on is the one to work on");
}

/// One undo, because a document with the mask attached and the layer still
/// there would be showing it twice.
#[test]
fn making_a_mask_from_a_layer_undoes_in_one_step() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    let (below, _) = stacked(&mut h);
    h.app.push(Action::LayerToMask);
    h.settle(2);
    h.app.push(Action::Undo);
    h.settle(2);

    let doc = &h.app.doc().unwrap().doc;
    assert_eq!(doc.tree.len(), 2, "the layer comes back");
    assert!(doc.tree.get(below).unwrap().mask.is_none(), "and the mask goes");
}

#[test]
fn a_layer_with_nothing_underneath_is_refused() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.open_document(Document::new("t", 32, 32, Background::Color(Rgba8::WHITE)));
    h.settle(2);
    h.app.push(Action::LayerToMask);
    h.settle(2);
    assert_eq!(h.app.doc().unwrap().doc.tree.len(), 1, "nothing should have happened");
    assert!(h.app.doc().unwrap().doc.tree.iter_all().iter().all(|id| h
        .app
        .doc()
        .unwrap()
        .doc
        .tree
        .get(*id)
        .unwrap()
        .mask
        .is_none()));
}

/// A mask read back as a selection, with its coverage carried over rather than
/// thresholded — a soft mask makes a soft selection.
#[test]
fn a_mask_becomes_a_selection_with_its_softness() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    let (below, _) = stacked(&mut h);
    h.app.push(Action::LayerToMask);
    h.settle(2);
    h.app.push(Action::SelectionFromMask);
    h.settle(2);

    let doc = &h.app.doc().unwrap().doc;
    let selection = doc.selection.as_ref().expect("a selection");
    assert!(selection.coverage(1, 16) < 20, "the dark end is out of the selection");
    assert!(selection.coverage(62, 16) > 235, "the bright end is in it");
    let mid = selection.coverage(32, 16);
    assert!((60..200).contains(&mid), "and the middle is partly in: {mid}");
    let _ = below;
}

#[test]
fn a_layer_with_no_mask_has_no_selection_to_give() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.open_document(Document::new("t", 32, 32, Background::Color(Rgba8::WHITE)));
    h.settle(2);
    h.app.push(Action::SelectionFromMask);
    h.settle(2);
    assert!(h.app.doc().unwrap().doc.selection.is_none(), "nothing should have been selected");
}

/// Depth straight onto the layer as a mask, without going via a layer.
#[test]
fn depth_can_become_a_mask_directly() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let path = std::path::PathBuf::from(std::env::var("HOME").unwrap())
        .join("assets/samples/dog.jpg");
    let Ok(doc) = cshop_io::load_document(&path) else { return };
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.open_document(doc);
    h.settle(2);
    h.app.push(Action::ResizeImage {
        width: 200,
        height: 300,
        filter: cshop_core::resample::Resampling::Bilinear,
    });
    h.settle(2);
    let id = h.app.doc().unwrap().doc.active.unwrap();

    h.app.push(Action::AddLayerMaskFromDepth { invert: false });
    for _ in 0..900 {
        if h.app.doc().unwrap().doc.tree.get(id).unwrap().mask.is_some() {
            break;
        }
        h.settle(1);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let doc = &h.app.doc().unwrap().doc;
    let mask = doc.tree.get(id).and_then(|l| l.mask.as_ref()).expect("a mask from the depth");

    // The dog and the bench are near the camera and the trees are not, so the
    // bottom of the frame should survive and the top should not.
    let mean = |y0: i32, y1: i32| {
        let mut total = 0u64;
        let mut n = 0u64;
        for y in y0..y1 {
            for x in 0..200i32 {
                total += mask.data.get(x, y) as u64;
                n += 1;
            }
        }
        total as f32 / n as f32
    };
    assert!(
        mean(200, 300) > mean(0, 100) + 40.0,
        "near should reveal more than far: {} against {}",
        mean(200, 300),
        mean(0, 100)
    );
}
