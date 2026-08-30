//! Lighting a picture from a guess at its shape.
//!
//! Every one of these is about direction, which is the part with a right
//! answer: a surface tilted towards the lamp must brighten and one tilted away
//! must not. A photograph would forgive getting that backwards; a ramp will
//! not.

use cshop_core::color::Rgba8;
use cshop_core::pixels::PixelBuffer;
use cshop_core::relight::{apply, DepthMap, Relight};

/// A flat grey field, so any change is the lighting rather than the picture.
fn flat(w: u32, h: u32) -> PixelBuffer {
    PixelBuffer::filled(w, h, Rgba8::opaque(128, 128, 128))
}

/// Depth that rises steadily from left to right: a slope facing left.
fn ramp_x(w: u32, h: u32) -> DepthMap {
    let data = (0..h)
        .flat_map(|_| (0..w).map(move |x| x as f32 / (w - 1) as f32))
        .collect();
    DepthMap::from_values(w, h, data).unwrap()
}

fn luma(p: &PixelBuffer, x: i32, y: i32) -> f32 {
    let c = p.get(x, y);
    (c.r as f32 + c.g as f32 + c.b as f32) / 3.0
}

#[test]
fn a_lamp_with_nothing_to_light_changes_nothing() {
    let src = flat(32, 32);
    let lamp = Relight { intensity: 0.0, ambient: 1.0, ..Default::default() };
    assert!(lamp.is_identity());
    assert_eq!(apply(&src, &DepthMap::new(32, 32), lamp).pixels(), src.pixels());
}

/// Flat depth is a flat surface: every pixel faces the camera, so every pixel
/// takes the same light however the lamp is placed.
#[test]
fn a_flat_surface_lights_evenly() {
    let src = flat(40, 40);
    for azimuth in [0.0, 90.0, 200.0, 315.0] {
        let out = apply(
            &src,
            &DepthMap::new(40, 40),
            Relight { azimuth, intensity: 0.8, ..Default::default() },
        );
        let first = luma(&out, 5, 5);
        for (x, y) in [(20, 5), (35, 20), (10, 30)] {
            assert!(
                (luma(&out, x, y) - first).abs() < 0.6,
                "at {azimuth}° the flat surface is uneven: {first} against {}",
                luma(&out, x, y)
            );
        }
    }
}

/// The slope rises to the right, so its surfaces face left. A lamp on the left
/// should light it and one on the right should not — and swapping the lamp
/// must swap the answer.
#[test]
fn a_slope_takes_light_from_the_side_it_faces() {
    let src = flat(64, 64);
    let depth = ramp_x(64, 64);
    let common = Relight { elevation: 20.0, intensity: 1.0, ambient: 0.6, ..Default::default() };

    let from_left = apply(&src, &depth, Relight { azimuth: 0.0, ..common });
    let from_right = apply(&src, &depth, Relight { azimuth: 180.0, ..common });

    let left = luma(&from_left, 32, 32);
    let right = luma(&from_right, 32, 32);
    assert!(
        left > right + 4.0,
        "a slope facing left should be brighter lit from the left: {left} against {right}"
    );
}

/// Relief is how much shape to read into the depth, so more of it means more
/// difference between a lit slope and an unlit one.
#[test]
fn relief_decides_how_much_shape_there_is() {
    let src = flat(64, 64);
    let depth = ramp_x(64, 64);
    let lit = |relief: f32| {
        let a = apply(
            &src,
            &depth,
            Relight { azimuth: 0.0, elevation: 20.0, intensity: 1.0, ambient: 0.6, relief, ..Default::default() },
        );
        let b = apply(
            &src,
            &depth,
            Relight { azimuth: 180.0, elevation: 20.0, intensity: 1.0, ambient: 0.6, relief, ..Default::default() },
        );
        luma(&a, 32, 32) - luma(&b, 32, 32)
    };
    let gentle = lit(0.2);
    let strong = lit(2.0);
    assert!(strong > gentle, "more relief, more shading: {gentle} against {strong}");
    assert!(
        lit(0.0).abs() < 0.6,
        "and none at all should light a flat picture evenly: {}",
        lit(0.0)
    );
}

/// Ambient is what survives where the lamp does not reach. At zero, the side
/// facing away goes dark; at one, the lamp can only ever add.
#[test]
fn ambient_decides_what_survives_in_the_shade() {
    let src = flat(64, 64);
    let depth = ramp_x(64, 64);
    let away = |ambient: f32| {
        let out = apply(
            &src,
            &depth,
            Relight { azimuth: 180.0, elevation: 5.0, intensity: 1.0, ambient, relief: 3.0, ..Default::default() },
        );
        luma(&out, 32, 32)
    };
    assert!(away(0.0) < 20.0, "with no ambient the unlit side is dark: {}", away(0.0));
    assert!(away(1.0) >= 127.0, "with full ambient the picture is never darkened: {}", away(1.0));
}

/// A coloured lamp tints what it lights and leaves the rest alone.
#[test]
fn a_coloured_lamp_tints_only_what_it_lights() {
    let src = flat(64, 64);
    let depth = ramp_x(64, 64);
    let out = apply(
        &src,
        &depth,
        Relight {
            azimuth: 0.0,
            elevation: 20.0,
            intensity: 1.2,
            ambient: 1.0,
            relief: 1.0,
            color: Rgba8::opaque(255, 40, 40),
        },
    );
    let c = out.get(32, 32);
    assert!(c.r > c.g && c.r > c.b, "a red lamp should light it red: {c:?}");
}

/// Alpha is coverage; a lamp has no opinion about it.
#[test]
fn lighting_leaves_alpha_alone() {
    let mut src = PixelBuffer::new(32, 32);
    for (i, p) in src.pixels_mut().iter_mut().enumerate() {
        *p = Rgba8::new(200, 180, 160, (i % 256) as u8);
    }
    let before: Vec<u8> = src.pixels().iter().map(|p| p.a).collect();
    let out = apply(
        &src,
        &ramp_x(32, 32),
        Relight { intensity: 1.0, ambient: 0.2, ..Default::default() },
    );
    let after: Vec<u8> = out.pixels().iter().map(|p| p.a).collect();
    assert_eq!(before, after);
}

// --- depth as a mask -------------------------------------------------------

/// Near reveals and far hides, which is the way round that makes the obvious
/// edit obvious: mask a layer by its own depth and the subject survives.
#[test]
fn depth_becomes_a_mask_the_right_way_round() {
    let depth = ramp_x(32, 32); // rises to the right, so the right is nearest
    let near = cshop_core::relight::to_mask(&depth, false);
    assert!(near.get(31, 16) > 200, "the near side should be revealed");
    assert!(near.get(0, 16) < 55, "the far side should be hidden");

    let far = cshop_core::relight::to_mask(&depth, true);
    assert!(far.get(31, 16) < 55, "inverted, the near side hides");
    assert!(far.get(0, 16) > 200, "and the far side reveals");

    // The two are complements, give or take rounding.
    for x in [0, 7, 16, 24, 31] {
        let sum = near.get(x, 16) as i32 + far.get(x, 16) as i32;
        assert!((sum - 255).abs() <= 1, "at {x} they sum to {sum}, not 255");
    }
}
