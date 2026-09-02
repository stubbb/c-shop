//! Numbers that no dialog would ever produce, handed to every filter.
//!
//! A filter's settings are bounded by the sliders in its dialog, and three
//! doors into the editor miss the dialog entirely: a script, a project file,
//! and another program talking over MCP. A radius is a loop bound and an
//! allocation size once it reaches the compositor, so a value arriving through
//! one of those doors as infinity — or merely as a very large number — used to
//! be a crash or a wait without end. The editor is built with `panic = abort`,
//! so a crash anywhere is the whole session, unsaved document included.

use cshop_core::color::Rgba8;
use cshop_core::filters::{Filter, FilterContext, MAX_AREA_RADIUS, MAX_RADIUS};
use cshop_core::pixels::PixelBuffer;

const N: f32 = f32::NAN;
const I: f32 = f32::INFINITY;

/// One of each way a number can be hostile, spread across the filters that
/// size something with it.
fn hostile() -> Vec<Filter> {
    vec![
        Filter::GaussianBlur { radius: I },
        Filter::GaussianBlur { radius: 1e30 },
        Filter::GaussianBlur { radius: N },
        Filter::GaussianBlur { radius: -5.0 },
        Filter::BoxBlur { radius: I },
        Filter::BoxBlur { radius: 1e12 },
        Filter::MotionBlur { angle: N, distance: I },
        Filter::MotionBlur { angle: 45.0, distance: 1e9 },
        Filter::SurfaceBlur { radius: 1e6, threshold: I },
        Filter::UnsharpMask { amount: I, radius: I, threshold: N },
        Filter::HighPass { radius: I },
        Filter::Median { radius: 100_000 },
        Filter::Maximum { radius: 100_000 },
        Filter::Minimum { radius: u32::MAX },
        Filter::DustAndScratches { radius: 100_000, threshold: N },
        Filter::Diffuse { amount: 100_000, seed: 1 },
        Filter::Mosaic { size: u32::MAX },
        Filter::Crystallize { size: u32::MAX, seed: 1 },
        Filter::Fragment { distance: i32::MAX },
        Filter::Fragment { distance: i32::MIN },
        Filter::Offset { dx: i32::MIN, dy: i32::MAX, wrap: true },
        Filter::Wave { amplitude: I, wavelength: 0.0, vertical: false },
        Filter::Wave { amplitude: N, wavelength: N, vertical: true },
        Filter::RadialBlur { amount: I, spin: false, centre: (N, N) },
        Filter::Twirl { angle: I },
        Filter::Pinch { amount: I },
        Filter::Spherize { amount: N },
        Filter::Clouds { scale: 0.0, seed: 1, difference: false },
        Filter::Fibers { strength: I, length: I, seed: 1 },
        Filter::Emboss { angle: I, height: I, amount: I },
        Filter::AddNoise { amount: I, monochromatic: false, gaussian: true, seed: 1 },
        Filter::Custom { kernel: [N; 25], divisor: 0.0, offset: I },
    ]
}

/// The bound is on the settings, not on the clock.
///
/// Timing a filter would make this test a race on a loaded machine, and worse,
/// a filter left unbounded would hang the suite rather than fail it. What is
/// actually required is that no number survives in a state that can run away,
/// which is a property of the settings and can be read straight off them.
#[test]
fn clamping_bounds_every_number() {
    for f in hostile() {
        let c = f.clamped();

        // Debug prints every field of every variant, so this covers the
        // numbers without a second copy of the match to fall out of date.
        let shown = format!("{c:?}");
        assert!(
            !shown.contains("inf") && !shown.contains("NaN"),
            "{f:?} clamped to {shown}, which still holds a value that is not a number"
        );

        // `support` is the filter's own account of how far it reaches, which
        // is the quantity that becomes a loop bound and an allocation.
        if let Some(reach) = c.support() {
            assert!(
                reach as f32 <= MAX_RADIUS * 3.0,
                "{f:?} clamped to a reach of {reach} pixels"
            );
        }
    }
}

/// The same filters, actually run. A crash here is the bug this file is about.
#[test]
fn every_hostile_filter_still_produces_a_picture() {
    let src = PixelBuffer::filled(48, 36, Rgba8::opaque(120, 60, 30));
    let ctx = FilterContext::default();
    for f in hostile() {
        let out = f.apply(&src, &ctx);
        assert_eq!(out.width(), src.width());
        assert_eq!(out.height(), src.height());
    }
}

/// The fix must be invisible to everybody who was not attacking the editor.
///
/// Every setting a dialog can produce, and every default the program ships,
/// has to come back through the clamp untouched — otherwise this is not a
/// guard against absurd values but a change to what the filters do.
#[test]
fn ordinary_settings_pass_through_unchanged() {
    for f in Filter::examples().into_iter().chain(Filter::all_defaults()) {
        assert_eq!(f.clamped(), f, "{f:?} was altered by clamping");
    }
}

/// The ceilings have to stay above what the dialogs themselves can ask for,
/// or the guard against absurd values would be trimming values that somebody
/// chose deliberately with a slider. The widest radius slider reaches 400 and
/// the widest area one reaches 20.
///
/// Checked as the constants they are, rather than as a test: both sides are
/// known at compile time, so a test could only re-confirm what the compiler
/// has already worked out.
const _: () = assert!(MAX_RADIUS >= 800.0);
const _: () = assert!(MAX_AREA_RADIUS >= 80);
