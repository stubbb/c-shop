//! Layer effects.
//!
//! Eight effects — drop shadow, outer glow, bevel and emboss, inner shadow,
//! inner glow, satin, colour overlay and stroke — rendered from a layer's own
//! alpha and composited around it.
//!
//! # One distance field does most of the work
//!
//! Every effect here is some function of *how far a pixel is from the layer's
//! edge*. A stroke is a band around distance zero; a glow is a ramp away from
//! it; a bevel lights a height map built from it; spread and choke move the
//! contour before blurring. So the renderer computes one signed distance field
//! per layer — negative inside, positive outside — and every effect reads it.
//! Offsetting a shadow is then just sampling that field at a shifted position
//! rather than rebuilding anything.
//!
//! # Where the result goes
//!
//! Effects reach outside the layer, so the rendered result is larger than the
//! layer's pixels and reports how far its top-left moved. Like type and shape
//! layers, the composited raster is cached and handed to the compositor, which
//! needs no knowledge of effects at all.
//!
//! Fill opacity is applied here, to the layer's own pixels only, which is what
//! makes a stroke-only layer possible: drop the fill to zero and the effects
//! remain.

use crate::blend::{composite, BlendMode};
use crate::color::{Rgba, Rgba8};
use crate::mask::MaskBuffer;
use crate::pixels::PixelBuffer;
use rayon::prelude::*;

/// Where a stroke sits relative to the layer's edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StrokePosition {
    #[default]
    Outside,
    Center,
    Inside,
}

impl StrokePosition {
    pub fn name(self) -> &'static str {
        match self {
            StrokePosition::Outside => "Outside",
            StrokePosition::Center => "Center",
            StrokePosition::Inside => "Inside",
        }
    }
}

/// Where an inner glow starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GlowSource {
    /// Brightest at the layer's edge, fading inward.
    #[default]
    Edge,
    /// Brightest in the middle, fading toward the edge.
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BevelStyle {
    /// Raises the layer, with the lighting inside its edge.
    #[default]
    Inner,
    /// Raises the layer, with the lighting outside its edge.
    Outer,
    /// Raises the layer and lowers what surrounds it.
    Emboss,
    /// Presses the layer's edges into the surface.
    Pillow,
}

impl BevelStyle {
    pub fn name(self) -> &'static str {
        match self {
            BevelStyle::Inner => "Inner Bevel",
            BevelStyle::Outer => "Outer Bevel",
            BevelStyle::Emboss => "Emboss",
            BevelStyle::Pillow => "Pillow Emboss",
        }
    }
}

/// Drop shadow and inner shadow, which differ only in which side they fall on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    pub color: Rgba8,
    pub mode: BlendMode,
    pub opacity: f32,
    /// Degrees, counter-clockwise from east, as a light direction.
    pub angle: f32,
    pub use_global_light: bool,
    pub distance: f32,
    /// Moves the contour before blurring: `0` is a pure blur, `1` a hard edge.
    /// Called choke on the inner shadow, where it moves the other way.
    pub spread: f32,
    /// Blur radius.
    pub size: f32,
}

