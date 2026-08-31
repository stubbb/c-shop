//! Profiles: naming them, assigning them, and converting through them.

use cshop_core::color::Rgba8;
use cshop_core::profile::{Profile, RenderingIntent, Space};

const GS: &str = "/usr/share/color/icc/ghostscript";
const COLORD: &str = "/usr/share/color/icc/colord";

fn maybe(path: &str) -> Option<Profile> {
    Profile::load(std::path::Path::new(path)).ok()
}

#[test]
fn the_default_profile_is_srgb_and_names_itself() {
    let p = Profile::srgb();
    assert_eq!(p.space(), Space::Rgb);
    assert_eq!(p.name(), "sRGB");
    assert!(p.is_srgb());
    assert!(!p.bytes().is_empty(), "it has to be embeddable in a file");
    // An encoded profile starts with its own length.
    let len = u32::from_be_bytes(p.bytes()[0..4].try_into().unwrap()) as usize;
    assert_eq!(len, p.bytes().len(), "the ICC header counts the whole profile");
}

#[test]
fn rubbish_is_refused_rather_than_guessed_at() {
    assert!(Profile::parse(b"not a profile").is_err());
    assert!(Profile::parse(&[]).is_err());
}

#[test]
fn a_profile_from_disk_carries_its_name_and_space() {
    let Some(cmyk) = maybe(&format!("{GS}/default_cmyk.icc")) else { return };
    assert_eq!(cmyk.space(), Space::Cmyk);
    assert!(!cmyk.name().is_empty());
    assert!(!cmyk.is_srgb());
}

/// Converting to the profile you are already in must not touch a single pixel.
#[test]
fn converting_to_the_same_space_changes_nothing() {
    let srgb = Profile::srgb();
    let mut px = vec![Rgba8::new(220, 175, 143, 200), Rgba8::new(0, 0, 0, 0)];
    let before = px.clone();
    srgb.convert_rgba8(&srgb, &mut px, RenderingIntent::Perceptual).unwrap();
    assert_eq!(px, before);
}

/// Ordinary colours must survive a trip through another space and back.
#[test]
fn a_round_trip_through_a_wider_space_survives() {
    let Some(wide) = maybe(&format!("{COLORD}/WideGamutRGB.icc")) else { return };
    let srgb = Profile::srgb();
    // A sweep through the middle of the gamut: skin, foliage, sky, grey.
    let original: Vec<Rgba8> = (0..64)
        .map(|i| Rgba8::new(60 + (i * 2) as u8, 90 + i as u8, 120 - i as u8, 255))
        .collect();

    let mut there = original.clone();
    srgb.convert_rgba8(&wide, &mut there, RenderingIntent::RelativeColorimetric).unwrap();
    assert_ne!(there, original, "a different space must mean different numbers");

    let mut back = there.clone();
    wide.convert_rgba8(&srgb, &mut back, RenderingIntent::RelativeColorimetric).unwrap();

    let worst = original
        .iter()
        .zip(&back)
        .flat_map(|(a, b)| {
            [a.r as i32 - b.r as i32, a.g as i32 - b.g as i32, a.b as i32 - b.b as i32]
        })
        .map(i32::abs)
        .max()
        .unwrap();
    assert!(worst <= 2, "the colours should come home again, off by {worst}");
}

/// And the saturated end must not, which is worth pinning down rather than
/// discovering later.
///
/// A wide space spends its 256 steps over a much larger volume, so sRGB's
/// corners land where its own numbers are the small difference between large
/// ones. Around a saturated green, one count of wide-gamut red is worth more
/// than twenty counts of sRGB red: the trip out quantises, and the trip back
/// multiplies what quantising cost. Nothing is wrong with the transform. Eight
/// bits a channel is simply not enough to hold the journey, and this is the
/// measurement that says so.
#[test]
fn eight_bits_cannot_hold_a_wide_round_trip_at_the_edges() {
    let Some(wide) = maybe(&format!("{COLORD}/WideGamutRGB.icc")) else { return };
    let srgb = Profile::srgb();
    let original = vec![Rgba8::new(16, 243, 8, 255), Rgba8::new(20, 240, 10, 255)];

    let mut there = original.clone();
    srgb.convert_rgba8(&wide, &mut there, RenderingIntent::RelativeColorimetric).unwrap();
    // One count apart in the wide space...
    assert_eq!(there[0].r as i32 - there[1].r as i32, 1);

    let mut back = there.clone();
    wide.convert_rgba8(&srgb, &mut back, RenderingIntent::RelativeColorimetric).unwrap();
    // ...and worlds apart coming home.
    let spread = (back[0].r as i32 - back[1].r as i32).abs();
    assert!(spread > 10, "the amplification is the point; it was only {spread}");
}

