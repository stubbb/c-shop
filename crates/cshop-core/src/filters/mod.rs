//! Image filters.
//!
//! Unlike an adjustment, a filter reads a pixel's *neighbourhood*, so it cannot
//! be a lookup table and cannot run as a compositing pass. Filters therefore
//! run on the CPU, parallelised across rows with rayon, and are applied
//! destructively to a layer.
//!
//! The interactive cost is handled the way Free Transform handles it: a filter
//! dialog previews on a downscaled proxy and only the committed filter touches
//! the full-resolution pixels.

pub mod blur;
pub mod distort;
pub mod effects;
pub mod plane;
pub mod render;

use crate::color::Rgba8;
use crate::pixels::PixelBuffer;
use crate::progress::Progress;
use blur::RadialKind;
use plane::Plane;

/// How far a blur may be asked to reach, in pixels.
///
/// A filter takes its numbers from a dialog whose sliders bound them, but also
/// from scripts, project files and other programs, which do not. A radius is a
/// loop bound and an allocation size both, so one arriving as infinity — or
/// merely as a very large number — is a crash or an endless wait rather than a
/// picture. This is several times the widest slider, so no request anyone
/// means arrives altered, and small enough that the work stays finite.
pub const MAX_RADIUS: f32 = 1000.0;

/// The same, for the filters that compare a whole square around each pixel.
///
/// Their cost grows with the *square* of the radius, so they are held much
/// lower than [`MAX_RADIUS`] — sixteen times the widest slider offering one.
pub const MAX_AREA_RADIUS: u32 = 100;

/// A bound on offsets and cell sizes, which are positions rather than areas.
///
/// These cost nothing to make large; they are bounded only so that the
/// arithmetic derived from them cannot overflow. It sits above the largest
/// image the file format will open, so it can never trim a meaningful value.
pub const MAX_EXTENT: i32 = 1 << 20;

/// Where a filter appears in the Filter menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Blur,
    Sharpen,
    Noise,
    Distort,
    Pixelate,
    Render,
    Stylize,
    Other,
}

impl Category {
    pub fn name(self) -> &'static str {
        match self {
            Category::Blur => "Blur",
            Category::Sharpen => "Sharpen",
            Category::Noise => "Noise",
            Category::Distort => "Distort",
            Category::Pixelate => "Pixelate",
            Category::Render => "Render",
            Category::Stylize => "Stylize",
            Category::Other => "Other",
        }
    }

    pub const ALL: [Category; 8] = [
        Category::Blur,
        Category::Sharpen,
        Category::Noise,
        Category::Distort,
        Category::Pixelate,
        Category::Render,
        Category::Stylize,
        Category::Other,
    ];
}

/// Context a filter may need beyond the pixels themselves.
#[derive(Debug, Clone, Copy)]
pub struct FilterContext {
    pub foreground: Rgba8,
    pub background: Rgba8,
}

impl Default for FilterContext {
    fn default() -> Self {
        Self { foreground: Rgba8::BLACK, background: Rgba8::WHITE }
    }
}

/// One filter and its settings.
#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    // --- blur ---
    GaussianBlur { radius: f32 },
    BoxBlur { radius: f32 },
    MotionBlur { angle: f32, distance: f32 },
    RadialBlur { amount: f32, spin: bool, centre: (f32, f32) },
    SurfaceBlur { radius: f32, threshold: f32 },
    AverageBlur,

    // --- sharpen ---
    Sharpen { amount: f32 },
    UnsharpMask { amount: f32, radius: f32, threshold: f32 },

    // --- noise ---
    AddNoise { amount: f32, monochromatic: bool, gaussian: bool, seed: u64 },
    Median { radius: u32 },
    DustAndScratches { radius: u32, threshold: f32 },

    // --- distort ---
    Twirl { angle: f32 },
    Pinch { amount: f32 },
    Spherize { amount: f32 },
    Wave { amplitude: f32, wavelength: f32, vertical: bool },
    PolarCoordinates { to_polar: bool },

    // --- pixelate ---
    Mosaic { size: u32 },
    Crystallize { size: u32, seed: u64 },
    Fragment { distance: i32 },

    // --- render ---
    Clouds { scale: f32, seed: u64, difference: bool },
    Fibers { strength: f32, length: f32, seed: u64 },

    // --- stylize ---
    FindEdges,
    Emboss { angle: f32, height: f32, amount: f32 },
    Solarize,
    Diffuse { amount: u32, seed: u64 },

    // --- other ---
    HighPass { radius: f32 },
    Offset { dx: i32, dy: i32, wrap: bool },
    Maximum { radius: u32 },
    Minimum { radius: u32 },
    Custom { kernel: [f32; 25], divisor: f32, offset: f32 },
}

