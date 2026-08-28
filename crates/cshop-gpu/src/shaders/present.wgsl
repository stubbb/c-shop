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
    let rgb = clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb * a, a);
}
