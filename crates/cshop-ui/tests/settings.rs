//! What the editor remembers between one run and the next.

use cshop_core::color::Rgba8;
use cshop_ui::settings::Settings;
use cshop_ui::tools::Tool;
use std::path::PathBuf;

fn round_trip(s: &Settings) -> Settings {
    Settings::from_json(&cshop_core::json::parse(&s.to_json().write()).expect("valid JSON"))
}

#[test]
fn everything_remembered_survives_the_trip() {
    let s = Settings {
        tool: Tool::CloneStamp,
        brush: cshop_core::paint::Brush {
            size: 42.5,
            hardness: 0.25,
            opacity: 0.5,
            flow: 0.75,
            spacing: 0.2,
            scatter: cshop_core::paint::Scatter {
                spread: 1.25,
                count: 6,
                scale: 0.4,
                size_jitter: 0.65,
                angle: 30.0,
                follow: true,
            },
            pressure: cshop_core::paint::Pressure { size: true, flow: false, opacity: true },
        },
        brush_shape: Some(cshop_core::tips::TipShape::Star),
        foreground: Rgba8::opaque(0x12, 0x34, 0x56),
        background: Rgba8::opaque(0xab, 0xcd, 0xef),
        show_rulers: false,
        show_guides: false,
        show_grid: true,
        snap: false,
        grid_spacing: 64.0,
        show_panels: false,
        retouch: cshop_core::retouch::Retouch {
            kind: cshop_core::retouch::RetouchKind::Burn,
            range: cshop_core::retouch::Tones::Highlights,
            exposure: 0.35,
            soak: false,
        },
        brush_filter_strength: 0.8,
        shortcuts: vec![("Undo".into(), "Ctrl+G".into())],
        window: Some((1234, 900)),
        recent: vec![PathBuf::from("/one.png"), PathBuf::from("/two.psd")],
    };

    assert_eq!(round_trip(&s), s);
}

/// A settings file is a convenience, and a convenience that can stop the
/// program starting is not one. Anything unreadable falls back to a default.
#[test]
fn rubbish_falls_back_rather_than_failing() {
    for text in ["", "{", "[]", "null", "\"a string\"", "{\"tool\": 7}"] {
        let json = cshop_core::json::parse(text).unwrap_or(cshop_core::json::Json::Null);
        let s = Settings::from_json(&json);
        assert_eq!(s.tool, Settings::default().tool, "on {text:?}");
        assert_eq!(s.brush.size, Settings::default().brush.size, "on {text:?}");
    }
}

/// A file from an older build is missing fields; one from a newer build has
/// extra. Neither may lose what this build does understand.
#[test]
fn missing_and_unknown_fields_are_both_survivable() {
    let older = cshop_core::json::parse(r#"{"tool":"Eraser","snap":false}"#).unwrap();
    let s = Settings::from_json(&older);
    assert_eq!(s.tool, Tool::Eraser);
    assert!(!s.snap);
    assert_eq!(s.grid_spacing, Settings::default().grid_spacing, "the rest are defaults");

    let newer = cshop_core::json::parse(
        r#"{"tool":"Eraser","something_from_the_future":{"a":[1,2]},"snap":true}"#,
    )
    .unwrap();
    let s = Settings::from_json(&newer);
    assert_eq!(s.tool, Tool::Eraser);
    assert!(s.snap);
}

/// Values that would leave the editor unusable are not honoured — a window
/// larger than any screen, or a brush of no size.
#[test]
fn values_that_would_break_the_editor_are_refused() {
    let json = cshop_core::json::parse(
        r#"{"window_width":999999,"window_height":10,"brush_size":-5,"grid_spacing":0}"#,
    )
    .unwrap();
    let s = Settings::from_json(&json);
    assert_eq!(s.window, None, "an impossible window is not honoured");
    assert!(s.brush.size >= 1.0, "a brush has to have a size: {}", s.brush.size);
    assert!(s.grid_spacing >= 1.0, "and a grid a spacing: {}", s.grid_spacing);
}

/// Most recent first, no repeats, and bounded.
#[test]
fn the_recent_list_is_ordered_and_bounded() {
    let mut s = Settings::default();
    for i in 0..20 {
        s.remember(&PathBuf::from(format!("/file{i}.png")));
    }
    assert!(s.recent.len() <= 12, "it kept {}", s.recent.len());
    assert_eq!(s.recent[0], PathBuf::from("/file19.png"), "newest first");

    // Opening one again moves it to the front rather than repeating it.
    let again = PathBuf::from("/file15.png");
    s.remember(&again);
    assert_eq!(s.recent[0], again);
    assert_eq!(s.recent.iter().filter(|p| **p == again).count(), 1);
}

/// The editor starts in the state it was left in.
#[test]
fn the_editor_opens_with_what_it_was_given() {
    let Some(gpu) = cshop_gpu::context::GpuContext::headless().ok() else { return };
    let settings = Settings {
        tool: Tool::Eraser,
        show_rulers: false,
        snap: false,
        grid_spacing: 12.0,
        foreground: Rgba8::opaque(9, 9, 9),
        ..Settings::default()
    };

    let app = cshop_ui::app::CShopApp::with_settings(gpu, settings.clone());
    assert_eq!(app.tool, Tool::Eraser);
    assert!(!app.show_rulers);
    assert!(!app.snap);
    assert_eq!(app.grid_spacing, 12.0);
    assert_eq!(app.foreground, settings.foreground);

    // And gives back what it was given, so nothing is lost on the way out.
    assert_eq!(app.current_settings(), settings);
}
