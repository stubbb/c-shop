//! Image XObjects and inline images (ISO 32000-1 §8.9), as straight-alpha
//! pixels.
//!
//! JPEG goes through the decoder this crate already has. JPEG 2000, JBIG2 and
//! fax images are refused by name, so the import can say which were left out.

use super::file::File;
use super::filters;
use super::syntax::{Dict, Object};
use cshop_core::color::Rgba8;
use cshop_core::pixels::PixelBuffer;

/// How an image's samples become colour.
pub enum Colors {
    Gray,
    Rgb,
    Cmyk,
    /// A spot ink: the sample is how much ink, shown as a tint of black.
    Ink,
    Indexed(Vec<Rgba8>),
}

impl Colors {
    fn components(&self) -> usize {
        match self {
            Colors::Rgb => 3,
            Colors::Cmyk => 4,
            _ => 1,
        }
    }
}

const MAX_SIDE: i64 = 30_000;
const MAX_PIXELS: i64 = 100_000_000;

/// Decode an image to pixels. `fill` paints a stencil mask; `space` resolves
/// a colour space to how its samples are read.
pub fn decode(
    file: &File,
    dict: &Dict,
    raw: &[u8],
    inline: bool,
    fill: Rgba8,
    space: &dyn Fn(&Object) -> Option<Colors>,
) -> Result<PixelBuffer, &'static str> {
    let get = |long: &[u8], short: &[u8]| -> Object {
        let v = dict.get(long).or_else(|| if inline { dict.get(short) } else { None });
        v.map(|v| file.resolve(v)).unwrap_or(Object::Null)
    };
    let w = get(b"Width", b"W").as_int().unwrap_or(0);
    let h = get(b"Height", b"H").as_int().unwrap_or(0);
    if w <= 0 || h <= 0 || w > MAX_SIDE || h > MAX_SIDE || w * h > MAX_PIXELS {
        return Err("images with an impossible size");
    }
    let (w, h) = (w as usize, h as usize);
    let stencil = get(b"ImageMask", b"IM") == Object::Bool(true);
    let decode_array: Vec<f32> = get(b"Decode", b"D").as_array().map(|a| a.iter().filter_map(|v| v.as_f32()).collect()).unwrap_or_default();

    let decoded = filters::decode_stream(dict, raw, &|o| file.resolve(o)).map_err(|_| "images whose data would not decode")?;
    let mut pixels = match decoded.image_codec.as_ref().map(|(c, _)| c.as_slice()) {
        Some(b"DCTDecode") => {
            let px = crate::decode(&decoded.data, None).map_err(|_| "JPEG images that would not decode")?;
            // A JPEG in an ink colour space would need its inks read; the
            // decoder hands back RGB, which is right for nearly all of them.
            px
        }
        Some(b"JPXDecode") => return Err("JPEG 2000 images"),
        Some(b"JBIG2Decode") => return Err("JBIG2 images"),
        Some(_) => return Err("fax-encoded (CCITT) images"),
        None if stencil => {
            let inverted = decode_array.first().is_some_and(|v| *v > 0.5);
            let mask = bits_to_mask(&decoded.data, w, h, inverted);
            let px = mask.iter().map(|&painted| if painted { fill } else { Rgba8::TRANSPARENT }).collect();
            return PixelBuffer::from_pixels(w as u32, h as u32, px).ok_or("images with an impossible size");
        }
        None => {
            let colors = space(&get(b"ColorSpace", b"CS")).ok_or("images in pattern colour spaces")?;
            let bpc = get(b"BitsPerComponent", b"BPC").as_int().unwrap_or(8);
            if ![1, 2, 4, 8, 16].contains(&bpc) {
                return Err("images with an unusual sample depth");
            }
            samples_to_pixels(&decoded.data, w, h, bpc as usize, &colors, &decode_array)
        }
    };

    // Transparency: a soft mask, a stencil mask, or a range of colours keyed out.
    if !inline {
        match get(b"SMask", b"") {
            Object::Stream(s) => {
                if let Some(alpha) = soft_mask(file, &s.dict, &s.raw) {
                    apply_alpha(&mut pixels, &alpha);
                }
            }
            _ => match get(b"Mask", b"") {
                Object::Stream(s) => {
                    let mw = file.get_in(&s.dict, b"Width").as_int().unwrap_or(0).clamp(0, MAX_SIDE) as usize;
                    let mh = file.get_in(&s.dict, b"Height").as_int().unwrap_or(0).clamp(0, MAX_SIDE) as usize;
                    if let Ok(d) = file.stream_data(&s.dict, &s.raw) {
                        if mw > 0 && mh > 0 {
                            let inverted = file.get_in(&s.dict, b"Decode").as_array().and_then(|a| a.first()).and_then(|v| v.as_f32()).is_some_and(|v| v > 0.5);
                            let mask = bits_to_mask(&d, mw, mh, inverted);
                            let alpha = Alpha { w: mw, h: mh, values: mask.iter().map(|&p| if p { 255 } else { 0 }).collect() };
                            apply_alpha(&mut pixels, &alpha);
                        }
                    }
                }
                Object::Array(ranges) => {
                    let ranges: Vec<i64> = ranges.iter().filter_map(|v| v.as_int()).collect();
                    key_out(&mut pixels, &decoded.data, w, h, &ranges, get(b"BitsPerComponent", b"BPC").as_int().unwrap_or(8) as usize);
                }
                _ => {}
            },
        }
    }
    Ok(pixels)
}

