//! File ▸ Open and Place: a file brought into the open document as a smart
//! object, rather than opened beside it.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::IRect;
use cshop_core::layer::Layer;
use cshop_core::pixels::PixelBuffer;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

/// A flat picture on disk, in a directory of its own.
fn picture(name: &str, w: u32, h: u32, colour: Rgba8) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cshop-ui-place-{name}"));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.png"));
    let mut px = PixelBuffer::new(w, h);
    px.fill_rect(IRect::at(0, 0, w, h), colour);
    cshop_io::save(&path, &px, 90).expect("writing the fixture");
    path
}

fn app_with_canvas(w: u32, h: u32) -> Option<CShopApp> {
    let gpu = GpuContext::headless().ok()?;
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("t", w, h, Background::Transparent));
    Some(app)
}

fn active_layer(app: &CShopApp) -> &Layer {
    let view = app.doc().unwrap();
    view.doc.tree.get(view.doc.active.unwrap()).unwrap()
}

#[test]
fn a_placed_file_is_a_smart_object_centred_on_the_canvas() {
    let Some(mut app) = app_with_canvas(200, 100) else { return };
    let path = picture("small", 40, 20, Rgba8::opaque(10, 200, 30));
    app.dispatch(Action::PlacePath(path));

    let view = app.doc().unwrap();
    assert_eq!(view.doc.tree.len(), 2, "one layer more than the document had");
    let layer = active_layer(&app);
    assert_eq!(layer.name, "small", "named after the file, without its extension");
    let smart = layer.smart().expect("placed as a smart object");
    assert_eq!(layer.offset, (80, 40), "centred");
    assert_eq!((smart.raster().width(), smart.raster().height()), (40, 20), "at its own size");
    assert_eq!(smart.raster().get(0, 0), Rgba8::opaque(10, 200, 30));
    assert_eq!(
        view.doc.sources.get(smart.source()).map(|s| s.name.as_str()),
        Some("small.png"),
        "the picture behind it is named after the file"
    );
    assert_eq!(view.history.labels().last().map(String::as_str), Some("Place"));
    assert!(view.doc.modified, "placing changes the document");
}

#[test]
fn a_file_larger_than_the_canvas_is_fitted_and_keeps_its_whole_picture() {
    let Some(mut app) = app_with_canvas(100, 100) else { return };
    let path = picture("large", 400, 200, Rgba8::opaque(200, 10, 30));
    app.dispatch(Action::PlacePath(path));

    let view = app.doc().unwrap();
    let layer = active_layer(&app);
    let smart = layer.smart().unwrap();
    assert_eq!((smart.raster().width(), smart.raster().height()), (100, 50), "fitted to the width");
    assert_eq!(layer.offset, (0, 25), "and centred");
    assert_eq!(smart.raster().get(50, 25), Rgba8::opaque(200, 10, 30));
    // The point of placing it as a smart object: nothing was thrown away.
    let source = view.doc.sources.pixels(smart.source()).unwrap();
    assert_eq!((source.width(), source.height()), (400, 200));
    assert!(
        app.toast.as_ref().is_some_and(|(m, _)| m.contains("fitted") && m.contains("25%")),
        "got {:?}",
        app.toast
    );
}

#[test]
fn undoing_a_place_takes_the_layer_and_its_picture_away() {
    let Some(mut app) = app_with_canvas(100, 100) else { return };
    let path = picture("undo", 10, 10, Rgba8::WHITE);
    app.dispatch(Action::PlacePath(path));
    assert_eq!(app.doc().unwrap().doc.sources.len(), 1);

    app.dispatch(Action::Undo);
    let view = app.doc().unwrap();
    assert_eq!(view.doc.tree.len(), 1, "one undo takes the whole place back");
    assert_eq!(view.doc.sources.len(), 0, "the store keeps nothing that nothing places");

    app.dispatch(Action::Redo);
    let view = app.doc().unwrap();
    assert_eq!(view.doc.tree.len(), 2);
    assert_eq!(view.doc.sources.len(), 1);
    assert!(active_layer(&app).smart().is_some());
}

#[test]
fn with_nothing_open_the_file_is_opened_instead() {
    let Ok(gpu) = GpuContext::headless() else { return };
    let mut app = CShopApp::new(gpu);
    let path = picture("alone", 30, 20, Rgba8::BLACK);
    app.dispatch(Action::PlacePath(path));

    assert_eq!(app.docs.len(), 1, "opened as a document of its own");
    assert_eq!((app.docs[0].doc.width, app.docs[0].doc.height), (30, 20));
    assert!(
        app.toast.as_ref().is_some_and(|(m, _)| m.contains("opened instead")),
        "got {:?}",
        app.toast
    );
}

/// A project or a PSD arrives with its layers; what is placed is the picture
/// they make together.
#[test]
fn a_layered_file_places_as_the_picture_its_layers_make() {
    let dir = std::env::temp_dir().join("cshop-ui-place-layered");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("two.cshop");
    {
        let mut doc = Document::new("two", 20, 20, Background::White);
        let mut top = PixelBuffer::new(10, 10);
        top.fill_rect(IRect::at(0, 0, 10, 10), Rgba8::opaque(0, 0, 250));
        let id = doc.tree.alloc_id();
        doc.tree.push(Layer::raster(id, "top", top), None);
        let flat = PixelBuffer::new(20, 20);
        cshop_io::save_document(&path, &doc, &flat).expect("writing the project");
    }

    let Some(mut app) = app_with_canvas(100, 100) else { return };
    app.dispatch(Action::PlacePath(path));
    let layer = active_layer(&app);
    let smart = layer.smart().expect("a smart object of the whole picture");
    assert_eq!((smart.raster().width(), smart.raster().height()), (20, 20));
    assert_eq!(smart.raster().get(5, 5), Rgba8::opaque(0, 0, 250), "the top layer shows");
    assert_eq!(smart.raster().get(15, 15), Rgba8::WHITE, "over the white one");
    assert_eq!(app.doc().unwrap().doc.tree.len(), 2, "as one layer, not two");
}

#[test]
fn a_file_that_cannot_be_read_says_so_and_adds_nothing() {
    let Some(mut app) = app_with_canvas(100, 100) else { return };
    app.dispatch(Action::PlacePath(std::path::PathBuf::from("/nowhere/at/all.png")));
    let view = app.doc().unwrap();
    assert_eq!(view.doc.tree.len(), 1);
    assert_eq!(view.doc.sources.len(), 0);
    assert!(
        app.toast.as_ref().is_some_and(|(m, err)| *err && m.contains("Could not open")),
        "got {:?}",
        app.toast
    );
}
