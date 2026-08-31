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
            ..Default::default()
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

// --- the edge of an object -------------------------------------------------

/// A depth map with a cliff down the middle: near on the left, far on the
/// right, with nothing in between. That is what the edge of any object looks
/// like to a depth model, and it is where the lighting used to draw a black
/// line round everything.
fn cliff(w: u32, h: u32) -> DepthMap {
    let data = (0..h)
        .flat_map(|_| (0..w).map(move |x| if x < w / 2 { 1.0 } else { 0.0 }))
        .collect();
    DepthMap::from_values(w, h, data).unwrap()
}

/// A step in the depth may not light as a line at all — dark or bright.
///
/// The picture is two flat surfaces at different distances with a step between
/// them. Both face the camera, so both take exactly the same light, and the
/// step between them is not a surface: there is nothing in the picture that
/// says how the near one joins the far one. So the whole frame should come out
/// as evenly lit as either half of it, and any line along the join is the
/// lighting inventing a wall that is not there.
#[test]
fn a_step_in_the_depth_does_not_light_as_a_line() {
    let src = flat(80, 40);
    let depth = cliff(80, 40);
    let lamp = Relight {
        azimuth: 0.0,
        elevation: 20.0,
        intensity: 1.6,
        ambient: 0.4,
        relief: 4.0,
        ..Default::default()
    };
    let out = apply(&src, &depth, lamp);

    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    let (mut where_lo, mut where_hi) = ((0, 0), (0, 0));
    for y in 0..40 {
        for x in 0..80 {
            let v = luma(&out, x, y);
            if v < lo {
                lo = v;
                where_lo = (x, y);
            }
            if v > hi {
                hi = v;
                where_hi = (x, y);
            }
        }
    }
    assert!(
        hi - lo < 2.0,
        "the step lights as a line: {lo:.0} at {where_lo:?} against {hi:.0} at {where_hi:?}"
    );
}

/// Softening must not spread a step. It exists to take the model's noise off
/// a surface, and a step is not a surface: blurring across one puts a ramp on
/// both sides of an outline, and a ramp lights — which is a glow hanging in
/// the air beside the subject and a shadow thrown onto what is behind it.
#[test]
fn softening_does_not_reach_across_a_step() {
    // Square, because the radius is a fraction of the picture's shorter side:
    // on a 200 by 40 strip a five percent softening is two pixels, which is
    // right and would make this test look like it had failed.
    let depth = cliff(200, 200);
    assert_eq!(depth.softening_radius(0.05), 10);
    let soft = depth.smoothed(10);

    // How many columns are neither fully near nor fully far, along one row.
    let transition = |m: &DepthMap| {
        (0..200).filter(|&x| { let v = m.at(x, 100); v > 0.02 && v < 0.98 }).count()
    };
    assert!(transition(&depth) <= 2, "a cliff is a cliff: {}", transition(&depth));
    assert_eq!(
        transition(&soft), 0,
        "the step was smeared into a ramp {} columns wide",
        transition(&soft)
    );

    // And it is still a step, in the same place: the near side stays near.
    assert!(soft.at(2, 100) > 0.9, "the near side is still near");
    assert!(soft.at(197, 100) < 0.1, "and the far side still far");
    assert!(soft.at(99, 100) > 0.9 && soft.at(100, 100) < 0.1, "and it did not move");
}

/// Within one surface it still softens, or it is not doing its job.
#[test]
fn softening_still_smooths_what_is_one_surface() {
    // A gentle ramp with noise on it: the noise is what has to go.
    let mut data = Vec::with_capacity(200 * 200);
    for y in 0..200 {
        for x in 0..200 {
            let ramp = x as f32 / 199.0 * 0.5;
            let noise = if (x + y) % 2 == 0 { 0.02 } else { -0.02 };
            data.push(ramp + noise);
        }
    }
    let rough = DepthMap::from_values(200, 200, data).unwrap();
    let smooth = rough.smoothed(6);

    // Roughness as the mean step between neighbours along a row.
    let roughness = |m: &DepthMap| {
        (1..200).map(|x| (m.at(x, 100) - m.at(x - 1, 100)).abs()).sum::<f32>() / 199.0
    };
    assert!(
        roughness(&smooth) < roughness(&rough) / 4.0,
        "it should have smoothed the noise: {} against {}",
        roughness(&smooth),
        roughness(&rough)
    );
}

