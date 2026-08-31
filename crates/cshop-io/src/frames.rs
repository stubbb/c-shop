//! Animations: every frame of one, and how long each is held.
//!
//! # Why not just open it
//!
//! Opening an animated GIF already worked, in the sense that a picture came
//! back. It was the first frame. Everything after it — which is to say the
//! animation — was discarded silently, which is the worst way to not support
//! something: the file opens, looks right, and is not what was in it.
//!
//! # What a frame is here
//!
//! A layer, with a delay. That is the whole model. An animation is a stack of
//! layers shown one at a time instead of composited together, so everything
//! that already works on a layer — painting, masks, adjustments, effects —
//! works on a frame without being taught anything, and a still document is
//! simply one with no timeline on it.
//!
//! # Composing
//!
//! GIF frames are rarely whole pictures. Most are a small rectangle that
//! changes, drawn over whatever the previous frame left behind, with a
//! disposal rule saying what to do with the region afterwards. So the frames
//! are composed here into complete pictures, and what the editor gets is what
//! each moment of the animation actually looked like — not the difference
//! between it and the one before, which is a thing nobody wants to paint on.

use crate::IoError;
use cshop_core::color::Rgba8;
use cshop_core::pixels::PixelBuffer;

/// The most frames one file may bring in, so a hostile or broken animation
/// asks for a lot of memory rather than all of it.
const MAX_FRAMES: usize = 2_000;

/// One moment of an animation.
#[derive(Debug, Clone)]
pub struct Frame {
    pub pixels: PixelBuffer,
    /// How long it is held, in milliseconds.
    pub delay_ms: u16,
}

/// A whole animation.
#[derive(Debug, Clone)]
pub struct Animation {
    pub frames: Vec<Frame>,
    /// `0` loops forever, which is what most animations mean.
    pub loops: u16,
}

impl Animation {
    pub fn size(&self) -> (u32, u32) {
        self.frames
            .first()
            .map(|f| (f.pixels.width(), f.pixels.height()))
            .unwrap_or((0, 0))
    }
}

/// Whether these bytes are an animation worth reading as one.
///
/// A single-frame GIF is a picture, and treating it as an animation would put
/// a timeline on every one of them.
pub fn is_animation(bytes: &[u8]) -> bool {
    frame_count(bytes).is_some_and(|n| n > 1)
}

/// How many frames, without decoding them.
pub fn frame_count(bytes: &[u8]) -> Option<usize> {
    if bytes.starts_with(b"GIF8") {
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = options.read_info(std::io::Cursor::new(bytes)).ok()?;
        let mut n = 0;
        while n < MAX_FRAMES {
            match decoder.read_next_frame() {
                Ok(Some(_)) => n += 1,
                _ => break,
            }
        }
        return Some(n);
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let reader = decoder.read_info().ok()?;
        // An APNG says how many frames it has in its acTL chunk; a plain PNG
        // has no such chunk and is one picture.
        return Some(reader.info().animation_control().map_or(1, |a| a.num_frames as usize));
    }
    None
}

/// Every frame, composed so each is the whole picture at that moment.
pub fn read(bytes: &[u8]) -> Result<Animation, IoError> {
    if bytes.starts_with(b"GIF8") {
        return read_gif(bytes);
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return read_apng(bytes);
    }
    Err(IoError::Unsupported("that file is not an animation".into()))
}