/// Alpha is coverage, not colour. A transform must leave it exactly alone.
#[test]
fn alpha_is_not_a_colour_channel() {
    let Some(wide) = maybe(&format!("{COLORD}/WideGamutRGB.icc")) else { return };
    let mut px: Vec<Rgba8> = (0..=255u8).map(|a| Rgba8::new(200, 100, 50, a)).collect();
    Profile::srgb().convert_rgba8(&wide, &mut px, RenderingIntent::Perceptual).unwrap();
    for (i, p) in px.iter().enumerate() {
        assert_eq!(p.a as usize, i, "alpha {i} came back as {}", p.a);
    }
}

#[test]
fn ink_goes_to_colour_and_back_again() {
    let Some(cmyk) = maybe(&format!("{GS}/default_cmyk.icc")) else { return };
    let srgb = Profile::srgb();
    // Mid greys and a few saturated colours: the parts of the gamut a generic
    // press profile can actually hold, so the trip should be close.
    let original = vec![
        Rgba8::new(128, 128, 128, 255),
        Rgba8::new(200, 60, 60, 255),
        Rgba8::new(60, 140, 90, 255),
        Rgba8::new(240, 240, 240, 255),
    ];
    let inks = srgb.rgba8_to_inks(&cmyk, &original, RenderingIntent::RelativeColorimetric).unwrap();
    assert_eq!(inks.len(), original.len() * 4, "four inks a pixel");

    let back = cmyk.inks_to_rgba8(&srgb, &inks, RenderingIntent::RelativeColorimetric).unwrap();
    assert_eq!(back.len(), original.len());
    for (a, b) in original.iter().zip(&back) {
        let d = (a.r as i32 - b.r as i32).abs().max(
            (a.g as i32 - b.g as i32).abs().max((a.b as i32 - b.b as i32).abs()),
        );
        assert!(d <= 12, "{a:?} came back as {b:?}, off by {d}");
        assert_eq!(b.a, 255, "ink has no transparency to come back with");
    }
}

/// Paper is not transparent. Anything short of opaque has to land on white
/// rather than on nothing at all.
#[test]
fn transparency_lands_on_paper() {
    let Some(cmyk) = maybe(&format!("{GS}/default_cmyk.icc")) else { return };
    let srgb = Profile::srgb();
    let clear = srgb
        .rgba8_to_inks(&cmyk, &[Rgba8::new(0, 0, 0, 0)], RenderingIntent::RelativeColorimetric)
        .unwrap();
    let white = srgb
        .rgba8_to_inks(&cmyk, &[Rgba8::WHITE], RenderingIntent::RelativeColorimetric)
        .unwrap();
    assert_eq!(clear, white, "clear should print as the paper does");
}

#[test]
fn the_wrong_kind_of_profile_is_refused() {
    let Some(cmyk) = maybe(&format!("{GS}/default_cmyk.icc")) else { return };
    let srgb = Profile::srgb();
    let mut px = vec![Rgba8::WHITE];
    assert!(srgb.convert_rgba8(&cmyk, &mut px, RenderingIntent::Perceptual).is_err());
    assert!(srgb.inks_to_rgba8(&cmyk, &[0, 0, 0, 0], RenderingIntent::Perceptual).is_err());
}

// --- on a document ---------------------------------------------------------

use cshop_core::document::{Background, Document};
use cshop_core::history::{History, SetProfile};

fn wide() -> Option<Profile> {
    maybe(&format!("{COLORD}/WideGamutRGB.icc"))
}

#[test]
fn a_new_document_is_in_srgb() {
    let doc = Document::new("t", 4, 4, Background::White);
    assert!(doc.profile.is_srgb());
}