/// One bit per pixel, rows padded to a byte. `true` where the mask paints:
/// a sample of 0, unless the decode array turns that round.
fn bits_to_mask(data: &[u8], w: usize, h: usize, inverted: bool) -> Vec<bool> {
    let stride = w.div_ceil(8);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let byte = data.get(y * stride + x / 8).copied().unwrap_or(0xFF);
            let bit = byte >> (7 - x % 8) & 1;
            out.push((bit == 0) != inverted);
        }
    }
    out
}

/// Read `bpc`-bit samples, `n` to a pixel, rows padded to a byte.
fn read_samples(data: &[u8], w: usize, h: usize, n: usize, bpc: usize) -> Vec<u16> {
    let stride = (w * n * bpc).div_ceil(8);
    let mut out = Vec::with_capacity(w * h * n);
    for y in 0..h {
        let row = data.get(y * stride..).unwrap_or(&[]);
        for i in 0..w * n {
            let v = match bpc {
                8 => row.get(i).copied().unwrap_or(0) as u16,
                16 => u16::from_be_bytes([row.get(2 * i).copied().unwrap_or(0), row.get(2 * i + 1).copied().unwrap_or(0)]),
                _ => {
                    let bit = i * bpc;
                    let byte = row.get(bit / 8).copied().unwrap_or(0);
                    (byte >> (8 - bpc - bit % 8)) as u16 & ((1 << bpc) - 1)
                }
            };
            out.push(v);
        }
    }
    out
}

fn samples_to_pixels(data: &[u8], w: usize, h: usize, bpc: usize, colors: &Colors, decode: &[f32]) -> PixelBuffer {
    let n = colors.components();
    let samples = read_samples(data, w, h, n, bpc);
    let max = ((1u32 << bpc) - 1) as f32;
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    // A decode array maps each sample's range onto [dmin, dmax].
    let level = |s: u16, k: usize| -> f32 {
        let t = s as f32 / max;
        match (decode.get(2 * k), decode.get(2 * k + 1)) {
            (Some(&lo), Some(&hi)) => lo + t * (hi - lo),
            _ => t,
        }
    };
    let mut px = Vec::with_capacity(w * h);
    for p in samples.chunks_exact(n) {
        let c = match colors {
            Colors::Gray => {
                let g = byte(level(p[0], 0));
                Rgba8::new(g, g, g, 255)
            }
            Colors::Ink => {
                let g = byte(1.0 - level(p[0], 0));
                Rgba8::new(g, g, g, 255)
            }
            Colors::Rgb => Rgba8::new(byte(level(p[0], 0)), byte(level(p[1], 1)), byte(level(p[2], 2)), 255),
            Colors::Cmyk => {
                let k = 1.0 - level(p[3], 3);
                Rgba8::new(
                    byte((1.0 - level(p[0], 0)) * k),
                    byte((1.0 - level(p[1], 1)) * k),
                    byte((1.0 - level(p[2], 2)) * k),
                    255,
                )
            }
            Colors::Indexed(palette) => {
                // An index's decode array, when there is one, is in index units.
                let i = match (decode.first(), decode.get(1)) {
                    (Some(&lo), Some(&hi)) => (lo + p[0] as f32 / max * (hi - lo)).round().max(0.0) as usize,
                    _ => p[0] as usize,
                };
                palette.get(i.min(palette.len().saturating_sub(1))).copied().unwrap_or(Rgba8::BLACK)
            }
        };
        px.push(c);
    }
    px.resize(w * h, Rgba8::TRANSPARENT);
    PixelBuffer::from_pixels(w as u32, h as u32, px).expect("sized to match")
}

struct Alpha {
    w: usize,
    h: usize,
    values: Vec<u8>,
}

fn soft_mask(file: &File, dict: &Dict, raw: &[u8]) -> Option<Alpha> {
    let gray = |_: &Object| Some(Colors::Gray);
    let px = decode(file, dict, raw, false, Rgba8::BLACK, &gray).ok()?;
    Some(Alpha {
        w: px.width() as usize,
        h: px.height() as usize,
        values: (0..px.height() as i32).flat_map(|y| (0..px.width() as i32).map(move |x| (x, y))).map(|(x, y)| px.get(x, y).r).collect(),
    })
}

/// Multiply in a mask, scaled to the image by nearest neighbour when the two
/// differ in size, as they are allowed to.
fn apply_alpha(pixels: &mut PixelBuffer, alpha: &Alpha) {
    let (w, h) = (pixels.width() as usize, pixels.height() as usize);
    if alpha.w == 0 || alpha.h == 0 {
        return;
    }
    let data = pixels.pixels_mut();
    for y in 0..h {
        let ay = y * alpha.h / h;
        for x in 0..w {
            let ax = x * alpha.w / w;
            let a = alpha.values[ay * alpha.w + ax] as u32;
            let p = &mut data[y * w + x];
            p.a = (p.a as u32 * a / 255) as u8;
        }
    }
}

/// Colour-key masking: pixels whose samples all fall in the ranges vanish.
fn key_out(pixels: &mut PixelBuffer, data: &[u8], w: usize, h: usize, ranges: &[i64], bpc: usize) {
    let n = ranges.len() / 2;
    if n == 0 || ![1, 2, 4, 8, 16].contains(&bpc) || pixels.width() as usize != w || pixels.height() as usize != h {
        return;
    }
    let samples = read_samples(data, w, h, n, bpc);
    let out = pixels.pixels_mut();
    for (i, p) in samples.chunks_exact(n).enumerate() {
        if (0..n).all(|k| (ranges[2 * k]..=ranges[2 * k + 1]).contains(&(p[k] as i64))) {
            out[i].a = 0;
        }
    }
}
