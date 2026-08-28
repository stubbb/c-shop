// One layer composited onto a backdrop.
//
// This is the GPU twin of `cshop_core::blend::composite`. The two are checked
// against each other in the compositor's tests, so any change here must be
// mirrored there.
//
// Conventions:
//  * All colours are **straight** (non-premultiplied) alpha.
//  * All colours are **sRGB-encoded**, not linear light — established editors blend in
//    the document's gamma space and we match it.
//  * `blend_mode` is `cshop_core::blend::BlendMode as u32`; the numbering is a
//    hard contract between the two files.

// Field order is chosen so the vec4 lands at offset 0 and the whole struct is
// 80 bytes; `LayerParams` in compositor.rs must match byte for byte.
struct Params {
    // Colour for fill layers; ignored unless FLAG_SOLID is set.
    solid_color: vec4<f32>,
    // Document-space origin of the region being composited. Region-local pixel
    // coordinates plus this gives document coordinates.
    region_origin: vec2<i32>,
    layer_origin: vec2<i32>,
    layer_size: vec2<u32>,
    mask_origin: vec2<i32>,
    mask_size: vec2<u32>,
    blend_mode: u32,
    // Layer opacity times fill opacity.
    opacity: f32,
    flags: u32,
    // Which adjustment to apply; see `AdjustKind`. Only read when
    // FLAG_ADJUSTMENT is set.
    adjust_kind: u32,
    // Two scalars, not a vec2: in the uniform address space the alignment
    // rules would otherwise pad the struct out and stop it matching
    // `LayerParams`.
    _pad0: u32,
    _pad1: u32,
    // Parameters for the formula-based adjustments.
    adjust_params: array<vec4<f32>, 4>,
}

const FLAG_HAS_MASK: u32 = 1u;
const FLAG_CLIPPING: u32 = 2u;
// The layer is a uniform colour (a fill layer) rather than a texture.
const FLAG_SOLID: u32    = 4u;
// The layer recolours the backdrop instead of drawing over it. `layer_tex`
// then holds a 256-entry lookup table rather than an image.
const FLAG_ADJUSTMENT: u32 = 8u;

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var backdrop_tex: texture_2d<f32>;
@group(0) @binding(2) var layer_tex: texture_2d<f32>;
@group(0) @binding(3) var mask_tex: texture_2d<f32>;
@group(0) @binding(4) var clip_tex: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
}

// Full-screen triangle; no vertex buffer needed.
@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(idx) / 2) * 4.0 - 1.0;
    let y = f32(i32(idx) & 1) * 4.0 - 1.0;
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

// ---------------------------------------------------------------------------
// Separable blend functions
// ---------------------------------------------------------------------------

fn b_multiply(cb: f32, cs: f32) -> f32 { return cb * cs; }
fn b_screen(cb: f32, cs: f32) -> f32 { return cb + cs - cb * cs; }

fn b_color_burn(cb: f32, cs: f32) -> f32 {
    if (cb >= 1.0) { return 1.0; }
    if (cs <= 0.0) { return 0.0; }
    return 1.0 - min(1.0, (1.0 - cb) / cs);
}

fn b_color_dodge(cb: f32, cs: f32) -> f32 {
    if (cb <= 0.0) { return 0.0; }
    if (cs >= 1.0) { return 1.0; }
    return min(1.0, cb / (1.0 - cs));
}

fn b_hard_light(cb: f32, cs: f32) -> f32 {
    if (cs <= 0.5) { return b_multiply(cb, 2.0 * cs); }
    return b_screen(cb, 2.0 * cs - 1.0);
}

fn b_soft_light(cb: f32, cs: f32) -> f32 {
    if (cs <= 0.5) {
        return cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb);
    }
    var d: f32;
    if (cb <= 0.25) {
        d = ((16.0 * cb - 12.0) * cb + 4.0) * cb;
    } else {
        d = sqrt(max(cb, 0.0));
    }
    return cb + (2.0 * cs - 1.0) * (d - cb);
}

