//! Lens correction: does each control move the picture the way it says?
//!
//! Direction is the whole difficulty here. Every one of these corrections has
//! an obvious sign and a fifty-fifty chance of being the wrong one, and a
//! photograph is forgiving enough that the wrong one still looks plausible.
//! So each is measured against a shape whose right answer is arithmetic.

use cshop_core::color::Rgba8;
use cshop_core::filters::plane::Plane;
use cshop_core::progress::Progress;
use cshop_core::lens::{apply, largest_opaque_rect, Lens};
use cshop_core::pixels::PixelBuffer;

fn white(w: u32, h: u32) -> Plane {
    Plane::from_pixels(&PixelBuffer::filled(w, h, Rgba8::WHITE))
}

/// A white field with one horizontal black line, which is the thing every
/// distortion test is really about: does it stay straight?
fn ruled(w: u32, h: u32, row: u32) -> Plane {
    let mut px = PixelBuffer::filled(w, h, Rgba8::WHITE);
    for x in 0..w as i32 {
        px.set(x, row as i32, Rgba8::BLACK);
    }
    Plane::from_pixels(&px)
}

/// Where the dark line sits in a column, to sub-pixel accuracy, by taking the
/// centre of mass of the darkness.
///
/// `None` where the column has no picture in it at all, which is a real answer
/// once a correction has pushed the frame's edge out past the image.
fn line_row(p: &Plane, x: u32) -> Option<f32> {
    let (mut weight, mut total) = (0.0f32, 0.0f32);
    for y in 0..p.height {
        let i = (y as usize * p.width as usize + x as usize) * 4;
        let a = p.data[i + 3];
        // Premultiplied, so an empty pixel is black. Weigh darkness by
        // coverage or the empty corners read as the darkest thing in frame.
        let dark = (a - p.data[i]).max(0.0);
        weight += dark * y as f32;
        total += dark;
    }
    (total > 0.5).then(|| weight / total)
}

/// The outermost column that still has a line in it, and where the line is
/// there. Found rather than assumed, because how far the picture reaches
/// depends on the correction being tested.
fn outermost_line(p: &Plane) -> (u32, f32) {
    for x in 0..p.width / 2 {
        if let Some(row) = line_row(p, x) {
            return (x, row);
        }
    }
    panic!("the line went out of frame entirely");
}

fn alpha(p: &Plane, x: u32, y: u32) -> f32 {
    p.data[(y as usize * p.width as usize + x as usize) * 4 + 3]
}

fn luma(p: &Plane, x: u32, y: u32) -> f32 {
    p.data[(y as usize * p.width as usize + x as usize) * 4]
}

#[test]
fn doing_nothing_changes_nothing() {
    let lens = Lens::default();
    assert!(lens.is_identity());
    let src = ruled(64, 48, 12);
    let out = apply(&src, lens, &Progress::ignored());
    assert_eq!(out.data, src.data, "the identity must be exactly the identity");
}

/// Positive distortion bends a straight line *toward* the centre at the edges
/// — pincushion — which is what corrects the barrel of a wide-angle lens.
/// Negative does the opposite. Measured on a line that does not pass through
/// the centre, because one that does cannot bend.
#[test]
fn distortion_bends_a_straight_line_the_way_it_says() {
    let src = ruled(200, 200, 50); // a quarter of the way down
    let centre_column = 100;

    let pincushion = apply(&src, Lens { distortion: 0.3, ..Default::default() }, &Progress::ignored());
    let mid = line_row(&pincushion, centre_column).expect("the middle keeps its line");
    let (at, edge) = outermost_line(&pincushion);
    assert!(
        edge > mid + 1.0,
        "positive should pull the ends toward the middle of the frame: \
         centre at {mid:.1}, column {at} at {edge:.1}"
    );

    let barrel = apply(&src, Lens { distortion: -0.3, ..Default::default() }, &Progress::ignored());
    let mid = line_row(&barrel, centre_column).expect("the middle keeps its line");
    let (at, edge) = outermost_line(&barrel);
    assert!(
        edge < mid - 1.0,
        "negative should push them away: centre at {mid:.1}, column {at} at {edge:.1}"
    );
}

/// And the line through the centre cannot bend, whatever the setting — a
/// radial distortion that moved it would not be radial.
#[test]
fn the_line_through_the_centre_stays_straight() {
    let src = ruled(200, 200, 100);
    for d in [-0.5, -0.2, 0.2, 0.5] {
        let out = apply(&src, Lens { distortion: d, ..Default::default() }, &Progress::ignored());
        let mid = line_row(&out, 100).expect("the middle keeps its line");
        let (at, edge) = outermost_line(&out);
        assert!(
            (mid - edge).abs() < 0.5,
            "at distortion {d} the centre line moved: {mid:.2} at 100 against \
             {edge:.2} at {at}"
        );
    }
}

/// Rotation empties the corners and keeps the middle, and turns the way it is
/// asked to.
#[test]
fn rotation_leaves_the_corners_empty() {
    let src = white(100, 100);
    let out = apply(&src, Lens { rotation: 20.0, ..Default::default() }, &Progress::ignored());
    assert!(alpha(&out, 50, 50) > 0.99, "the middle should still be there");
    assert!(alpha(&out, 1, 1) < 0.01, "and the corners should not");
    assert!(alpha(&out, 98, 1) < 0.01);
}

