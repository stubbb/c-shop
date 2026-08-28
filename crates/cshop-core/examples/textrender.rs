//! Render sample type to a PNG, so the rasteriser can be looked at.
use cshop_core::color::Rgba8;
use cshop_core::font::FontDb;
use cshop_core::pixels::PixelBuffer;
use cshop_core::text::{TextAlign, TextContent, TextStyle};

fn main() {
    let db = FontDb::global();
    let family = db.default_family();
    let mut sheet = PixelBuffer::filled(900, 560, Rgba8::new(0, 0, 0, 0));

    let cases: Vec<(&str, TextContent)> = vec![
        ("regular 48", TextContent::new("Handgloves 123", TextStyle { family: family.clone(), size: 48.0, ..Default::default() })),
        ("bold", TextContent::new("Handgloves bold", TextStyle { family: family.clone(), size: 48.0, bold: true, ..Default::default() })),
        ("italic", TextContent::new("Handgloves italic", TextStyle { family: family.clone(), size: 48.0, italic: true, ..Default::default() })),
        ("small 14", TextContent::new("The quick brown fox jumps over the lazy dog", TextStyle { family: family.clone(), size: 14.0, ..Default::default() })),
        ("tracking +200", TextContent::new("S P A C E D", TextStyle { family: family.clone(), size: 32.0, tracking: 200.0, ..Default::default() })),
        ("aliased", TextContent::new("No antialias", TextStyle { family: family.clone(), size: 32.0, antialias: false, ..Default::default() })),
    ];

    let mut y = 20;
    for (label, content) in &cases {
        if let Some(r) = cshop_core::text::render(content) {
            println!("{label:16} raster {}x{} anchor {:?}", r.pixels.width(), r.pixels.height(), r.anchor);
            sheet.paste(&r.pixels, 20, y);
            y += r.pixels.height() as i32 + 4;
        } else {
            println!("{label:16} FAILED to render");
        }
    }

    // A wrapped paragraph box.
    let para = TextContent {
        text: "A paragraph box wraps its text at the width it was drawn, breaking between words the way a text frame should.".into(),
        style: TextStyle { family: family.clone(), size: 20.0, align: TextAlign::Left, ..Default::default() },
        wrap_width: Some(360.0),
    };
    if let Some(r) = cshop_core::text::render(&para) {
        println!("paragraph      raster {}x{} anchor {:?}", r.pixels.width(), r.pixels.height(), r.anchor);
        sheet.paste(&r.pixels, 480, 20);
    }
    for (i, align) in [TextAlign::Left, TextAlign::Center, TextAlign::Right].iter().enumerate() {
        let c = TextContent {
            text: "aligned\nin a box".into(),
            style: TextStyle { family: family.clone(), size: 22.0, align: *align, ..Default::default() },
            wrap_width: Some(300.0),
        };
        if let Some(r) = cshop_core::text::render(&c) {
            sheet.paste(&r.pixels, 480, 180 + i as i32 * 80);
        }
    }

    let path = std::env::args().nth(1).unwrap_or_else(|| "text.png".into());
    std::fs::write(&path, sheet.as_bytes()).unwrap();
    println!("wrote {path} {}x{}", sheet.width(), sheet.height());
}
