//! Reading PDF: pages into layers.
//!
//! Most of these build a small PDF byte by byte, so each structure the reader
//! claims to understand can be produced on demand. The last few read a real
//! four-page NDA printed by PDFium, which is a hard case for reading order;
//! that file is not in the repository (it names people), so they read
//! `~/Downloads/nda.pdf` or `$NDA_PDF` and say so when it is missing.

use cshop_core::color::Rgba8;
use cshop_core::layer::LayerKind;
use cshop_core::shape::ShapeKind;
use cshop_io::pdf::{self, NewLayer, ReadOptions, TextMode};

// ---------------------------------------------------------------------------
// Writing test files
// ---------------------------------------------------------------------------

fn stream(data: &[u8], entries: &str, compress: bool) -> Vec<u8> {
    let (data, filter) = if compress {
        (miniz_oxide::deflate::compress_to_vec_zlib(data, 6), " /Filter /FlateDecode")
    } else {
        (data.to_vec(), "")
    };
    let mut out = format!("<< /Length {} {entries}{filter} >>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(&data);
    out.extend_from_slice(b"\nendstream");
    out
}

/// A whole file with a classic cross-reference table.
fn build(objects: &[(u32, Vec<u8>)], trailer: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = std::collections::BTreeMap::new();
    let mut sorted: Vec<&(u32, Vec<u8>)> = objects.iter().collect();
    sorted.sort_by_key(|(n, _)| *n);
    for (num, body) in sorted {
        offsets.insert(*num, out.len());
        out.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let size = offsets.keys().max().unwrap() + 1;
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for n in 1..size {
        match offsets.get(&n) {
            Some(at) => out.extend_from_slice(format!("{at:010} 00000 n \n").as_bytes()),
            None => out.extend_from_slice(b"0000000000 65535 f \n"),
        }
    }
    out.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R{trailer} >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

/// Every width 600, as Courier: positions are easy to work out by hand.
fn mono() -> Vec<u8> {
    format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding /FirstChar 32 /LastChar 255 /Widths [{}] >>",
        " 600".repeat(224)
    )
    .into_bytes()
}

fn bold_serif() -> Vec<u8> {
    format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Bold /Encoding /WinAnsiEncoding /FirstChar 32 /LastChar 255 /Widths [{}] >>",
        " 500".repeat(224)
    )
    .into_bytes()
}

/// One letter-size page showing `content`, with Courier as /F1 and Times Bold
/// as /F2.
fn page(content: &[u8], resources: &str, extra: &[(u32, Vec<u8>)], page_extra: &str) -> Vec<u8> {
    let mut objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (
            3,
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources {resources} /Contents 4 0 R{page_extra} >>")
                .into_bytes(),
        ),
        (4, stream(content, "", true)),
        (5, mono()),
        (6, bold_serif()),
    ];
    for (n, body) in extra {
        objects.retain(|(m, _)| m != n);
        objects.push((*n, body.clone()));
    }
    build(&objects, "")
}

const FONTS: &str = "<< /Font << /F1 5 0 R /F2 6 0 R >> >>";

/// At 72 dpi a point is a pixel, which keeps the arithmetic readable.
fn at72(text: TextMode) -> ReadOptions {
    ReadOptions { dpi: 72.0, text, paper: false }
}

fn layers(data: &[u8], options: ReadOptions) -> Vec<NewLayer> {
    let mut imported = pdf::read(data, &options).expect("it should read");
    imported.pages.remove(0).layers
}

fn have_fonts() -> bool {
    let ok = !cshop_core::font::FontDb::global().families().is_empty();
    if !ok {
        eprintln!("(skipped: no fonts installed to set type in)");
    }
    ok
}

/// A type layer's text and where its anchor — the first baseline's start, for
/// point text — sits in the document.
fn type_at(l: &NewLayer) -> Option<(String, (i32, i32))> {
    match &l.kind {
        LayerKind::Text(t) => Some((t.content().text.clone(), (l.offset.0 + t.anchor().0, l.offset.1 + t.anchor().1))),
        _ => None,
    }
}