/// Assigning changes what the numbers mean and not the numbers.
#[test]
fn assigning_leaves_every_pixel_alone() {
    let Some(wide) = wide() else { return };
    let mut doc = Document::new("t", 4, 4, Background::Color(Rgba8::new(200, 60, 60, 255)));
    let mut history = History::new("Open");
    let before = doc.tree.get(doc.active.unwrap()).unwrap().pixels().unwrap().clone();

    history.apply(&mut doc, Box::new(SetProfile::assign(wide.clone())));
    assert_eq!(doc.profile, wide);
    assert_eq!(
        doc.tree.get(doc.active.unwrap()).unwrap().pixels().unwrap(),
        &before,
        "assigning must not touch a pixel"
    );
}

/// Converting changes the numbers so the colour can stay put.
#[test]
fn converting_rewrites_pixels_and_undoes_cleanly() {
    let Some(wide) = wide() else { return };
    let mut doc = Document::new("t", 4, 4, Background::Color(Rgba8::new(200, 60, 60, 255)));
    let mut history = History::new("Open");
    let before = doc.tree.get(doc.active.unwrap()).unwrap().pixels().unwrap().clone();

    history.apply(&mut doc, Box::new(SetProfile::convert(wide.clone())));
    assert_eq!(doc.profile, wide);
    let after = doc.tree.get(doc.active.unwrap()).unwrap().pixels().unwrap().clone();
    assert_ne!(after, before, "converting must rewrite them");

    history.undo(&mut doc);
    assert!(doc.profile.is_srgb(), "and undo must put the space back");
    assert_eq!(
        doc.tree.get(doc.active.unwrap()).unwrap().pixels().unwrap(),
        &before,
        "along with the pixels"
    );
}

/// A type layer holds the colour it will be drawn from next time. Converting
/// has to reach that, or the conversion comes undone the moment the text is
/// edited — and undo has to put the text back as text.
#[test]
fn converting_reaches_the_colour_behind_a_type_layer() {
    let Some(wide) = wide() else { return };
    let mut doc = Document::new("t", 64, 32, Background::White);
    let style = cshop_core::text::TextStyle {
        color: Rgba8::new(200, 60, 60, 255),
        ..Default::default()
    };
    let content = cshop_core::text::TextContent::new("Hi", style);
    let Some(text) = cshop_core::layer::TextLayer::new(content) else { return };
    let id = doc.tree.alloc_id();
    let layer = cshop_core::layer::Layer::new(
        id,
        "Hi",
        cshop_core::layer::LayerKind::Text(Box::new(text)),
    );
    doc.tree.push(layer, None);

    let mut history = History::new("Open");
    history.apply(&mut doc, Box::new(SetProfile::convert(wide)));

    let after = doc.tree.get(id).unwrap().text().unwrap().content().style.color;
    assert_ne!(after, Rgba8::new(200, 60, 60, 255), "the source colour must move too");

    history.undo(&mut doc);
    let layer = doc.tree.get(id).unwrap();
    assert!(layer.text().is_some(), "and undo must leave it as type, not as a picture of type");
    assert_eq!(layer.text().unwrap().content().style.color, Rgba8::new(200, 60, 60, 255));
}

// --- and the same journey, deeper ------------------------------------------

use cshop_core::color::Rgba16;

/// The measurement that justifies the depth.
///
/// The pair of colours that came home twenty-three counts apart at eight bits
/// should come home together at sixteen. Same transform, same profiles, same
/// rendering intent; only the room to hold the intermediate answer changes.
#[test]
fn sixteen_bits_holds_the_round_trip_that_eight_could_not() {
    let Some(wide) = wide() else { return };
    let srgb = Profile::srgb();
    let original = [
        Rgba16::from_rgba8(Rgba8::new(16, 243, 8, 255)),
        Rgba16::from_rgba8(Rgba8::new(20, 240, 10, 255)),
    ];

    let mut there = original;
    srgb.convert_rgba16(&wide, &mut there, RenderingIntent::RelativeColorimetric).unwrap();
    let mut back = there;
    wide.convert_rgba16(&srgb, &mut back, RenderingIntent::RelativeColorimetric).unwrap();

    // Measured in eight-bit counts, so it can be read against the other test.
    for (a, b) in original.iter().zip(&back) {
        let (a, b) = (a.to_rgba8(), b.to_rgba8());
        let worst = (a.r as i32 - b.r as i32)
            .abs()
            .max((a.g as i32 - b.g as i32).abs())
            .max((a.b as i32 - b.b as i32).abs());
        assert!(worst <= 1, "{a:?} came home as {b:?}, off by {worst}");
    }
}

