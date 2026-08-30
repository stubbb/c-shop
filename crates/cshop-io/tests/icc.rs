//! Profiles and ink: what comes out of a file, and what goes back in.
//!
//! The fixture is a thirty-two pixel CMYK JPEG, committed because nothing in
//! this program can write one — four-component JPEG is read-only here. The
//! TIFFs are made as the tests run, by the writer being tested.
//!
//! The conversions want a real CMYK profile, which is a system file rather
//! than something to check in at two hundred kilobytes. Where it is missing,
//! those tests step aside; the ones about ink itself still run.

use cshop_core::profile::{Profile, RenderingIntent, Space};

const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/assets");
const CMYK_ICC: &str = "/usr/share/color/icc/ghostscript/default_cmyk.icc";
const WIDE_ICC: &str = "/usr/share/color/icc/colord/WideGamutRGB.icc";

fn asset(name: &str) -> Vec<u8> {
    std::fs::read(format!("{ASSETS}/{name}")).expect("the fixture is checked in")
}

fn press() -> Option<Profile> {
    Profile::load(std::path::Path::new(CMYK_ICC)).ok()
}

/// A CMYK TIFF, written by the code under test, from the fixture's inks.
fn ink_tiff(icc: &[u8]) -> (cshop_io::cmyk::Inks, Vec<u8>) {
    let inks = cshop_io::cmyk::read(&asset("ink.jpg")).unwrap();
    let bytes = cshop_io::cmyk::write_tiff(&inks, icc).unwrap();
    (inks, bytes)
}

#[test]
fn the_profile_comes_out_of_every_container() {
    let Ok(icc) = std::fs::read(WIDE_ICC) else { return };
    let source = image::load_from_memory(&asset("ink-source.png")).unwrap().to_rgb8();

    for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg, image::ImageFormat::Tiff] {
        let mut out = std::io::Cursor::new(Vec::new());
        // Written through `image`, which is what the editor's own encoder
        // uses, so this covers the way profiles actually leave here.
        let mut enc = image::codecs::png::PngEncoder::new(&mut out);
        let bytes = match format {
            image::ImageFormat::Png => {
                use image::ImageEncoder;
                enc.set_icc_profile(icc.clone()).unwrap();
                enc.write_image(&source, 32, 32, image::ExtendedColorType::Rgb8).unwrap();
                out.into_inner()
            }
            _ => continue,
        };
        let found = cshop_io::icc::embedded(&bytes).expect("the profile went in and must come out");
        assert_eq!(found, icc, "{format:?} lost bytes on the way");
        assert_eq!(Profile::parse(&found).unwrap().space(), Space::Rgb);
    }
}

/// Profiles run to hundreds of kilobytes, which is where readers that take
/// shortcuts on the TIFF tag give up.
#[test]
fn a_large_profile_survives_the_tiff_tag() {
    let Ok(icc) = std::fs::read(CMYK_ICC) else { return };
    assert!(icc.len() > 100_000, "the point of this test is that it is big");
    let (_, tif) = ink_tiff(&icc);
    assert_eq!(cshop_io::icc::embedded(&tif).as_deref(), Some(&icc[..]));
}

#[test]
fn rubbish_yields_nothing_rather_than_panicking() {
    assert!(cshop_io::icc::embedded(b"").is_none());
    assert!(cshop_io::icc::embedded(b"\x89PNG\r\n\x1a\n").is_none());
    // A PNG whose chunk claims a length past the end of the file.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&u32::MAX.to_be_bytes());
    png.extend_from_slice(b"iCCP");
    assert!(cshop_io::icc::embedded(&png).is_none());
    // A JPEG that stops in the middle of a segment header.
    assert!(cshop_io::icc::embedded(&[0xFF, 0xD8, 0xFF, 0xE2, 0x00]).is_none());
    // A TIFF pointing its directory into space.
    let mut tif = b"II\x2a\x00".to_vec();
    tif.extend_from_slice(&u32::MAX.to_le_bytes());
    assert!(cshop_io::icc::embedded(&tif).is_none());
}

#[test]
fn ink_files_are_told_from_pictures() {
    assert!(cshop_io::cmyk::is_separated(&asset("ink.jpg")));
    assert!(cshop_io::cmyk::is_separated(&ink_tiff(&[]).1));
    assert!(!cshop_io::cmyk::is_separated(&asset("ink-source.png")));
    assert!(!cshop_io::cmyk::is_separated(b""));
}

/// White paper takes no ink. This is what catches Adobe's inversion being
/// read the wrong way round, which is the easiest mistake to make here and
/// the hardest to see, because the result is merely a dull picture.
#[test]
fn white_is_bare_paper_and_black_is_well_covered() {
    let inks = cshop_io::cmyk::read(&asset("ink.jpg")).unwrap();
    assert_eq!(inks.width, 32);
    assert_eq!(inks.data.len(), 32 * 32 * 4);

    let coverage = |x: usize, y: usize| -> u32 {
        let i = (y * 32 + x) * 4;
        inks.data[i..i + 4].iter().map(|&v| v as u32).sum()
    };
    // The fixture has a white square in one corner and a black one in the other.
    assert!(coverage(3, 3) < 40, "white should be nearly bare: {}", coverage(3, 3));
    assert!(coverage(28, 28) > 300, "black should be laid on: {}", coverage(28, 28));
}

/// Ink in, colour out — and the colour the profile says, not the one the four
/// raw numbers suggest.
#[test]
fn ink_becomes_colour_through_the_profile() {
    let Some(press) = press() else { return };
    let inks = cshop_io::cmyk::read(&asset("ink.jpg")).unwrap();
    let px = press
        .inks_to_rgba8(&Profile::srgb(), &inks.data, RenderingIntent::RelativeColorimetric)
        .unwrap();
    assert_eq!(px.len(), 32 * 32);
    assert!(px.iter().all(|p| p.a == 255), "ink has no transparency");

    let at = |x: usize, y: usize| px[y * 32 + x];
    assert!(at(3, 3).r > 230 && at(3, 3).g > 230, "the white corner: {:?}", at(3, 3));

    // The black corner comes back around 68, not 0, and that is the profile
    // telling the truth: the densest ink this press can lay on paper is a very
    // dark grey, and a screen's black is out of its reach. A conversion that
    // returned 0 here would be one that had ignored the profile.
    let black = at(28, 28);
    assert!(black.r < 90, "the black corner should be dark: {black:?}");
    assert!(black.r > 20, "but not darker than ink goes: {black:?}");
    assert!(
        black.r.abs_diff(black.b) < 12,
        "and it should be neutral rather than cast: {black:?}"
    );
}

#[test]
fn ink_written_out_reads_back_the_same() {
    let icc = std::fs::read(CMYK_ICC).unwrap_or_default();
    let (inks, tif) = ink_tiff(&icc);
    let again = cshop_io::cmyk::read(&tif).unwrap();
    assert_eq!((again.width, again.height), (inks.width, inks.height));
    assert_eq!(again.data, inks.data, "every ink sample, unchanged");
    if !icc.is_empty() {
        assert_eq!(
            cshop_io::icc::embedded(&tif).as_deref(),
            Some(&icc[..]),
            "and it still says which press it was made for"
        );
    }
}
