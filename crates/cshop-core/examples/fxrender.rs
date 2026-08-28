//! Render each layer effect on its own, to look at.
use cshop_core::color::Rgba8;
use cshop_core::effects::*;
use cshop_core::font::FontDb;
use cshop_core::pixels::PixelBuffer;
use cshop_core::text::{TextContent, TextStyle};

/// Chunky letterforms: effects read on a shape with both curves and corners.
fn word() -> PixelBuffer {
    let db = FontDb::global();
    let content = TextContent::new(
        "Fx",
        TextStyle {
            family: db.default_family(),
            size: 110.0,
            bold: true,
            color: Rgba8::opaque(200, 200, 205),
            ..Default::default()
        },
    );
    cshop_core::text::render(&content).map(|r| r.pixels).unwrap_or_else(|| {
        let mut p = PixelBuffer::new(160, 120);
        p.fill_rect(cshop_core::geom::IRect::at(20, 20, 120, 80), Rgba8::opaque(200, 200, 205));
        p
    })
}

fn main() {
    let base = word();
    let mut sheet = PixelBuffer::filled(1240, 1300, Rgba8::new(0, 0, 0, 0));

    let mut cases: Vec<(&str, LayerEffects)> = Vec::new();
    let mut fx = LayerEffects::new();

    fx.drop_shadow = Some(Shadow { distance: 10.0, size: 10.0, ..Default::default() });
    cases.push(("Drop Shadow", fx));

    let mut fx = LayerEffects::new();
    fx.outer_glow = Some(Glow { size: 16.0, spread: 0.1, ..Default::default() });
    cases.push(("Outer Glow", fx));

    let mut fx = LayerEffects::new();
    fx.bevel = Some(Bevel { size: 10.0, depth: 1.4, soften: 2.0, ..Default::default() });
    cases.push(("Bevel (inner)", fx));

    let mut fx = LayerEffects::new();
    fx.bevel = Some(Bevel { style: BevelStyle::Emboss, size: 8.0, depth: 1.4, ..Default::default() });
    cases.push(("Emboss", fx));

    let mut fx = LayerEffects::new();
    fx.bevel = Some(Bevel { style: BevelStyle::Pillow, size: 9.0, depth: 1.4, ..Default::default() });
    cases.push(("Pillow Emboss", fx));

    let mut fx = LayerEffects::new();
    fx.inner_shadow = Some(Shadow { distance: 6.0, size: 8.0, ..Default::default() });
    cases.push(("Inner Shadow", fx));

    let mut fx = LayerEffects::new();
    fx.inner_glow = Some(Glow { size: 14.0, color: Rgba8::opaque(255, 200, 80), ..Default::default() });
    cases.push(("Inner Glow", fx));

    let mut fx = LayerEffects::new();
    fx.satin = Some(Satin::default());
    cases.push(("Satin", fx));

    let mut fx = LayerEffects::new();
    fx.color_overlay = Some(ColorOverlay::default());
    cases.push(("Color Overlay", fx));

    let mut fx = LayerEffects::new();
    fx.stroke = Some(Stroke { size: 4.0, ..Default::default() });
    cases.push(("Stroke", fx));

    let mut fx = LayerEffects::new();
    fx.gradient_overlay = Some(GradientOverlay {
        from: Rgba8::opaque(250, 190, 60),
        to: Rgba8::opaque(210, 60, 120),
        angle: 90.0,
        ..Default::default()
    });
    cases.push(("Gradient Overlay", fx));

    let mut fx = LayerEffects::new();
    fx.gradient_overlay = Some(GradientOverlay {
        from: Rgba8::opaque(90, 220, 255),
        to: Rgba8::opaque(20, 40, 120),
        kind: cshop_core::fill::GradientKind::Radial,
        ..Default::default()
    });
    cases.push(("Gradient (radial)", fx));

    let mut fx = LayerEffects::new();
    fx.pattern_overlay = Some(PatternOverlay {
        kind: PatternKind::Checker,
        scale: 12.0,
        opacity: 0.6,
        ..Default::default()
    });
    cases.push(("Pattern (checker)", fx));

    let mut fx = LayerEffects::new();
    fx.pattern_overlay = Some(PatternOverlay {
        kind: PatternKind::Stripes,
        scale: 10.0,
        angle: 45.0,
        opacity: 0.55,
        ..Default::default()
    });
    cases.push(("Pattern (stripes)", fx));

    let mut fx = LayerEffects::new();
    fx.pattern_overlay = Some(PatternOverlay {
        kind: PatternKind::Dots,
        scale: 14.0,
        opacity: 0.7,
        ..Default::default()
    });
    cases.push(("Pattern (dots)", fx));

    let mut fx = LayerEffects::new();
    fx.pattern_overlay = Some(PatternOverlay {
        kind: PatternKind::CrossHatch,
        scale: 12.0,
        opacity: 0.6,
        ..Default::default()
    });
    cases.push(("Pattern (hatch)", fx));

    // A gradient under a pattern, which is how the two are usually stacked.
    let mut fx = LayerEffects::new();
    fx.gradient_overlay = Some(GradientOverlay {
        from: Rgba8::opaque(255, 200, 90),
        to: Rgba8::opaque(180, 40, 70),
        ..Default::default()
    });
    fx.pattern_overlay = Some(PatternOverlay {
        kind: PatternKind::Grid,
        scale: 9.0,
        color: Rgba8::opaque(255, 255, 255),
        opacity: 0.35,
        ..Default::default()
    });
    fx.bevel = Some(Bevel { size: 6.0, depth: 1.2, ..Default::default() });
    cases.push(("Gradient + pattern", fx));

    // Stroke only: fill opacity to zero leaves the effects behind.
    let mut fx = LayerEffects::new();
    fx.stroke = Some(Stroke { size: 3.0, color: Rgba8::opaque(240, 240, 240), ..Default::default() });
    fx.outer_glow = Some(Glow { size: 12.0, ..Default::default() });
    cases.push(("Stroke only (fill 0)", fx));

    // Everything at once.
    let mut fx = LayerEffects::new();
    fx.drop_shadow = Some(Shadow { distance: 8.0, size: 8.0, ..Default::default() });
    fx.outer_glow = Some(Glow { size: 10.0, opacity: 0.5, ..Default::default() });
    fx.bevel = Some(Bevel { size: 7.0, depth: 1.2, soften: 1.0, ..Default::default() });
    fx.inner_shadow = Some(Shadow { distance: 3.0, size: 5.0, opacity: 0.5, ..Default::default() });
    fx.satin = Some(Satin { opacity: 0.35, ..Default::default() });
    fx.stroke = Some(Stroke { size: 2.0, ..Default::default() });
    cases.push(("All together", fx));

    for (i, (label, fx)) in cases.iter().enumerate() {
        let fill = if label.starts_with("Stroke only") { 0.0 } else { 1.0 };
        match render(&base, fx, fill) {
            Some(r) => {
                let (cx, cy) = ((i % 4) as i32 * 310 + 20, (i / 4) as i32 * 260 + 20);
                println!("{label:22} {}x{} origin {:?}", r.pixels.width(), r.pixels.height(), r.origin);
                sheet.paste(&r.pixels, cx, cy);
            }
            None => println!("{label:22} rendered nothing"),
        }
    }

    let path = std::env::args().nth(1).unwrap_or_else(|| "fx.raw".into());
    std::fs::write(&path, sheet.as_bytes()).unwrap();
    println!("wrote {path} {}x{}", sheet.width(), sheet.height());
}