fn blend_channel(mode: u32, cb: f32, cs: f32) -> f32 {
    switch mode {
        case 2u:  { return min(cb, cs); }                       // Darken
        case 3u:  { return b_multiply(cb, cs); }                // Multiply
        case 4u:  { return b_color_burn(cb, cs); }              // Color Burn
        case 5u:  { return cb + cs - 1.0; }                     // Linear Burn
        case 7u:  { return max(cb, cs); }                       // Lighten
        case 8u:  { return b_screen(cb, cs); }                  // Screen
        case 9u:  { return b_color_dodge(cb, cs); }             // Color Dodge
        case 10u: { return cb + cs; }                           // Linear Dodge
        case 12u: { return b_hard_light(cs, cb); }              // Overlay
        case 13u: { return b_soft_light(cb, cs); }              // Soft Light
        case 14u: { return b_hard_light(cb, cs); }              // Hard Light
        case 15u: {                                             // Vivid Light
            if (cs <= 0.5) { return b_color_burn(cb, 2.0 * cs); }
            return b_color_dodge(cb, 2.0 * cs - 1.0);
        }
        case 16u: { return cb + 2.0 * cs - 1.0; }               // Linear Light
        case 17u: {                                             // Pin Light
            if (cs <= 0.5) { return min(cb, 2.0 * cs); }
            return max(cb, 2.0 * cs - 1.0);
        }
        case 18u: {                                             // Hard Mix
            if (cb + cs >= 1.0) { return 1.0; }
            return 0.0;
        }
        case 19u: { return abs(cb - cs); }                      // Difference
        case 20u: { return cb + cs - 2.0 * cb * cs; }           // Exclusion
        case 21u: { return cb - cs; }                           // Subtract
        case 22u: {                                             // Divide
            if (cs <= 0.0) { return 1.0; }
            return min(1.0, cb / cs);
        }
        // Normal, Dissolve, and every non-separable mode fall through: the
        // latter are handled wholesale in blend_rgb.
        default: { return cs; }
    }
}

// ---------------------------------------------------------------------------
// Non-separable blend functions
// ---------------------------------------------------------------------------

fn lum(c: vec3<f32>) -> f32 {
    return 0.30 * c.r + 0.59 * c.g + 0.11 * c.b;
}

fn clip_color(c_in: vec3<f32>) -> vec3<f32> {
    var c = c_in;
    let l = lum(c);
    let n = min(c.r, min(c.g, c.b));
    let x = max(c.r, max(c.g, c.b));
    if (n < 0.0) {
        let d = l - n;
        if (d > 1e-6) { c = vec3<f32>(l) + (c - vec3<f32>(l)) * l / d; }
        else { c = vec3<f32>(l); }
    }
    if (x > 1.0) {
        let d = x - l;
        if (d > 1e-6) { c = vec3<f32>(l) + (c - vec3<f32>(l)) * (1.0 - l) / d; }
        else { c = vec3<f32>(l); }
    }
    return c;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
    return clip_color(c + vec3<f32>(l - lum(c)));
}

fn sat(c: vec3<f32>) -> f32 {
    return max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b));
}

// Rescale so the colour has saturation `s`, preserving channel ordering.
fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let cmax = max(c.r, max(c.g, c.b));
    let cmin = min(c.r, min(c.g, c.b));
    if (cmax <= cmin) {
        return vec3<f32>(0.0);
    }
    let scale = s / (cmax - cmin);
    // The min channel maps to 0, the max to s, the middle proportionally.
    return (c - vec3<f32>(cmin)) * scale;
}

