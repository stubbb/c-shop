//! Linked smart objects: one picture, several layers.
//!
//! What makes two layers linked is only that they name the same picture in the
//! document's store — there is no link to switch on, and nothing to keep in
//! step. So the tests here are about the consequences of that: replacing the
//! picture moves every layer using it, breaking one out leaves the rest alone,
//! and a file written with four placements of one photograph holds the
//! photograph once.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::layer::{LayerId, LayerKind};
use cshop_core::pixels::PixelBuffer;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

fn flat(w: u32, h: u32, c: Rgba8) -> PixelBuffer {
    PixelBuffer::filled(w, h, c)
}

/// Noise, because the project format deflates its pixels: a flat colour
/// written four times is barely larger than one, so a flat picture cannot tell
/// a shared source from four copies of it.
fn noise(w: u32, h: u32) -> PixelBuffer {
    let mut px = PixelBuffer::new(w, h);
    let mut s: u32 = 0x1234_5678;
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = (s >> 16) as u8;
            px.set(x, y, Rgba8::opaque(n, n.rotate_left(3), n.wrapping_mul(7)));
        }
    }
    px
}

fn app_with(px: PixelBuffer) -> Option<CShopApp> {
    let gpu = GpuContext::headless().ok()?;
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("t", 200, 200, Background::Transparent));
    let view = app.doc_mut()?;
    let id = view.doc.active?;
    view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(px);
    view.invalidate();
    Some(app)
}

fn layer_ids(app: &CShopApp) -> Vec<LayerId> {
    app.doc().unwrap().doc.tree.iter_all()
}

fn colour_of(app: &CShopApp, id: LayerId) -> Rgba8 {
    app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().get(1, 1)
}

/// Convert, then duplicate: two layers, one picture.
fn two_linked(app: &mut CShopApp) -> (LayerId, LayerId) {
    app.dispatch(Action::ConvertToSmartObject);
    app.dispatch(Action::DuplicateLayer);
    let ids = layer_ids(app);
    assert_eq!(ids.len(), 2, "there should be two layers now");
    (ids[0], ids[1])
}

#[test]
fn duplicating_a_smart_object_shares_its_picture() {
    let Some(mut app) = app_with(flat(20, 20, Rgba8::opaque(200, 40, 40))) else { return };
    let (first, second) = two_linked(&mut app);

    let doc = &app.doc().unwrap().doc;
    let a = doc.tree.get(first).unwrap().smart().unwrap().source();
    let b = doc.tree.get(second).unwrap().smart().unwrap().source();
    assert_eq!(a, b, "a duplicate should place the same picture, not a copy of it");
    assert_eq!(doc.sources.len(), 1, "and the document should hold one picture, not two");
    assert_eq!(app.smart_link(), Some((a, 2)));
}

