//! A colour-managed canvas: the document's numbers put through a transform on
//! the way to the screen, rather than sent straight there.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::layer::LayerKind;
use cshop_core::pixels::PixelBuffer;
use cshop_core::profile::Profile;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

const WIDE: &str = "/usr/share/color/icc/colord/WideGamutRGB.icc";
const CMYK: &str = "/usr/share/color/icc/ghostscript/default_cmyk.icc";

fn app_with(profile: Profile) -> Option<CShopApp> {
    let gpu = GpuContext::headless().ok()?;
    let mut app = CShopApp::new(gpu);
    let mut doc = Document::new("t", 16, 16, Background::Transparent);
    doc.profile = profile;
    let id = doc.tree.iter_all()[0];
    // A saturated colour, which is where two spaces differ most.
    doc.tree.get_mut(id).unwrap().kind =
        LayerKind::raster(PixelBuffer::filled(16, 16, Rgba8::opaque(220, 30, 40)));
    app.open_document(doc);
    app.doc_mut().unwrap().invalidate();
    Some(app)
}

/// What the canvas is actually showing, read off the texture the screen gets.
fn shown(app: &mut CShopApp) -> Rgba8 {
    let gpu = app.gpu.clone();
    let i = app.active.unwrap();
    app.render_display(&gpu, i).get(8, 8)
}

#[test]
fn a_document_in_the_screens_own_space_is_shown_unchanged() {
    let Some(mut app) = app_with(Profile::srgb()) else { return };
    let c = shown(&mut app);
    assert_eq!(
        (c.r, c.g, c.b),
        (220, 30, 40),
        "sRGB on an sRGB screen must be exact, not nearly exact"
    );
}

/// The point of the whole thing: a wide-gamut file's numbers mean different
/// colours, and sending them straight to the screen shows them oversaturated.
#[test]
fn a_wide_gamut_document_is_shown_through_its_profile() {
    if !std::path::Path::new(WIDE).exists() {
        return;
    }
    let wide = Profile::load(std::path::Path::new(WIDE)).unwrap();
    let Some(mut app) = app_with(wide.clone()) else { return };
    let managed = shown(&mut app);

    // What the colour engine says the answer is.
    let mut want = [Rgba8::opaque(220, 30, 40)];
    wide.convert_rgba8(
        &Profile::srgb(),
        &mut want,
        cshop_core::profile::RenderingIntent::RelativeColorimetric,
    )
    .unwrap();

    let off = (managed.r as i32 - want[0].r as i32)
        .abs()
        .max((managed.g as i32 - want[0].g as i32).abs())
        .max((managed.b as i32 - want[0].b as i32).abs());
    assert!(off <= 3, "the canvas should agree with the engine; it is off by {off}");
    assert_ne!(
        (managed.r, managed.g, managed.b),
        (220, 30, 40),
        "and it must not be the raw numbers, which is the bug this fixes"
    );
}

/// Soft proofing: the picture as a press would print it, on a screen that can
/// reach more than the press can.
#[test]
fn proofing_shows_what_the_press_cannot_reach() {
    if !std::path::Path::new(CMYK).exists() {
        return;
    }
    let Some(mut app) = app_with(Profile::srgb()) else { return };
    let plain = shown(&mut app);

    app.dispatch(Action::SetProofProfile(Some(CMYK.into())));
    let proofed = shown(&mut app);
    assert_ne!(plain, proofed, "a saturated red is outside a press's reach and should move");

    app.dispatch(Action::SetProofProfile(None));
    assert_eq!(shown(&mut app), plain, "and turning it off puts it back exactly");
}

#[test]
fn a_screen_profile_that_cannot_be_read_says_so_and_changes_nothing() {
    let Some(mut app) = app_with(Profile::srgb()) else { return };
    let before = shown(&mut app);
    app.dispatch(Action::SetDisplayProfile(Some("/nowhere/at/all.icc".into())));
    let (msg, bad) = app.toast.clone().expect("it should have said something");
    assert!(bad && msg.contains("profile"), "{msg}");
    assert_eq!(shown(&mut app), before);
}

/// Every open document is shown for the same screen, and each has its own
/// profile to come from.
#[test]
fn each_document_gets_its_own_transform() {
    if !std::path::Path::new(WIDE).exists() {
        return;
    }
    let Some(mut app) = app_with(Profile::srgb()) else { return };
    let mut second = Document::new("wide", 16, 16, Background::Transparent);
    second.profile = Profile::load(std::path::Path::new(WIDE)).unwrap();
    let id = second.tree.iter_all()[0];
    second.tree.get_mut(id).unwrap().kind =
        LayerKind::raster(PixelBuffer::filled(16, 16, Rgba8::opaque(220, 30, 40)));
    app.open_document(second);
    app.doc_mut().unwrap().invalidate();

    let wide_shown = shown(&mut app);
    app.active = Some(0);
    let srgb_shown = shown(&mut app);
    assert_ne!(wide_shown, srgb_shown, "the same numbers in two spaces are two colours");
    assert_eq!((srgb_shown.r, srgb_shown.g, srgb_shown.b), (220, 30, 40));
}
