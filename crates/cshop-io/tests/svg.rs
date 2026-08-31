//! Reading SVG.

use cshop_core::shape::ShapeKind;
use cshop_io::svg;

fn read(text: &str) -> svg::Drawing {
    svg::read(text.as_bytes()).expect("it should read")
}

/// Every point the shapes cover, in document coordinates.
fn points(d: &svg::Drawing) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    for shape in &d.shapes {
        if let ShapeKind::Path(p) = &shape.content.kind {
            for a in p.anchors() {
                out.push((a.at.x + shape.offset.0 as f32, a.at.y + shape.offset.1 as f32));
            }
        }
    }
    out
}

fn near(a: (f32, f32), b: (f32, f32), tol: f32) -> bool {
    (a.0 - b.0).abs() <= tol && (a.1 - b.1).abs() <= tol
}

fn has_point(d: &svg::Drawing, want: (f32, f32), tol: f32) -> bool {
    points(d).iter().any(|&p| near(p, want, tol))
}

#[test]
fn a_rectangle_reads_at_the_size_and_place_it_says() {
    let d = read(
        r##"<svg width="200" height="100"><rect x="20" y="10" width="60" height="30" fill="#ff0000"/></svg>"##,
    );
    assert_eq!((d.width, d.height), (200, 100));
    assert_eq!(d.shapes.len(), 1);
    let s = &d.shapes[0];
    assert_eq!(s.offset, (20, 10));
    assert_eq!(s.content.size, (60.0, 30.0));
    assert_eq!(s.content.style.fill, Some(cshop_core::color::Rgba8::opaque(255, 0, 0)));
    assert!(s.content.style.stroke.is_none(), "no stroke was asked for");
}

/// A viewBox is how an SVG says "these are my coordinates, draw them this
/// big". Ignoring it puts everything in the wrong place at the wrong scale.
#[test]
fn a_view_box_maps_the_files_coordinates_onto_the_size() {
    let d = read(
        r##"<svg width="400" height="400" viewBox="0 0 100 100"><rect x="10" y="10" width="20" height="20"/></svg>"##,
    );
    assert_eq!((d.width, d.height), (400, 400));
    // Four times the scale: the rectangle at (10,10) 20x20 lands at (40,40) 80x80.
    assert_eq!(d.shapes[0].offset, (40, 40));
    assert_eq!(d.shapes[0].content.size, (80.0, 80.0));
}

#[test]
fn transforms_compose_through_nesting() {
    let d = read(
        r##"<svg width="200" height="200">
             <g transform="translate(50,20)">
               <g transform="scale(2)">
                 <rect x="5" y="5" width="10" height="10"/>
               </g>
             </g>
           </svg>"##,
    );
    // Scaled first, then translated: (5,5) → (10,10) → (60,30).
    assert_eq!(d.shapes[0].offset, (60, 30));
    assert_eq!(d.shapes[0].content.size, (20.0, 20.0));
}

