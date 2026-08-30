//! A layer read as a mask.

use cshop_core::color::Rgba8;
use cshop_core::mask::MaskBuffer;
use cshop_core::pixels::PixelBuffer;

#[test]
fn bright_reveals_and_dark_hides() {
    let mut px = PixelBuffer::new(4, 1);
    px.set(0, 0, Rgba8::WHITE);
    px.set(1, 0, Rgba8::BLACK);
    px.set(2, 0, Rgba8::opaque(128, 128, 128));
    px.set(3, 0, Rgba8::opaque(255, 255, 255));
    let m = MaskBuffer::from_luminance(&px);
    assert_eq!(m.get(0, 0), 255);
    assert_eq!(m.get(1, 0), 0);
    assert!((m.get(2, 0) as i32 - 128).abs() <= 1);
    assert_eq!(m.get(3, 0), 255);
}

/// Luminance rather than a plain average, so a mask painted in colour behaves
/// the way the eye reads that colour's brightness.
#[test]
fn colour_is_read_by_how_bright_it_looks() {
    let mut px = PixelBuffer::new(3, 1);
    px.set(0, 0, Rgba8::opaque(255, 0, 0));
    px.set(1, 0, Rgba8::opaque(0, 255, 0));
    px.set(2, 0, Rgba8::opaque(0, 0, 255));
    let m = MaskBuffer::from_luminance(&px);
    let (r, g, b) = (m.get(0, 0), m.get(1, 0), m.get(2, 0));
    assert!(g > r && r > b, "green reads brightest and blue darkest: {g}, {r}, {b}");
    // A plain average would have made all three the same.
    assert_ne!(r, g);
}

/// A greyscale picture on transparency should mask only where it actually is.
/// Without weighting by alpha, its empty part — black and invisible — would
/// read as "hide", which is right by accident for black and wrong otherwise.
#[test]
fn what_is_not_there_does_not_mask() {
    let mut px = PixelBuffer::new(2, 1);
    px.set(0, 0, Rgba8::new(255, 255, 255, 255));
    px.set(1, 0, Rgba8::new(255, 255, 255, 0));
    let m = MaskBuffer::from_luminance(&px);
    assert_eq!(m.get(0, 0), 255, "white and present reveals");
    assert_eq!(m.get(1, 0), 0, "white but absent does not");
}
