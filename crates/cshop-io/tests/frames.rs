//! Animations in and out.

use cshop_core::color::Rgba8;
use cshop_core::pixels::PixelBuffer;
use cshop_io::frames::{self, Animation, Frame};

/// Three frames of a square moving right, each a whole picture.
fn moving_square() -> Animation {
    let frames = (0..3)
        .map(|i| {
            let mut px = PixelBuffer::filled(32, 16, Rgba8::opaque(20, 20, 20));
            for y in 4..12 {
                for x in 0..8 {
                    px.set(x + i * 10, y, Rgba8::opaque(220, 40, 40));
                }
            }
            Frame { pixels: px, delay_ms: 120 }
        })
        .collect();
    Animation { frames, loops: 0 }
}

#[test]
fn a_gif_animation_survives_a_round_trip() {
    let out = moving_square();
    let bytes = frames::write_gif(&out, 10).expect("it should write");
    assert!(frames::is_animation(&bytes), "and read back as an animation");
    assert_eq!(frames::frame_count(&bytes), Some(3));

    let back = frames::read(&bytes).expect("it should read");
    assert_eq!(back.frames.len(), 3);
    assert_eq!(back.size(), (32, 16));
    for (i, frame) in back.frames.iter().enumerate() {
        assert_eq!(frame.delay_ms, 120, "the timing is part of the animation");
        // The square is where it was put. GIF quantises to 256 colours, so
        // the check is that it is red and there, not that it is exact.
        let at = frame.pixels.get(2 + i as i32 * 10, 8);
        assert!(at.r > 150 && at.g < 100, "frame {i} has its square: {at:?}");
    }
}

#[test]
fn an_apng_keeps_every_colour_and_every_level_of_transparency() {
    let mut out = moving_square();
    // Something GIF cannot hold: a half-transparent pixel.
    out.frames[0].pixels.set(20, 8, Rgba8::new(10, 200, 90, 128));
    let bytes = frames::write_apng(&out).expect("it should write");
    assert_eq!(frames::frame_count(&bytes), Some(3));

    let back = frames::read(&bytes).expect("it should read");
    assert_eq!(back.frames.len(), 3);
    assert_eq!(back.frames[0].pixels.get(20, 8), Rgba8::new(10, 200, 90, 128));
    assert_eq!(back.frames[0].delay_ms, 120);
}

/// Frames are composed on the way in, so what comes back is what each moment
/// looked like — not the small rectangle that changed.
#[test]
fn a_frame_that_only_changes_a_corner_still_reads_as_a_whole_picture() {
    // Written by hand: a full first frame, then a second that only redraws a
    // small square, with disposal set to leave what is underneath.
    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut bytes, 16, 16, &[]).unwrap();
        encoder.set_repeat(gif::Repeat::Infinite).unwrap();
        let mut whole = vec![0u8; 16 * 16 * 4];
        for p in whole.chunks_exact_mut(4) {
            p.copy_from_slice(&[40, 160, 60, 255]);
        }
        let mut f = gif::Frame::from_rgba_speed(16, 16, &mut whole, 10);
        f.delay = 10;
        encoder.write_frame(&f).unwrap();

        let mut patch = vec![0u8; 4 * 4 * 4];
        for p in patch.chunks_exact_mut(4) {
            p.copy_from_slice(&[220, 30, 30, 255]);
        }
        let mut f2 = gif::Frame::from_rgba_speed(4, 4, &mut patch, 10);
        f2.left = 2;
        f2.top = 2;
        f2.delay = 10;
        f2.dispose = gif::DisposalMethod::Keep;
        encoder.write_frame(&f2).unwrap();
    }

    let back = frames::read(&bytes).expect("it should read");
    assert_eq!(back.frames.len(), 2);
    let second = &back.frames[1];
    assert_eq!(second.pixels.width(), 16, "the frame is the whole picture");
    let patch = second.pixels.get(3, 3);
    assert!(patch.r > 150, "the corner that changed: {patch:?}");
    let rest = second.pixels.get(12, 12);
    assert!(rest.g > 100, "and the rest, left from the frame before: {rest:?}");
}

/// A still picture must not become a one-frame animation, or every PNG in the
/// world grows a timeline.
#[test]
fn a_still_picture_is_not_an_animation() {
    let px = PixelBuffer::filled(8, 8, Rgba8::opaque(100, 100, 100));
    let png = cshop_io::encode(&px, cshop_io::ImageFormat::Png, 92).unwrap();
    assert!(!frames::is_animation(&png));
    assert_eq!(frames::frame_count(&png), Some(1));

    let single = Animation {
        frames: vec![Frame { pixels: px, delay_ms: 100 }],
        loops: 0,
    };
    let gif = frames::write_gif(&single, 10).unwrap();
    assert!(!frames::is_animation(&gif), "one frame is a picture, not an animation");
}

#[test]
fn something_that_is_not_an_animation_says_so() {
    assert!(frames::read(b"not a file at all").is_err());
    assert_eq!(frames::frame_count(b"not a file at all"), None);
}

#[test]
fn an_animation_with_no_frames_is_refused_rather_than_written_empty() {
    let empty = Animation { frames: Vec::new(), loops: 0 };
    assert!(frames::write_gif(&empty, 10).is_err());
    assert!(frames::write_apng(&empty).is_err());
}

/// Thirty years of viewers agree that a zero delay means a tenth of a second,
/// and a file that says zero looks broken without that.
#[test]
fn a_zero_delay_becomes_the_conventional_tenth_of_a_second() {
    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut bytes, 8, 8, &[]).unwrap();
        for _ in 0..2 {
            let mut rgba = vec![255u8; 8 * 8 * 4];
            let mut f = gif::Frame::from_rgba_speed(8, 8, &mut rgba, 10);
            f.delay = 0;
            encoder.write_frame(&f).unwrap();
        }
    }
    let back = frames::read(&bytes).unwrap();
    assert!(back.frames.iter().all(|f| f.delay_ms == 100));
}