#[test]
fn a_path_reads_its_commands_absolute_and_relative() {
    let d = read(r##"<svg width="100" height="100"><path d="M10 10 L 30 10 l 0 20 Z"/></svg>"##);
    assert!(has_point(&d, (10.0, 10.0), 0.01));
    assert!(has_point(&d, (30.0, 10.0), 0.01));
    assert!(has_point(&d, (30.0, 30.0), 0.01), "the relative line goes down, not to (0,20)");
}

/// A repeated `moveto` argument list is a `lineto`, which is the one place in
/// the grammar where the command silently changes.
#[test]
fn a_repeated_moveto_becomes_a_lineto() {
    let d = read(r##"<svg width="100" height="100"><path d="M10 10 40 10 40 40 Z"/></svg>"##);
    assert!(has_point(&d, (40.0, 10.0), 0.01));
    assert!(has_point(&d, (40.0, 40.0), 0.01));
    // One subpath, not three separate moves.
    if let ShapeKind::Path(p) = &d.shapes[0].content.kind {
        assert_eq!(p.parts[0].subpaths.len(), 1);
    }
}

#[test]
fn a_circle_becomes_a_closed_path_of_the_right_size() {
    let d = read(r##"<svg width="100" height="100"><circle cx="50" cy="50" r="20"/></svg>"##);
    assert_eq!(d.shapes[0].offset, (30, 30));
    assert!((d.shapes[0].content.size.0 - 40.0).abs() < 0.5);
    if let ShapeKind::Path(p) = &d.shapes[0].content.kind {
        assert!(p.parts[0].subpaths[0].closed);
    }
}

/// Arcs are the one SVG primitive that is not a cubic, so they are converted —
/// and getting the conversion wrong is easy and silent.
#[test]
fn an_arc_ends_where_it_says_it_does() {
    let d = read(r##"<svg width="100" height="100"><path d="M10 50 A 40 40 0 0 1 90 50"/></svg>"##);
    assert!(has_point(&d, (10.0, 50.0), 0.5), "it starts where it was told");
    assert!(has_point(&d, (90.0, 50.0), 0.5), "and ends there");
    // A semicircle bulges: something should be well above the chord.
    let top = points(&d).iter().map(|p| p.1).fold(f32::MAX, f32::min);
    assert!(top < 40.0, "the arc should bow upward, not run straight: {top}");
}

/// The arc flags are single characters and may run into the number after
/// them: `a1 1 0 118 9` is five arguments, not two.
#[test]
fn the_arc_flags_can_run_into_the_number_after_them() {
    let d = read(r##"<svg width="100" height="100"><path d="M10 50a40 40 0 1180 0"/></svg>"##);
    assert!(!d.shapes.is_empty(), "it should have parsed at all");
    assert!(has_point(&d, (90.0, 50.0), 1.0), "and reached the far end");
}

#[test]
fn style_attributes_win_over_presentation_attributes() {
    let d = read(
        r##"<svg width="50" height="50"><rect width="10" height="10" fill="#00ff00" style="fill:#0000ff"/></svg>"##,
    );
    assert_eq!(d.shapes[0].content.style.fill, Some(cshop_core::color::Rgba8::opaque(0, 0, 255)));
}

#[test]
fn fill_none_means_no_fill_rather_than_black() {
    let d = read(
        r##"<svg width="50" height="50"><rect width="10" height="10" fill="none" stroke="red" stroke-width="3"/></svg>"##,
    );
    let style = d.shapes[0].content.style;
    assert!(style.fill.is_none());
    assert_eq!(style.stroke, Some(cshop_core::color::Rgba8::opaque(255, 0, 0)));
    assert_eq!(style.stroke_width, 3.0);
}

#[test]
fn opacity_reaches_the_colours() {
    let d = read(r##"<svg width="50" height="50"><rect width="10" height="10" fill="#000" fill-opacity="0.5"/></svg>"##);
    let a = d.shapes[0].content.style.fill.unwrap().a;
    assert!((a as i32 - 128).abs() <= 2, "half transparent, got {a}");
}

/// What is not supported has to be said out loud: a picture missing its text
/// looks like a bug in this program.
#[test]
fn what_cannot_be_drawn_is_reported_rather_than_dropped() {
    let d = read(
        r##"<svg width="100" height="100">
             <rect width="10" height="10"/>
             <text x="10" y="20">Hello</text>
             <image href="a.png" width="10" height="10"/>
           </svg>"##,
    );
    assert_eq!(d.shapes.len(), 1);
    assert!(d.skipped.contains(&"text".to_string()));
    assert!(d.skipped.contains(&"image".to_string()));
}

#[test]
fn something_that_is_not_an_svg_is_refused() {
    assert!(svg::read(b"<html><body>hello</body></html>").is_err());
    assert!(svg::read(b"").is_err());
}

#[test]
fn entities_and_comments_do_not_stop_it() {
    let d = read(
        r##"<svg width="60" height="60"><!-- a comment --><rect width="10" height="10" id="a &amp; b"/></svg>"##,
    );
    assert_eq!(d.shapes.len(), 1);
    assert_eq!(d.shapes[0].name.as_deref(), Some("a & b"));
}

#[test]
fn units_other_than_pixels_are_converted() {
    let d = read(r##"<svg width="1in" height="0.5in"><rect width="10" height="10"/></svg>"##);
    assert_eq!((d.width, d.height), (96, 48));
}

#[test]
fn a_matrix_transform_reads_in_the_order_svg_writes_it() {
    // matrix(a b c d e f) is [[a c e], [b d f]]: a scale of 2 and a shift.
    let d = read(
        r##"<svg width="100" height="100"><rect x="0" y="0" width="10" height="10" transform="matrix(2 0 0 2 5 7)"/></svg>"##,
    );
    assert_eq!(d.shapes[0].offset, (5, 7));
    assert_eq!(d.shapes[0].content.size, (20.0, 20.0));
}

// --- Writing ---------------------------------------------------------------

/// Vector in, vector out: what comes back from a round trip should be editable
/// geometry rather than a picture of it.
#[test]
fn a_drawing_round_trips_through_the_document_and_back() {
    let source = r##"<svg width="200" height="150">
        <path d="M20 20 L 120 20 L 120 90 Z" fill="#3050c0" stroke="#000000" stroke-width="4"/>
      </svg>"##;
    let doc = cshop_io::decode_document(source.as_bytes(), None).expect("it should open");
    assert_eq!((doc.width, doc.height), (200, 150));

    let shapes: Vec<_> = doc
        .tree
        .iter_all()
        .into_iter()
        .filter_map(|id| doc.tree.get(id))
        .filter(|l| l.shape().is_some())
        .collect();
    assert_eq!(shapes.len(), 1, "one path, one shape layer");

    let composite = cshop_core::pixels::PixelBuffer::new(200, 150);
    let bytes = cshop_io::svg::write(&doc, &composite).expect("it should write");
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains("<path"), "the geometry went out as geometry");
    assert!(text.contains("#3050c0"), "with its fill: {text}");
    assert!(!text.contains("<image"), "and no raster, since there was none");

    // And it reads back as the same triangle, in the same place.
    let again = svg::read(&bytes).expect("it should read back");
    assert_eq!(again.shapes.len(), 1);
    for want in [(20.0, 20.0), (120.0, 20.0), (120.0, 90.0)] {
        assert!(has_point(&again, want, 1.0), "{want:?} should have survived");
    }
}

/// A raster layer is not vector, and leaving it out would lose most of the
/// document. It goes out as a picture embedded in the file.
#[test]
fn a_raster_layer_is_embedded_rather_than_dropped() {
    let mut doc = cshop_core::document::Document::new(
        "t",
        32,
        24,
        cshop_core::document::Background::Transparent,
    );
    let id = doc.tree.iter_all()[0];
    doc.tree.get_mut(id).unwrap().kind = cshop_core::layer::LayerKind::raster(
        cshop_core::pixels::PixelBuffer::filled(32, 24, cshop_core::color::Rgba8::opaque(200, 40, 40)),
    );
    let composite =
        cshop_core::pixels::PixelBuffer::filled(32, 24, cshop_core::color::Rgba8::opaque(200, 40, 40));

    let text = String::from_utf8(cshop_io::svg::write(&doc, &composite).unwrap()).unwrap();
    assert!(text.contains("<image"), "the pixels went out as pixels");
    assert!(text.contains("data:image/png;base64,"), "embedded, not linked");
}

/// An SVG with nothing drawable in it still opens: the file may be all text,
/// and an empty document of the right size beats an error.
#[test]
fn a_drawing_of_nothing_but_text_still_opens() {
    let source = r##"<svg width="120" height="60"><text x="5" y="20">Only words</text></svg>"##;
    let doc = cshop_io::decode_document(source.as_bytes(), None).expect("it should open");
    assert_eq!((doc.width, doc.height), (120, 60));
    assert_eq!(doc.tree.len(), 1);
}