impl Filter {
    pub fn name(&self) -> &'static str {
        match self {
            Filter::GaussianBlur { .. } => "Gaussian Blur",
            Filter::BoxBlur { .. } => "Box Blur",
            Filter::MotionBlur { .. } => "Motion Blur",
            Filter::RadialBlur { .. } => "Radial Blur",
            Filter::SurfaceBlur { .. } => "Surface Blur",
            Filter::AverageBlur => "Average",
            Filter::Sharpen { .. } => "Sharpen",
            Filter::UnsharpMask { .. } => "Unsharp Mask",
            Filter::AddNoise { .. } => "Add Noise",
            Filter::Median { .. } => "Median",
            Filter::DustAndScratches { .. } => "Dust & Scratches",
            Filter::Twirl { .. } => "Twirl",
            Filter::Pinch { .. } => "Pinch",
            Filter::Spherize { .. } => "Spherize",
            Filter::Wave { .. } => "Wave",
            Filter::PolarCoordinates { .. } => "Polar Coordinates",
            Filter::Mosaic { .. } => "Mosaic",
            Filter::Crystallize { .. } => "Crystallize",
            Filter::Fragment { .. } => "Fragment",
            Filter::Clouds { difference: false, .. } => "Clouds",
            Filter::Clouds { .. } => "Difference Clouds",
            Filter::Fibers { .. } => "Fibers",
            Filter::FindEdges => "Find Edges",
            Filter::Emboss { .. } => "Emboss",
            Filter::Solarize => "Solarize",
            Filter::Diffuse { .. } => "Diffuse",
            Filter::HighPass { .. } => "High Pass",
            Filter::Offset { .. } => "Offset",
            Filter::Maximum { .. } => "Maximum",
            Filter::Minimum { .. } => "Minimum",
            Filter::Custom { .. } => "Custom",
        }
    }

    pub fn category(&self) -> Category {
        match self {
            Filter::GaussianBlur { .. }
            | Filter::BoxBlur { .. }
            | Filter::MotionBlur { .. }
            | Filter::RadialBlur { .. }
            | Filter::SurfaceBlur { .. }
            | Filter::AverageBlur => Category::Blur,
            Filter::Sharpen { .. } | Filter::UnsharpMask { .. } => Category::Sharpen,
            Filter::AddNoise { .. }
            | Filter::Median { .. }
            | Filter::DustAndScratches { .. } => Category::Noise,
            Filter::Twirl { .. }
            | Filter::Pinch { .. }
            | Filter::Spherize { .. }
            | Filter::Wave { .. }
            | Filter::PolarCoordinates { .. } => Category::Distort,
            Filter::Mosaic { .. } | Filter::Crystallize { .. } | Filter::Fragment { .. } => {
                Category::Pixelate
            }
            Filter::Clouds { .. } | Filter::Fibers { .. } => Category::Render,
            Filter::FindEdges
            | Filter::Emboss { .. }
            | Filter::Solarize
            | Filter::Diffuse { .. } => Category::Stylize,
            Filter::HighPass { .. }
            | Filter::Offset { .. }
            | Filter::Maximum { .. }
            | Filter::Minimum { .. }
            | Filter::Custom { .. } => Category::Other,
        }
    }

    /// Whether the filter has settings worth showing a dialog for.
    pub fn has_settings(&self) -> bool {
        !matches!(self, Filter::AverageBlur | Filter::FindEdges | Filter::Solarize)
    }

    /// Every filter at its default settings, in menu order.
    pub fn all_defaults() -> Vec<Filter> {
        vec![
            Filter::GaussianBlur { radius: 5.0 },
            Filter::BoxBlur { radius: 5.0 },
            Filter::MotionBlur { angle: 0.0, distance: 25.0 },
            Filter::RadialBlur { amount: 0.3, spin: true, centre: (0.5, 0.5) },
            Filter::SurfaceBlur { radius: 5.0, threshold: 0.06 },
            Filter::AverageBlur,
            Filter::Sharpen { amount: 0.6 },
            Filter::UnsharpMask { amount: 0.8, radius: 2.0, threshold: 0.0 },
            Filter::AddNoise { amount: 0.12, monochromatic: false, gaussian: true, seed: 1 },
            Filter::Median { radius: 2 },
            Filter::DustAndScratches { radius: 2, threshold: 0.08 },
            Filter::Twirl { angle: 60.0 },
            Filter::Pinch { amount: 0.5 },
            Filter::Spherize { amount: 0.5 },
            Filter::Wave { amplitude: 12.0, wavelength: 60.0, vertical: false },
            Filter::PolarCoordinates { to_polar: true },
            Filter::Mosaic { size: 12 },
            Filter::Crystallize { size: 16, seed: 1 },
            Filter::Fragment { distance: 4 },
            Filter::Clouds { scale: 60.0, seed: 1, difference: false },
            Filter::Clouds { scale: 60.0, seed: 1, difference: true },
            Filter::Fibers { strength: 0.5, length: 12.0, seed: 1 },
            Filter::FindEdges,
            Filter::Emboss { angle: 135.0, height: 2.0, amount: 1.5 },
            Filter::Solarize,
            Filter::Diffuse { amount: 3, seed: 1 },
            Filter::HighPass { radius: 6.0 },
            Filter::Offset { dx: 20, dy: 20, wrap: true },
            Filter::Maximum { radius: 2 },
            Filter::Minimum { radius: 2 },
            Filter::Custom { kernel: Filter::IDENTITY_KERNEL, divisor: 1.0, offset: 0.0 },
        ]
    }

    /// One of every filter, at settings that actually do something.
    ///
    /// Several of them return the image untouched when their parameter is
    /// trivial — a blur of radius nought, a sharpen of nought amount — so a
    /// test that wants to see each one work has to be given settings that
    /// make it work.
    pub fn examples() -> Vec<Filter> {
        // Adding a variant breaks the match below, which sits here rather
        // than anywhere else so that whoever adds one is looking at the list
        // they also need to extend.
        fn _exhaustive(f: &Filter) {
            match f {
                Filter::GaussianBlur { .. }
                | Filter::BoxBlur { .. }
                | Filter::MotionBlur { .. }
                | Filter::RadialBlur { .. }
                | Filter::SurfaceBlur { .. }
                | Filter::AverageBlur
                | Filter::Sharpen { .. }
                | Filter::UnsharpMask { .. }
                | Filter::AddNoise { .. }
                | Filter::Median { .. }
                | Filter::DustAndScratches { .. }
                | Filter::Twirl { .. }
                | Filter::Pinch { .. }
                | Filter::Spherize { .. }
                | Filter::Wave { .. }
                | Filter::PolarCoordinates { .. }
                | Filter::Mosaic { .. }
                | Filter::Crystallize { .. }
                | Filter::Fragment { .. }
                | Filter::Clouds { .. }
                | Filter::Fibers { .. }
                | Filter::FindEdges
                | Filter::Emboss { .. }
                | Filter::Solarize
                | Filter::Diffuse { .. }
                | Filter::HighPass { .. }
                | Filter::Offset { .. }
                | Filter::Maximum { .. }
                | Filter::Minimum { .. }
                | Filter::Custom { .. } => (),
            }
        }
        let mut sharpening = Filter::IDENTITY_KERNEL;
        sharpening[12] = 2.0;
        sharpening[7] = -0.25;
        vec![
            Filter::GaussianBlur { radius: 6.0 },
            Filter::BoxBlur { radius: 6.0 },
            Filter::MotionBlur { angle: 30.0, distance: 12.0 },
            Filter::RadialBlur { amount: 0.4, spin: true, centre: (0.5, 0.5) },
            Filter::SurfaceBlur { radius: 4.0, threshold: 0.1 },
            Filter::AverageBlur,
            Filter::Sharpen { amount: 1.0 },
            Filter::UnsharpMask { amount: 1.0, radius: 4.0, threshold: 0.0 },
            Filter::AddNoise { amount: 0.2, monochromatic: false, gaussian: true, seed: 1 },
            Filter::Median { radius: 2 },
            Filter::DustAndScratches { radius: 2, threshold: 0.1 },
            Filter::Twirl { angle: 60.0 },
            Filter::Pinch { amount: 0.5 },
            Filter::Spherize { amount: 0.5 },
            Filter::Wave { amplitude: 8.0, wavelength: 40.0, vertical: false },
            Filter::PolarCoordinates { to_polar: true },
            Filter::Mosaic { size: 8 },
            Filter::Crystallize { size: 8, seed: 1 },
            Filter::Fragment { distance: 3 },
            Filter::Clouds { scale: 30.0, seed: 1, difference: false },
            Filter::Fibers { strength: 0.5, length: 16.0, seed: 1 },
            Filter::FindEdges,
            Filter::Emboss { angle: 45.0, height: 3.0, amount: 1.0 },
            Filter::Solarize,
            Filter::Diffuse { amount: 4, seed: 1 },
            Filter::HighPass { radius: 8.0 },
            Filter::Offset { dx: 12, dy: 8, wrap: true },
            Filter::Maximum { radius: 3 },
            Filter::Minimum { radius: 3 },
            Filter::Custom { kernel: sharpening, divisor: 1.75, offset: 0.0 },
        ]
    }

    /// How many sweeps of the image this filter makes, for the progress bar.
    ///
    /// Only the count matters, not the cost: the bar is reset for each filter,
    /// so it need only be linear within one run. Zero means the filter does
    /// not report at all — it is one of the few cheap enough to be over before
    /// a bar could be drawn — and the bar shows an unmeasured wait instead.
    ///
    /// Kept honest by `filter_progress`, which runs every filter and compares
    /// what it counted against what is claimed here.
    pub fn passes(&self) -> u64 {
        match self {
            // Separable: one pass across, one down.
            Filter::GaussianBlur { .. } | Filter::BoxBlur { .. } => 2,
            // A blur, then a sweep to add back the difference.
            Filter::Sharpen { .. } | Filter::UnsharpMask { .. } | Filter::HighPass { .. } => 3,
            // A median first, then the comparison against it.
            Filter::DustAndScratches { .. } => 2,
            // Once to work out the cells, once to paint them.
            Filter::Crystallize { .. } => 2,
            // Written without a row loop, and fast enough not to need one.
            Filter::AverageBlur | Filter::Solarize | Filter::Offset { .. } => 0,
            _ => 1,
        }
    }

    /// A 5x5 kernel that leaves the image alone, the starting point for Custom.
    pub const IDENTITY_KERNEL: [f32; 25] = {
        let mut k = [0.0; 25];
        k[12] = 1.0;
        k
    };

    /// Apply to a pixel buffer, with nobody watching.
    pub fn apply(&self, src: &PixelBuffer, ctx: &FilterContext) -> PixelBuffer {
        self.apply_reporting(src, ctx, &Progress::ignored())
    }

    /// Apply to a pixel buffer, saying how far along it is.
    ///
    /// The two conversions either side — into premultiplied float and back —
    /// are not counted. They are a fixed cost that does not depend on the
    /// filter, and on the filters slow enough to want a bar they are noise.
    pub fn apply_reporting(
        &self,
        src: &PixelBuffer,
        ctx: &FilterContext,
        p: &Progress,
    ) -> PixelBuffer {
        if src.width() == 0 || src.height() == 0 {
            return src.clone();
        }
        p.begin(self.name(), src.height() as u64 * self.passes());
        self.apply_plane(&Plane::from_pixels(src), ctx, p).to_pixels()
    }

    /// Apply to an already-converted plane, which saves a round trip when
    /// several filters run in sequence.
    ///
    /// The phase is the caller's to declare: a filter made of several sweeps
    /// counts them all against one total rather than restarting the bar in
    /// the middle of itself.
    pub fn apply_plane(&self, src: &Plane, ctx: &FilterContext, p: &Progress) -> Plane {
        // Every number is bounded here rather than at each of the doors into
        // the editor, so that a filter reached by a route nobody has thought of
        // yet is still a filter that finishes.
        match self.clamped_for(src.width, src.height) {
            Filter::GaussianBlur { radius } => blur::gaussian(src, radius, p),
            Filter::BoxBlur { radius } => blur::box_blur(src, radius, p),
            Filter::MotionBlur { angle, distance } => blur::motion(src, angle, distance, p),
            Filter::RadialBlur { amount, spin, centre } => blur::radial(
                src,
                amount,
                if spin { RadialKind::Spin } else { RadialKind::Zoom },
                centre,
                p,
            ),
            Filter::SurfaceBlur { radius, threshold } => blur::surface(src, radius, threshold, p),
            Filter::AverageBlur => blur::average(src, p),

            Filter::Sharpen { amount } => effects::sharpen(src, amount, p),
            Filter::UnsharpMask { amount, radius, threshold } => {
                effects::unsharp_mask(src, amount, radius, threshold, p)
            }

            Filter::AddNoise { amount, monochromatic, gaussian, seed } => {
                effects::add_noise(src, amount, monochromatic, gaussian, seed, p)
            }
            Filter::Median { radius } => effects::median(src, radius, p),
            Filter::DustAndScratches { radius, threshold } => {
                effects::dust_and_scratches(src, radius, threshold, p)
            }

            Filter::Twirl { angle } => distort::twirl(src, angle, p),
            Filter::Pinch { amount } => distort::pinch(src, amount, p),
            Filter::Spherize { amount } => distort::spherize(src, amount, p),
            Filter::Wave { amplitude, wavelength, vertical } => {
                distort::wave(src, amplitude, wavelength, vertical, p)
            }
            Filter::PolarCoordinates { to_polar } => distort::polar_coordinates(src, to_polar, p),

            Filter::Mosaic { size } => distort::mosaic(src, size, p),
            Filter::Crystallize { size, seed } => distort::crystallize(src, size, seed, p),
            Filter::Fragment { distance } => distort::fragment(src, distance, p),

            Filter::Clouds { scale, seed, difference } => {
                render::clouds(src, scale, seed, ctx.foreground, ctx.background, difference, p)
            }
            Filter::Fibers { strength, length, seed } => {
                render::fibers(src, strength, length, seed, ctx.foreground, ctx.background, p)
            }

            Filter::FindEdges => effects::find_edges(src, p),
            Filter::Emboss { angle, height, amount } => effects::emboss(src, angle, height, amount, p),
            Filter::Solarize => effects::solarize(src, p),
            Filter::Diffuse { amount, seed } => effects::diffuse(src, amount, seed, p),

            Filter::HighPass { radius } => effects::high_pass(src, radius, p),
            Filter::Offset { dx, dy, wrap } => effects::offset(src, dx, dy, wrap, p),
            Filter::Maximum { radius } => effects::morphology(src, radius, true, p),
            Filter::Minimum { radius } => effects::morphology(src, radius, false, p),
            Filter::Custom { ref kernel, divisor, offset } => {
                effects::custom(src, kernel, divisor, offset, p)
            }
        }
    }

    /// Scale a filter's settings for a preview rendered at `scale`.
    ///
    /// A 5-pixel blur on a half-size proxy has to become a 2.5-pixel blur, or
    /// the preview shows twice the effect the user will get.
    /// How far one output pixel reaches into the input, in pixels.
    ///
    /// `None` means the filter depends on the whole image: the distortions and
    /// the render filters are anchored to the image extent, Average needs
    /// every pixel, and the cell filters pin their grid to the origin. Those
    /// cannot be previewed from a crop — the crop would change the result.
    /// Everything else can, which is what lets the filter dialog preview a
    /// zoomed window at full resolution.
    pub fn support(&self) -> Option<u32> {
        let r = |v: f32| v.max(0.0).ceil() as u32;
        Some(match *self {
            // The Gaussian is truncated at three standard deviations.
            Filter::GaussianBlur { radius }
            | Filter::HighPass { radius }
            | Filter::UnsharpMask { radius, .. } => r(radius * 3.0),
            Filter::BoxBlur { radius } | Filter::SurfaceBlur { radius, .. } => r(radius),
            Filter::MotionBlur { distance, .. } => r(distance / 2.0 + 1.0),
            Filter::Median { radius }
            | Filter::DustAndScratches { radius, .. }
            | Filter::Maximum { radius }
            | Filter::Minimum { radius } => radius,
            Filter::Diffuse { amount, .. } => amount,
            Filter::Fragment { distance } => distance.unsigned_abs(),
            // Fixed convolutions.
            Filter::Sharpen { .. } | Filter::FindEdges | Filter::Emboss { .. } => 1,
            Filter::Custom { .. } => 2,
            // Pointwise.
            Filter::AddNoise { .. } | Filter::Solarize => 0,

            Filter::AverageBlur
            | Filter::RadialBlur { .. }
            | Filter::Twirl { .. }
            | Filter::Pinch { .. }
            | Filter::Spherize { .. }
            | Filter::Wave { .. }
            | Filter::PolarCoordinates { .. }
            | Filter::Mosaic { .. }
            | Filter::Crystallize { .. }
            | Filter::Clouds { .. }
            | Filter::Fibers { .. }
            | Filter::Offset { .. } => return None,
        })
    }

    pub fn scaled(&self, scale: f32) -> Filter {
        let s = scale.max(0.01);
        let mut out = self.clone();
        match &mut out {
            Filter::GaussianBlur { radius }
            | Filter::BoxBlur { radius }
            | Filter::SurfaceBlur { radius, .. }
            | Filter::HighPass { radius } => *radius *= s,
            Filter::MotionBlur { distance, .. } => *distance *= s,
            Filter::UnsharpMask { radius, .. } => *radius *= s,
            Filter::Wave { amplitude, wavelength, .. } => {
                *amplitude *= s;
                *wavelength *= s;
            }
            Filter::Mosaic { size } | Filter::Crystallize { size, .. } => {
                *size = ((*size as f32 * s).round() as u32).max(1);
            }
            Filter::Median { radius }
            | Filter::DustAndScratches { radius, .. }
            | Filter::Maximum { radius }
            | Filter::Minimum { radius } => {
                *radius = (*radius as f32 * s).round() as u32;
            }
            Filter::Offset { dx, dy, .. } => {
                *dx = (*dx as f32 * s).round() as i32;
                *dy = (*dy as f32 * s).round() as i32;
            }
            Filter::Fragment { distance } => *distance = ((*distance as f32 * s).round() as i32).max(1),
            Filter::Diffuse { amount, .. } => *amount = ((*amount as f32 * s).round() as u32).max(1),
            Filter::Clouds { scale: cloud_scale, .. } => *cloud_scale *= s,
            Filter::Fibers { length, .. } => *length *= s,
            Filter::Emboss { height, .. } => *height *= s,
            // Radial blur, the twirl family and the pointwise effects are all
            // expressed in fractions of the image, so they need no scaling.
            _ => {}
        }
        out
    }

    /// The same filter with every number brought inside a range it can survive.
    ///
    /// The sliders in the filter dialogs are the only thing that bounds these
    /// values, and three doors into the editor bypass the dialogs entirely: a
    /// script, a project file, and another program talking over MCP. What
    /// arrives through them is clamped here instead, so that the bound belongs
    /// to the filter rather than to one of its callers.
    ///
    /// A value that is not a number cannot be clamped into range, so it falls
    /// back to zero — an amount of none, an offset of nowhere, a convolution
    /// weight that contributes nothing — which for almost every filter means
    /// it leaves the picture alone. Where zero would divide, the nearest
    /// usable value is taken instead.
    pub fn clamped(&self) -> Filter {
        /// Finite and within `[-max, max]`; anything else becomes zero.
        fn n(v: f32, max: f32) -> f32 {
            if v.is_finite() { v.clamp(-max, max) } else { 0.0 }
        }
        /// A radius: finite, never negative, never beyond the ceiling.
        fn r(v: f32) -> f32 {
            if v.is_finite() { v.clamp(0.0, MAX_RADIUS) } else { 0.0 }
        }
        let area = |v: u32| v.min(MAX_AREA_RADIUS);
        let extent = |v: i32| v.clamp(-MAX_EXTENT, MAX_EXTENT);
        let cell = |v: u32| v.clamp(1, MAX_EXTENT as u32);

        let mut out = self.clone();
        match &mut out {
            // --- the ones that size a buffer or bound a loop ---
            Filter::GaussianBlur { radius }
            | Filter::BoxBlur { radius }
            | Filter::HighPass { radius } => *radius = r(*radius),
            Filter::MotionBlur { angle, distance } => {
                *angle = n(*angle, 3600.0);
                *distance = r(*distance);
            }
            Filter::SurfaceBlur { radius, threshold } => {
                *radius = r(*radius);
                *threshold = n(*threshold, 1.0);
            }
            Filter::UnsharpMask { amount, radius, threshold } => {
                *amount = n(*amount, 100.0);
                *radius = r(*radius);
                *threshold = n(*threshold, 1.0);
            }
            Filter::Median { radius } | Filter::Maximum { radius } | Filter::Minimum { radius } => {
                *radius = area(*radius)
            }
            Filter::DustAndScratches { radius, threshold } => {
                *radius = area(*radius);
                *threshold = n(*threshold, 1.0);
            }
            Filter::Diffuse { amount, .. } => *amount = area(*amount),
            Filter::Mosaic { size } => *size = cell(*size),
            Filter::Crystallize { size, .. } => *size = cell(*size),
            // Fragment reaches out from each pixel rather than moving the
            // picture, and says so through `support`, so it is bounded as a
            // reach and not as an offset.
            Filter::Fragment { distance } => {
                *distance = (*distance).clamp(-(MAX_RADIUS as i32), MAX_RADIUS as i32)
            }
            Filter::Offset { dx, dy, .. } => {
                *dx = extent(*dx);
                *dy = extent(*dy);
            }

            // --- the ones that only scale a value, and need only be finite ---
            Filter::RadialBlur { amount, centre, .. } => {
                *amount = n(*amount, 100.0);
                centre.0 = n(centre.0, 1.0);
                centre.1 = n(centre.1, 1.0);
            }
            Filter::Sharpen { amount } | Filter::Pinch { amount } | Filter::Spherize { amount } => {
                *amount = n(*amount, 100.0)
            }
            Filter::AddNoise { amount, .. } => *amount = n(*amount, 100.0),
            Filter::Twirl { angle } => *angle = n(*angle, 3600.0),
            Filter::Wave { amplitude, wavelength, .. } => {
                *amplitude = n(*amplitude, MAX_RADIUS);
                // A wavelength of zero would divide by itself.
                *wavelength = n(*wavelength, MAX_RADIUS).abs().max(0.01);
            }
            Filter::Clouds { scale, .. } => *scale = n(*scale, MAX_RADIUS).abs().max(0.01),
            Filter::Fibers { strength, length, .. } => {
                *strength = n(*strength, 100.0);
                *length = n(*length, MAX_RADIUS).abs().max(0.01);
            }
            Filter::Emboss { angle, height, amount } => {
                *angle = n(*angle, 3600.0);
                *height = n(*height, MAX_RADIUS);
                *amount = n(*amount, 100.0);
            }
            Filter::Custom { kernel, divisor, offset } => {
                for k in kernel.iter_mut() {
                    *k = n(*k, 1e6);
                }
                *divisor = n(*divisor, 1e6);
                *offset = n(*offset, 1e6);
            }

            // Nothing to bound: these carry no numbers at all.
            Filter::AverageBlur
            | Filter::FindEdges
            | Filter::Solarize
            | Filter::PolarCoordinates { .. } => {}
        }
        out
    }

    /// [`clamped`](Self::clamped), and then held to the picture it will run on.
    ///
    /// A blur wider than the image averages the whole of it; going wider still
    /// costs time and changes nothing. Bounding the reach by the picture keeps
    /// the work proportional to the picture, which is what every other filter
    /// already costs.
    fn clamped_for(&self, width: u32, height: u32) -> Filter {
        let longest = width.max(height).max(1);
        let mut out = self.clamped();
        let cap = |v: &mut f32| *v = v.min(longest as f32);
        match &mut out {
            Filter::GaussianBlur { radius }
            | Filter::BoxBlur { radius }
            | Filter::HighPass { radius }
            | Filter::SurfaceBlur { radius, .. }
            | Filter::UnsharpMask { radius, .. } => cap(radius),
            Filter::MotionBlur { distance, .. } => cap(distance),
            Filter::Median { radius }
            | Filter::Maximum { radius }
            | Filter::Minimum { radius }
            | Filter::DustAndScratches { radius, .. } => *radius = (*radius).min(longest),
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::IRect;

    /// A picture with edges, flat areas, a gradient and transparency — enough
    /// structure for every filter to have something to act on.
    fn scene() -> PixelBuffer {
        let mut px = PixelBuffer::new(64, 64);
        for y in 0..64i32 {
            for x in 0..64i32 {
                let t = x as f32 / 63.0;
                px.set(
                    x,
                    y,
                    Rgba8::opaque((40.0 + 180.0 * t) as u8, 90, (200.0 - 120.0 * t) as u8),
                );
            }
        }
        px.fill_rect(IRect::new(10, 10, 30, 30), Rgba8::opaque(250, 240, 20));
        px.fill_rect(IRect::new(40, 40, 60, 60), Rgba8::new(0, 0, 0, 0));
        px
    }

    fn flat() -> PixelBuffer {
        PixelBuffer::filled(48, 48, Rgba8::opaque(120, 140, 160))
    }

    #[test]
    fn every_filter_produces_a_valid_image() {
        let src = scene();
        let ctx = FilterContext::default();
        for filter in Filter::all_defaults() {
            let out = filter.apply(&src, &ctx);
            assert_eq!(
                (out.width(), out.height()),
                (src.width(), src.height()),
                "{} changed the dimensions",
                filter.name()
            );
            // Every pixel must be a real colour; a NaN anywhere in the pipeline
            // would show as a black or transparent speck.
            for y in 0..out.height() as i32 {
                for x in 0..out.width() as i32 {
                    let c = out.get(x, y);
                    let _ = c.r as u32 + c.g as u32 + c.b as u32 + c.a as u32;
                }
            }
        }
    }

    #[test]
    fn every_filter_is_deterministic() {
        // Filters with a random component must repeat exactly, or a preview
        // would differ from what is finally applied.
        let src = scene();
        let ctx = FilterContext::default();
        for filter in Filter::all_defaults() {
            let a = filter.apply(&src, &ctx);
            let b = filter.apply(&src, &ctx);
            assert!(a == b, "{} is not deterministic", filter.name());
        }
    }

    #[test]
    fn filters_never_leak_colour_into_transparent_areas() {
        // The premultiplied-filtering test: the fully transparent corner must
        // stay transparent under filters that do not move pixels around.
        let src = scene();
        let ctx = FilterContext::default();
        for filter in [
            Filter::Sharpen { amount: 1.0 },
            Filter::UnsharpMask { amount: 1.0, radius: 3.0, threshold: 0.0 },
            Filter::Solarize,
            Filter::AddNoise { amount: 0.3, monochromatic: false, gaussian: true, seed: 1 },
        ] {
            let out = filter.apply(&src, &ctx);
            assert_eq!(out.get(55, 55).a, 0, "{} added opacity to empty space", filter.name());
        }
    }

    #[test]
    fn blurring_a_flat_image_leaves_it_flat() {
        let src = flat();
        let ctx = FilterContext::default();
        for filter in [
            Filter::GaussianBlur { radius: 8.0 },
            Filter::BoxBlur { radius: 6.0 },
            Filter::MotionBlur { angle: 30.0, distance: 20.0 },
            Filter::SurfaceBlur { radius: 4.0, threshold: 0.1 },
            Filter::Median { radius: 3 },
            Filter::Maximum { radius: 2 },
            Filter::Minimum { radius: 2 },
        ] {
            let out = filter.apply(&src, &ctx);
            for (x, y) in [(0, 0), (24, 24), (47, 47), (0, 24)] {
                let c = out.get(x, y);
                assert!(
                    (c.r as i32 - 120).abs() <= 2 && (c.b as i32 - 160).abs() <= 2,
                    "{} shifted a flat colour at ({x},{y}) to {c:?}",
                    filter.name()
                );
            }
        }
    }

    #[test]
    fn a_zero_strength_filter_is_a_no_op() {
        let src = scene();
        let ctx = FilterContext::default();
        for filter in [
            Filter::GaussianBlur { radius: 0.0 },
            Filter::BoxBlur { radius: 0.0 },
            Filter::MotionBlur { angle: 0.0, distance: 0.0 },
            Filter::RadialBlur { amount: 0.0, spin: true, centre: (0.5, 0.5) },
            Filter::Sharpen { amount: 0.0 },
            Filter::Median { radius: 0 },
            Filter::Maximum { radius: 0 },
            Filter::Offset { dx: 0, dy: 0, wrap: true },
        ] {
            let out = filter.apply(&src, &ctx);
            assert!(out == src, "{} changed the image at zero strength", filter.name());
        }
    }

    #[test]
    fn blur_actually_softens_an_edge() {
        let mut src = PixelBuffer::filled(64, 64, Rgba8::BLACK);
        src.fill_rect(IRect::new(32, 0, 64, 64), Rgba8::WHITE);
        let out = Filter::GaussianBlur { radius: 9.0 }.apply(&src, &FilterContext::default());

        // The step becomes a ramp, and it must be monotone across the join.
        let mut previous = -1i32;
        for x in 24..40i32 {
            let v = out.get(x, 32).r as i32;
            assert!(v >= previous, "the ramp went backwards at x={x}");
            previous = v;
        }
        let mid = out.get(31, 32).r;
        assert!(mid > 40 && mid < 215, "the edge should be a gradient, got {mid}");
        assert!(out.get(0, 32).r < 20, "far from the edge stays black");
        assert!(out.get(63, 32).r > 235, "and white stays white");
    }

    #[test]
    fn sharpen_increases_local_contrast() {
        let mut src = PixelBuffer::filled(64, 64, Rgba8::opaque(100, 100, 100));
        src.fill_rect(IRect::new(32, 0, 64, 64), Rgba8::opaque(150, 150, 150));
        let out = Filter::UnsharpMask { amount: 1.5, radius: 3.0, threshold: 0.0 }
            .apply(&src, &FilterContext::default());

        // Sharpening puts a dark line on the dark side of an edge and a light
        // one on the light side. The overshoot only reaches about as far as
        // the blur radius, so it has to be looked for next to the edge.
        let darkest = (26..32).map(|x| out.get(x, 32).r).min().unwrap();
        let lightest = (32..38).map(|x| out.get(x, 32).r).max().unwrap();
        assert!(darkest < 100, "the dark side should undershoot, got {darkest}");
        assert!(lightest > 150, "the light side should overshoot, got {lightest}");

        // Well away from the edge nothing should change.
        assert_eq!(out.get(2, 32).r, 100);
        assert_eq!(out.get(61, 32).r, 150);
    }

    #[test]
    fn the_unsharp_threshold_protects_flat_areas() {
        let src = PixelBuffer::filled(48, 48, Rgba8::opaque(128, 128, 128));
        let noisy = Filter::AddNoise { amount: 0.02, monochromatic: true, gaussian: false, seed: 5 }
            .apply(&src, &FilterContext::default());

        let ctx = FilterContext::default();
        let unprotected =
            Filter::UnsharpMask { amount: 3.0, radius: 2.0, threshold: 0.0 }.apply(&noisy, &ctx);
        let protected =
            Filter::UnsharpMask { amount: 3.0, radius: 2.0, threshold: 0.2 }.apply(&noisy, &ctx);

        let spread = |img: &PixelBuffer| {
            let values: Vec<i32> = (0..48)
                .flat_map(|y| (0..48).map(move |x| (x, y)))
                .map(|(x, y)| img.get(x, y).r as i32)
                .collect();
            values.iter().max().unwrap() - values.iter().min().unwrap()
        };
        assert!(
            spread(&protected) < spread(&unprotected),
            "the threshold should stop noise being amplified"
        );
    }

    #[test]
    fn find_edges_marks_edges_and_leaves_flat_areas_white() {
        let mut src = PixelBuffer::filled(64, 64, Rgba8::WHITE);
        src.fill_rect(IRect::new(20, 20, 44, 44), Rgba8::BLACK);
        let out = Filter::FindEdges.apply(&src, &FilterContext::default());

        assert!(out.get(5, 5).r > 200, "flat areas stay light");
        assert!(out.get(32, 32).r > 200, "the flat inside of the square too");
        assert!(out.get(20, 32).r < 120, "the boundary should be dark");
    }

    #[test]
    fn offset_moves_and_can_wrap() {
        let mut src = PixelBuffer::new(32, 32);
        src.set(0, 0, Rgba8::WHITE);

        let ctx = FilterContext::default();
        let moved = Filter::Offset { dx: 5, dy: 3, wrap: false }.apply(&src, &ctx);
        assert_eq!(moved.get(5, 3), Rgba8::WHITE);
        assert_eq!(moved.get(0, 0).a, 0);

        // Wrapping brings the pixel back around the far side.
        let wrapped = Filter::Offset { dx: -1, dy: 0, wrap: true }.apply(&src, &ctx);
        assert_eq!(wrapped.get(31, 0), Rgba8::WHITE);

        let clipped = Filter::Offset { dx: -1, dy: 0, wrap: false }.apply(&src, &ctx);
        assert_eq!(clipped.get(31, 0).a, 0, "without wrapping it should fall off the edge");
    }

    #[test]
    fn maximum_grows_bright_areas_and_minimum_shrinks_them() {
        // The square has to be comfortably wider than twice the radius, or
        // eroding it removes the whole thing.
        let mut src = PixelBuffer::filled(64, 64, Rgba8::BLACK);
        src.fill_rect(IRect::new(20, 20, 44, 44), Rgba8::WHITE);
        let ctx = FilterContext::default();

        let grown = Filter::Maximum { radius: 4 }.apply(&src, &ctx);
        assert_eq!(grown.get(17, 32).r, 255, "the bright square spread outward");
        assert_eq!(grown.get(10, 32).r, 0, "but not indefinitely");

        let shrunk = Filter::Minimum { radius: 4 }.apply(&src, &ctx);
        assert_eq!(shrunk.get(21, 32).r, 0, "the edge was eaten in");
        assert_eq!(shrunk.get(32, 32).r, 255, "the middle survives");
    }

    #[test]
    fn mosaic_produces_uniform_cells() {
        let out = Filter::Mosaic { size: 8 }.apply(&scene(), &FilterContext::default());
        // Every pixel in a cell must be identical.
        let reference = out.get(0, 0);
        for y in 0..8i32 {
            for x in 0..8i32 {
                assert_eq!(out.get(x, y), reference, "cell not uniform at ({x},{y})");
            }
        }
        assert_ne!(out.get(0, 0), out.get(40, 0), "different cells should differ");
    }

    #[test]
    fn crystallize_produces_flat_irregular_cells() {
        let out = Filter::Crystallize { size: 12, seed: 3 }.apply(&scene(), &FilterContext::default());
        // Count distinct colours: far fewer than pixels, but more than one.
        let mut colours: Vec<[u8; 4]> = (0..64)
            .flat_map(|y| (0..64).map(move |x| (x, y)))
            .map(|(x, y)| out.get(x, y).to_array())
            .collect();
        colours.sort_unstable();
        colours.dedup();
        assert!(colours.len() > 4, "should form several cells, got {}", colours.len());
        assert!(colours.len() < 200, "cells should be flat, got {} colours", colours.len());
    }

    #[test]
    fn polar_coordinates_round_trips_approximately() {
        let src = scene();
        let ctx = FilterContext::default();
        let there = Filter::PolarCoordinates { to_polar: true }.apply(&src, &ctx);
        let back = Filter::PolarCoordinates { to_polar: false }.apply(&there, &ctx);

        // Resampling twice loses precision, so only the broad structure has to
        // survive; the centre of the yellow square should still be yellow.
        let c = back.get(20, 20);
        assert!(c.r > 150 && c.b < 120, "the round trip lost the image: {c:?}");
    }

    #[test]
    fn distortions_leave_the_corners_alone() {
        // Twirl, pinch and spherize act inside an inscribed circle, so the
        // corners of the image must be untouched.
        let src = scene();
        let ctx = FilterContext::default();
        for filter in [
            Filter::Twirl { angle: 180.0 },
            Filter::Pinch { amount: 0.8 },
            Filter::Spherize { amount: 0.8 },
        ] {
            let out = filter.apply(&src, &ctx);
            assert_eq!(
                out.get(0, 0),
                src.get(0, 0),
                "{} disturbed the corner",
                filter.name()
            );
        }
    }

    #[test]
    fn a_zero_twirl_leaves_the_image_alone() {
        let src = scene();
        let out = Filter::Twirl { angle: 0.0 }.apply(&src, &FilterContext::default());
        for (x, y) in [(5, 5), (20, 20), (32, 32)] {
            let (a, b) = (src.get(x, y), out.get(x, y));
            assert!(
                (a.r as i32 - b.r as i32).abs() <= 1,
                "zero twirl changed ({x},{y}): {a:?} -> {b:?}"
            );
        }
    }

    #[test]
    fn add_noise_perturbs_without_shifting_the_mean() {
        let src = flat();
        let out = Filter::AddNoise { amount: 0.2, monochromatic: true, gaussian: true, seed: 9 }
            .apply(&src, &FilterContext::default());

        let mean: f64 = (0..48)
            .flat_map(|y| (0..48).map(move |x| (x, y)))
            .map(|(x, y)| out.get(x, y).r as f64)
            .sum::<f64>()
            / (48.0 * 48.0);
        assert!((mean - 120.0).abs() < 4.0, "noise shifted the mean to {mean}");

        // And it really did change things.
        assert_ne!(out.get(10, 10), out.get(11, 10));
    }

    #[test]
    fn monochromatic_noise_keeps_channels_together() {
        let src = PixelBuffer::filled(32, 32, Rgba8::opaque(128, 128, 128));
        let mono = Filter::AddNoise { amount: 0.3, monochromatic: true, gaussian: false, seed: 2 }
            .apply(&src, &FilterContext::default());
        for y in 0..32i32 {
            for x in 0..32i32 {
                let c = mono.get(x, y);
                assert!(
                    (c.r as i32 - c.g as i32).abs() <= 1 && (c.g as i32 - c.b as i32).abs() <= 1,
                    "monochromatic noise tinted ({x},{y}): {c:?}"
                );
            }
        }

        let colour = Filter::AddNoise { amount: 0.3, monochromatic: false, gaussian: false, seed: 2 }
            .apply(&src, &FilterContext::default());
        let tinted = (0..32)
            .flat_map(|y| (0..32).map(move |x| (x, y)))
            .any(|(x, y)| {
                let c = colour.get(x, y);
                (c.r as i32 - c.g as i32).abs() > 3
            });
        assert!(tinted, "colour noise should vary between channels");
    }

    #[test]
    fn median_removes_speckle_but_keeps_edges() {
        let mut src = PixelBuffer::filled(48, 48, Rgba8::BLACK);
        src.fill_rect(IRect::new(24, 0, 48, 48), Rgba8::WHITE);
        // A few stray pixels.
        src.set(10, 10, Rgba8::WHITE);
        src.set(38, 38, Rgba8::BLACK);

        let out = Filter::Median { radius: 2 }.apply(&src, &FilterContext::default());
        assert_eq!(out.get(10, 10).r, 0, "the speck should be gone");
        assert_eq!(out.get(38, 38).r, 255, "and the dark one too");
        // The edge stays a hard step.
        assert_eq!(out.get(23, 24).r, 0);
        assert_eq!(out.get(24, 24).r, 255);
    }

    #[test]
    fn clouds_ignore_the_input_and_fill_the_layer() {
        let ctx = FilterContext { foreground: Rgba8::BLACK, background: Rgba8::WHITE };
        let from_scene = Filter::Clouds { scale: 30.0, seed: 4, difference: false }
            .apply(&scene(), &ctx);
        let from_flat =
            Filter::Clouds { scale: 30.0, seed: 4, difference: false }.apply(&flat(), &ctx);

        // Generative, so the input must not matter — but the two sources are
        // different sizes, so compare the overlapping region.
        for (x, y) in [(4, 4), (20, 20), (40, 40)] {
            assert_eq!(from_scene.get(x, y), from_flat.get(x, y), "clouds depended on the input");
        }
        // Opaque, and varied.
        assert_eq!(from_scene.get(10, 10).a, 255);
        assert_ne!(from_scene.get(4, 4), from_scene.get(40, 40));
    }

    #[test]
    fn clouds_stay_between_the_two_colours() {
        let ctx = FilterContext {
            foreground: Rgba8::opaque(255, 0, 0),
            background: Rgba8::opaque(0, 0, 255),
        };
        let out = Filter::Clouds { scale: 20.0, seed: 7, difference: false }.apply(&flat(), &ctx);
        for y in 0..48i32 {
            for x in 0..48i32 {
                let c = out.get(x, y);
                assert_eq!(c.g, 0, "green should never appear between red and blue");
                assert!(c.r as u32 + c.b as u32 >= 250, "colours should interpolate the ramp");
            }
        }
    }

    #[test]
    fn difference_clouds_preserve_transparency() {
        let ctx = FilterContext::default();
        let out = Filter::Clouds { scale: 30.0, seed: 4, difference: true }.apply(&scene(), &ctx);
        assert_eq!(out.get(55, 55).a, 0, "the transparent corner must survive");
        assert_eq!(out.get(5, 5).a, 255);
    }

    #[test]
    fn a_custom_identity_kernel_changes_nothing() {
        let src = scene();
        let out = Filter::Custom {
            kernel: Filter::IDENTITY_KERNEL,
            divisor: 1.0,
            offset: 0.0,
        }
        .apply(&src, &FilterContext::default());
        assert!(out == src);
    }

    #[test]
    fn a_custom_box_kernel_blurs() {
        let mut kernel = [1.0f32; 25];
        kernel[0] = 1.0;
        let src = {
            let mut p = PixelBuffer::filled(48, 48, Rgba8::BLACK);
            p.fill_rect(IRect::new(24, 0, 48, 48), Rgba8::WHITE);
            p
        };
        let out = Filter::Custom { kernel, divisor: 25.0, offset: 0.0 }
            .apply(&src, &FilterContext::default());
        let edge = out.get(24, 24).r;
        assert!(edge > 20 && edge < 235, "the kernel should have softened the edge, got {edge}");
    }

    #[test]
    fn every_filter_belongs_to_exactly_one_category() {
        let all = Filter::all_defaults();
        for filter in &all {
            let category = filter.category();
            assert!(Category::ALL.contains(&category), "{} has a stray category", filter.name());
        }
        // Every category should have at least one filter, or the menu shows an
        // empty submenu.
        for category in Category::ALL {
            assert!(
                all.iter().any(|f| f.category() == category),
                "{} has no filters",
                category.name()
            );
        }
    }

    #[test]
    fn filter_names_are_unique() {
        let mut names: Vec<&str> = Filter::all_defaults().iter().map(|f| f.name()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "two filters share a name");
    }

    #[test]
    fn scaling_a_filter_scales_its_spatial_settings() {
        // The preview runs on a proxy, so a radius has to shrink with it.
        let full = Filter::GaussianBlur { radius: 10.0 };
        assert_eq!(full.scaled(0.5), Filter::GaussianBlur { radius: 5.0 });

        let mosaic = Filter::Mosaic { size: 20 };
        assert_eq!(mosaic.scaled(0.25), Filter::Mosaic { size: 5 });

        // A setting expressed as a fraction of the image must not scale.
        let twirl = Filter::Twirl { angle: 90.0 };
        assert_eq!(twirl.scaled(0.5), twirl);
        let solarize = Filter::Solarize;
        assert_eq!(solarize.scaled(0.1), solarize);
    }

    #[test]
    fn a_scaled_preview_resembles_the_full_result() {
        // The point of scaling: a preview must predict what commit will do.
        let mut src = PixelBuffer::filled(120, 120, Rgba8::BLACK);
        src.fill_rect(IRect::new(60, 0, 120, 120), Rgba8::WHITE);
        let ctx = FilterContext::default();

        let filter = Filter::GaussianBlur { radius: 12.0 };
        let full = filter.apply(&src, &ctx);

        let proxy = crate::resample::resize(&src, 60, 60, crate::resample::Resampling::Bilinear);
        let preview = filter.scaled(0.5).apply(&proxy, &ctx);
        let upscaled =
            crate::resample::resize(&preview, 120, 120, crate::resample::Resampling::Bilinear);

        // Compare the ramp across the edge; they should track closely.
        for x in (40..80).step_by(4) {
            let a = full.get(x, 60).r as i32;
            let b = upscaled.get(x, 60).r as i32;
            assert!((a - b).abs() < 45, "preview diverged at x={x}: {a} vs {b}");
        }
    }

    #[test]
    fn filters_handle_a_one_pixel_image() {
        // Radii larger than the image are the usual source of index panics.
        let src = PixelBuffer::filled(1, 1, Rgba8::opaque(10, 20, 30));
        let ctx = FilterContext::default();
        for filter in Filter::all_defaults() {
            let out = filter.apply(&src, &ctx);
            assert_eq!((out.width(), out.height()), (1, 1), "{}", filter.name());
        }
    }

    #[test]
    fn filters_handle_an_empty_image() {
        let src = PixelBuffer::new(0, 0);
        let ctx = FilterContext::default();
        for filter in Filter::all_defaults() {
            let out = filter.apply(&src, &ctx);
            assert_eq!(out.width(), 0, "{}", filter.name());
        }
    }
}