fn read_gif(bytes: &[u8]) -> Result<Animation, IoError> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options
        .read_info(std::io::Cursor::new(bytes))
        .map_err(|e| IoError::Decode(format!("this GIF's header is unreadable: {e}")))?;
    let (w, h) = (decoder.width() as u32, decoder.height() as u32);
    crate::project::check_size(w, h)?;
    let loops = match decoder.repeat() {
        gif::Repeat::Infinite => 0,
        gif::Repeat::Finite(n) => n,
    };

    let mut frames = Vec::new();
    // The picture as it stands, which each frame is drawn over.
    let mut canvas = PixelBuffer::new(w, h);
    while frames.len() < MAX_FRAMES {
        let frame = match decoder.read_next_frame() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                // A truncated animation is worth what was read: the frames
                // before the damage are a real animation.
                if frames.is_empty() {
                    return Err(IoError::Decode(format!("this GIF stops early: {e}")));
                }
                log::warn!("this GIF stops early after {} frames: {e}", frames.len());
                break;
            }
        };
        // What the region held before this frame drew on it, for the disposal
        // rule that puts it back.
        let region = cshop_core::geom::IRect::at(
            frame.left as i32,
            frame.top as i32,
            frame.width as u32,
            frame.height as u32,
        );
        let restore = matches!(frame.dispose, gif::DisposalMethod::Previous)
            .then(|| canvas.copy_rect(region));

        for y in 0..frame.height as i32 {
            for x in 0..frame.width as i32 {
                let i = (y as usize * frame.width as usize + x as usize) * 4;
                let Some(px) = frame.buffer.get(i..i + 4) else { continue };
                // A transparent pixel in a GIF frame means "leave what is
                // underneath", not "make this transparent".
                if px[3] == 0 {
                    continue;
                }
                canvas.set(
                    frame.left as i32 + x,
                    frame.top as i32 + y,
                    Rgba8::new(px[0], px[1], px[2], px[3]),
                );
            }
        }

        frames.push(Frame {
            pixels: canvas.clone(),
            // GIF counts in hundredths of a second, and a great many files say
            // zero meaning "as fast as possible", which every viewer has
            // agreed for thirty years to read as a tenth of a second.
            delay_ms: match frame.delay {
                0 | 1 => 100,
                d => d.saturating_mul(10),
            },
        });

        match frame.dispose {
            gif::DisposalMethod::Background => {
                for y in region.y0..region.y1 {
                    for x in region.x0..region.x1 {
                        canvas.set(x, y, Rgba8::TRANSPARENT);
                    }
                }
            }
            gif::DisposalMethod::Previous => {
                if let Some(was) = restore {
                    canvas.paste(&was, region.x0, region.y0);
                }
            }
            _ => {}
        }
    }

    if frames.is_empty() {
        return Err(IoError::Decode("this GIF has no frames in it".into()));
    }
    Ok(Animation { frames, loops })
}

fn read_apng(bytes: &[u8]) -> Result<Animation, IoError> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|e| IoError::Decode(format!("this PNG's header is unreadable: {e}")))?;
    let info = reader.info();
    let (w, h) = (info.width, info.height);
    crate::project::check_size(w, h)?;
    let loops = info.animation_control().map_or(0, |a| a.num_plays as u16);

    let mut frames = Vec::new();
    let mut canvas = PixelBuffer::new(w, h);
    let mut buffer = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
    while frames.len() < MAX_FRAMES {
        let frame = match reader.next_frame(&mut buffer) {
            Ok(f) => f,
            Err(png::DecodingError::Format(_)) | Err(png::DecodingError::LimitsExceeded) => break,
            Err(_) => break,
        };
        let control = reader.info().frame_control();
        let (fx, fy, fw, fh) = control
            .map(|c| (c.x_offset, c.y_offset, c.width, c.height))
            .unwrap_or((0, 0, w, h));
        let blend_over = control.is_some_and(|c| c.blend_op == png::BlendOp::Over);
        let dispose = control.map(|c| c.dispose_op).unwrap_or(png::DisposeOp::None);
        let region =
            cshop_core::geom::IRect::at(fx as i32, fy as i32, fw, fh);
        let restore =
            matches!(dispose, png::DisposeOp::Previous).then(|| canvas.copy_rect(region));

        let data = &buffer[..frame.buffer_size()];
        let stride = frame.line_size;
        for y in 0..fh as i32 {
            for x in 0..fw as i32 {
                let i = y as usize * stride + x as usize * 4;
                let Some(px) = data.get(i..i + 4) else { continue };
                let c = Rgba8::new(px[0], px[1], px[2], px[3]);
                // `Over` composites onto what is there; `Source` replaces it,
                // transparency included.
                if blend_over && c.a == 0 {
                    continue;
                }
                canvas.set(fx as i32 + x, fy as i32 + y, c);
            }
        }
        frames.push(Frame {
            pixels: canvas.clone(),
            delay_ms: control
                .map(|c| {
                    let den = if c.delay_den == 0 { 100 } else { c.delay_den };
                    let ms = c.delay_num as u32 * 1000 / den as u32;
                    if ms == 0 { 100 } else { ms.min(u16::MAX as u32) as u16 }
                })
                .unwrap_or(100),
        });

        match dispose {
            png::DisposeOp::Background => {
                for y in region.y0..region.y1 {
                    for x in region.x0..region.x1 {
                        canvas.set(x, y, Rgba8::TRANSPARENT);
                    }
                }
            }
            png::DisposeOp::Previous => {
                if let Some(was) = restore {
                    canvas.paste(&was, region.x0, region.y0);
                }
            }
            png::DisposeOp::None => {}
        }
        if reader.info().animation_control().is_none() {
            break; // Not an animation: one picture, and it has been read.
        }
    }
    if frames.is_empty() {
        return Err(IoError::Decode("this PNG has no frames in it".into()));
    }
    Ok(Animation { frames, loops })
}

