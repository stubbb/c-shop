// Converts the compositor's working buffer into a texture egui can draw.
//
// In:  Rgba16Float, **straight** alpha, **sRGB-encoded** components.
// Out: Rgba8Unorm, **premultiplied** alpha, still **sRGB-encoded**.
//
// egui blends with premultiplied source-over, and expects the textures it is
// given to be plain (non-`*Srgb`) and gamma-encoded: its own shader takes what
// it samples as gamma and converts to linear for the framebuffer. So the only
// thing to do here is premultiply. Converting to linear-light as well was what
// made the canvas display darker than the file it was saved to.

@group(0) @binding(0) var src_tex: texture_2d<f32>;

// The colour transform from the document's profile to the display's, as a
// grid. The engine that builds it runs on the processor and cannot run here;
// converting every pixel there and uploading the result cannot happen at
// thirty frames a second. A table is what both can use.
//
// When the two profiles are the same the table is the identity, so this pass
// is unchanged from what it always did — which is the common case and must
// stay exact.
@group(0) @binding(1) var lut_tex: texture_3d<f32>;
@group(0) @binding(2) var lut_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(idx) / 2) * 4.0 - 1.0;
    let y = f32(i32(idx) & 1) * 4.0 - 1.0;
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(i32(in.pos.x), i32(in.pos.y));
    let c = textureLoad(src_tex, px, 0);
    let a = clamp(c.a, 0.0, 1.0);
    var rgb = clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0));

    // Sampled at the middle of each cell rather than at its edge, which is
    // where a texture's coordinates would otherwise put a value of zero: half
    // a cell in at each end, and the rest scaled to fit between.
    let n = f32(textureDimensions(lut_tex).x);
    let scale = (n - 1.0) / n;
    let offset = 0.5 / n;
    rgb = textureSampleLevel(lut_tex, lut_sampler, rgb * scale + offset, 0.0).rgb;

    return vec4<f32>(rgb * a, a);
}
