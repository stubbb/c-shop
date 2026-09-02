//! Scatter and pattern brushes: what turns a line of dabs into a texture.
//!
//! Everything here is measured off the painted pixels rather than off the
//! settings, because the settings are easy to get right and the marks are what
//! anybody actually sees.

use cshop_core::color::Rgba8;
use cshop_core::geom::{IRect, Vec2};
use cshop_core::paint::{Brush, PaintMode, Scatter, Stroke};
use cshop_core::pixels::PixelBuffer;
use cshop_core::tips::TipShape;
use std::sync::Arc;

const W: u32 = 400;
const H: u32 = 200;
/// The y a horizontal test stroke runs along.
const MID: f32 = H as f32 / 2.0;

/// Paint one horizontal stroke and hand back what it put down.
fn painted(brush: Brush, shape: Option<TipShape>) -> PixelBuffer {
    painted_along(brush, shape, &[Vec2::new(40.0, MID), Vec2::new(360.0, MID)])
}

fn painted_along(brush: Brush, shape: Option<TipShape>, path: &[Vec2]) -> PixelBuffer {
    let mut px = PixelBuffer::filled(W, H, Rgba8::TRANSPARENT);
    let mut stroke = Stroke::new(W, H, brush, PaintMode::Paint, Rgba8::opaque(0, 0, 0));
    if let Some(s) = shape {
        stroke = stroke.with_tip(Some(Arc::new(s.tip())));
    }
    // A path of one point is one dab, which `windows(2)` would skip entirely.
    if path.len() == 1 {
        stroke.add_point(path[0]);
    }
    // Walked in small steps, the way a pointer sends samples.
    for pair in path.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let steps = ((b - a).length() / 4.0).ceil().max(1.0) as usize;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            stroke.add_point(a + (b - a) * t);
        }
    }
    stroke.commit(&mut px, None);
    px
}

/// Every pixel the stroke actually marked.
fn marked(px: &PixelBuffer) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    for y in 0..px.height() as i32 {
        for x in 0..px.width() as i32 {
            if px.get(x, y).a > 8 {
                out.push((x, y));
            }
        }
    }
    out
}

/// The bounding box of the marks lying inside a band of rows.
///
/// The first dab of a stroke is laid before the stroke has a direction, so a
/// test about direction has to look at the part of the stroke that has one.
fn bounds_between(px: &PixelBuffer, y0: i32, y1: i32) -> IRect {
    let mut b = IRect::EMPTY;
    for (x, y) in marked(px) {
        if y >= y0 && y < y1 {
            b = b.union(&IRect::new(x, y, x + 1, y + 1));
        }
    }
    b
}

fn bounds_of(px: &PixelBuffer) -> IRect {
    let mut b = IRect::EMPTY;
    for (x, y) in marked(px) {
        b = b.union(&IRect::new(x, y, x + 1, y + 1));
    }
    b
}

/// The furthest any mark strayed from the line the stroke was drawn along.
fn furthest_from_the_line(px: &PixelBuffer) -> f32 {
    marked(px).iter().map(|(_, y)| (*y as f32 + 0.5 - MID).abs()).fold(0.0, f32::max)
}

fn scattered(spread: f32, count: u32) -> Brush {
    Brush {
        size: 20.0,
        spacing: 0.5,
        scatter: Scatter { spread, count, ..Scatter::default() },
        ..Brush::default()
    }
}

/// The whole feature has to be invisible until it is asked for: a brush that
/// knows nothing about scattering must draw the line it always drew.
#[test]
fn a_brush_left_alone_still_draws_a_line() {
    assert!(!Scatter::default().active(), "the default settings do nothing");

    let px = painted(Brush { size: 20.0, ..Brush::default() }, None);
    let reach = furthest_from_the_line(&px);
    // A 20px brush reaches 10 either side; its soft edge fades out just
    // short of that, so a hair under is right and anything over is not.
    assert!(
        (9.0..=11.0).contains(&reach),
        "an unscattered stroke should reach a radius from the line, not {reach}"
    );
}

/// Scatter is the whole point: the marks have to leave the line.
#[test]
fn scatter_throws_the_marks_off_the_line() {
    let plain = furthest_from_the_line(&painted(scattered(0.0, 1), None));
    let thrown = furthest_from_the_line(&painted(scattered(1.5, 4), None));
    assert!(
        thrown > plain * 2.0,
        "scattered marks should be well clear of the line: {thrown} against {plain}"
    );
}

