//! The bucket and the gradient were made fast; these check they stayed right.
//!
//! Each of the three changes replaced a straightforward implementation with a
//! less obvious one — a baked ramp, a banded blur, a flood fill over row
//! slices — so each is compared against something obviously correct rather
//! than trusted.

use cshop_core::blend::BlendMode;
use cshop_core::color::Rgba8;
use cshop_core::fill::{Gradient, GradientKind, GradientStop};
use cshop_core::geom::Vec2;
use cshop_core::mask::MaskBuffer;
use cshop_core::pixels::PixelBuffer;
use cshop_core::selection::{Rectf, Selection};
use cshop_core::wand::{magic_wand, WandOptions};

fn ramp_of(stops: Vec<GradientStop>) -> Gradient {
    Gradient {
        stops,
        kind: GradientKind::Linear,
        reverse: false,
        opacity: 1.0,
        mode: BlendMode::Normal,
        dither: false,
    }
}

/// The baked table has to agree with the function it was baked from.
#[test]
fn the_baked_ramp_matches_the_exact_colours() {
    let cases = vec![
        vec![
            GradientStop { position: 0.0, color: Rgba8::BLACK },
            GradientStop { position: 1.0, color: Rgba8::WHITE },
        ],
        // Out of order, uneven, and running into transparency — the awkward
        // cases the sort in `color_at` exists for.
        vec![
            GradientStop { position: 1.0, color: Rgba8::new(10, 200, 30, 0) },
            GradientStop { position: 0.15, color: Rgba8::opaque(255, 0, 0) },
            GradientStop { position: 0.6, color: Rgba8::new(0, 0, 255, 128) },
            GradientStop { position: 0.0, color: Rgba8::opaque(0, 255, 0) },
        ],
    ];
    for stops in cases {
        let g = ramp_of(stops);
        let baked = g.bake();
        let mut worst = 0i32;
        for i in 0..=1000 {
            let t = i as f32 / 1000.0;
            let (a, b) = (g.color_at(t), baked.at(t));
            for (x, y) in [(a.r, b.r), (a.g, b.g), (a.b, b.b), (a.a, b.a)] {
                worst = worst.max((x as i32 - y as i32).abs());
            }
        }
        // A thousand entries over a 0..255 range: any visible step would be
        // far larger than this.
        assert!(worst <= 2, "the table drifts from the exact ramp by {worst} levels");
    }
}

/// A band boundary in the vertical blur would show as a seam, and a seam is
/// not symmetric — so symmetry is the cheapest way to catch one.
#[test]
fn feathering_stays_symmetric_across_the_blur_bands() {
    // Tall enough to cross several 64-row bands.
    const W: u32 = 64;
    const H: u32 = 400;
    let mut mask = MaskBuffer::hide_all(W, H);
    // A block centred vertically, so the answer must be symmetric about it.
    mask.fill_rect(cshop_core::geom::IRect::new(8, 150, 56, 250), 255);
    let mut s = Selection::from_mask(mask);
    s.feather(9.0);

    for y in 0..H as i32 {
        let mirror = H as i32 - 1 - y;
        for x in 0..W as i32 {
            assert_eq!(
                s.coverage(x, y),
                s.coverage(x, mirror),
                "feather is not symmetric at ({x}, {y}) against ({x}, {mirror}); \
                 a blur band boundary has left a seam"
            );
        }
    }
}

/// The flood fill over row slices must select exactly what an obvious one does.
#[test]
fn the_flood_fill_matches_a_naive_one() {
    // A patterned image with irregular regions, so the runs are not trivial.
    let (w, h) = (137u32, 91u32);
    let mut px = PixelBuffer::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let on = ((x * 7 + y * 13) % 23 < 11) || (x > 40 && x < 90 && y > 20 && y < 60);
            px.set(x, y, if on { Rgba8::WHITE } else { Rgba8::BLACK });
        }
    }

    for seed in [(0, 0), (60, 40), (136, 90), (5, 77)] {
        let got = magic_wand(
            &px,
            seed.0,
            seed.1,
            WandOptions { tolerance: 10, contiguous: true, antialias: false },
        );

        // The obvious version: a queue of single pixels, four-connected.
        let target = px.get(seed.0, seed.1);
        let mut want = MaskBuffer::hide_all(w, h);
        let mut queue = vec![seed];
        while let Some((x, y)) = queue.pop() {
            if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 || want.get(x, y) != 0 {
                continue;
            }
            let c = px.get(x, y);
            let diff = |a: u8, b: u8| a.abs_diff(b);
            let d = diff(c.r, target.r)
                .max(diff(c.g, target.g))
                .max(diff(c.b, target.b))
                .max(diff(c.a, target.a));
            if d > 10 {
                continue;
            }
            want.set(x, y, 255);
            queue.extend_from_slice(&[(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]);
        }

        for y in 0..h as i32 {
            for x in 0..w as i32 {
                assert_eq!(
                    got.coverage(x, y),
                    want.get(x, y),
                    "seed {seed:?} disagrees at ({x}, {y})"
                );
            }
        }
    }
}

/// Rendering in parallel must give what rendering in order gave.
#[test]
fn a_gradient_renders_the_same_whatever_the_row_order() {
    let g = ramp_of(vec![
        GradientStop { position: 0.0, color: Rgba8::opaque(255, 0, 0) },
        GradientStop { position: 0.5, color: Rgba8::opaque(0, 255, 0) },
        GradientStop { position: 1.0, color: Rgba8::new(0, 0, 255, 40) },
    ]);
    let (w, h) = (200u32, 150u32);
    let mut px = PixelBuffer::filled(w, h, Rgba8::WHITE);
    let touched = g.render(
        &mut px,
        (0, 0),
        Vec2::new(10.0, 10.0),
        Vec2::new(180.0, 130.0),
        None,
        false,
    );
    assert_eq!(touched, cshop_core::geom::IRect::new(0, 0, w as i32, h as i32));

    // Every pixel must be exactly what the ramp says for its own parameter.
    let baked = g.bake();
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let t = g.parameter(p, Vec2::new(10.0, 10.0), Vec2::new(180.0, 130.0));
            let want = baked.at(t);
            let got = px.get(x, y);
            // Composited over white at full opacity, so an opaque stop lands
            // exactly; the transparent end blends.
            if want.a == 255 {
                assert_eq!(got, want, "at ({x}, {y})");
            }
        }
    }

    // And a selection must still confine it.
    let sel = Selection::from_rect(w, h, Rectf::from_points(Vec2::new(50.0, 50.0), Vec2::new(100.0, 100.0)), false);
    let mut px2 = PixelBuffer::filled(w, h, Rgba8::WHITE);
    g.render(&mut px2, (0, 0), Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0), Some(&sel.to_mask()), false);
    assert_eq!(px2.get(10, 10), Rgba8::WHITE, "outside the selection is untouched");
    assert_ne!(px2.get(75, 75), Rgba8::WHITE, "inside it is painted");
}