/// The point of all of it, on the shape that actually shows it: a lamp beside
/// a near object must not light the far side of the step. That is the glow
/// that hangs in the air next to a subject, and it comes from a blur that
/// crossed the outline.
#[test]
fn a_lamp_does_not_light_across_an_outline() {
    let src = flat(200, 200);
    let depth = cliff(200, 200);
    // Lit from the left, so the step at x=100 faces the lamp and the far side
    // beyond it is what must be left alone.
    let lamp = Relight {
        azimuth: 0.0,
        elevation: 20.0,
        intensity: 2.0,
        ambient: 1.0,
        relief: 4.0,
        softness: 0.05,
        ..Default::default()
    };
    let radius = depth.softening_radius(lamp.softness);
    let out = apply(&src, &depth.smoothed(radius), lamp);

    // The far side is flat, so every pixel of it faces the camera and takes
    // the same light. Measured against a column well clear of the outline
    // rather than against the unlit picture: the lamp is *supposed* to light
    // the far side, evenly. What it must not do is light the part of it that
    // happens to sit next to the near object more than the rest.
    let clear = luma(&out, 190, 100);
    let beside: Vec<(i32, f32)> =
        (102..160).map(|x| (x, luma(&out, x, 100) - clear)).collect();
    let (where_worst, worst) =
        beside.iter().copied().fold((0, 0.0f32), |a, b| if b.1.abs() > a.1.abs() { b } else { a });
    assert!(
        worst.abs() < 1.0,
        "the far side is {worst:+.1} levels different {} pixels past the outline — \
         the lamp is reaching across it",
        where_worst - 100
    );
}

/// Softening by nothing is the picture unchanged, so the control has a
/// meaningful zero.
#[test]
fn softening_by_nothing_changes_nothing() {
    let depth = ramp_x(32, 32);
    assert_eq!(depth.smoothed(0).data, depth.data);
    assert_eq!(depth.softening_radius(0.0), 0);
}

// --- lighten only ----------------------------------------------------------

/// The whole claim: with it set, nothing comes out darker than it went in.
#[test]
fn lighten_only_never_takes_light_away() {
    // A ramp facing left, lit from the right, so the near half of the picture
    // is turned away from the lamp — which is where darkening would happen.
    let src = flat(64, 48);
    let depth = ramp_x(64, 48);
    let settings = Relight {
        azimuth: 180.0,
        elevation: 30.0,
        intensity: 0.8,
        // Well below one, which is what makes the unlit side fall away.
        ambient: 0.5,
        relief: 1.5,
        ..Default::default()
    };

    let falls_away = apply(&src, &depth, settings);
    let adds_only = apply(&src, &depth, Relight { lighten_only: true, ..settings });

    let mut darkened = 0;
    let mut darkened_under_the_flag = 0;
    for y in 0..48 {
        for x in 0..64 {
            let was = luma(&src, x, y);
            if luma(&falls_away, x, y) < was - 0.5 {
                darkened += 1;
            }
            if luma(&adds_only, x, y) < was - 0.5 {
                darkened_under_the_flag += 1;
            }
        }
    }
    assert!(
        darkened > 500,
        "the ordinary lamp should have darkened plenty to compare against, got {darkened}"
    );
    assert_eq!(
        darkened_under_the_flag, 0,
        "lighten only darkened {darkened_under_the_flag} pixels"
    );
}