/// And it has to stay where it was sent. A scatter that wanders further than
/// it was told is one that paints outside whatever the user was looking at.
#[test]
fn scatter_stays_inside_what_it_was_asked_for() {
    let size = 20.0;
    for spread in [0.5f32, 1.0, 2.5] {
        let brush = Brush {
            size,
            spacing: 0.4,
            scatter: Scatter { spread, count: 6, ..Scatter::default() },
            ..Brush::default()
        };
        let reach = furthest_from_the_line(&painted(brush, None));
        // Thrown up to `spread` sizes, and the dab itself is a radius wide.
        let ceiling = spread * size + size / 2.0 + 1.5;
        assert!(reach <= ceiling, "spread {spread} reached {reach}, past {ceiling}");
        assert!(reach > spread * size * 0.4, "spread {spread} barely moved: {reach}");
    }
}

/// Count is how many stamps land at each step, so more of them means more ink.
#[test]
fn count_lays_down_more_marks() {
    let one = marked(&painted(scattered(1.0, 1), None)).len();
    let many = marked(&painted(scattered(1.0, 8), None)).len();
    assert!(many > one * 2, "eight stamps a step should cover far more than one: {many} vs {one}");
}

/// Jitter only ever takes size away, so that the size slider keeps describing
/// the largest mark the brush can make.
#[test]
fn size_jitter_only_makes_marks_smaller() {
    let base = Brush {
        size: 24.0,
        spacing: 0.6,
        scatter: Scatter { count: 3, ..Scatter::default() },
        ..Brush::default()
    };
    let jittery = Brush { scatter: Scatter { size_jitter: 0.8, ..base.scatter }, ..base };

    // Never wider: the size setting still describes the largest possible mark.
    let steady_reach = furthest_from_the_line(&painted(base, None));
    let jittered_reach = furthest_from_the_line(&painted(jittery, None));
    assert!(
        jittered_reach <= steady_reach + 0.01,
        "a jittered stroke should never be wider than an unjittered one: \
         {jittered_reach} vs {steady_reach}"
    );

    // Measured in ink rather than in width, because with several stamps a step
    // one of them is nearly always close to full size and the widest mark
    // therefore says nothing about the rest.
    let steady_ink = marked(&painted(base, None)).len();
    let jittered_ink = marked(&painted(jittery, None)).len();
    assert!(
        jittered_ink < steady_ink,
        "heavy jitter should lay down visibly less ink: {jittered_ink} vs {steady_ink}"
    );
}

/// Scale sizes the stamp within its slot, which is what makes a scatter brush
/// look like scattered objects rather than a thick line.
#[test]
fn scale_sizes_the_stamp_without_moving_the_slots() {
    let base = Brush { size: 30.0, spacing: 0.5, ..Brush::default() };
    let full = marked(&painted(base, Some(TipShape::Dot))).len();
    let small = marked(&painted(
        Brush { scatter: Scatter { scale: 0.3, ..Scatter::default() }, ..base },
        Some(TipShape::Dot),
    ))
    .len();
    assert!(small * 3 < full, "a third-size stamp should cover far less: {small} vs {full}");
}

/// A turned stamp has to actually be turned. A line lying flat is wide and
/// short; the same line stood on end is tall and narrow.
#[test]
fn angle_turns_the_stamp() {
    // One dab, so the measurement is of the stamp and not of the stroke.
    let one_dab = |angle: f32| {
        let brush = Brush {
            size: 60.0,
            scatter: Scatter { angle, ..Scatter::default() },
            ..Brush::default()
        };
        bounds_of(&painted_along(brush, Some(TipShape::Line), &[Vec2::new(200.0, MID)]))
    };
    let flat = one_dab(0.0);
    let upright = one_dab(90.0);
    assert!(
        flat.width() > flat.height() * 2,
        "a line at zero should lie flat: {}x{}",
        flat.width(),
        flat.height()
    );
    assert!(
        upright.height() > upright.width() * 2,
        "a line at ninety should stand up: {}x{}",
        upright.width(),
        upright.height()
    );
}

