use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::paint::PaintMode;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

fn main() {
    let gpu = GpuContext::headless().unwrap();
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("t", 64, 64, Background::White));

    app.dispatch(Action::NewLayer);
    let view = app.doc().unwrap();
    let id = view.doc.active.unwrap();
    let layer = view.doc.tree.get(id).unwrap();
    println!("new layer: name={} kind={} offset={:?} visible={} locks={:?}",
        layer.name, layer.kind.type_name(), layer.offset, layer.visible, layer.locks);
    println!("edit target: {:?}", view.doc.effective_edit_target());
    println!("has mask: {}", layer.mask.is_some());

    app.foreground = Rgba8::opaque(255, 0, 0);
    app.brush.size = 20.0;
    app.brush.hardness = 1.0;
    app.begin_stroke(Vec2::new(32.0, 32.0), PaintMode::Paint);
    println!("is_painting after begin: {}", app.is_painting());
    app.end_stroke();

    let view = app.doc().unwrap();
    let layer = view.doc.tree.get(id).unwrap();
    println!("pixel at centre: {:?}", layer.pixels().unwrap().get(32, 32));
    println!("history: {:?}", view.history.labels());
}
