//! PDFs through File ▸ Open and File ▸ Open and Place: asked how to read
//! them, then brought in as the layers they are made of.

use cshop_core::document::{Background, Document};
use cshop_core::layer::LayerKind;
use cshop_gpu::context::GpuContext;
use cshop_io::pdf::{ReadOptions, TextMode};
use cshop_ui::commands::Action;
use cshop_ui::dialogs::Dialog;
use cshop_ui::CShopApp;

/// A PDF on disk: `pages` letter-size pages, each with a heading and a box.
fn pdf_file(name: &str, pages: usize) -> std::path::PathBuf {
    let font = format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 126 /Widths [{}] >>",
        " 556".repeat(95)
    );
    let mut objects: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
            (0..pages).map(|i| format!("{} 0 R", 4 + 2 * i)).collect::<Vec<_>>().join(" ")
        ),
        font,
    ];
    for i in 0..pages {
        let content = format!("0 0 1 rg 72 600 200 100 re f BT /F1 24 Tf 72 700 Td (Page {} heading) Tj ET", i + 1);
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + 2 * i
        ));
        objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for at in offsets {
        out.extend_from_slice(format!("{at:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    let dir = std::env::temp_dir().join(format!("cshop-ui-pdf-{name}"));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.pdf"));
    std::fs::write(&path, out).expect("writing the fixture");
    path
}

fn app() -> Option<CShopApp> {
    let gpu = GpuContext::headless().inspect_err(|e| eprintln!("skipping: {e}")).ok()?;
    if cshop_core::font::FontDb::global().families().is_empty() {
        eprintln!("skipping: no fonts installed to set type in");
        return None;
    }
    Some(CShopApp::new(gpu))
}

fn at(dpi: f32, text: TextMode, paper: bool) -> ReadOptions {
    ReadOptions { dpi, text, paper }
}

#[test]
fn opening_a_pdf_asks_how_before_reading_it() {
    let Some(mut app) = app() else { return };
    let path = pdf_file("asks", 3);
    app.dispatch(Action::OpenPath(path.clone()));
    assert!(app.docs.is_empty(), "nothing is read until the question is answered");
    let Dialog::PdfImport(d) = &app.dialog else { panic!("the import dialog is up") };
    assert!(!d.place);
    assert_eq!(d.pages, vec![(612.0, 792.0); 3], "it knows the pages before reading them");
    assert_eq!(d.options, ReadOptions::default(), "300 dpi, a layer per line, on paper");
}

#[test]
fn answering_opens_each_page_as_a_group() {
    let Some(mut app) = app() else { return };
    let path = pdf_file("opens", 2);
    app.dispatch(Action::ImportPdf(path, false, at(72.0, TextMode::Lines, true)));
    let view = app.doc().expect("a document opened");
    let doc = &view.doc;
    assert_eq!((doc.width, doc.height, doc.dpi), (612, 792, 72.0));
    assert_eq!(doc.name, "opens.pdf");
    let groups: Vec<_> = doc.tree.root().iter().map(|id| doc.tree.get(*id).unwrap()).collect();
    assert_eq!(groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Page 2", "Page 1"]);
    let page1: Vec<_> = doc.tree.children(Some(groups[1].id)).iter().map(|id| doc.tree.get(*id).unwrap()).collect();
    assert_eq!(page1.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Paper", "Rectangle", "Page 1 heading"]);
    assert!(matches!(page1[2].kind, LayerKind::Text(_)));
    assert!(app.toast.as_ref().is_some_and(|(m, err)| !err && m.starts_with("Opened opens.pdf")), "{:?}", app.toast);
}

#[test]
fn placing_a_pdf_puts_its_layers_in_as_a_group_and_one_undo_takes_them_out() {
    let Some(mut app) = app() else { return };
    app.open_document(Document::new("t", 1000, 1000, Background::White));
    let before = app.doc().unwrap().doc.tree.len();
    let path = pdf_file("placed", 1);

    app.dispatch(Action::PlacePath(path.clone()));
    let Dialog::PdfImport(d) = &app.dialog else { panic!("placing a PDF asks too") };
    assert!(d.place && !d.options.paper, "placed onto what is there, not onto paper");
    // The document is 72 dpi by default; the page at 72 dpi is 612 x 792,
    // which fits a 1000 pixel canvas, so it comes in at the document's own.
    assert_eq!(d.options.dpi, app.doc().unwrap().doc.dpi.min(1000.0 / 792.0 * 72.0));
    let options = d.options;
    // What the dialog's Place button does: close, and send its answer.
    app.dialog = Dialog::None;

    app.dispatch(Action::ImportPdf(path, true, options));
    let view = app.doc().unwrap();
    let doc = &view.doc;
    assert_eq!(doc.tree.len(), before + 3, "a group, the box and the heading");
    let group = doc.tree.get(doc.active.expect("the group is selected")).unwrap();
    assert_eq!(group.name, "placed");
    let kids = doc.tree.children(Some(group.id));
    let heading = doc.tree.get(kids[1]).unwrap();
    let LayerKind::Text(t) = &heading.kind else { panic!("the heading is type") };
    let scale = options.dpi / 72.0;
    // Centred: the page is moved by half the room left over on the canvas.
    let (pw, ph) = ((612.0 * scale).round() as i32, (792.0 * scale).round() as i32);
    let shift = ((1000 - pw) / 2, (1000 - ph) / 2);
    let baseline = (heading.offset.0 + t.anchor().0, heading.offset.1 + t.anchor().1);
    let expected = ((72.0 * scale).round() as i32 + shift.0, ((792.0 - 700.0) * scale).round() as i32 + shift.1);
    assert!((baseline.0 - expected.0).abs() <= 1 && (baseline.1 - expected.1).abs() <= 1, "{baseline:?} vs {expected:?}");
    assert_eq!(view.history.labels().last().map(String::as_str), Some("Place PDF"));

    app.dispatch(Action::Undo);
    assert_eq!(app.doc().unwrap().doc.tree.len(), before, "one undo takes the whole page out");
    app.dispatch(Action::Redo);
    assert_eq!(app.doc().unwrap().doc.tree.len(), before + 3, "and redo puts it back");
}

#[test]
fn a_page_larger_than_the_canvas_is_placed_at_a_resolution_that_fits() {
    let Some(mut app) = app() else { return };
    app.open_document(Document::new("small", 400, 300, Background::White));
    app.dispatch(Action::PlacePath(pdf_file("fits", 3)));
    let Dialog::PdfImport(d) = &app.dialog else { panic!() };
    let options = d.options;
    // Height binds: 300 pixels for 792 points.
    assert!((options.dpi - 300.0 / 792.0 * 72.0).abs() < 0.01, "{}", options.dpi);
    app.dispatch(Action::ImportPdf(d.path.clone(), true, options));
    let doc = &app.doc().unwrap().doc;
    let groups: Vec<_> = doc.tree.root().iter().map(|id| doc.tree.get(*id).unwrap()).filter(|l| l.kind.is_group()).collect();
    assert_eq!(
        groups.iter().map(|g| (g.name.as_str(), g.visible)).collect::<Vec<_>>(),
        [("fits — Page 3", false), ("fits — Page 2", false), ("fits — Page 1", true)],
        "page 1 on top and the only one showing"
    );
}
