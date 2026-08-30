//! Sixteen bits a channel, in and out of files.
//!
//! The question every one of these asks is the same: did the extra bits
//! survive, or did something on the way quietly round them off?

use cshop_core::color::{Rgba16, Rgba8};
use cshop_core::pixels::DeepBuffer;
use cshop_core::profile::Profile;
use cshop_io::format::ImageFormat;

const CMYK_ICC: &str = "/usr/share/color/icc/ghostscript/default_cmyk.icc";

/// A gradient with values that eight bits cannot hold: each step is one
/// sixteen-bit count, so narrowing anywhere collapses them together.
fn fine_gradient() -> DeepBuffer {
    let data: Vec<Rgba16> = (0..256u32)
        .map(|i| Rgba16::new(30000 + i as u16, 40000 - i as u16, 1000 + i as u16 * 7, 65535))
        .collect();
    DeepBuffer::from_pixels(16, 16, data).unwrap()
}

#[test]
fn a_deep_png_keeps_every_count() {
    let srgb = Profile::srgb();
    let deep = fine_gradient();
    let bytes = cshop_io::encode_deep(&deep, ImageFormat::Png, &srgb, &srgb).unwrap();
    assert!(cshop_io::is_deep(&bytes, None), "what came out should still be deep");

    let (back, _) = cshop_io::decode_deep(&bytes, None, &srgb).unwrap();
    assert_eq!(back.pixels(), deep.pixels(), "every sample, unchanged");
}

#[test]
fn a_deep_tiff_keeps_every_count() {
    let srgb = Profile::srgb();
    let deep = fine_gradient();
    let bytes = cshop_io::encode_deep(&deep, ImageFormat::Tiff, &srgb, &srgb).unwrap();
    let (back, _) = cshop_io::decode_deep(&bytes, None, &srgb).unwrap();
    assert_eq!(back.pixels(), deep.pixels());
}

/// The point of the depth, stated as a file: written deep and read back at
/// eight bits, those distinct values collapse.
#[test]
fn reading_a_deep_file_at_eight_bits_is_where_it_is_lost() {
    let srgb = Profile::srgb();
    let bytes = cshop_io::encode_deep(&fine_gradient(), ImageFormat::Png, &srgb, &srgb).unwrap();

    let (shallow, _) = cshop_io::decode_managed(&bytes, None, &srgb).unwrap();
    let distinct: std::collections::HashSet<_> = shallow.pixels().iter().collect();
    assert!(
        distinct.len() < 200,
        "256 deep values should not survive as 256 eight-bit ones, but {} did",
        distinct.len()
    );
}

#[test]
fn an_eight_bit_file_opens_deep_without_inventing_anything() {
    let srgb = Profile::srgb();
    let mut shallow = cshop_core::pixels::PixelBuffer::new(4, 4);
    for (i, px) in shallow.pixels_mut().iter_mut().enumerate() {
        *px = Rgba8::new(i as u8 * 16, 255 - i as u8 * 16, 7, 255);
    }
    let bytes = cshop_io::encode(&shallow, ImageFormat::Png, 92).unwrap();

    let (deep, _) = cshop_io::decode_deep(&bytes, None, &srgb).unwrap();
    assert_eq!(deep.to_eight().pixels(), shallow.pixels(), "narrowing must give it back exactly");
}

/// The formats that cannot hold it say so rather than narrowing in silence.
#[test]
fn a_format_that_cannot_hold_the_depth_refuses() {
    let srgb = Profile::srgb();
    let err = cshop_io::encode_deep(&fine_gradient(), ImageFormat::Jpeg, &srgb, &srgb).unwrap_err();
    assert!(format!("{err}").contains("sixteen bits"), "{err}");
}

#[test]
fn ink_can_be_deep_too() {
    let Ok(press) = Profile::load(std::path::Path::new(CMYK_ICC)) else { return };
    let srgb = Profile::srgb();
    let deep = fine_gradient();

    let bytes = cshop_io::encode_deep(&deep, ImageFormat::Tiff, &srgb, &press).unwrap();
    assert!(cshop_io::cmyk::is_separated(&bytes), "it should be ink");
    let inks = cshop_io::cmyk::read(&bytes).unwrap();
    assert!(inks.deep.is_some(), "and deep ink at that");
    assert_eq!(inks.deep.as_ref().unwrap().len(), 16 * 16 * 4);
    assert_eq!(
        cshop_io::icc::embedded(&bytes).as_deref(),
        Some(press.bytes()),
        "carrying the press it was made for"
    );

    // And back to colour, deep the whole way.
    let (back, colors) = cshop_io::decode_deep(&bytes, None, &srgb).unwrap();
    assert!(colors.separated && colors.converted);
    assert_eq!(back.width(), 16);
}
