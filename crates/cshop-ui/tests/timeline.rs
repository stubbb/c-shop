//! Animations, from opening one to writing it back out.

use cshop_core::color::Rgba8;
use cshop_core::pixels::PixelBuffer;
use cshop_gpu::context::GpuContext;
use cshop_io::frames::{Animation, Frame};
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

/// Three frames of a square moving right.
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

fn app() -> Option<CShopApp> {
    Some(CShopApp::new(GpuContext::headless().ok()?))
}

#[test]
fn opening_an_animation_gives_a_layer_per_frame_and_a_timeline() {
    let Some(mut app) = app() else { return };
    let bytes = cshop_io::frames::write_gif(&moving_square(), 10).unwrap();
    let doc = cshop_io::decode_document(&bytes, None).expect("it should open");
    app.open_document(doc);

    let view = app.doc().unwrap();
    assert_eq!(view.doc.tree.len(), 3, "a layer per frame");
    let timeline = view.doc.timeline.as_ref().expect("and a timeline over them");
    assert_eq!(timeline.len(), 3);
    assert_eq!(timeline.frames[0].delay_ms, 120);

    // Only the first frame shows, or opening one would stack every frame.
    let visible: Vec<bool> = view
        .doc
        .tree
        .iter_all()
        .into_iter()
        .map(|id| view.doc.tree.get(id).unwrap().visible)
        .collect();
    assert_eq!(visible, vec![true, false, false]);
}

#[test]
fn stepping_through_frames_changes_what_the_canvas_shows() {
    let Some(mut app) = app() else { return };
    let bytes = cshop_io::frames::write_gif(&moving_square(), 10).unwrap();
    app.open_document(cshop_io::decode_document(&bytes, None).unwrap());

    let gpu = app.gpu.clone();
    let square_at = |app: &mut CShopApp| -> Option<i32> {
        let px = app.render_composite(&gpu, 0);
        (0..32).find(|&x| px.get(x, 8).r > 150)
    };
    let first = square_at(&mut app).expect("the square is somewhere");
    app.dispatch(Action::ShowFrame(2));
    let third = square_at(&mut app).expect("and somewhere else now");
    assert!(third > first + 10, "the square moved: {first} to {third}");
}

/// A still picture must not grow a timeline.
#[test]
fn a_still_picture_opens_without_one() {
    let Some(mut app) = app() else { return };
    let px = PixelBuffer::filled(16, 16, Rgba8::opaque(90, 90, 90));
    let bytes = cshop_io::encode(&px, cshop_io::ImageFormat::Png, 92).unwrap();
    app.open_document(cshop_io::decode_document(&bytes, None).unwrap());
    assert!(app.doc().unwrap().doc.timeline.is_none());
}

#[test]
fn layers_can_be_made_into_frames_and_back() {
    let Some(mut app) = app() else { return };
    let bytes = cshop_io::frames::write_gif(&moving_square(), 10).unwrap();
    app.open_document(cshop_io::decode_document(&bytes, None).unwrap());

    app.dispatch(Action::ToggleTimeline);
    assert!(app.doc().unwrap().doc.timeline.is_none(), "and now it is a stack");
    // Every layer comes back, or the frames would have vanished.
    let view = app.doc().unwrap();
    assert!(view
        .doc
        .tree
        .iter_all()
        .into_iter()
        .all(|id| view.doc.tree.get(id).unwrap().visible));

    app.dispatch(Action::ToggleTimeline);
    let view = app.doc().unwrap();
    assert_eq!(view.doc.timeline.as_ref().map(|t| t.len()), Some(3));
}

/// Every frame goes out, composited — not the one that happened to be showing.
#[test]
fn writing_an_animation_writes_all_of_it() {
    let Some(mut app) = app() else { return };
    let bytes = cshop_io::frames::write_gif(&moving_square(), 10).unwrap();
    app.open_document(cshop_io::decode_document(&bytes, None).unwrap());
    app.dispatch(Action::ShowFrame(1));

    let gpu = app.gpu.clone();
    let out = app.render_animation(&gpu, 0).expect("an animation should come back");
    assert_eq!(out.frames.len(), 3);
    for (i, frame) in out.frames.iter().enumerate() {
        let at = (0..32).find(|&x| frame.pixels.get(x, 8).r > 150);
        assert!(at.is_some(), "frame {i} has its square");
    }
    // And the timeline is where it was, since exporting is not a way of
    // moving through the animation.
    assert_eq!(app.doc().unwrap().doc.timeline.as_ref().unwrap().current, 1);

    let written = cshop_io::frames::write_gif(&out, 10).unwrap();
    assert_eq!(cshop_io::frames::frame_count(&written), Some(3));
}

#[test]
fn playing_does_not_make_the_document_need_saving() {
    let Some(mut app) = app() else { return };
    let bytes = cshop_io::frames::write_gif(&moving_square(), 10).unwrap();
    app.open_document(cshop_io::decode_document(&bytes, None).unwrap());
    assert!(!app.doc().unwrap().doc.modified);

    app.dispatch(Action::TogglePlayback);
    assert!(app.playing);
    let ctx = egui::Context::default();
    app.poll_playback(&ctx);
    assert!(!app.doc().unwrap().doc.modified, "watching is not editing");
}

#[test]
fn a_document_with_one_layer_has_nothing_to_animate() {
    let Some(mut app) = app() else { return };
    app.open_document(cshop_core::document::Document::new(
        "t",
        16,
        16,
        cshop_core::document::Background::Transparent,
    ));
    app.dispatch(Action::ToggleTimeline);
    assert!(app.doc().unwrap().doc.timeline.is_none());
    let (msg, _) = app.toast.clone().expect("it should have said why");
    assert!(msg.contains("two layers"), "{msg}");
}
