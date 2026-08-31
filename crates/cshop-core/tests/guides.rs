//! Guides, and the arithmetic of catching on them.

use cshop_core::geom::{IRect, Vec2};
use cshop_core::guides::{ruler_step, snap_offset, snap_point, Guide, SnapLines};

#[test]
fn a_point_catches_the_line_it_is_near_and_no_other() {
    let mut lines = SnapLines::default();
    lines.add_guides(&[Guide::vertical(100.0), Guide::horizontal(50.0)]);

    // Within reach on both axes.
    let snapped = snap_point(Vec2::new(103.0, 47.0), &lines, 6.0);
    assert_eq!((snapped.x, snapped.y), (100.0, 50.0));

    // Out of reach on both: left exactly alone.
    let free = snap_point(Vec2::new(120.0, 20.0), &lines, 6.0);
    assert_eq!((free.x, free.y), (120.0, 20.0));
}

/// Each axis decides for itself, so an edge can line up without the other
/// dimension being dragged along with it.
#[test]
fn the_two_axes_are_independent() {
    let mut lines = SnapLines::default();
    lines.add_guides(&[Guide::vertical(100.0)]);
    let snapped = snap_point(Vec2::new(102.0, 300.0), &lines, 6.0);
    assert_eq!(snapped.x, 100.0, "it caught the guide");
    assert_eq!(snapped.y, 300.0, "and nothing happened to the other axis");
}

#[test]
fn the_nearest_line_wins() {
    let mut lines = SnapLines::default();
    lines.add_guides(&[Guide::vertical(100.0), Guide::vertical(104.0)]);
    assert_eq!(snap_point(Vec2::new(103.0, 0.0), &lines, 6.0).x, 104.0);
    assert_eq!(snap_point(Vec2::new(101.0, 0.0), &lines, 6.0).x, 100.0);
}

/// A rectangle is moved, never stretched, and catches by whichever of its
/// edges comes closest — which is what someone aligning a picture is doing.
#[test]
fn a_rectangle_moves_by_whichever_edge_catches() {
    let mut lines = SnapLines::default();
    lines.add_guides(&[Guide::vertical(200.0)]);

    // Its right edge is near the guide, so the whole thing shifts right by 3.
    let shift = snap_offset(IRect::new(50, 10, 197, 90), &lines, 6.0);
    assert_eq!(shift.x, 3.0);
    assert_eq!(shift.y, 0.0, "nothing horizontal to catch on");

    // Its left edge is near it instead: the same rule, the other edge.
    let shift = snap_offset(IRect::new(203, 10, 400, 90), &lines, 6.0);
    assert_eq!(shift.x, -3.0);
}

/// The middle of a rectangle counts as an edge, which is how a thing gets
/// centred on a guide rather than aligned to it.
#[test]
fn the_middle_of_a_rectangle_catches_too() {
    let mut lines = SnapLines::default();
    lines.add_guides(&[Guide::vertical(100.0)]);
    // Centre is at 98, two away; the edges are 40 and 60 away.
    let shift = snap_offset(IRect::new(58, 0, 138, 20), &lines, 6.0);
    assert_eq!(shift.x, 2.0);
}

#[test]
fn a_document_offers_its_own_edges_and_middle() {
    let lines = SnapLines::for_document(800, 600);
    assert_eq!(lines.vertical, vec![0.0, 400.0, 800.0]);
    assert_eq!(lines.horizontal, vec![0.0, 300.0, 600.0]);
}

/// A grid is generated near the point rather than listed: a ten pixel grid on
/// a large document is tens of thousands of lines and only the few nearby can
/// possibly win.
#[test]
fn a_grid_is_only_generated_where_it_could_matter() {
    let mut lines = SnapLines::default();
    lines.add_grid(10.0, Vec2::new(1000.0, 1000.0), 25.0);
    assert!(lines.vertical.len() < 12, "not the whole document: {}", lines.vertical.len());
    assert!(lines.vertical.iter().any(|v| (*v - 1000.0).abs() < 0.01));
    assert!(lines.vertical.iter().all(|v| (v - 1000.0).abs() <= 35.0));
    // And it still catches.
    assert_eq!(snap_point(Vec2::new(1002.0, 1008.0), &lines, 6.0), Vec2::new(1000.0, 1010.0));
}

#[test]
fn a_grid_of_nothing_is_no_grid() {
    let mut lines = SnapLines::default();
    lines.add_grid(0.0, Vec2::new(10.0, 10.0), 50.0);
    assert!(lines.is_empty());
}

/// Ruler ticks step through one, two and five and their powers of ten, which
/// is what every ruler has always done: halves and fifths read without being
/// labelled.
#[test]
fn ruler_ticks_are_round_numbers() {
    for zoom in [0.05f32, 0.1, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0] {
        let step = ruler_step(zoom);
        let mantissa = step / 10f32.powf(step.log10().floor());
        assert!(
            [1.0, 2.0, 5.0].iter().any(|m| (mantissa - m).abs() < 0.01),
            "at zoom {zoom} the step is {step}, whose mantissa is {mantissa}"
        );
        // And it lands somewhere readable on screen.
        let on_screen = step * zoom;
        assert!(
            (20.0..400.0).contains(&on_screen),
            "at zoom {zoom} a tick is {on_screen} screen pixels apart"
        );
    }
}

/// Zooming in makes the ticks finer, never coarser.
#[test]
fn ticks_get_finer_as_the_picture_gets_bigger() {
    let mut last = f32::MAX;
    for zoom in [0.1f32, 0.5, 1.0, 4.0, 16.0] {
        let step = ruler_step(zoom);
        assert!(step <= last, "at zoom {zoom} the step grew to {step}");
        last = step;
    }
}