#[test]
fn rotation_turns_the_way_it_is_asked_to() {
    // One dark pixel to the right of centre. Turning anticlockwise should
    // carry it upward, so it appears above the middle.
    let mut px = PixelBuffer::filled(101, 101, Rgba8::WHITE);
    px.set(90, 50, Rgba8::BLACK);
    let out = apply(&Plane::from_pixels(&px), Lens { rotation: 30.0, ..Default::default() }, &Progress::ignored());

    // Darkest *covered* pixel: premultiplied, an empty corner is black too,
    // and looking for black without asking about coverage finds the corner.
    let mut found: Option<(u32, u32, f32)> = None;
    for y in 0..out.height {
        for x in 0..out.width {
            let a = alpha(&out, x, y);
            if a < 0.5 {
                continue;
            }
            let tone = luma(&out, x, y) / a;
            if found.is_none_or(|(_, _, best)| tone < best) {
                found = Some((x, y, tone));
            }
        }
    }
    let (x, y, tone) = found.expect("the mark should still be in frame");
    assert!(tone < 0.5, "and should still be dark: {tone:.2}");
    assert!(x < 90 && x > 50, "it should have swung in toward the top: at {x},{y}");
    assert!(y < 50, "and upward: at {x},{y}");
}

/// A keystone leans the frame, so one edge keeps more of itself than the
/// other. Which edge is what the sign chooses.
#[test]
fn a_keystone_leans_the_frame() {
    let src = white(120, 120);
    let out = apply(&src, Lens { perspective_v: 0.3, ..Default::default() }, &Progress::ignored());
    let width_at = |y: u32| (0..120).filter(|&x| alpha(&out, x, y) > 0.99).count();
    let (top, bottom) = (width_at(2), width_at(117));
    assert_ne!(top, bottom, "a keystone that keeps both edges the same is not a keystone");
    assert!(
        top < bottom,
        "positive should narrow the top: {top} against {bottom}"
    );
}

#[test]
fn the_vignette_darkens_or_brightens_only_the_outside() {
    let src = white(100, 100);

    let dark = apply(&src, Lens { vignette: -0.8, ..Default::default() }, &Progress::ignored());
    assert!((luma(&dark, 50, 50) - 1.0).abs() < 0.01, "the middle is left alone");
    assert!(luma(&dark, 1, 1) < 0.5, "and the corner is dimmed");
    assert!(alpha(&dark, 1, 1) > 0.99, "a vignette dims a picture, it does not make holes");

    let bright = apply(&src, Lens { vignette: 0.8, ..Default::default() }, &Progress::ignored());
    assert!(luma(&bright, 1, 1) > luma(&dark, 1, 1), "and it goes the other way too");
}

/// A vignette on a mid-grey field is the clearer test of brightening, since
/// white has nowhere left to go.
#[test]
fn brightening_a_vignette_lifts_the_corners() {
    let grey = Plane::from_pixels(&PixelBuffer::filled(100, 100, Rgba8::opaque(128, 128, 128)));
    let out = apply(&grey, Lens { vignette: 0.6, ..Default::default() }, &Progress::ignored());
    assert!(luma(&out, 1, 1) > luma(&grey, 1, 1) + 0.05, "the corner should be lifted");
    assert!((luma(&out, 50, 50) - luma(&grey, 50, 50)).abs() < 0.01, "the middle untouched");
}

// --- autocrop --------------------------------------------------------------

#[test]
fn a_full_frame_crops_to_itself() {
    let src = white(40, 30);
    let r = largest_opaque_rect(&src);
    assert_eq!((r.x0, r.y0, r.x1, r.y1), (0, 0, 40, 30));
}

#[test]
fn an_empty_frame_crops_to_nothing() {
    let r = largest_opaque_rect(&Plane::new(20, 20));
    assert!(r.is_empty());
}

/// The rectangle a rotation leaves must contain no transparency at all —
/// which is the only thing it actually promises — and must be worth having.
#[test]
fn the_crop_of_a_rotation_holds_no_transparency() {
    let src = white(200, 140);
    let out = apply(&src, Lens { rotation: 12.0, ..Default::default() }, &Progress::ignored());
    let r = largest_opaque_rect(&out);
    assert!(!r.is_empty());

    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            assert!(
                alpha(&out, x as u32, y as u32) > 0.99,
                "the crop kept a transparent pixel at {x},{y}"
            );
        }
    }
    let covered = r.width() as f32 * r.height() as f32 / (200.0 * 140.0);
    assert!(covered > 0.5, "a twelve-degree turn should leave most of the picture: {covered:.2}");
}

/// And it has to find the shape a distortion leaves, which bulges along the
/// middle of each edge rather than at the corners.
#[test]
fn the_crop_of_a_distortion_holds_no_transparency() {
    let src = white(160, 160);
    let out = apply(&src, Lens { distortion: 0.35, ..Default::default() }, &Progress::ignored());
    let r = largest_opaque_rect(&out);
    assert!(!r.is_empty(), "there should be something left");
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            assert!(alpha(&out, x as u32, y as u32) > 0.99, "transparent at {x},{y}");
        }
    }
}

#[test]
fn progress_counts_every_row() {
    let src = white(30, 24);
    let progress = Progress::new();
    apply(&src, Lens { rotation: 5.0, ..Default::default() }, &progress);
    assert_eq!(progress.done(), 24);
    assert_eq!(progress.total(), 24, "the total should be the rows it will do");
    assert_eq!(progress.fraction(), Some(1.0));
}