fn texts(ls: &[NewLayer]) -> Vec<String> {
    ls.iter().filter_map(type_at).map(|(t, _)| t).collect()
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

#[test]
fn a_line_of_text_becomes_a_type_layer_on_its_baseline() {
    if !have_fonts() {
        return;
    }
    let ls = layers(&page(b"BT /F1 20 Tf 72 700 Td (Hello there) Tj ET", FONTS, &[], ""), at72(TextMode::Lines));
    assert_eq!(ls.len(), 1);
    let (text, at) = type_at(&ls[0]).expect("a type layer");
    assert_eq!(text, "Hello there");
    // y flips: 700 up from the bottom of a 792 page is 92 down from the top.
    assert!((at.0 - 72).abs() <= 1 && (at.1 - 92).abs() <= 1, "anchored on the baseline start, got {at:?}");
}

#[test]
fn courier_is_set_in_a_monospaced_face_at_its_own_length() {
    if !have_fonts() {
        return;
    }
    let ls = layers(&page(b"BT /F1 10 Tf 72 700 Td (abcdefghij) Tj ET", FONTS, &[], ""), at72(TextMode::Lines));
    let LayerKind::Text(t) = &ls[0].kind else { panic!("type") };
    let db = cshop_core::font::FontDb::global();
    let style = &t.content().style;
    let font = db.load(&style.family, style.bold, style.italic).expect("the face it chose loads");
    let width = cshop_core::text::layout(t.content(), &font).width;
    // Ten glyphs 600/1000 of a 10 point em: 60 points, which is 60 pixels here.
    assert!((width - 60.0).abs() < 1.0, "{} sets the line {width} long, not 60", style.family);
}

#[test]
fn a_change_of_face_or_a_wide_gap_starts_a_new_layer() {
    if !have_fonts() {
        return;
    }
    let content = b"BT /F2 10 Tf 72 700 Td (Company:) Tj ET \
                    BT /F1 10 Tf 160 700 Td (ACME Corp) Tj ET \
                    BT /F1 10 Tf 72 680 Td (one) Tj 22 0 Td (run) Tj ET";
    let ls = layers(&page(content, FONTS, &[], ""), at72(TextMode::Lines));
    let mut t = texts(&ls);
    t.sort();
    assert_eq!(t, ["ACME Corp", "Company:", "one run"], "bold label, its value, and a line with a word gap");
    let bold = ls.iter().find(|l| l.name == "Company:").unwrap();
    let LayerKind::Text(b) = &bold.kind else { panic!() };
    assert!(b.content().style.bold, "Times-Bold is set bold");
}

#[test]
fn a_justified_line_keeps_every_word_within_a_quarter_em_of_its_place() {
    if !have_fonts() {
        return;
    }
    // Courier 10pt, words of 1 and 10 letters alternating, each gap stretched
    // to 0.35 em the way justification stretches spaces and not letters.
    let words = ["a", "bbbbbbbbbb", "c", "dddddddddd", "e", "ffffffffff", "g", "hhhhhhhhhh", "i", "jjjjjjjjjj"];
    let tj: Vec<String> = words.iter().map(|w| format!("({w})")).collect();
    let content = format!("BT /F1 10 Tf 40 700 Td [{}] TJ ET", tj.join(" -350 "));
    let scale = 4.0; // 288 dpi, so a pixel of rounding is a small part of an em
    let ls = layers(&page(content.as_bytes(), FONTS, &[], ""), ReadOptions { dpi: 72.0 * scale, text: TextMode::Lines, paper: false });
    // Where the page put each word, in pixels.
    let mut expected = Vec::new();
    let mut x = 40.0;
    for w in words {
        expected.push(x * scale);
        x += w.len() as f32 * 6.0 + 3.5;
    }
    // Where this face, with each layer's own tracking, sets each word.
    let db = cshop_core::font::FontDb::global();
    let mut placed = Vec::new();
    for l in &ls {
        let LayerKind::Text(t) = &l.kind else { continue };
        let c = t.content();
        let font = db.load(&c.style.family, c.style.bold, c.style.italic).unwrap();
        let laid = cshop_core::text::layout(c, &font);
        let mut glyphs = laid.lines[0].glyphs.iter();
        let mut at_word_start = true;
        for ch in c.text.chars() {
            if ch.is_whitespace() {
                at_word_start = true;
                continue;
            }
            let g = glyphs.next().expect("a glyph per letter");
            if at_word_start {
                placed.push(l.offset.0 as f32 + t.anchor().0 as f32 + g.x);
                at_word_start = false;
            }
        }
    }
    placed.sort_by(f32::total_cmp);
    assert_eq!(placed.len(), words.len());
    let em = 10.0 * scale;
    for (k, (got, want)) in placed.iter().zip(&expected).enumerate() {
        assert!((got - want).abs() <= 0.25 * em + 1.0, "word {k} is {:.1} px from its place", got - want);
    }
}

#[test]
fn paragraph_mode_makes_one_wrapping_box_per_paragraph() {
    if !have_fonts() {
        return;
    }
    let content = b"BT /F1 10 Tf 12 TL 72 700 Td (first line of a) Tj T* (paragraph that) Tj T* (goes on) Tj \
                    T* T* T* (A new one) Tj ET";
    let ls = layers(&page(content, FONTS, &[], ""), at72(TextMode::Paragraphs));
    let boxes: Vec<_> = ls
        .iter()
        .filter_map(|l| match &l.kind {
            LayerKind::Text(t) => Some(t.content().clone()),
            _ => None,
        })
        .collect();
    assert_eq!(boxes.len(), 2, "three lines at an even pitch, then a gap: {boxes:?}");
    let para = boxes.iter().find(|c| c.wrap_width.is_some()).expect("the paragraph wraps");
    assert_eq!(para.text, "first line of a paragraph that goes on");
    assert!((para.style.leading.unwrap() - 12.0).abs() < 0.1, "leading is the line pitch");
    let lines = cshop_core::text::layout(
        para,
        &cshop_core::font::FontDb::global().load(&para.style.family, false, false).unwrap(),
    );
    assert_eq!(lines.lines.len(), 3, "it breaks where the page broke it");
}

#[test]
fn ligatures_unmapped_codes_and_invisible_text() {
    if !have_fonts() {
        return;
    }
    let font = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding << /Differences [1 /fi /g37] >> >>".to_vec();
    let content = b"BT /F3 10 Tf 72 700 Td (\x01le \x02) Tj ET BT 3 Tr /F3 10 Tf 72 600 Td (hidden) Tj ET";
    let data = page(content, "<< /Font << /F3 7 0 R >> >>", &[(7, font)], "");
    let imported = pdf::read(&data, &at72(TextMode::Lines)).unwrap();
    let ls = &imported.pages[0].layers;
    assert_eq!(texts(ls), ["file \u{FFFD}", "hidden"], "fi reads as letters; a nameless glyph shows as missing");
    assert!(!ls[1].visible, "OCR-style invisible text comes in hidden, not over the picture");
    assert!(imported.notes.iter().any(|n| n.contains("no Unicode mapping")), "{:?}", imported.notes);
}

#[test]
fn turned_text_is_drawn_as_pixels_and_said_so() {
    if !have_fonts() {
        return;
    }
    let content = b"BT /F1 20 Tf 0 1 -1 0 300 100 Tm (Up the side) Tj ET";
    let imported = pdf::read(&page(content, FONTS, &[], ""), &at72(TextMode::Lines)).unwrap();
    let l = &imported.pages[0].layers[0];
    let LayerKind::Raster(s) = &l.kind else { panic!("turned text is pixels") };
    let px = s.eight().unwrap();
    assert!(px.height() > px.width(), "it runs up the page: {}x{}", px.width(), px.height());
    assert!(imported.notes.iter().any(|n| n.contains("turned text")), "{:?}", imported.notes);
}

// ---------------------------------------------------------------------------
// Shapes and images
// ---------------------------------------------------------------------------

#[test]
fn shapes_are_stacked_in_the_order_the_page_painted_them() {
    if !have_fonts() {
        return;
    }
    // A red box, text on it, then a white box over part of the text.
    let content = b"1 0 0 rg 50 600 200 100 re f \
                    BT /F1 10 Tf 60 650 Td (covered) Tj ET \
                    1 1 1 rg 55 640 30 30 re f \
                    0 0 1 RG 3 w 300 500 m 400 500 l S";
    let ls = layers(&page(content, FONTS, &[], ""), at72(TextMode::Lines));
    let names: Vec<&str> = ls.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Rectangle", "covered", "Rectangle", "Path"]);
    let LayerKind::Shape(red) = &ls[0].kind else { panic!() };
    assert!(matches!(red.content().kind, ShapeKind::Rectangle { .. }), "a rectangle stays one");
    assert_eq!(red.content().style.fill, Some(Rgba8::new(255, 0, 0, 255)));
    assert_eq!(red.content().size, (200.0, 100.0));
    let at = (ls[0].offset.0 + red.anchor().0, ls[0].offset.1 + red.anchor().1);
    assert_eq!(at, (50, 92), "the box's top-left: 792 - 700");
    let LayerKind::Shape(line) = &ls[3].kind else { panic!() };
    assert_eq!(line.content().style.stroke, Some(Rgba8::new(0, 0, 255, 255)));
    assert_eq!(line.content().style.stroke_width, 3.0);
    assert_eq!(line.content().style.fill, None, "a stroked line is not filled");
}

#[test]
fn even_odd_fill_cuts_the_hole() {
    let content = b"0 g 100 100 200 200 re 150 150 100 100 re f*";
    let ls = layers(&page(content, FONTS, &[], ""), at72(TextMode::Lines));
    let LayerKind::Shape(s) = &ls[0].kind else { panic!("one shape") };
    let ShapeKind::Path(p) = &s.content().kind else { panic!("two contours make a path") };
    assert_eq!(p.parts.len(), 2);
    assert_eq!(p.parts[1].op, cshop_core::path::BoolOp::Exclude);
    // The middle of the inner square is a hole in the raster.
    let r = s.raster();
    let (ax, ay) = s.anchor();
    assert_eq!(r.get(ax + 100, ay + 100).a, 0, "the hole");
    assert_eq!(r.get(ax + 20, ay + 20).a, 255, "the ring");
}

#[test]
fn an_image_lands_where_the_page_put_it() {
    // 2x2 RGB: red, green / blue, white, scaled up to 100x50 points.
    let image = stream(
        &[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255],
        "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8",
        true,
    );
    let content = b"q 100 0 0 50 10 20 cm /Im0 Do Q";
    let data = page(content, "<< /XObject << /Im0 7 0 R >> >>", &[(7, image)], "");
    let ls = layers(&data, at72(TextMode::Lines));
    let LayerKind::Raster(s) = &ls[0].kind else { panic!("a raster layer") };
    let px = s.eight().unwrap();
    assert_eq!((px.width(), px.height()), (100, 50));
    assert_eq!(ls[0].offset, (10, 792 - 20 - 50));
    let near = |c: Rgba8, r: u8, g: u8, b: u8| (c.r as i32 - r as i32).abs() < 40 && (c.g as i32 - g as i32).abs() < 40 && (c.b as i32 - b as i32).abs() < 40;
    assert!(near(px.get(10, 10), 255, 0, 0), "the image's first row is at the top: {:?}", px.get(10, 10));
    assert!(near(px.get(90, 40), 255, 255, 255), "{:?}", px.get(90, 40));
}

#[test]
fn a_stencil_mask_paints_the_fill_colour() {
    // 8x1, one bit each: 0 paints.
    let mask = stream(&[0b0000_1111], "/Type /XObject /Subtype /Image /Width 8 /Height 1 /ImageMask true", false);
    let content = b"0 0 1 rg q 8 0 0 1 0 0 cm /M Do Q";
    let ls = layers(&page(content, "<< /XObject << /M 7 0 R >> >>", &[(7, mask)], ""), at72(TextMode::Lines));
    let LayerKind::Raster(s) = &ls[0].kind else { panic!() };
    let px = s.eight().unwrap();
    assert_eq!(px.get(1, 0).b, 255);
    assert!(px.get(1, 0).a > 200 && px.get(6, 0).a < 50, "{:?} {:?}", px.get(1, 0), px.get(6, 0));
}

#[test]
fn the_editors_own_pdf_comes_back_as_its_picture() {
    let mut px = cshop_core::pixels::PixelBuffer::filled(40, 20, Rgba8::new(10, 120, 200, 255));
    px.set(0, 0, Rgba8::new(255, 255, 0, 255));
    let bytes = pdf::write(&px, 72.0).unwrap();
    let ls = layers(&bytes, at72(TextMode::Lines));
    let LayerKind::Raster(s) = &ls[0].kind else { panic!("the picture") };
    let back = s.eight().unwrap();
    assert_eq!((back.width(), back.height()), (40, 20));
    assert_eq!(ls[0].offset, (0, 0));
    let c = back.get(20, 10);
    assert!((c.r as i32 - 10).abs() < 3 && (c.g as i32 - 120).abs() < 3 && (c.b as i32 - 200).abs() < 3, "{c:?}");
}

#[test]
fn what_is_left_out_is_named() {
    let content = b"/Sh0 sh [3 2] 0 d 0 G 10 10 m 100 10 l S";
    let imported = pdf::read(&page(content, FONTS, &[], ""), &at72(TextMode::Lines)).unwrap();
    let notes = imported.notes.join("\n");
    assert!(notes.contains("smooth shadings"), "{notes}");
    assert!(notes.contains("dashed lines"), "{notes}");
}

// ---------------------------------------------------------------------------
// File structure
// ---------------------------------------------------------------------------

fn one_text(data: &[u8]) -> Vec<String> {
    texts(&layers(data, at72(TextMode::Lines)))
}

#[test]
fn incremental_updates_win() {
    if !have_fonts() {
        return;
    }
    let original = page(b"BT /F1 10 Tf 72 700 Td (first draft) Tj ET", FONTS, &[], "");
    let prev = String::from_utf8_lossy(&original).rsplit("startxref").next().unwrap().split_whitespace().next().unwrap().to_string();
    let mut updated = original.clone();
    let at = updated.len();
    updated.extend_from_slice(b"4 0 obj\n");
    updated.extend_from_slice(&stream(b"BT /F1 10 Tf 72 700 Td (signed copy) Tj ET", "", false));
    updated.extend_from_slice(b"\nendobj\n");
    let xref = updated.len();
    updated.extend_from_slice(format!("xref\n4 1\n{at:010} 00000 n \ntrailer\n<< /Size 7 /Root 1 0 R /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    assert_eq!(one_text(&original), ["first draft"]);
    assert_eq!(one_text(&updated), ["signed copy"]);
}

#[test]
fn damaged_and_missing_cross_references_are_repaired() {
    if !have_fonts() {
        return;
    }
    let good = page(b"BT /F1 10 Tf 72 700 Td (still here) Tj ET", FONTS, &[], "");
    // Bytes after the header put every offset in the table out.
    let mut shifted = b"%PDF-1.7\n% padding\n".to_vec();
    shifted.extend_from_slice(&good[9..]);
    assert_eq!(one_text(&shifted), ["still here"]);
    let imported = pdf::read(&shifted, &at72(TextMode::Lines)).unwrap();
    assert!(imported.notes[0].contains("damaged"), "{:?}", imported.notes);

    let cut = good.windows(6).position(|w| w == b"\nxref\n").unwrap() + 1;
    assert_eq!(one_text(&good[..cut]), ["still here"], "no table and no trailer at all");

    let mut junk = b"From: a mail gateway\r\n\r\n".to_vec();
    junk.extend_from_slice(&good);
    assert_eq!(one_text(&junk), ["still here"], "offsets count from the header");
}

#[test]
fn cross_reference_streams_and_object_streams() {
    if !have_fonts() {
        return;
    }
    // Objects 1-3 and 5 packed in object stream 7; the xref is stream 8,
    // its rows PNG-predicted the way Acrobat writes them.
    let packed: Vec<(u32, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec()),
        (5, mono()),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let content_at = out.len();
    out.extend_from_slice(b"4 0 obj\n");
    out.extend_from_slice(&stream(b"BT /F1 10 Tf 72 700 Td (packed away) Tj ET", "", true));
    out.extend_from_slice(b"\nendobj\n");
    let (mut head, mut body) = (String::new(), Vec::new());
    for (n, b) in &packed {
        head.push_str(&format!("{n} {} ", body.len()));
        body.extend_from_slice(b);
        body.push(b'\n');
    }
    let mut objstm = head.clone().into_bytes();
    objstm.extend_from_slice(&body);
    let objstm_at = out.len();
    out.extend_from_slice(b"7 0 obj\n");
    out.extend_from_slice(&stream(&objstm, &format!("/Type /ObjStm /N 4 /First {}", head.len()), true));
    out.extend_from_slice(b"\nendobj\n");
    let xref_at = out.len();
    let mut rows: Vec<[u8; 7]> = Vec::new();
    for n in 0..9u32 {
        let (kind, a, b): (u8, u32, u16) = match n {
            4 => (1, content_at as u32, 0),
            7 => (1, objstm_at as u32, 0),
            8 => (1, xref_at as u32, 0),
            1 | 2 | 3 | 5 => (2, 7, packed.iter().position(|(m, _)| *m == n).unwrap() as u16),
            _ => (0, 0, 0),
        };
        let mut r = [0u8; 7];
        r[0] = kind;
        r[1..5].copy_from_slice(&a.to_be_bytes());
        r[5..7].copy_from_slice(&b.to_be_bytes());
        rows.push(r);
    }
    let mut predicted = Vec::new();
    let mut prev = [0u8; 7];
    for r in &rows {
        predicted.push(2u8);
        predicted.extend(r.iter().zip(prev.iter()).map(|(a, b)| a.wrapping_sub(*b)));
        prev = *r;
    }
    out.extend_from_slice(b"8 0 obj\n");
    out.extend_from_slice(&stream(&predicted, "/Type /XRef /Size 9 /W [1 4 2] /Root 1 0 R /DecodeParms << /Predictor 12 /Columns 7 >>", true));
    out.extend_from_slice(format!("\nendobj\nstartxref\n{xref_at}\n%%EOF\n").as_bytes());
    let imported = pdf::read(&out, &at72(TextMode::Lines)).unwrap();
    assert!(imported.notes.iter().all(|n| !n.contains("damaged")), "read through the stream, not by scanning: {:?}", imported.notes);
    assert_eq!(texts(&imported.pages[0].layers), ["packed away"]);
}

#[test]
fn encrypted_and_page_less_files_are_refused_plainly() {
    let mut objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec()),
    ];
    let empty = build(&objects, "");
    assert!(matches!(pdf::read(&empty, &ReadOptions::default()), Err(cshop_io::IoError::Decode(_))));
    objects.push((3, b"<< /Filter /Standard /V 2 /R 3 >>".to_vec()));
    let locked = build(&objects, " /Encrypt 3 0 R");
    let err = pdf::read(&locked, &ReadOptions::default()).err().expect("refused");
    assert!(err.to_string().contains("encrypted"), "{err}");
}

#[test]
fn pages_become_groups_with_page_one_showing() {
    let mut objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 612 792] >>".to_vec()),
        (3, b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".to_vec()),
        (4, b"<< /Type /Page /Parent 2 0 R /Rotate 90 /Contents 5 0 R >>".to_vec()),
    ];
    objects.push((5, stream(b"0 g 10 10 50 50 re f", "", false)));
    let data = build(&objects, "");
    let doc = cshop_io::decode_document(&data, None).unwrap();
    assert_eq!((doc.width, doc.height), (3300, 3300), "300 dpi; the turned page is landscape, and both fit");
    assert_eq!(doc.dpi, 300.0);
    let root: Vec<_> = doc.tree.root().iter().map(|id| doc.tree.get(*id).unwrap()).collect();
    assert_eq!(root.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Page 2", "Page 1"], "page 1 on top");
    assert!(root[1].visible && !root[0].visible, "only page 1 shows");
    let first = doc.tree.children(Some(root[1].id));
    assert_eq!(doc.tree.get(first[0]).unwrap().name, "Paper", "white paper under each page");
}

// ---------------------------------------------------------------------------
// A real file
// ---------------------------------------------------------------------------

fn nda() -> Option<Vec<u8>> {
    let path = std::env::var("NDA_PDF").unwrap_or_else(|_| format!("{}/Downloads/nda.pdf", std::env::var("HOME").unwrap_or_default()));
    match std::fs::read(&path) {
        Ok(b) => Some(b),
        Err(_) => {
            eprintln!("(skipped: no {path}; set NDA_PDF)");
            None
        }
    }
}

/// Every non-space character on a page, sorted: what must survive whatever
/// the grouping, the order and the spacing.
fn letters(s: &str) -> Vec<char> {
    let mut v: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    v.sort_unstable();
    v
}

#[test]
fn the_nda_comes_in_as_four_pages_of_type_and_rules() {
    let Some(bytes) = nda() else { return };
    if !have_fonts() {
        return;
    }
    let imported = pdf::read(&bytes, &ReadOptions::default()).unwrap();
    assert_eq!(imported.pages.len(), 4);
    for p in &imported.pages {
        assert_eq!((p.width, p.height), (2550, 3300), "US letter at 300 dpi");
        assert_eq!(p.layers[0].name, "Paper");
        let t = texts(&p.layers);
        assert!(t.contains(&p.number.to_string()), "page {} has its number", p.number);
        assert!(t.contains(&"Mutual NDA (entity) 16JAN2023".to_string()), "and its footer");
    }
    assert!(imported.notes.iter().any(|n| n.contains("TimesNewRomanPSMT as Liberation Serif")), "the substitution is named: {:?}", imported.notes);
    assert!(imported.notes.iter().all(|n| !n.contains("Not reproduced")), "nothing on these pages is lost: {:?}", imported.notes);

    let first = &imported.pages[0].layers;
    let title = first.iter().find(|l| texts(std::slice::from_ref(l)).first().is_some_and(|t| t == "MUTUAL CONFIDENTIALITY AND NON-DISCLOSURE AGREEMENT")).expect("the title is one type layer");
    let LayerKind::Text(t) = &title.kind else { unreachable!() };
    assert!(t.content().style.bold, "the title is bold");
    let centre = title.offset.0 as f32 + t.raster().width() as f32 / 2.0;
    assert!((centre - 1275.0).abs() < 20.0, "and centred on the page: {centre}");
    let shapes = first.iter().filter(|l| matches!(l.kind, LayerKind::Shape(_))).count();
    assert!(shapes >= 10, "the header's rules and underlines: {shapes}");
}

#[test]
fn the_nda_loses_no_text_in_either_mode() {
    let Some(bytes) = nda() else { return };
    if !have_fonts() {
        return;
    }
    let Ok(out) = std::process::Command::new("pdftotext").arg(std::env::var("NDA_PDF").unwrap_or_else(|_| format!("{}/Downloads/nda.pdf", std::env::var("HOME").unwrap())).as_str()).arg("-").output() else {
        eprintln!("(skipped: pdftotext is not installed to compare against)");
        return;
    };
    let reference = String::from_utf8_lossy(&out.stdout).into_owned();
    let reference_pages: Vec<&str> = reference.split('\u{c}').collect();
    for mode in [TextMode::Lines, TextMode::Paragraphs] {
        let imported = pdf::read(&bytes, &ReadOptions { text: mode, ..Default::default() }).unwrap();
        for p in &imported.pages {
            let ours = texts(&p.layers).join(" ");
            // poppler writes the fi ligature as two letters, as this reader does.
            assert_eq!(letters(&ours), letters(reference_pages[p.number - 1]), "{mode:?}, page {}", p.number);
        }
    }
}

#[test]
fn the_nda_in_paragraphs_is_far_fewer_layers_and_keeps_its_line_breaks() {
    let Some(bytes) = nda() else { return };
    if !have_fonts() {
        return;
    }
    let lines = pdf::read(&bytes, &ReadOptions::default()).unwrap();
    let paras = pdf::read(&bytes, &ReadOptions { text: TextMode::Paragraphs, ..Default::default() }).unwrap();
    let count = |i: &pdf::Imported| i.pages.iter().map(|p| texts(&p.layers).len()).sum::<usize>();
    assert!(count(&paras) * 2 < count(&lines), "{} paragraphs vs {} lines", count(&paras), count(&lines));
    // Each box breaks where the page broke: the first word of every line it
    // lays out is the first word of a line on the page, as line mode found them.
    let starts: std::collections::HashSet<String> = lines
        .pages
        .iter()
        .flat_map(|p| texts(&p.layers))
        .filter_map(|t| t.split_whitespace().next().map(str::to_string))
        .collect();
    let db = cshop_core::font::FontDb::global();
    let (mut checked, mut misses) = (0, Vec::new());
    for p in &paras.pages {
        for l in &p.layers {
            let LayerKind::Text(t) = &l.kind else { continue };
            let c = t.content();
            if c.wrap_width.is_none() {
                continue;
            }
            let font = db.load(&c.style.family, c.style.bold, c.style.italic).unwrap();
            for line in cshop_core::text::layout(c, &font).lines {
                let first = c.text[line.range.clone()].split_whitespace().next().unwrap_or("").to_string();
                checked += 1;
                if !starts.contains(&first) {
                    misses.push(format!("page {}: {:?}", p.number, &c.text[line.range.clone()]));
                }
            }
        }
    }
    assert!(checked >= 50, "{checked} lines in boxes");
    assert!(misses.is_empty(), "{} of {checked} box lines start somewhere no page line did:\n{}", misses.len(), misses.join("\n"));
}