/// Write an animated GIF.
///
/// Every frame goes out whole rather than as a difference from the one before.
/// That makes larger files than an optimiser would, and it makes files that
/// are exactly what was asked for; the alternative is a second implementation
/// of disposal rules, on the writing side, where getting it wrong is silent.
pub fn write_gif(animation: &Animation, quality: u8) -> Result<Vec<u8>, IoError> {
    let (w, h) = animation.size();
    if w == 0 || h == 0 || animation.frames.is_empty() {
        return Err(IoError::Unsupported("there are no frames to write".into()));
    }
    let mut out = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut out, w as u16, h as u16, &[])
            .map_err(|e| IoError::Decode(e.to_string()))?;
        encoder
            .set_repeat(if animation.loops == 0 {
                gif::Repeat::Infinite
            } else {
                gif::Repeat::Finite(animation.loops)
            })
            .map_err(|e| IoError::Decode(e.to_string()))?;
        for frame in &animation.frames {
            let mut rgba: Vec<u8> = Vec::with_capacity((w * h * 4) as usize);
            for y in 0..h as i32 {
                for x in 0..w as i32 {
                    let c = frame.pixels.get(x, y);
                    rgba.extend_from_slice(&[c.r, c.g, c.b, c.a]);
                }
            }
            // GIF holds 256 colours a frame, so every frame is quantised on
            // its own — which is better than one palette for the whole
            // animation whenever the colours change through it.
            let mut f = gif::Frame::from_rgba_speed(
                w as u16,
                h as u16,
                &mut rgba,
                quality.clamp(1, 30) as i32,
            );
            f.delay = (frame.delay_ms / 10).max(1);
            encoder.write_frame(&f).map_err(|e| IoError::Decode(e.to_string()))?;
        }
    }
    Ok(out)
}

/// Write an animated PNG, which keeps every colour and every level of
/// transparency where GIF keeps 256 and one.
pub fn write_apng(animation: &Animation) -> Result<Vec<u8>, IoError> {
    let (w, h) = animation.size();
    if w == 0 || h == 0 || animation.frames.is_empty() {
        return Err(IoError::Unsupported("there are no frames to write".into()));
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .set_animated(animation.frames.len() as u32, animation.loops as u32)
            .map_err(|e| IoError::Decode(e.to_string()))?;
        let mut writer =
            encoder.write_header().map_err(|e| IoError::Decode(e.to_string()))?;
        for frame in &animation.frames {
            writer
                .set_frame_delay(frame.delay_ms.max(1), 1000)
                .map_err(|e| IoError::Decode(e.to_string()))?;
            let mut rgba: Vec<u8> = Vec::with_capacity((w * h * 4) as usize);
            for y in 0..h as i32 {
                for x in 0..w as i32 {
                    let c = frame.pixels.get(x, y);
                    rgba.extend_from_slice(&[c.r, c.g, c.b, c.a]);
                }
            }
            writer
                .write_image_data(&rgba)
                .map_err(|e| IoError::Decode(e.to_string()))?;
        }
    }
    Ok(out)
}