/// Widening and narrowing must be exact in the direction that can be.
#[test]
fn eight_bits_widened_and_narrowed_again_is_the_same_picture() {
    for v in 0..=255u8 {
        let c = Rgba8::new(v, 255 - v, v.wrapping_mul(3), v);
        assert_eq!(Rgba16::from_rgba8(c).to_rgba8(), c, "{v} did not survive the trip");
    }
    // And the two ends land exactly where they should.
    assert_eq!(Rgba16::from_rgba8(Rgba8::WHITE), Rgba16::WHITE);
    assert_eq!(Rgba16::from_rgba8(Rgba8::TRANSPARENT), Rgba16::TRANSPARENT);
}

#[test]
fn deep_ink_goes_to_deep_colour_and_back() {
    let Some(cmyk) = maybe(&format!("{GS}/default_cmyk.icc")) else { return };
    let srgb = Profile::srgb();
    let original: Vec<Rgba16> = [
        Rgba8::new(128, 128, 128, 255),
        Rgba8::new(200, 60, 60, 255),
        Rgba8::new(60, 140, 90, 255),
    ]
    .iter()
    .map(|&c| Rgba16::from_rgba8(c))
    .collect();

    let inks = srgb.rgba16_to_inks16(&cmyk, &original, RenderingIntent::RelativeColorimetric).unwrap();
    assert_eq!(inks.len(), original.len() * 4);
    let back = cmyk.inks16_to_rgba16(&srgb, &inks, RenderingIntent::RelativeColorimetric).unwrap();
    for (a, b) in original.iter().zip(&back) {
        let (a, b) = (a.to_rgba8(), b.to_rgba8());
        let d = (a.r as i32 - b.r as i32).abs().max((a.g as i32 - b.g as i32).abs());
        assert!(d <= 12, "{a:?} came back as {b:?}");
    }
}

/// A profile carries the moment it was written. Two copies of sRGB stamped a
/// minute apart are not the same bytes, but converting between them is still
/// nothing, and doing it anyway costs precision — which is how a picture used
/// to lose a little every time it was exported and opened again.
#[test]
fn a_different_timestamp_is_not_a_different_colour() {
    let srgb = Profile::srgb();
    let mut later = srgb.bytes().to_vec();
    later[33] = later[33].wrapping_add(1); // the minute in the header's date
    later[35] = later[35].wrapping_add(7); // and the second
    let later = Profile::parse(&later).expect("still a profile");

    assert_ne!(later, srgb, "the bytes really do differ");
    assert!(later.same_transform(&srgb), "but nothing about colour does");
    assert!(srgb.same_transform(&later), "and it reads the same both ways");

    // So a conversion between them leaves every sample where it was.
    let original: Vec<Rgba8> =
        (0..=255u8).map(|v| Rgba8::new(v, 255 - v, v.wrapping_mul(5), 255)).collect();
    let mut through = original.clone();
    later.convert_rgba8(&srgb, &mut through, RenderingIntent::RelativeColorimetric).unwrap();
    assert_eq!(through, original, "a stamp is not a transform");
}

/// The other half: everything that does describe colour still has to match.
#[test]
fn a_different_colour_is_a_different_profile() {
    let srgb = Profile::srgb();
    // Byte 128 is the first byte past the header, where the tag table starts,
    // and byte 400 is in the middle of the tag data. Neither is a timestamp.
    for at in [128, 400] {
        let mut edited = srgb.bytes().to_vec();
        edited[at] = edited[at].wrapping_add(1);
        let Ok(edited) = Profile::parse(&edited) else { continue };
        assert!(
            !edited.same_transform(&srgb),
            "a change at byte {at} is not one of the two fields worth excusing"
        );
    }
    // And a profile of a different length is never the same one.
    if let Some(other) = maybe(&format!("{COLORD}/AdobeRGB1998.icc"))
        .or_else(|| maybe(&format!("{GS}/a98.icc")))
    {
        assert!(!other.same_transform(&srgb));
    }
}