/// Following the stroke means the same brush makes a different mark depending
/// on which way it is going — which is what hatching and fur need.
#[test]
fn the_angle_can_follow_the_stroke() {
    let brush = Brush {
        size: 40.0,
        spacing: 0.2,
        scatter: Scatter { follow: true, ..Scatter::default() },
        ..Brush::default()
    };
    // A stroke straight down the picture. Only the far end is measured: the
    // first dab is laid before the stroke has a direction to follow, and the
    // stamp is longer than the few pixels the stroke has travelled by then.
    let down = painted_along(
        brush,
        Some(TipShape::Line),
        &[Vec2::new(200.0, 40.0), Vec2::new(200.0, 160.0)],
    );
    let tail = bounds_between(&down, 120, 200);
    assert!(
        tail.height() > tail.width() * 2,
        "a followed line should stand up when the stroke goes down: {}x{}",
        tail.width(),
        tail.height()
    );

    // The same brush going across lies flat, so the mark is the stroke's and
    // not a fixed shape that happens to suit one direction.
    let across = painted_along(
        brush,
        Some(TipShape::Line),
        &[Vec2::new(60.0, MID), Vec2::new(340.0, MID)],
    );
    let band = bounds_between(&across, 0, H as i32);
    assert!(
        band.height() < tail.height(),
        "and lie flat when it goes across: {} against {}",
        band.height(),
        tail.height()
    );
}

/// The scatter is random, and it also has to be the same random every time.
///
/// A stroke is re-rendered on every frame of its own preview and again when it
/// is undone and redone. If the randomness moved, the picture would change
/// under the pointer and an undo would give back something else.
#[test]
fn the_same_stroke_scatters_the_same_way_twice() {
    let brush = scattered(1.5, 4);
    let once = painted(brush, Some(TipShape::Star));
    let twice = painted(brush, Some(TipShape::Star));
    assert_eq!(once.pixels(), twice.pixels(), "the same stroke should scatter identically");
}

/// But two strokes are not the same stroke, or a spray brush would stencil the
/// identical spatter everywhere it was used.
#[test]
fn strokes_in_different_places_scatter_differently() {
    let brush = scattered(1.5, 4);
    let here = painted_along(brush, None, &[Vec2::new(40.0, MID), Vec2::new(200.0, MID)]);
    let there = painted_along(brush, None, &[Vec2::new(41.0, MID + 7.0), Vec2::new(201.0, MID + 7.0)]);
    // Compare the shape of the mark rather than where it is: shifted back, an
    // identical spatter would line up exactly.
    let a: Vec<_> = marked(&here);
    let b: Vec<_> = marked(&there).iter().map(|(x, y)| (x - 1, y - 7)).collect();
    assert_ne!(a, b, "two strokes should not lay down the identical pattern");
}

/// Every shape has to draw something, and something different from the others.
#[test]
fn each_shape_stamps_its_own_mark() {
    let brush = Brush { size: 60.0, ..Brush::default() };
    let mut seen: Vec<(TipShape, Vec<(i32, i32)>)> = Vec::new();
    for shape in TipShape::ALL {
        let px = painted_along(brush, Some(shape), &[Vec2::new(200.0, MID)]);
        let marks = marked(&px);
        assert!(!marks.is_empty(), "{} stamped nothing", shape.name());
        for (other, previous) in &seen {
            assert_ne!(
                &marks,
                previous,
                "{} and {} stamp the same mark",
                shape.name(),
                other.name()
            );
        }
        seen.push((shape, marks));
    }
}

/// A dot is a dot whichever way up it is, so the editor does not offer to turn
/// one — and the shapes that do have a direction say so.
#[test]
fn only_the_shapes_with_a_direction_claim_one() {
    assert!(!TipShape::Dot.has_direction());
    assert!(!TipShape::Bubble.has_direction());
    assert!(TipShape::Star.has_direction());
    assert!(TipShape::Line.has_direction());
}

/// These settings arrive from an editable file as well as from a slider, and a
/// count multiplies the cost of every dab in the stroke.
#[test]
fn absurd_settings_are_brought_back_into_range() {
    let wild = Scatter {
        spread: f32::INFINITY,
        count: u32::MAX,
        scale: f32::NAN,
        size_jitter: 50.0,
        angle: f32::INFINITY,
        follow: true,
    }
    .sane();
    assert!(wild.spread.is_finite() && wild.spread <= Scatter::MAX_SPREAD);
    assert!(wild.count <= Scatter::MAX_COUNT && wild.count >= 1);
    assert!(wild.scale.is_finite() && wild.scale <= Scatter::MAX_SCALE);
    assert!((0.0..=1.0).contains(&wild.size_jitter));
    assert!(wild.angle.is_finite());

    // And a sane one is left exactly as it was.
    let ordinary = Scatter { spread: 1.0, count: 4, scale: 0.5, size_jitter: 0.3, angle: 45.0, follow: false };
    assert_eq!(ordinary.sane(), ordinary);
    assert_eq!(Scatter::default().sane(), Scatter::default());
}