/// It must still *light* things, or "never darkens" is satisfied by doing
/// nothing at all.
#[test]
fn lighten_only_still_lights_what_faces_the_lamp() {
    let src = flat(64, 48);
    let depth = ramp_x(64, 48);
    let lamp = Relight {
        azimuth: 0.0,
        elevation: 20.0,
        intensity: 1.0,
        ambient: 1.0,
        relief: 1.5,
        lighten_only: true,
        ..Default::default()
    };
    let lit = apply(&src, &depth, lamp);
    let brightest = (0..64).map(|x| luma(&lit, x, 24)).fold(0.0f32, f32::max);
    assert!(
        brightest > luma(&src, 0, 24) + 8.0,
        "nothing was lit: brightest {brightest} against {}",
        luma(&src, 0, 24)
    );
}

/// Below one, ambient stops being a darkener and becomes a threshold: the
/// lamp has to beat it before anything shows. So a lower ambient lights less
/// of the picture, and what it does light it lights no less brightly.
#[test]
fn under_the_flag_ambient_narrows_the_light_rather_than_darkening() {
    let src = flat(64, 48);
    // A hill rather than a ramp: a constant slope faces the lamp equally
    // everywhere, so it is either all lit or none of it and a threshold has
    // nothing to bite on.
    let depth = {
        let data: Vec<f32> = (0..48)
            .flat_map(|_| {
                (0..64).map(move |x| {
                    (x as f32 / 63.0 * std::f32::consts::PI).sin()
                })
            })
            .collect();
        DepthMap::from_values(64, 48, data).unwrap()
    };
    let lamp = |ambient: f32| Relight {
        azimuth: 0.0,
        elevation: 20.0,
        intensity: 1.0,
        ambient,
        relief: 1.5,
        lighten_only: true,
        ..Default::default()
    };
    let touched = |px: &PixelBuffer| {
        (0..64).filter(|&x| luma(px, x, 24) > luma(&src, x, 24) + 0.5).count()
    };

    let wide = apply(&src, &depth, lamp(1.0));
    let narrow = apply(&src, &depth, lamp(0.7));
    assert!(
        touched(&narrow) < touched(&wide),
        "a lower ambient should light less of the row, got {} against {}",
        touched(&narrow),
        touched(&wide)
    );
    assert!(touched(&narrow) > 0, "and not none of it");
}

/// Whether a slope is a surface or an outline must not depend on how big the
/// picture is. The same scene at two sizes has to shade the same way.
///
/// Guarding a constant rather than a behaviour, which is unusual — but the
/// first two attempts at that constant were absolute depth changes, and an
/// absolute depth change per pixel means something different on a thumbnail
/// and on a print. One of them silently stopped lighting anything at all on a
/// small test pattern while looking right on a photograph.
#[test]
fn the_same_shape_shades_the_same_at_any_size() {
    let lamp = Relight {
        azimuth: 0.0,
        elevation: 20.0,
        intensity: 1.0,
        ambient: 0.6,
        relief: 1.0,
        softness: 0.0,
        ..Default::default()
    };
    // A ramp across the whole frame is the same surface whatever the frame's
    // size: the depth range and the distance it is spread over both scale.
    // Measured as the difference between lighting it from one side and from
    // the other, which is the shading with the ambient taken out of it.
    let shading = |side: u32| {
        let src = flat(side, side);
        let depth = ramp_x(side, side);
        let left = apply(&src, &depth, Relight { azimuth: 0.0, ..lamp });
        let right = apply(&src, &depth, Relight { azimuth: 180.0, ..lamp });
        let at = side as i32 / 2;
        luma(&left, at, at) - luma(&right, at, at)
    };
    let small = shading(64);
    let large = shading(512);
    assert!(small.abs() > 2.0, "the small one should be shaded at all: {small}");
    assert!(
        (small - large).abs() < small.abs() * 0.25,
        "the same slope shaded {small:.1} at 64 pixels and {large:.1} at 512"
    );
}
