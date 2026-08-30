//! The Upscale window, driven through the real interface.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_ui::commands::Action;
use cshop_ui::dialogs::Dialog;
use cshop_ui::input_harness::Harness;

fn open(h: &mut Harness, w: u32, hgt: u32) {
    h.app.open_document(Document::new("t", w, hgt, Background::Color(Rgba8::opaque(120, 90, 70))));
    h.settle(2);
}

#[test]
fn the_window_works_out_the_new_size() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h, 200, 150);
    h.app.push(Action::ShowUpscale);
    h.settle(1);
    let Dialog::Upscale(d) = &mut h.app.dialog else { panic!("the window should be open") };
    assert_eq!(d.from, (200, 150));
    assert_eq!(d.to(), (400, 300), "it opens at two times");
    d.scale = 4.0;
    assert_eq!(d.to(), (800, 600));
    d.scale = 1.5;
    assert_eq!(d.to(), (300, 225));
}

/// The whole point: the document ends up bigger, and one undo puts it back.
#[test]
fn enlarging_grows_the_document_and_undoes() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h, 96, 64);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();

    h.app.push(Action::ShowUpscale);
    h.settle(1);
    if let Dialog::Upscale(d) = &mut h.app.dialog {
        d.scale = 2.0;
    }
    h.app.push(Action::RunUpscale);
    h.settle(1);
    for _ in 0..2000 {
        if !matches!(h.app.dialog, Dialog::Upscale(_)) {
            break;
        }
        h.settle(1);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!matches!(h.app.dialog, Dialog::Upscale(_)), "the window closes when it lands");

    let doc = &h.app.doc().unwrap().doc;
    assert_eq!((doc.width, doc.height), (192, 128));
    let after = doc.tree.get(id).unwrap().pixels().unwrap();
    assert_eq!((after.width(), after.height()), (192, 128), "and so is the layer");

    h.app.push(Action::Undo);
    h.settle(2);
    let doc = &h.app.doc().unwrap().doc;
    assert_eq!((doc.width, doc.height), (96, 64), "one undo should put it back");
    assert_eq!(doc.tree.get(id).unwrap().pixels().unwrap(), &before);
}

/// A document with nothing but a group in it has nothing to enlarge.
#[test]
fn a_document_with_no_pixels_is_refused() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.open_document(Document::new("t", 64, 64, Background::Transparent));
    h.settle(2);
    // Replace the only layer with a group, so nothing raster remains.
    let id = h.app.doc().unwrap().doc.active.unwrap();
    if let Some(view) = h.app.doc_mut() {
        if let Some(layer) = view.doc.tree.get_mut(id) {
            layer.kind = cshop_core::layer::LayerKind::Group { children: Vec::new() };
        }
    }
    h.settle(1);
    h.app.push(Action::ShowUpscale);
    h.settle(1);
    assert!(!matches!(h.app.dialog, Dialog::Upscale(_)), "there is nothing to enlarge");
}