/// The point of the whole exercise: one correction, every placement.
#[test]
fn replacing_the_contents_moves_every_layer_that_shares_it() {
    let Some(mut app) = app_with(flat(20, 20, Rgba8::opaque(200, 40, 40))) else { return };
    let (first, second) = two_linked(&mut app);
    assert_eq!(colour_of(&app, first).r, 200);
    assert_eq!(colour_of(&app, second).r, 200);

    // Write a different picture out and put it behind them.
    let dir = std::env::temp_dir().join(format!("cshop-linked-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("blue.png");
    cshop_io::save(&path, &flat(20, 20, Rgba8::opaque(30, 60, 220)), 100).unwrap();

    app.dispatch(Action::ReplaceSmartContents(path));
    assert_eq!(colour_of(&app, first).b, 220, "the layer it was done on");
    assert_eq!(colour_of(&app, second).b, 220, "and the one sharing its picture");

    // And it is one undo, not two.
    app.dispatch(Action::Undo);
    assert_eq!(colour_of(&app, first).r, 200);
    assert_eq!(colour_of(&app, second).r, 200, "undo puts both back");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn making_a_copy_unique_leaves_the_others_alone() {
    let Some(mut app) = app_with(flat(20, 20, Rgba8::opaque(200, 40, 40))) else { return };
    let (first, second) = two_linked(&mut app);
    // `second` is the duplicate and is active; break it out.
    app.dispatch(Action::MakeSmartUnique);

    let doc = &app.doc().unwrap().doc;
    let a = doc.tree.get(first).unwrap().smart().unwrap().source();
    let b = doc.tree.get(second).unwrap().smart().unwrap().source();
    assert_ne!(a, b, "it should have its own picture now");
    assert_eq!(doc.sources.len(), 2);

    // Nothing on screen changed, which is worth checking: an operation that
    // silently altered the picture while claiming only to unshare it would be
    // much worse than one that does nothing.
    assert_eq!(colour_of(&app, first), colour_of(&app, second));

    let dir = std::env::temp_dir().join(format!("cshop-unique-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("green.png");
    cshop_io::save(&path, &flat(20, 20, Rgba8::opaque(20, 200, 20)), 100).unwrap();
    app.dispatch(Action::ReplaceSmartContents(path));

    assert_eq!(colour_of(&app, second).g, 200, "the one it was done on changed");
    assert_eq!(colour_of(&app, first).r, 200, "and the one it was unlinked from did not");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Undoing a break puts the layer back on the shared picture *and* takes the
/// copy back out of the store, or the document grows a picture per undone
/// gesture.
#[test]
fn undoing_a_break_takes_the_copy_away_again() {
    let Some(mut app) = app_with(flat(20, 20, Rgba8::opaque(200, 40, 40))) else { return };
    let (first, second) = two_linked(&mut app);
    app.dispatch(Action::MakeSmartUnique);
    assert_eq!(app.doc().unwrap().doc.sources.len(), 2);

    app.dispatch(Action::Undo);
    let doc = &app.doc().unwrap().doc;
    assert_eq!(doc.sources.len(), 1, "the copy should be gone, not merely unused");
    assert_eq!(
        doc.tree.get(first).unwrap().smart().unwrap().source(),
        doc.tree.get(second).unwrap().smart().unwrap().source(),
        "and they should be sharing again"
    );

    // Redo has to be able to put it back, which is why the command carries the
    // picture rather than expecting the store to still have it.
    app.dispatch(Action::Redo);
    let doc = &app.doc().unwrap().doc;
    assert_eq!(doc.sources.len(), 2);
    assert_ne!(
        doc.tree.get(first).unwrap().smart().unwrap().source(),
        doc.tree.get(second).unwrap().smart().unwrap().source(),
    );
}

/// The saved file has to hold the picture once. That is not a nicety: four
/// placements of a 24-megapixel photograph is 96 megabytes of the same thing.
#[test]
fn a_shared_picture_is_written_once() {
    let Some(mut app) = app_with(noise(200, 200)) else { return };
    app.dispatch(Action::ConvertToSmartObject);
    let one = cshop_io::project::write(&app.doc().unwrap().doc).len();
    // The picture has to be most of the file, or this measures nothing. Noise
    // still deflates somewhat — this one to about a third — so the bar is what
    // it actually comes to rather than the raw 160,000 bytes.
    assert!(one > 40_000, "the subject should be hard to compress, got {one} bytes");

    for _ in 0..3 {
        app.dispatch(Action::DuplicateLayer);
    }
    assert_eq!(layer_ids(&app).len(), 4);
    let four = cshop_io::project::write(&app.doc().unwrap().doc).len();

    // Three more copies of the picture would be three times the file. Three
    // more placements are a few dozen bytes each.
    assert!(
        four < one + one / 4,
        "four placements wrote {four} bytes against {one} for one — the picture is \
         being written per layer"
    );

    // And it comes back as four layers still sharing one picture.
    let back = cshop_io::project::read(&cshop_io::project::write(&app.doc().unwrap().doc)).unwrap();
    assert_eq!(back.sources.len(), 1);
    let sources: Vec<_> = back
        .tree
        .iter_all()
        .into_iter()
        .filter_map(|id| Some(back.tree.get(id)?.smart()?.source()))
        .collect();
    assert_eq!(sources.len(), 4);
    assert!(sources.windows(2).all(|w| w[0] == w[1]), "still one picture between them");
}

/// A picture nothing places any more stays in memory for undo's sake, and
/// stays out of the file.
#[test]
fn a_picture_no_layer_uses_is_kept_for_undo_and_not_written() {
    let Some(mut app) = app_with(flat(40, 40, Rgba8::opaque(200, 40, 40))) else { return };
    app.dispatch(Action::ConvertToSmartObject);
    // Something else to be left holding the document, since the last layer
    // cannot be deleted.
    app.dispatch(Action::NewLayer);
    let smart = layer_ids(&app)[0];
    app.dispatch(Action::SelectLayer(smart));
    app.dispatch(Action::DeleteLayer);
    assert!(
        app.doc().unwrap().doc.tree.get(smart).is_none(),
        "the smart layer should be gone"
    );

    let doc = &app.doc().unwrap().doc;
    assert_eq!(doc.sources.len(), 1, "still held, because undo will want it");
    assert!(doc.used_sources().is_empty(), "but nothing places it");
    let written = cshop_io::project::read(&cshop_io::project::write(doc)).unwrap();
    assert_eq!(written.sources.len(), 0, "so the file does not carry it");

    app.dispatch(Action::Undo);
    let doc = &app.doc().unwrap().doc;
    assert_eq!(doc.used_sources().len(), 1, "and undo finds it where it left it");
}

/// A replacement that is not the same size as what it replaces. Each layer
/// keeps its own placement and its own top-left, so two layers at different
/// scales both change and neither moves.
#[test]
fn a_differently_sized_replacement_keeps_each_placement() {
    let Some(mut app) = app_with(flat(20, 20, Rgba8::opaque(200, 40, 40))) else { return };
    let (first, second) = two_linked(&mut app);

    // Scale the duplicate to half, which is a placement and not an edit.
    {
        let view = app.doc_mut().unwrap();
        let dirty = view.history.apply(
            &mut view.doc,
            Box::new(cshop_core::history::PlaceSmart::new(
                second,
                cshop_core::transform::Transform::scale(0.5, 0.5),
                (0, 0),
                None,
                cshop_core::resample::Resampling::Bilinear,
                "Scale",
            )),
        );
        view.mark_dirty(dirty);
    }
    let size = |app: &CShopApp, id: LayerId| {
        let px = app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap();
        (px.width(), px.height())
    };
    let at = |app: &CShopApp, id: LayerId| app.doc().unwrap().doc.tree.get(id).unwrap().offset;
    assert_eq!(size(&app, first), (20, 20));
    assert_eq!(size(&app, second), (10, 10), "the duplicate is placed at half");
    let (was_first, was_second) = (at(&app, first), at(&app, second));

    let dir = std::env::temp_dir().join(format!("cshop-resize-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bigger.png");
    cshop_io::save(&path, &flat(40, 40, Rgba8::opaque(30, 60, 220)), 100).unwrap();
    app.dispatch(Action::ReplaceSmartContents(path));

    assert_eq!(size(&app, first), (40, 40), "full size for the one placed at full size");
    assert_eq!(size(&app, second), (20, 20), "and half of it for the one placed at half");
    assert_eq!(at(&app, first), was_first, "neither layer moved");
    assert_eq!(at(&app, second), was_second);
    let _ = std::fs::remove_dir_all(&dir);
}
