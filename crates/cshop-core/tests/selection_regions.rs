//! Bounding a selection edit to the region it can reach must not change it.
//!
//! `feather`, `expand`, `contract`, `border` and `combine` now work over the
//! selection's own extent plus the distance the operation reaches, rather than
//! over the whole canvas. That is only sound if the answer is identical, so
//! these compare the two paths against each other on one canvas.
//!
//! The trick is to force the old path: a selection whose bounds already span
//! the canvas takes the whole-canvas branch, so putting a speck in each corner
//! makes the same shape go the long way round. Everything away from the specks
//! must then come out byte for byte the same.

use cshop_core::geom::{IRect, Vec2};
use cshop_core::selection::{Rectf, Selection, SelectionMode};

const SIDE: u32 = 400;

fn shape() -> Selection {
    Selection::from_rect(
        SIDE,
        SIDE,
        Rectf::from_points(Vec2::new(150.0, 150.0), Vec2::new(250.0, 260.0)),
        true,
    )
}

/// The same shape, but with its bounds forced to the whole canvas.
fn shape_spanning() -> Selection {
    let mut s = shape();
    // The mask is stored only where there is coverage, so make room before
    // writing into the corners of the document.
    s.widen_to_document();
    for (x, y) in [(0, 0), (SIDE as i32 - 1, 0), (0, SIDE as i32 - 1), (SIDE as i32 - 1, SIDE as i32 - 1)] {
        s.mask_mut().set(x, y, 255);
    }
    s.invalidate();
    assert_eq!(s.bounds(), IRect::from_size(SIDE, SIDE), "the specks should span it");
    s
}

/// Compare everywhere except a margin around the corner specks.
fn same_away_from_corners(a: &Selection, b: &Selection, margin: i32, what: &str) {
    let mut differences = 0;
    for y in 0..SIDE as i32 {
        for x in 0..SIDE as i32 {
            let near_corner = (x < margin || x >= SIDE as i32 - margin)
                && (y < margin || y >= SIDE as i32 - margin);
            if near_corner {
                continue;
            }
            if a.coverage(x, y) != b.coverage(x, y) {
                differences += 1;
                if differences == 1 {
                    panic!(
                        "{what}: bounded and whole-canvas disagree at ({x}, {y}): \
                         {} against {}",
                        a.coverage(x, y),
                        b.coverage(x, y)
                    );
                }
            }
        }
    }
}

#[test]
fn feather_is_the_same_either_way() {
    for radius in [1.0, 3.0, 12.0] {
        let (mut bounded, mut whole) = (shape(), shape_spanning());
        bounded.feather(radius);
        whole.feather(radius);
        same_away_from_corners(&bounded, &whole, 60, &format!("feather({radius})"));
    }
}

#[test]
fn expand_and_contract_are_the_same_either_way() {
    for n in [1u32, 5, 20] {
        let (mut bounded, mut whole) = (shape(), shape_spanning());
        bounded.expand(n);
        whole.expand(n);
        same_away_from_corners(&bounded, &whole, 60, &format!("expand({n})"));

        let (mut bounded, mut whole) = (shape(), shape_spanning());
        bounded.contract(n);
        whole.contract(n);
        same_away_from_corners(&bounded, &whole, 60, &format!("contract({n})"));
    }
}

#[test]
fn border_is_the_same_either_way() {
    for n in [1u32, 6, 16] {
        let (mut bounded, mut whole) = (shape(), shape_spanning());
        bounded.border(n);
        whole.border(n);
        same_away_from_corners(&bounded, &whole, 60, &format!("border({n})"));
    }
}

#[test]
fn smooth_is_the_same_either_way() {
    let (mut bounded, mut whole) = (shape(), shape_spanning());
    bounded.smooth(6);
    whole.smooth(6);
    same_away_from_corners(&bounded, &whole, 80, "smooth(6)");
}

/// The bounds must still be right afterwards, or tools skip part of the
/// selection and the marching ants are drawn round the wrong thing.
#[test]
fn the_bounds_are_still_correct_after_a_bounded_edit() {
    let check = |s: &Selection, what: &str| {
        let mut expected = IRect::EMPTY;
        for y in 0..SIDE as i32 {
            for x in 0..SIDE as i32 {
                if s.coverage(x, y) != 0 {
                    expected = expected.union(&IRect::new(x, y, x + 1, y + 1));
                }
            }
        }
        assert_eq!(s.bounds(), expected, "{what}");
    };

    let mut s = shape();
    s.feather(8.0);
    check(&s, "after feather");

    let mut s = shape();
    s.expand(11);
    check(&s, "after expand");

    let mut s = shape();
    s.contract(9);
    check(&s, "after contract");

    let mut s = shape();
    s.border(5);
    check(&s, "after border");
}

#[test]
fn combining_is_the_same_either_way() {
    let other = Selection::from_rect(
        SIDE,
        SIDE,
        Rectf::from_points(Vec2::new(200.0, 200.0), Vec2::new(320.0, 300.0)),
        true,
    );
    for mode in [SelectionMode::Add, SelectionMode::Subtract, SelectionMode::Intersect] {
        let mut bounded = shape();
        let mut whole = shape_spanning();
        bounded.combine(&other, mode);
        whole.combine(&other, mode);
        same_away_from_corners(&bounded, &whole, 3, &format!("{mode:?}"));

        // And the bounds must match what is really there.
        let mut expected = IRect::EMPTY;
        for y in 0..SIDE as i32 {
            for x in 0..SIDE as i32 {
                if bounded.coverage(x, y) != 0 {
                    expected = expected.union(&IRect::new(x, y, x + 1, y + 1));
                }
            }
        }
        assert_eq!(bounded.bounds(), expected, "bounds after {mode:?}");
    }
}

/// A selection stores coverage where it has some, not across the document.
#[test]
fn a_small_selection_on_a_big_canvas_holds_little() {
    let big = 10_000u32;
    let s = Selection::from_rect(
        big,
        big,
        Rectf::from_points(Vec2::new(100.0, 100.0), Vec2::new(300.0, 260.0)),
        true,
    );
    // 200x160 of coverage, not 10000x10000.
    assert_eq!(s.memory_bytes(), 200 * 160, "should hold its own area");
    assert!(
        s.memory_bytes() < big as u64 * big as u64 / 1000,
        "held {} bytes on a {big}x{big} canvas",
        s.memory_bytes()
    );

    // And it must still answer correctly everywhere, including far outside.
    assert_eq!(s.coverage(200, 200), 255);
    assert_eq!(s.coverage(9_000, 9_000), 0);
    assert_eq!(s.coverage(-5, -5), 0);
    assert_eq!(s.bounds(), IRect::new(100, 100, 300, 260));

    // Compressing and restoring keeps both the coverage and the thrift.
    let back = s.compress().restore();
    assert_eq!(back.bounds(), s.bounds());
    assert_eq!(back.memory_bytes(), s.memory_bytes());
    assert_eq!(back.coverage(200, 200), 255);

    // Inverting genuinely does cover the document, and says so.
    let mut inverted = s.clone();
    inverted.invert();
    assert_eq!(inverted.coverage(9_000, 9_000), 255);
    assert_eq!(inverted.memory_bytes(), big as u64 * big as u64);
}