impl Default for Shadow {
    fn default() -> Self {
        Self {
            color: Rgba8::BLACK,
            mode: BlendMode::Multiply,
            opacity: 0.75,
            angle: 120.0,
            use_global_light: true,
            distance: 8.0,
            spread: 0.0,
            size: 8.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glow {
    pub color: Rgba8,
    pub mode: BlendMode,
    pub opacity: f32,
    pub spread: f32,
    pub size: f32,
    /// Inner glow only.
    pub source: GlowSource,
}

impl Default for Glow {
    fn default() -> Self {
        Self {
            color: Rgba8::opaque(255, 235, 160),
            mode: BlendMode::Screen,
            opacity: 0.75,
            spread: 0.0,
            size: 10.0,
            source: GlowSource::Edge,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bevel {
    pub style: BevelStyle,
    /// Width of the bevel, in pixels.
    pub size: f32,
    /// Blurs the height map, rounding the bevel off.
    pub soften: f32,
    /// Steepness of the surface, and so how hard the lighting reads.
    pub depth: f32,
    pub angle: f32,
    pub use_global_light: bool,
    /// Degrees above the surface; 90 is straight on and flattens the bevel.
    pub altitude: f32,
    /// Down inverts the lighting, so the bevel reads as carved in.
    pub down: bool,
    pub highlight: Rgba8,
    pub highlight_mode: BlendMode,
    pub highlight_opacity: f32,
    pub shadow: Rgba8,
    pub shadow_mode: BlendMode,
    pub shadow_opacity: f32,
}

impl Default for Bevel {
    fn default() -> Self {
        Self {
            style: BevelStyle::Inner,
            size: 8.0,
            soften: 0.0,
            depth: 1.0,
            angle: 120.0,
            use_global_light: true,
            altitude: 30.0,
            down: false,
            highlight: Rgba8::WHITE,
            highlight_mode: BlendMode::Screen,
            highlight_opacity: 0.75,
            shadow: Rgba8::BLACK,
            shadow_mode: BlendMode::Multiply,
            shadow_opacity: 0.75,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Satin {
    pub color: Rgba8,
    pub mode: BlendMode,
    pub opacity: f32,
    pub angle: f32,
    pub distance: f32,
    pub size: f32,
    pub invert: bool,
}

impl Default for Satin {
    fn default() -> Self {
        Self {
            color: Rgba8::opaque(20, 20, 20),
            mode: BlendMode::Multiply,
            opacity: 0.5,
            angle: 19.0,
            distance: 11.0,
            size: 14.0,
            invert: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorOverlay {
    pub color: Rgba8,
    pub mode: BlendMode,
    pub opacity: f32,
}

impl Default for ColorOverlay {
    fn default() -> Self {
        Self { color: Rgba8::opaque(220, 60, 60), mode: BlendMode::Normal, opacity: 1.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke {
    pub color: Rgba8,
    pub mode: BlendMode,
    pub opacity: f32,
    pub size: f32,
    pub position: StrokePosition,
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            color: Rgba8::opaque(30, 30, 30),
            mode: BlendMode::Normal,
            opacity: 1.0,
            size: 3.0,
            position: StrokePosition::Outside,
        }
    }
}

/// The effects attached to one layer.
///
/// Each is `Some` when enabled, so the set is also the on/off state the panel
/// shows.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LayerEffects {
    /// Turns the whole set off without forgetting the settings.
    pub enabled: bool,
    /// Shared by every effect that has `use_global_light` set, so a document
    /// can be lit consistently from one place.
    pub global_light_angle: f32,
    pub global_light_altitude: f32,

    pub drop_shadow: Option<Shadow>,
    pub outer_glow: Option<Glow>,
    pub bevel: Option<Bevel>,
    pub inner_shadow: Option<Shadow>,
    pub inner_glow: Option<Glow>,
    pub satin: Option<Satin>,
    pub color_overlay: Option<ColorOverlay>,
    pub stroke: Option<Stroke>,
}

impl LayerEffects {
    pub fn new() -> Self {
        Self { enabled: true, global_light_angle: 120.0, global_light_altitude: 30.0, ..Default::default() }
    }

    /// True when there is at least one effect that would draw something.
    pub fn any(&self) -> bool {
        self.enabled
            && (self.drop_shadow.is_some()
                || self.outer_glow.is_some()
                || self.bevel.is_some()
                || self.inner_shadow.is_some()
                || self.inner_glow.is_some()
                || self.satin.is_some()
                || self.color_overlay.is_some()
                || self.stroke.is_some())
    }

    /// Names of the effects currently on, top-first as they are drawn.
    pub fn active_names(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.stroke.is_some() {
            out.push("Stroke");
        }
        if self.color_overlay.is_some() {
            out.push("Color Overlay");
        }
        if self.satin.is_some() {
            out.push("Satin");
        }
        if self.inner_glow.is_some() {
            out.push("Inner Glow");
        }
        if self.inner_shadow.is_some() {
            out.push("Inner Shadow");
        }
        if self.bevel.is_some() {
            out.push("Bevel & Emboss");
        }
        if self.outer_glow.is_some() {
            out.push("Outer Glow");
        }
        if self.drop_shadow.is_some() {
            out.push("Drop Shadow");
        }
        out
    }

    /// How far the effects reach outside the layer's own pixels.
    pub fn outer_extent(&self) -> f32 {
        let mut m: f32 = 0.0;
        if let Some(s) = self.drop_shadow {
            m = m.max(s.distance + s.size + s.spread * s.size);
        }
        if let Some(g) = self.outer_glow {
            m = m.max(g.size + g.spread * g.size);
        }
        if let Some(s) = self.stroke {
            if s.position != StrokePosition::Inside {
                m = m.max(s.size);
            }
        }
        if let Some(b) = self.bevel {
            if matches!(b.style, BevelStyle::Outer | BevelStyle::Emboss | BevelStyle::Pillow) {
                m = m.max(b.size + b.soften);
            }
        }
        // A couple of pixels so a blur's tail is never clipped.
        if m > 0.0 { m + 2.0 } else { 0.0 }
    }
}

// ---------------------------------------------------------------------------
// Fields
// ---------------------------------------------------------------------------

/// A scalar field the size of the padded canvas.
type Field = Vec<f32>;

/// Separable Gaussian on a scalar field.
///
/// A dedicated scalar pass rather than [`crate::filters::plane`], which carries
/// four channels: every field here is coverage, and blurring three copies of it
/// would cost four times as much for nothing.
fn blur(src: &Field, w: usize, h: usize, sigma: f32) -> Field {
    if sigma <= 0.05 || w == 0 || h == 0 {
        return src.clone();
    }
    let kernel = crate::filters::plane::gaussian_kernel(sigma);
    let r = kernel.len() / 2;

    let mut tmp = vec![0.0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, out) in row.iter_mut().enumerate() {
            let mut sum = 0.0;
            for (i, k) in kernel.iter().enumerate() {
                // Clamp to the edge, so a blur does not darken against the
                // border of the padded canvas.
                let sx = (x as isize + i as isize - r as isize).clamp(0, w as isize - 1) as usize;
                sum += src[y * w + sx] * k;
            }
            *out = sum;
        }
    });

    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, slot) in row.iter_mut().enumerate() {
            let mut sum = 0.0;
            for (i, k) in kernel.iter().enumerate() {
                let sy = (y as isize + i as isize - r as isize).clamp(0, h as isize - 1) as usize;
                sum += tmp[sy * w + x] * k;
            }
            *slot = sum;
        }
    });
    out
}

/// Signed distance to the layer's edge: negative inside, positive outside.
///
/// The transform underneath is exact, but it measures to the nearest pixel
/// *centre* of a binary mask, so it reads about half a pixel long and knows
/// nothing about an antialiased edge. Two corrections follow, and both have to
/// keep the field **continuous**: a bevel differentiates it, and any step —
/// including one introduced by a correction that only applies near the edge —
/// shows up as stripes down every diagonal.
fn signed_distance(alpha: &Field, w: usize, h: usize) -> Field {
    let mut mask = MaskBuffer::new(w as u32, h as u32, 0);
    let bytes = mask.as_bytes_mut();
    for (i, a) in alpha.iter().enumerate() {
        bytes[i] = (a * 255.0).clamp(0.0, 255.0) as u8;
    }
    let outside = crate::selection::distance_field(&mask, w as u32, h as u32, false);
    let inside = crate::selection::distance_field(&mask, w as u32, h as u32, true);

    (0..w * h)
        .map(|i| {
            let d = outside[i] - inside[i];
            // Pull the magnitude in by half a pixel, which is where the
            // contour actually lies between two pixel centres. Continuous,
            // and zero on the contour itself.
            let coarse = if d > 0.0 { (d - 0.5).max(0.0) } else { (d + 0.5).min(0.0) };
            // Within a pixel of the edge the alpha is the better estimate;
            // beyond it the transform is. Cross-fade rather than switch.
            let fine = 0.5 - alpha[i];
            let t = coarse.abs().clamp(0.0, 1.0);
            fine * (1.0 - t) + coarse * t
        })
        .collect()
}

/// Sample a field at an offset, clamping at the border.
fn shifted(src: &Field, w: usize, h: usize, dx: f32, dy: f32) -> Field {
    if dx == 0.0 && dy == 0.0 {
        return src.clone();
    }
    let (dx, dy) = (dx.round() as isize, dy.round() as isize);
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let sy = (y as isize - dy).clamp(0, h as isize - 1) as usize;
        for (x, slot) in row.iter_mut().enumerate() {
            let sx = (x as isize - dx).clamp(0, w as isize - 1) as usize;
            *slot = src[sy * w + sx];
        }
    });
    out
}

/// Light direction from an angle in degrees, as an offset in pixels.
fn light_offset(angle: f32, distance: f32) -> (f32, f32) {
    let r = angle.to_radians();
    // The angle points at the light, so the shadow falls the other way, and
    // screen y grows downward.
    (-r.cos() * distance, r.sin() * distance)
}

/// Coverage of the shape grown (or shrunk) by `spread`, then blurred.
///
/// Spread and choke are one control: the contour is moved before the blur, so
/// at 1.0 the result is a hard dilated edge and at 0.0 a pure blur of the
/// original. Splitting the radius between the two is what makes the slider
/// behave the way it reads.
fn spread_blur(sdf: &Field, w: usize, h: usize, spread: f32, size: f32, inward: bool) -> Field {
    let spread = spread.clamp(0.0, 1.0);
    let grow = spread * size;
    let sigma = ((1.0 - spread) * size).max(0.0) / 2.0;
    let shaped: Field = sdf
        .par_iter()
        .map(|d| {
            // Inside is negative, so an inward field flips the comparison.
            let d = if inward { -*d } else { *d };
            (0.5 + grow - d).clamp(0.0, 1.0)
        })
        .collect();
    blur(&shaped, w, h, sigma)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// A layer with its effects drawn around it.
pub struct Rendered {
    pub pixels: PixelBuffer,
    /// Where the layer's own top-left sits inside `pixels`. The layer's offset
    /// moves by the negative of this to leave it where it was.
    pub origin: (i32, i32),
}

/// Composite a flat colour through a coverage field.
fn lay(
    dst: &mut [Rgba],
    cov: &Field,
    color: Rgba8,
    mode: BlendMode,
    opacity: f32,
    clip: Option<&Field>,
) {
    let c = color.to_f32();
    let opacity = opacity.clamp(0.0, 1.0);
    dst.par_iter_mut().enumerate().for_each(|(i, px)| {
        let mut a = cov[i].clamp(0.0, 1.0) * opacity * c.a;
        if let Some(clip) = clip {
            a *= clip[i].clamp(0.0, 1.0);
        }
        if a > 0.0 {
            *px = composite(mode, *px, c, a);
        }
    });
}

/// The angle an effect actually uses, once global light is taken into account.
fn angle_of(fx: &LayerEffects, own: f32, global: bool) -> f32 {
    if global {
        fx.global_light_angle
    } else {
        own
    }
}

/// How far the rendered result extends beyond the layer's own pixels.
///
/// The renderer and [`crate::layer::Layer::render_bounds`] both read this, so
/// the compositor's idea of where a layer draws cannot drift from where it
/// actually drew.
pub fn padding(fx: &LayerEffects) -> i32 {
    if !fx.any() {
        return 0;
    }
    fx.outer_extent().ceil().max(0.0) as i32
}

/// Draw `base` with its effects. `fill_opacity` scales the layer's own pixels
/// and nothing else.
pub fn render(base: &PixelBuffer, fx: &LayerEffects, fill_opacity: f32) -> Option<Rendered> {
    if !fx.any() {
        return None;
    }
    let pad = padding(fx);
    let w = base.width() as usize + 2 * pad as usize;
    let h = base.height() as usize + 2 * pad as usize;
    if w == 0 || h == 0 || w > 32768 || h > 32768 {
        return None;
    }

    // Alpha of the layer, on the padded canvas.
    let mut alpha = vec![0.0f32; w * h];
    for y in 0..base.height() as usize {
        for x in 0..base.width() as usize {
            alpha[(y + pad as usize) * w + x + pad as usize] =
                base.pixels()[y * base.width() as usize + x].a as f32 / 255.0;
        }
    }
    let sdf = signed_distance(&alpha, w, h);
    let mut canvas = vec![Rgba::new(0.0, 0.0, 0.0, 0.0); w * h];

    // --- behind the layer --------------------------------------------------
    if let Some(s) = fx.drop_shadow {
        let angle = angle_of(fx, s.angle, s.use_global_light);
        let (dx, dy) = light_offset(angle, s.distance);
        let moved = shifted(&sdf, w, h, dx, dy);
        let mut cov = spread_blur(&moved, w, h, s.spread, s.size, false);
        // The layer knocks its own shadow out, or a translucent layer would be
        // darkened by the shadow showing through it.
        for (c, a) in cov.iter_mut().zip(alpha.iter()) {
            *c *= 1.0 - a;
        }
        lay(&mut canvas, &cov, s.color, s.mode, s.opacity, None);
    }

    if let Some(g) = fx.outer_glow {
        let mut cov = spread_blur(&sdf, w, h, g.spread, g.size, false);
        for (c, a) in cov.iter_mut().zip(alpha.iter()) {
            *c *= 1.0 - a;
        }
        lay(&mut canvas, &cov, g.color, g.mode, g.opacity, None);
    }

    // --- the layer itself --------------------------------------------------
    let fill = fill_opacity.clamp(0.0, 1.0);
    if fill > 0.0 {
        for y in 0..base.height() as usize {
            for x in 0..base.width() as usize {
                let src = base.pixels()[y * base.width() as usize + x].to_f32();
                let a = src.a * fill;
                if a > 0.0 {
                    let i = (y + pad as usize) * w + x + pad as usize;
                    canvas[i] = composite(BlendMode::Normal, canvas[i], src, a);
                }
            }
        }
    }

    // --- on top, clipped to the layer --------------------------------------
    if let Some(b) = fx.bevel {
        draw_bevel(&mut canvas, &sdf, &alpha, w, h, fx, &b);
    }

    if let Some(s) = fx.inner_shadow {
        let angle = angle_of(fx, s.angle, s.use_global_light);
        let (dx, dy) = light_offset(angle, s.distance);
        let moved = shifted(&sdf, w, h, dx, dy);
        // Inward: the shadow lives in the hole left where the shape is not.
        let cov = spread_blur(&moved, w, h, s.spread, s.size, true);
        lay(&mut canvas, &cov, s.color, s.mode, s.opacity, Some(&alpha));
    }

    if let Some(g) = fx.inner_glow {
        let cov = match g.source {
            GlowSource::Edge => spread_blur(&sdf, w, h, g.spread, g.size, true),
            GlowSource::Center => {
                // Brightest deep inside, fading out to the edge.
                let t: Field = sdf
                    .par_iter()
                    .map(|d| (-*d / g.size.max(0.5)).clamp(0.0, 1.0))
                    .collect();
                blur(&t, w, h, (g.size * (1.0 - g.spread)).max(0.0) / 3.0)
            }
        };
        lay(&mut canvas, &cov, g.color, g.mode, g.opacity, Some(&alpha));
    }

    if let Some(s) = fx.satin {
        let (dx, dy) = light_offset(s.angle, s.distance);
        let a = blur(&shifted(&alpha, w, h, dx, dy), w, h, s.size.max(0.0) / 2.0);
        let b = blur(&shifted(&alpha, w, h, -dx, -dy), w, h, s.size.max(0.0) / 2.0);
        // Two offset copies differenced: the interference pattern is what
        // gives satin its folded sheen.
        let cov: Field = a
            .par_iter()
            .zip(b.par_iter())
            .map(|(x, y)| {
                // The difference of two offset blurs is small by nature, so it
                // is amplified; otherwise satin is invisible at any sane
                // opacity.
                let d = ((x - y).abs() * 3.0).clamp(0.0, 1.0);
                if s.invert {
                    1.0 - d
                } else {
                    d
                }
            })
            .collect();
        lay(&mut canvas, &cov, s.color, s.mode, s.opacity, Some(&alpha));
    }

    if let Some(o) = fx.color_overlay {
        let full = vec![1.0f32; w * h];
        lay(&mut canvas, &full, o.color, o.mode, o.opacity, Some(&alpha));
    }

    if let Some(s) = fx.stroke {
        // A band around the contour, exactly as a shape's stroke is: the only
        // difference between the three positions is where it is centred.
        let half = s.size.max(0.0) / 2.0;
        let centre = match s.position {
            StrokePosition::Outside => half,
            StrokePosition::Center => 0.0,
            StrokePosition::Inside => -half,
        };
        let cov: Field = sdf
            .par_iter()
            .map(|d| (0.5 - ((d - centre).abs() - half)).clamp(0.0, 1.0))
            .collect();
        lay(&mut canvas, &cov, s.color, s.mode, s.opacity, None);
    }

    let mut pixels = PixelBuffer::new(w as u32, h as u32);
    pixels
        .pixels_mut()
        .par_iter_mut()
        .zip(canvas.par_iter())
        .for_each(|(dst, src)| *dst = src.to_u8());
    Some(Rendered { pixels, origin: (pad, pad) })
}

/// Bevel and emboss: build a height map from the distance field, light it, and
/// lay the highlight and shadow either side of the edge.
fn draw_bevel(
    canvas: &mut [Rgba],
    sdf: &Field,
    alpha: &Field,
    w: usize,
    h: usize,
    fx: &LayerEffects,
    b: &Bevel,
) {
    let size = b.size.max(0.5);
    // The distance transform is exact against a *binary* mask, so a diagonal
    // edge leaves a staircase in the field. A height map is fine with that,
    // but the bevel differentiates it, and the staircase then shows up as
    // stripes down every diagonal. Half a pixel of smoothing removes it
    // without visibly rounding a sharp bevel.
    let sdf = blur(sdf, w, h, 0.8);
    let height: Field = sdf
        .par_iter()
        .map(|d| match b.style {
            BevelStyle::Inner => (1.0 + *d / size).clamp(0.0, 1.0),
            BevelStyle::Outer => (1.0 - *d / size).clamp(0.0, 1.0),
            // Emboss rises inside and falls outside, so the surface crosses
            // the edge rather than meeting it.
            BevelStyle::Emboss => (0.5 - *d / (size * 2.0)).clamp(0.0, 1.0),
            // Pillow is the inner bevel inverted: the edge presses inward.
            BevelStyle::Pillow => 1.0 - (d.abs() / size).clamp(0.0, 1.0),
        })
        .collect();
    // A bevel always gets a touch of smoothing: the ridge where the two sides
    // of a stroke meet is a crease, and an unrounded crease reads as a fault
    // line rather than a bevel.
    let height = blur(&height, w, h, (b.soften.max(0.0) / 2.0).max(size / 8.0));

    let angle = angle_of(fx, b.angle, b.use_global_light).to_radians();
    let altitude = b.altitude.clamp(0.0, 90.0).to_radians();
    // Light direction; screen y grows downward, so the vertical term is negated.
    let lx = angle.cos() * altitude.cos();
    let ly = -angle.sin() * altitude.cos();
    let lz = altitude.sin();
    let depth = b.depth.max(0.0);
    let flip = if b.down { -1.0 } else { 1.0 };

    let mut light = vec![0.0f32; w * h];
    light.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, slot) in row.iter_mut().enumerate() {
            let at = |xx: usize, yy: usize| height[yy * w + xx];
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(w - 1);
            let ym = y.saturating_sub(1);
            let yp = (y + 1).min(h - 1);
            // Sobel rather than central differences. A height map built from a
            // distance field has a crease along the shape's medial axis, and
            // on a diagonal that crease alternates with pixel parity — central
            // differences turn it into a plaid. Averaging over three rows and
            // columns removes the parity without blurring the bevel.
            let gx = ((at(xp, ym) + 2.0 * at(xp, y) + at(xp, yp))
                - (at(xm, ym) + 2.0 * at(xm, y) + at(xm, yp)))
                / 8.0
                * depth
                * size;
            let gy = ((at(xm, yp) + 2.0 * at(x, yp) + at(xp, yp))
                - (at(xm, ym) + 2.0 * at(x, ym) + at(xp, ym)))
                / 8.0
                * depth
                * size;
            // Surface normal of a height field is (-dz/dx, -dz/dy, 1).
            let (nx, ny, nz) = (-gx, -gy, 1.0);
            let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-6);
            *slot = ((nx * lx + ny * ly + nz * lz) / len) * flip;
        }
    });

    // Lighting straight on gives NdotL near one everywhere, so the flat
    // interior is subtracted out and only the slopes are drawn.
    let flat = lz;
    let highlight: Field = light.par_iter().map(|v| ((v - flat) * 2.0).clamp(0.0, 1.0)).collect();
    let shade: Field = light.par_iter().map(|v| ((flat - v) * 2.0).clamp(0.0, 1.0)).collect();

    // An inner or pillow bevel is confined to the layer; the others may fall
    // outside it.
    let clip = match b.style {
        BevelStyle::Inner | BevelStyle::Pillow => Some(alpha),
        BevelStyle::Outer | BevelStyle::Emboss => None,
    };
    lay(canvas, &highlight, b.highlight, b.highlight_mode, b.highlight_opacity, clip);
    lay(canvas, &shade, b.shadow, b.shadow_mode, b.shadow_opacity, clip);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::IRect;

    /// A solid square with clear space around it.
    fn square() -> PixelBuffer {
        let mut px = PixelBuffer::new(60, 60);
        px.fill_rect(IRect::at(20, 20, 20, 20), Rgba8::opaque(200, 200, 200));
        px
    }

    fn at(r: &Rendered, x: i32, y: i32) -> Rgba8 {
        r.pixels.get(x + r.origin.0, y + r.origin.1)
    }

    #[test]
    fn no_effects_renders_nothing() {
        assert!(render(&square(), &LayerEffects::default(), 1.0).is_none());
        // Switched off as a set, even with an effect configured.
        let fx = LayerEffects { enabled: false, drop_shadow: Some(Shadow::default()), ..Default::default() };
        assert!(render(&square(), &fx, 1.0).is_none());
    }

    /// A shadow has to land outside the layer, or it is not a shadow.
    #[test]
    fn a_drop_shadow_falls_outside_the_layer_and_away_from_the_light() {
        let mut fx = LayerEffects::new();
        // Light from the upper left, so the shadow goes down and right.
        fx.global_light_angle = 135.0;
        fx.drop_shadow = Some(Shadow { distance: 10.0, size: 3.0, opacity: 1.0, ..Default::default() });
        let r = render(&square(), &fx, 1.0).unwrap();

        assert!(r.pixels.width() > 60, "the raster should grow to hold the shadow");
        let lit = at(&r, 14, 14);
        let shadowed = at(&r, 46, 46);
        assert_eq!(lit.a, 0, "nothing should fall on the lit side, got {lit:?}");
        assert!(shadowed.a > 60, "the shadow should fall away from the light, got {shadowed:?}");
    }

    #[test]
    fn an_outer_glow_surrounds_the_layer_evenly() {
        let mut fx = LayerEffects::new();
        fx.outer_glow = Some(Glow { size: 8.0, opacity: 1.0, ..Default::default() });
        let r = render(&square(), &fx, 1.0).unwrap();
        // The square covers 20..40, so its edges sit at 20 and 40 and these
        // four probes are each 3.5 pixels outside one of them.
        let sides =
            [at(&r, 30, 16), at(&r, 30, 43), at(&r, 16, 30), at(&r, 43, 30)].map(|p| p.a as i32);
        assert!(sides.iter().all(|a| *a > 40), "the glow should reach every side: {sides:?}");
        let spread = sides.iter().max().unwrap() - sides.iter().min().unwrap();
        assert!(spread <= 12, "and evenly, got {sides:?}");
    }

    /// Fill opacity scales the layer's pixels but not its effects. That is the
    /// whole point of the separate control.
    #[test]
    fn fill_opacity_removes_the_pixels_and_leaves_the_effects() {
        let mut fx = LayerEffects::new();
        fx.stroke = Some(Stroke {
            size: 3.0,
            position: StrokePosition::Outside,
            opacity: 1.0,
            color: Rgba8::opaque(255, 0, 0),
            ..Default::default()
        });
        let solid = render(&square(), &fx, 1.0).unwrap();
        let hollow = render(&square(), &fx, 0.0).unwrap();

        assert!(at(&solid, 30, 30).a > 200, "the fill is there at full opacity");
        assert_eq!(at(&hollow, 30, 30).a, 0, "and gone at zero");
        // The stroke sits just outside the square's edge in both.
        assert!(at(&solid, 30, 18).a > 200);
        assert!(at(&hollow, 30, 18).a > 200, "the stroke must survive the fill going to zero");
        assert_eq!(at(&hollow, 30, 18).r, 255, "and keep its own colour");
    }

    #[test]
    fn stroke_position_puts_the_band_on_the_right_side() {
        let make = |position| {
            let mut fx = LayerEffects::new();
            fx.stroke = Some(Stroke { size: 4.0, position, opacity: 1.0, ..Default::default() });
            render(&square(), &fx, 0.0).unwrap()
        };
        // The square spans 20..40; its left edge is at x=20.
        let outside = make(StrokePosition::Outside);
        assert!(at(&outside, 18, 30).a > 200, "outside should paint left of the edge");
        assert_eq!(at(&outside, 24, 30).a, 0, "and not inside it");

        let inside = make(StrokePosition::Inside);
        assert!(at(&inside, 22, 30).a > 200, "inside should paint right of the edge");
        assert_eq!(at(&inside, 17, 30).a, 0, "and not outside it");
    }

    /// Inner effects are clipped to the layer, so they never leak outside it.
    #[test]
    fn inner_effects_stay_inside_the_layer() {
        for fx in [
            LayerEffects { inner_shadow: Some(Shadow::default()), ..LayerEffects::new() },
            LayerEffects { inner_glow: Some(Glow::default()), ..LayerEffects::new() },
            LayerEffects { satin: Some(Satin::default()), ..LayerEffects::new() },
            LayerEffects { color_overlay: Some(ColorOverlay::default()), ..LayerEffects::new() },
        ] {
            let r = render(&square(), &fx, 0.0).unwrap();
            for (x, y) in [(10, 30), (50, 30), (30, 10), (30, 50), (15, 15)] {
                assert_eq!(
                    at(&r, x, y).a,
                    0,
                    "{:?} leaked outside the layer at ({x}, {y})",
                    fx.active_names()
                );
            }
        }
    }

    /// The bevel differentiates a height map built from the distance field.
    /// That field has a crease along the shape's medial axis, and on a diagonal
    /// the crease alternates with pixel parity — which used to come out as a
    /// plaid across every diagonal stroke.
    #[test]
    fn a_bevel_on_a_diagonal_has_no_parity_striping() {
        // A diagonal bar, drawn by hand so the test does not depend on the
        // shape or text rasterisers.
        let mut px = PixelBuffer::new(120, 120);
        for y in 0..120i32 {
            for x in 0..120i32 {
                if (x - y).abs() < 12 {
                    px.set(x, y, Rgba8::opaque(200, 200, 200));
                }
            }
        }
        let mut fx = LayerEffects::new();
        fx.bevel = Some(Bevel { size: 8.0, depth: 1.4, soften: 0.0, ..Default::default() });
        let r = render(&px, &fx, 1.0).unwrap();

        // Down the bar's centreline the lighting is constant, so any
        // oscillation is the artefact.
        let along: Vec<i32> = (20..90).map(|i| at(&r, i, i).r as i32).collect();
        let swing = along.iter().max().unwrap() - along.iter().min().unwrap();
        assert!(swing <= 6, "the centreline should be flat, but swung by {swing}: {along:?}");
    }

    #[test]
    fn the_reported_padding_matches_what_is_rendered() {
        let mut fx = LayerEffects::new();
        fx.drop_shadow = Some(Shadow { distance: 14.0, size: 9.0, ..Default::default() });
        let pad = padding(&fx);
        let r = render(&square(), &fx, 1.0).unwrap();
        assert_eq!(r.origin, (pad, pad), "the compositor is told this before rendering happens");
        assert_eq!(r.pixels.width(), 60 + 2 * pad as u32);
    }
}