fn blend_rgb(mode: u32, cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 6u: {                                              // Darker Color
            if (lum(cs) < lum(cb)) { return cs; }
            return cb;
        }
        case 11u: {                                             // Lighter Color
            if (lum(cs) > lum(cb)) { return cs; }
            return cb;
        }
        case 23u: { return set_lum(set_sat(cs, sat(cb)), lum(cb)); }  // Hue
        case 24u: { return set_lum(set_sat(cb, sat(cs)), lum(cb)); }  // Saturation
        case 25u: { return set_lum(cs, lum(cb)); }                    // Color
        case 26u: { return set_lum(cb, lum(cs)); }                    // Luminosity
        default: {
            return vec3<f32>(
                blend_channel(mode, cb.r, cs.r),
                blend_channel(mode, cb.g, cs.g),
                blend_channel(mode, cb.b, cs.b),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Adjustments
// ---------------------------------------------------------------------------

const ADJ_LUT: u32          = 0u;
const ADJ_GRADIENT_MAP: u32 = 1u;
const ADJ_HUE_SAT: u32      = 2u;
const ADJ_VIBRANCE: u32     = 3u;
const ADJ_COLOR_BALANCE: u32 = 4u;
const ADJ_BLACK_WHITE: u32  = 5u;
const ADJ_CHANNEL_MIXER: u32 = 6u;
const ADJ_PHOTO_FILTER: u32 = 7u;

// Read the baked table. `layer_tex` is 256x1; index by the 8-bit level so the
// GPU reads exactly the entry the CPU reference would.
fn lut_at(v: f32) -> vec4<f32> {
    let i = i32(clamp(v, 0.0, 1.0) * 255.0 + 0.5);
    return textureLoad(layer_tex, vec2<i32>(i, 0), 0);
}

fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let cmax = max(c.r, max(c.g, c.b));
    let cmin = min(c.r, min(c.g, c.b));
    let d = cmax - cmin;
    var h = 0.0;
    if (d != 0.0) {
        if (cmax == c.r) {
            h = ((c.g - c.b) / d) % 6.0 / 6.0;
        } else if (cmax == c.g) {
            h = (((c.b - c.r) / d) + 2.0) / 6.0;
        } else {
            h = (((c.r - c.g) / d) + 4.0) / 6.0;
        }
    }
    if (h < 0.0) { h = h + 1.0; }
    var s = 0.0;
    if (cmax != 0.0) { s = d / cmax; }
    return vec3<f32>(h, s, cmax);
}

fn hsv_to_rgb(hsv: vec3<f32>) -> vec3<f32> {
    let h = (hsv.x - floor(hsv.x)) * 6.0;
    let i = floor(h);
    let f = h - i;
    let p = hsv.z * (1.0 - hsv.y);
    let q = hsv.z * (1.0 - hsv.y * f);
    let t = hsv.z * (1.0 - hsv.y * (1.0 - f));
    let k = i32(i) % 6;
    switch k {
        case 0: { return vec3<f32>(hsv.z, t, p); }
        case 1: { return vec3<f32>(q, hsv.z, p); }
        case 2: { return vec3<f32>(p, hsv.z, t); }
        case 3: { return vec3<f32>(p, q, hsv.z); }
        case 4: { return vec3<f32>(t, p, hsv.z); }
        default: { return vec3<f32>(hsv.z, p, q); }
    }
}

fn adjust(c: vec3<f32>) -> vec3<f32> {
    switch params.adjust_kind {
        // Per-channel table: one fetch per channel.
        case ADJ_LUT: {
            return vec3<f32>(lut_at(c.r).r, lut_at(c.g).g, lut_at(c.b).b);
        }
        // Table indexed by luma.
        case ADJ_GRADIENT_MAP: {
            let luma = lum(c);
            return lut_at(luma).rgb;
        }
        case ADJ_HUE_SAT: {
            let p0 = params.adjust_params[0];
            var hsv = rgb_to_hsv(c);
            if (p0.w > 0.5) {
                hsv.x = p0.x - floor(p0.x);
                hsv.y = clamp((p0.y + 1.0) * 0.5, 0.0, 1.0);
            } else {
                hsv.x = hsv.x + p0.x;
                hsv.y = clamp(hsv.y * (1.0 + p0.y), 0.0, 1.0);
            }
            var out = hsv_to_rgb(hsv);
            if (p0.z >= 0.0) {
                let t = clamp(p0.z, 0.0, 1.0);
                out = out + (vec3<f32>(1.0) - out) * t;
            } else {
                out = out * clamp(1.0 + p0.z, 0.0, 1.0);
            }
            return clamp(out, vec3<f32>(0.0), vec3<f32>(1.0));
        }
        case ADJ_VIBRANCE: {
            let p0 = params.adjust_params[0];
            let sat = max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b));
            // Less-saturated colours get proportionally more.
            let amount = p0.y + p0.x * (1.0 - sat);
            let luma = lum(c);
            return clamp(vec3<f32>(luma) + (c - vec3<f32>(luma)) * (1.0 + amount),
                         vec3<f32>(0.0), vec3<f32>(1.0));
        }
        case ADJ_COLOR_BALANCE: {
            let sh = params.adjust_params[0].rgb;
            let mid = params.adjust_params[1].rgb;
            let hi = params.adjust_params[2].rgb;
            let preserve = params.adjust_params[3].x;
            let before = lum(c);

            let w_shadow = clamp(1.0 - c * 2.0, vec3<f32>(0.0), vec3<f32>(1.0));
            let w_high = clamp((c - 0.5) * 2.0, vec3<f32>(0.0), vec3<f32>(1.0));
            let w_mid = clamp(vec3<f32>(1.0) - w_shadow - w_high, vec3<f32>(0.0), vec3<f32>(1.0));
            let shift = sh * w_shadow + mid * w_mid + hi * w_high;
            var out = clamp(c + shift * 0.5, vec3<f32>(0.0), vec3<f32>(1.0));

            if (preserve > 0.5) {
                out = clamp(out + vec3<f32>(before - lum(out)), vec3<f32>(0.0), vec3<f32>(1.0));
            }
            return out;
        }
        case ADJ_BLACK_WHITE: {
            let w0 = params.adjust_params[0];
            let w1 = params.adjust_params[1];
            let tint = params.adjust_params[2];
            var weights = array<f32, 6>(w0.x, w0.y, w0.z, w0.w, w1.x, w1.y);

            let cmax = max(c.r, max(c.g, c.b));
            let cmin = min(c.r, min(c.g, c.b));
            let h = rgb_to_hsv(c).x;
            // Blend the two sliders either side of the hue so the result stays
            // continuous as colour rotates through the wheel.
            let sector = h * 6.0;
            let i = i32(floor(sector)) % 6;
            let f = sector - floor(sector);
            var weight = 0.0;
            for (var k = 0; k < 6; k = k + 1) {
                if (k == i) { weight = weight + weights[k] * (1.0 - f); }
                if (k == (i + 1) % 6) { weight = weight + weights[k] * f; }
            }
            let grey = clamp(cmin + (cmax - cmin) * weight, 0.0, 1.0);
            if (tint.w > 0.5) {
                return clamp(tint.rgb * grey * 2.0, vec3<f32>(0.0), vec3<f32>(1.0));
            }
            return vec3<f32>(grey);
        }
        case ADJ_CHANNEL_MIXER: {
            let r = params.adjust_params[0];
            let g = params.adjust_params[1];
            let b = params.adjust_params[2];
            let mono = params.adjust_params[3].x;
            let orr = clamp(dot(c, r.rgb) + r.w, 0.0, 1.0);
            if (mono > 0.5) {
                return vec3<f32>(orr);
            }
            return vec3<f32>(
                orr,
                clamp(dot(c, g.rgb) + g.w, 0.0, 1.0),
                clamp(dot(c, b.rgb) + b.w, 0.0, 1.0),
            );
        }
        case ADJ_PHOTO_FILTER: {
            let p0 = params.adjust_params[0];
            let preserve = params.adjust_params[1].x;
            let d = clamp(p0.w, 0.0, 1.0);
            let before = lum(c);
            // A filter absorbs light, so it multiplies rather than blends.
            var out = clamp(c * (vec3<f32>(1.0 - d) + p0.rgb * (2.0 * d)),
                            vec3<f32>(0.0), vec3<f32>(1.0));
            if (preserve > 0.5) {
                let after = lum(out);
                if (after > 1e-4) {
                    out = clamp(out * (before / after), vec3<f32>(0.0), vec3<f32>(1.0));
                }
            }
            return out;
        }
        default: { return c; }
    }
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(i32(in.pos.x), i32(in.pos.y));
    let doc = px + params.region_origin;

    let backdrop = textureLoad(backdrop_tex, px, 0);

    // --- source colour -----------------------------------------------------
    var src = vec4<f32>(0.0);
    if ((params.flags & FLAG_SOLID) != 0u) {
        src = params.solid_color;
    } else {
        let lp = doc - params.layer_origin;
        if (lp.x >= 0 && lp.y >= 0 &&
            lp.x < i32(params.layer_size.x) && lp.y < i32(params.layer_size.y)) {
            src = textureLoad(layer_tex, lp, 0);
        }
    }

    // --- coverage ----------------------------------------------------------
    var coverage = params.opacity;

    if ((params.flags & FLAG_HAS_MASK) != 0u) {
        let mp = doc - params.mask_origin;
        // Outside the mask reads as hidden, matching MaskBuffer::get.
        var m = 0.0;
        if (mp.x >= 0 && mp.y >= 0 &&
            mp.x < i32(params.mask_size.x) && mp.y < i32(params.mask_size.y)) {
            m = textureLoad(mask_tex, mp, 0).r;
        }
        coverage *= m;
    }

    if ((params.flags & FLAG_CLIPPING) != 0u) {
        // Clipped layers are limited to the alpha of their clipping base.
        coverage *= textureLoad(clip_tex, px, 0).a;
    }

    // --- adjustment layers -------------------------------------------------
    // An adjustment recolours what is already there: alpha must not change,
    // and coverage mixes between the original and the adjusted colour. Running
    // it through the source-over path below would instead add opacity wherever
    // the adjustment applied.
    if ((params.flags & FLAG_ADJUSTMENT) != 0u) {
        if (backdrop.a <= 0.0 || coverage <= 0.0) {
            return backdrop;
        }
        let adjusted = adjust(backdrop.rgb);
        // The layer's own blend mode applies between the two, so an adjustment
        // set to Luminosity affects brightness only.
        let blended = blend_rgb(params.blend_mode, backdrop.rgb, adjusted);
        return vec4<f32>(mix(backdrop.rgb, blended, coverage), backdrop.a);
    }

    let a_src = clamp(src.a * coverage, 0.0, 1.0);
    if (a_src <= 0.0) {
        return backdrop;
    }

    let ab = backdrop.a;
    let cb = backdrop.rgb;
    let cs = src.rgb;

    // Fade the blend result back toward the plain source where the backdrop is
    // transparent, so blend modes do not produce halos against empty areas.
    let blended = blend_rgb(params.blend_mode, cb, cs);
    let mixed = cs + ab * (blended - cs);

    let a_out = a_src + ab * (1.0 - a_src);
    if (a_out <= 0.0) {
        return vec4<f32>(0.0);
    }
    // Premultiplied source-over, converted back to straight alpha.
    let rgb = (a_src * mixed + ab * (1.0 - a_src) * cb) / a_out;
    return vec4<f32>(rgb, a_out);
}
