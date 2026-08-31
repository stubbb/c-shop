//! Malformed input, on every path that reads bytes from outside the program.
//!
//! Not a fuzzer, but the same idea: take something well-formed, damage it in
//! every way that is cheap to describe, and require an error rather than a
//! panic. A panic in a decoder is a denial of service for the server and a
//! crash for everyone else.

const CMYK_ICC: &str = "/usr/share/color/icc/ghostscript/default_cmyk.icc";
const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/assets");

/// An ICC profile is attacker-controlled data handed to a parser this project
/// did not write. It arrives from any PNG, JPEG or TIFF anyone opens.
#[test]
fn a_damaged_colour_profile_is_refused_not_fatal() {
    let Ok(good) = std::fs::read(CMYK_ICC) else { return };
    use cshop_core::profile::Profile;

    assert!(Profile::parse(&[]).is_err());
    assert!(Profile::parse(b"not a profile").is_err());

    // Truncated at every length.
    for cut in (0..good.len()).step_by(1021) {
        let _ = Profile::parse(&good[..cut]);
    }
    // Flipped bytes throughout, including the header that says how long it is
    // and where its tags are.
    for at in (0..good.len()).step_by(997) {
        let mut bad = good.clone();
        bad[at] ^= 0xa5;
        let _ = Profile::parse(&bad);
    }
    // A header claiming a wildly wrong size.
    let mut lying = good.clone();
    lying[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
    let _ = Profile::parse(&lying);
}

/// The same profile, but reached the way a file actually delivers one: inside
/// a project chunk that anyone can hand over.
#[test]
fn a_project_carrying_a_damaged_profile_still_opens() {
    let Ok(icc) = std::fs::read(CMYK_ICC) else { return };
    let mut doc = cshop_core::document::Document::new(
        "t",
        8,
        8,
        cshop_core::document::Background::White,
    );
    // A profile that will not parse, written into a project as if it would.
    doc.profile = cshop_core::profile::Profile::parse(&icc).unwrap();
    let good = cshop_io::project::write(&doc);

    for at in (0..good.len()).step_by(503) {
        let mut bad = good.clone();
        bad[at] ^= 0x5a;
        // Must not panic. Refusing is fine; opening in sRGB is fine.
        let _ = cshop_io::project::read(&bad);
    }
    for cut in (0..good.len()).step_by(1013) {
        let _ = cshop_io::project::read(&good[..cut]);
    }
}

/// Ink files come from other people's presses and other people's software.
#[test]
fn a_damaged_ink_file_is_refused_not_fatal() {
    let good = std::fs::read(format!("{ASSETS}/ink.jpg")).expect("the fixture");

    assert!(cshop_io::cmyk::read(&[]).is_err());
    for cut in (0..good.len()).step_by(37) {
        let _ = cshop_io::cmyk::read(&good[..cut]);
        let _ = cshop_io::cmyk::is_separated(&good[..cut]);
    }
    for at in (0..good.len()).step_by(53) {
        let mut bad = good.clone();
        bad[at] ^= 0xff;
        let _ = cshop_io::cmyk::read(&bad);
        let _ = cshop_io::cmyk::is_separated(&bad);
    }
}

/// And the deep path, which reads sixteen-bit samples out of the same
/// containers.
#[test]
fn a_damaged_deep_file_is_refused_not_fatal() {
    let srgb = cshop_core::profile::Profile::srgb();
    let deep = cshop_core::pixels::DeepBuffer::new(16, 16);
    let good =
        cshop_io::encode_deep(&deep, cshop_io::format::ImageFormat::Png, &srgb, &srgb).unwrap();

    for cut in (0..good.len()).step_by(11) {
        let _ = cshop_io::decode_deep(&good[..cut], None, &srgb);
        let _ = cshop_io::is_deep(&good[..cut], None);
    }
    for at in (0..good.len()).step_by(17) {
        let mut bad = good.clone();
        bad[at] ^= 0x3c;
        let _ = cshop_io::decode_deep(&bad, None, &srgb);
    }
}

/// The container scanners walk offsets that the file itself supplies, which is
/// the shape of bug that reads past the end of a buffer.
#[test]
fn hostile_containers_yield_nothing_rather_than_panicking() {
    let mut cases: Vec<Vec<u8>> = vec![
        b"\x89PNG\r\n\x1a\n".to_vec(),
        vec![0xFF, 0xD8],
        b"II\x2a\x00".to_vec(),
        b"MM\x00\x2a".to_vec(),
    ];
    // Every length claim, including the ones that overflow.
    for len in [0u32, 1, 12, u32::MAX, u32::MAX - 8] {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&len.to_be_bytes());
        png.extend_from_slice(b"iCCP");
        png.extend_from_slice(&[0u8; 8]);
        cases.push(png);

        let mut tif = b"II\x2a\x00".to_vec();
        tif.extend_from_slice(&len.to_le_bytes());
        tif.extend_from_slice(&[0xff; 16]);
        cases.push(tif);
    }
    // A JPEG whose segment lengths point backwards and off the end.
    for len in [0u16, 1, 2, u16::MAX] {
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE2];
        jpg.extend_from_slice(&len.to_be_bytes());
        jpg.extend_from_slice(b"ICC_PROFILE\0\x01\x01somebytes");
        cases.push(jpg);
    }
    for case in cases {
        let _ = cshop_io::icc::embedded(&case);
        let _ = cshop_io::cmyk::is_separated(&case);
    }
}
