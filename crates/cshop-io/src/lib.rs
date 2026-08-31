//! # cshop-io
//!
//! Image decoding and encoding.
//!
//! Everything here is pure Rust, so the build needs no system image libraries.
//! PSD — the format that actually matters for interoperability — gets its own
//! hand-written reader and writer in a later phase; this module covers the
//! flat formats.

pub mod frames;
pub mod pdf;
pub mod raw;
pub mod svg;
pub mod bytes;
pub mod format;
pub mod cmyk;
pub mod icc;
pub mod project;
pub mod psd;

use cshop_core::color::{Rgba8, Rgba16};
use cshop_core::profile::{Profile, RenderingIntent, Space};
use cshop_core::pixels::{DeepBuffer, PixelBuffer};
use std::path::Path;

pub use format::ImageFormat;

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Decode(String),
    #[error("unsupported file type: {0}")]
    Unsupported(String),
    #[error("image is {0}x{1}, which exceeds the {2} pixel limit")]
    TooLarge(u32, u32, u32),
    /// The file's own structure is wrong: truncated, mis-signed, or claiming
    /// something inconsistent. Kept separate from `Decode` so a corrupt
    /// project file can be reported differently from an image the decoder
    /// merely did not like.
    #[error("this file is damaged or not what it claims to be: {0}")]
    Malformed(String),
}

/// Refuses anything larger than this in either dimension.
///
/// Not a GPU limit — it is a guard against a malformed header claiming a
/// gigantic size and provoking a multi-gigabyte allocation before decoding
/// even starts.
pub const MAX_DIMENSION: u32 = 65_536;

/// Load a layered document: a project, a PSD, or any flat image as a single
/// layer.
///
/// Which one it is comes from the file's own bytes where they say so, and from
/// the extension otherwise — a project renamed to `.png` still opens.
pub fn load_document(path: &std::path::Path) -> Result<cshop_core::document::Document, IoError> {
    Ok(load_document_reporting(path)?.0)
}

/// As [`load_document`], and also what had to be done to the colours on the
/// way in — which is worth telling someone about rather than doing quietly.
pub fn load_document_reporting(
    path: &std::path::Path,
) -> Result<(cshop_core::document::Document, Colors), IoError> {
    let bytes = std::fs::read(path)?;
    let (mut doc, colors) = decode_document_reporting(&bytes, Some(path))?;
    doc.name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| doc.name.clone());
    doc.path = Some(path.to_path_buf());
    Ok((doc, colors))
}

/// Decode a layered document from memory.
pub fn decode_document(
    bytes: &[u8],
    hint: Option<&std::path::Path>,
) -> Result<cshop_core::document::Document, IoError> {
    Ok(decode_document_reporting(bytes, hint)?.0)
}

/// As [`decode_document`], reporting what happened to the colours.
pub fn decode_document_reporting(
    bytes: &[u8],
    hint: Option<&std::path::Path>,
) -> Result<(cshop_core::document::Document, Colors), IoError> {
    if bytes.starts_with(b"CSHOP\0") {
        return project::read(bytes).map(|d| (d, Colors::default()));
    }
    if bytes.starts_with(b"8BPS") {
        return psd::read(bytes).map(|d| (d, Colors::default()));
    }
    // A raw file is developed on the way in, at sixteen bits: narrowing there
    // would throw away the reason for shooting raw. The defaults are the
    // camera's own — its white balance and its colour matrix — which is the
    // picture the camera would have made.
    if raw::is_raw(bytes, hint) {
        let developed = raw::read(bytes)?.develop(raw::Develop::default());
        return Ok((
            document_from_deep(developed, "Raw"),
            Colors { converted: true, ..Default::default() },
        ));
    }

    // Vector in, vector out: an SVG becomes shape layers, so what comes back
    // from a round trip is editable geometry rather than a picture of it.
    if svg::is_svg(bytes) {
        let drawing = svg::read(bytes)?;
        return Ok((document_from_svg(drawing), Colors::default()));
    }

    // An animation becomes a layer per frame with a timeline over them, since
    // opening one and getting its first frame is the worst kind of not
    // supporting something: the file opens, looks right, and is not what was
    // in it.
    if frames::is_animation(bytes) {
        return frames::read(bytes).map(|a| (document_from_animation(a), Colors::default()));
    }

    // Not layered: one image becomes one background layer, in the working
    // space every new document starts in — and at the depth the file holds,
    // rather than narrowed on the way in and called sixteen bits on the way
    // out.
    let working = cshop_core::profile::Profile::srgb();
    let deep_file = is_deep(bytes, hint);
    let (surface, colors, w, h) = if deep_file {
        let (deep, colors) = decode_deep(bytes, hint, &working)?;
        let (w, h) = (deep.width(), deep.height());
        (cshop_core::layer::Surface::Sixteen(deep), colors, w, h)
    } else {
        let (pixels, colors) = decode_managed(bytes, hint, &working)?;
        let (w, h) = (pixels.width(), pixels.height());
        (cshop_core::layer::Surface::Eight(pixels), colors, w, h)
    };
    let mut doc = cshop_core::document::Document::new(
        "Untitled",
        w,
        h,
        cshop_core::document::Background::Transparent,
    );
    doc.tree = Default::default();
    let id = doc.tree.alloc_id();
    let mut layer = cshop_core::layer::Layer::new(
        id,
        "Background",
        cshop_core::layer::LayerKind::Raster(surface),
    );
    layer.is_background = true;
    doc.tree.push(layer, None);
    doc.active = doc.tree.root().last().copied();
    doc.selected_layers = doc.active.into_iter().collect();
    doc.modified = false;
    Ok((doc, colors))
}

/// A sixteen-bit picture as a one-layer document.
fn document_from_deep(
    deep: cshop_core::pixels::DeepBuffer,
    name: &str,
) -> cshop_core::document::Document {
    let (w, h) = (deep.width(), deep.height());
    let mut doc = cshop_core::document::Document::new(
        "Untitled",
        w.max(1),
        h.max(1),
        cshop_core::document::Background::Transparent,
    );
    doc.tree = Default::default();
    let id = doc.tree.alloc_id();
    let mut layer = cshop_core::layer::Layer::new(
        id,
        name,
        cshop_core::layer::LayerKind::Raster(cshop_core::layer::Surface::Sixteen(deep)),
    );
    layer.is_background = true;
    doc.tree.push(layer, None);
    doc.active = doc.tree.root().last().copied();
    doc.selected_layers = doc.active.into_iter().collect();
    doc.modified = false;
    doc
}

/// A drawing as a document: one shape layer per element.
fn document_from_svg(drawing: svg::Drawing) -> cshop_core::document::Document {
    let mut doc = cshop_core::document::Document::new(
        "Untitled",
        drawing.width.max(1),
        drawing.height.max(1),
        cshop_core::document::Background::Transparent,
    );
    doc.tree = Default::default();
    for (i, shape) in drawing.shapes.into_iter().enumerate() {
        let Some(rendered) = cshop_core::layer::ShapeLayer::new(shape.content) else {
            continue;
        };
        let id = doc.tree.alloc_id();
        let mut layer = cshop_core::layer::Layer::new(
            id,
            shape.name.unwrap_or_else(|| format!("Shape {}", i + 1)),
            cshop_core::layer::LayerKind::Shape(Box::new(rendered)),
        );
        layer.offset = shape.offset;
        doc.tree.push(layer, None);
    }
    if doc.tree.is_empty() {
        // Nothing drawable in it. An empty document with the right size is
        // more use than an error, since the file may be all text or all
        // gradients — which the caller is told about separately.
        let id = doc.tree.alloc_id();
        doc.tree.push(
            cshop_core::layer::Layer::new(
                id,
                "Empty",
                cshop_core::layer::LayerKind::raster(cshop_core::pixels::PixelBuffer::new(
                    drawing.width.max(1),
                    drawing.height.max(1),
                )),
            ),
            None,
        );
    }
    doc.active = doc.tree.root().last().copied();
    doc.selected_layers = doc.active.into_iter().collect();
    doc.modified = false;
    doc
}

/// An animation as a document: one layer per frame, and a timeline saying
/// which is which.
///
/// Only the first frame is left visible, so opening one shows the animation's
/// first moment rather than every frame stacked on top of each other.
fn document_from_animation(animation: frames::Animation) -> cshop_core::document::Document {
    let (w, h) = animation.size();
    let mut doc = cshop_core::document::Document::new(
        "Untitled",
        w.max(1),
        h.max(1),
        cshop_core::document::Background::Transparent,
    );
    doc.tree = Default::default();

    let mut timeline = cshop_core::timeline::Timeline {
        frames: Vec::with_capacity(animation.frames.len()),
        loops: animation.loops,
        current: 0,
    };
    for (i, frame) in animation.frames.into_iter().enumerate() {
        let id = doc.tree.alloc_id();
        let mut layer = cshop_core::layer::Layer::new(
            id,
            format!("Frame {}", i + 1),
            cshop_core::layer::LayerKind::raster(frame.pixels),
        );
        layer.visible = i == 0;
        doc.tree.push(layer, None);
        timeline
            .frames
            .push(cshop_core::timeline::Frame { layer: id, delay_ms: frame.delay_ms });
    }
    doc.active = doc.tree.root().first().copied();
    doc.selected_layers = doc.active.into_iter().collect();
    doc.timeline = Some(timeline);
    doc.modified = false;
    doc
}

/// Write a layered document. `composite` is the flattened image, which PSD
/// carries so other programs can show something without reading layers.
pub fn save_document(
    path: &std::path::Path,
    doc: &cshop_core::document::Document,
    composite: &PixelBuffer,
) -> Result<(), IoError> {
    let format = ImageFormat::from_path(path)
        .ok_or_else(|| IoError::Unsupported(path.display().to_string()))?;
    let bytes = match format {
        ImageFormat::Cshop => project::write(doc),
        ImageFormat::Psd => psd::write(doc, composite)?,
        ImageFormat::Svg => svg::write(doc, composite)?,
        ImageFormat::Pdf => pdf::write(composite, doc.dpi)?,
        // Everything else is flat, so only the composite goes out.
        other => {
            let _ = other;
            return save(path, composite, 92);
        }
    };
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Decode an image file into a pixel buffer.
///
/// Whatever the source format, the result is 8-bit straight-alpha sRGB, which
/// is what a raster layer holds.
pub fn load(path: &Path) -> Result<PixelBuffer, IoError> {
    let bytes = std::fs::read(path)?;
    decode(&bytes, Some(path))
}

/// Decode from memory. `hint` supplies a filename, used both for error
/// messages and to identify formats that carry no magic bytes.
///
/// Colours are read as sRGB, which is the right assumption for a file that
/// does not say otherwise and the wrong one for a file that does. Prefer
/// [`decode_managed`] anywhere the answer matters.
pub fn decode(bytes: &[u8], hint: Option<&Path>) -> Result<PixelBuffer, IoError> {
    Ok(decode_managed(bytes, hint, &Profile::srgb())?.0)
}

/// What a file said about its own colours, and what was done about it.
///
/// Worth reporting rather than swallowing: converting a picture on the way in
/// is the correct thing to do and also the thing most likely to surprise
/// someone comparing the result against another program.
#[derive(Debug, Default, Clone)]
pub struct Colors {
    /// The profile the file carried, if it carried one.
    pub embedded: Option<Profile>,
    /// Set when the pixels were re-encoded into the working space.
    pub converted: bool,
    /// Set when the file was four inks rather than three colours.
    pub separated: bool,
    /// Set when ink had to be read without a profile to say what it meant.
    pub guessed: bool,
}

impl Colors {
    /// One line for a report, or nothing when there is nothing to say.
    pub fn note(&self) -> Option<String> {
        let name = self.embedded.as_ref().map(|p| p.name().to_string());
        match (self.separated, self.converted, name) {
            (true, _, Some(n)) => Some(format!("four inks, converted from {n}")),
            (true, _, None) => Some("four inks, converted without a profile to go by".into()),
            (false, true, Some(n)) => Some(format!("converted from {n}")),
            _ => None,
        }
    }
}

/// Decode from memory into `working`, honouring whatever the file says about
/// its own colours.
///
/// Three things can happen. A file with no profile is taken at its word as
/// sRGB and left alone. A file with an RGB profile is re-encoded into the
/// working space, so its colours look the same here as they did wherever it
/// came from. A file made of ink is read as ink and asked what it prints as —
/// see [`crate::cmyk`].
pub fn decode_managed(
    bytes: &[u8],
    hint: Option<&Path>,
    working: &Profile,
) -> Result<(PixelBuffer, Colors), IoError> {
    let mut colors = Colors {
        embedded: icc::embedded(bytes).and_then(|b| Profile::parse(&b).ok()),
        ..Default::default()
    };

    if cmyk::is_separated(bytes) {
        colors.separated = true;
        let inks = cmyk::read(bytes)?;
        if inks.width > MAX_DIMENSION || inks.height > MAX_DIMENSION {
            return Err(IoError::TooLarge(inks.width, inks.height, MAX_DIMENSION));
        }
        let press = colors.embedded.clone().filter(|p| p.space() == Space::Cmyk);
        let pixels = match press {
            Some(press) => {
                colors.converted = true;
                press
                    .inks_to_rgba8(working, &inks.data, RenderingIntent::RelativeColorimetric)
                    .map_err(|e| IoError::Decode(e.to_string()))?
            }
            None => {
                // No profile, so no way to know which press. The old formula
                // is the only thing left, and it is a guess rather than a
                // conversion — which is exactly what gets reported.
                colors.guessed = true;
                naive_inks(&inks.data)
            }
        };
        return PixelBuffer::from_pixels(inks.width, inks.height, pixels)
            .map(|p| (p, colors))
            .ok_or_else(|| IoError::Decode("ink and size disagreed".into()));
    }

    // Check the declared size before decoding, so a hostile header cannot make
    // us allocate first and fail second.
    if let Ok((w, h)) = reader_for(bytes, hint)?.into_dimensions() {
        if w > MAX_DIMENSION || h > MAX_DIMENSION {
            return Err(IoError::TooLarge(w, h, MAX_DIMENSION));
        }
    }

    let img = reader_for(bytes, hint)?.decode().map_err(|e| IoError::Decode(e.to_string()))?;

    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut pixels = PixelBuffer::from_rgba_bytes(w, h, rgba.as_raw())
        .ok_or_else(|| IoError::Decode("decoder returned a malformed buffer".into()))?;

    if let Some(from) = colors.embedded.as_ref() {
        if from.space() == Space::Rgb && !from.same_transform(working) {
            from.convert_rgba8(
                working,
                pixels.pixels_mut(),
                RenderingIntent::RelativeColorimetric,
            )
            .map_err(|e| IoError::Decode(e.to_string()))?;
            colors.converted = true;
        }
    }
    Ok((pixels, colors))
}

/// Does this file hold more than eight bits a channel?
///
/// Worth asking before decoding, because the answer decides whether opening it
/// at eight bits would throw anything away.
pub fn is_deep(bytes: &[u8], hint: Option<&Path>) -> bool {
    let Ok(reader) = reader_for(bytes, hint) else { return false };
    let Ok(decoder) = reader.into_decoder() else { return false };
    use image::ImageDecoder;
    matches!(
        decoder.original_color_type(),
        image::ExtendedColorType::L16
            | image::ExtendedColorType::La16
            | image::ExtendedColorType::Rgb16
            | image::ExtendedColorType::Rgba16
    )
}

/// Decode at sixteen bits a channel, honouring the file's own profile.
///
/// A file that only has eight is widened exactly rather than refused: the
/// caller asked for a deep buffer and gets one, holding precisely what the
/// file held. What it buys is everything that happens next — see
/// [`cshop_core::color::Rgba16`].
pub fn decode_deep(
    bytes: &[u8],
    hint: Option<&Path>,
    working: &Profile,
) -> Result<(DeepBuffer, Colors), IoError> {
    let mut colors = Colors {
        embedded: icc::embedded(bytes).and_then(|b| Profile::parse(&b).ok()),
        ..Default::default()
    };

    if cmyk::is_separated(bytes) {
        colors.separated = true;
        let inks = cmyk::read(bytes)?;
        if inks.width > MAX_DIMENSION || inks.height > MAX_DIMENSION {
            return Err(IoError::TooLarge(inks.width, inks.height, MAX_DIMENSION));
        }
        // Deep ink where the file had it, widened where it did not.
        let deep: Vec<u16> = match &inks.deep {
            Some(d) => d.clone(),
            None => inks.data.iter().map(|&v| v as u16 * 257).collect(),
        };
        let press = colors.embedded.clone().filter(|p| p.space() == Space::Cmyk);
        let pixels = match press {
            Some(press) => {
                colors.converted = true;
                press
                    .inks16_to_rgba16(working, &deep, RenderingIntent::RelativeColorimetric)
                    .map_err(|e| IoError::Decode(e.to_string()))?
            }
            None => {
                colors.guessed = true;
                naive_inks(&inks.data).into_iter().map(Rgba16::from_rgba8).collect()
            }
        };
        return DeepBuffer::from_pixels(inks.width, inks.height, pixels)
            .map(|p| (p, colors))
            .ok_or_else(|| IoError::Decode("ink and size disagreed".into()));
    }

    if let Ok((w, h)) = reader_for(bytes, hint)?.into_dimensions() {
        if w > MAX_DIMENSION || h > MAX_DIMENSION {
            return Err(IoError::TooLarge(w, h, MAX_DIMENSION));
        }
    }
    let img = reader_for(bytes, hint)?.decode().map_err(|e| IoError::Decode(e.to_string()))?;
    let rgba = img.to_rgba16();
    let (w, h) = rgba.dimensions();
    let data: Vec<Rgba16> =
        rgba.pixels().map(|p| Rgba16::new(p.0[0], p.0[1], p.0[2], p.0[3])).collect();
    let mut pixels = DeepBuffer::from_pixels(w, h, data)
        .ok_or_else(|| IoError::Decode("decoder returned a malformed buffer".into()))?;

    if let Some(from) = colors.embedded.as_ref() {
        if from.space() == Space::Rgb && !from.same_transform(working) {
            from.convert_rgba16(
                working,
                pixels.pixels_mut(),
                RenderingIntent::RelativeColorimetric,
            )
            .map_err(|e| IoError::Decode(e.to_string()))?;
            colors.converted = true;
        }
    }
    Ok((pixels, colors))
}

/// Encode a deep buffer at sixteen bits a channel.
///
/// Only three formats can hold it: PNG, TIFF, and TIFF again for ink. Anything
/// else is refused rather than quietly narrowed, because a caller that asked
/// for depth and silently got eight bits is worse off than one that was told.
pub fn encode_deep(
    pixels: &DeepBuffer,
    format: ImageFormat,
    working: &Profile,
    out: &Profile,
) -> Result<Vec<u8>, IoError> {
    use image::ImageEncoder;

    if out.space() == Space::Cmyk {
        let deep = working
            .rgba16_to_inks16(out, pixels.pixels(), RenderingIntent::RelativeColorimetric)
            .map_err(|e| IoError::Decode(e.to_string()))?;
        let inks = cmyk::Inks {
            width: pixels.width(),
            height: pixels.height(),
            data: deep.iter().map(|&v| (v >> 8) as u8).collect(),
            deep: Some(deep),
        };
        return cmyk::write_tiff(&inks, out.bytes());
    }
    if out.space() != Space::Rgb {
        return Err(IoError::Unsupported(format!("exporting to {}", out.space().name())));
    }

    let converted;
    let source = if working.same_transform(out) {
        pixels
    } else {
        let mut copy = pixels.clone();
        working
            .convert_rgba16(out, copy.pixels_mut(), RenderingIntent::RelativeColorimetric)
            .map_err(|e| IoError::Decode(e.to_string()))?;
        converted = copy;
        &converted
    };

    let (w, h) = (source.width(), source.height());
    let raw: &[u8] = bytemuck::cast_slice(source.pixels());
    let icc = out.bytes().to_vec();
    let mut cursor = std::io::Cursor::new(Vec::new());
    let fail = |e: image::ImageError| IoError::Decode(e.to_string());

    match format {
        ImageFormat::Png => {
            let mut enc = image::codecs::png::PngEncoder::new(&mut cursor);
            let _ = enc.set_icc_profile(icc);
            enc.write_image(raw, w, h, image::ExtendedColorType::Rgba16).map_err(fail)?;
        }
        ImageFormat::Tiff => {
            let mut enc = image::codecs::tiff::TiffEncoder::new(&mut cursor);
            let _ = enc.set_icc_profile(icc);
            enc.write_image(raw, w, h, image::ExtendedColorType::Rgba16).map_err(fail)?;
        }
        other => {
            return Err(IoError::Unsupported(format!(
                "{} cannot hold sixteen bits a channel; PNG and TIFF can",
                other.display_name()
            )))
        }
    }
    Ok(cursor.into_inner())
}

/// The conversion every program used before profiles: subtract the ink from
/// the light and hope the press agrees. It rarely does — the result is flat
/// and dark against a managed conversion — but it beats refusing to open a
/// file that never said which press it was for.
fn naive_inks(inks: &[u8]) -> Vec<Rgba8> {
    inks.chunks_exact(4)
        .map(|c| {
            let k = 255 - c[3] as u32;
            let mix = |v: u8| (((255 - v as u32) * k) / 255) as u8;
            Rgba8::opaque(mix(c[0]), mix(c[1]), mix(c[2]))
        })
        .collect()
}

/// Build a reader with the format resolved.
///
/// Magic-byte sniffing comes first, but TGA — and to a lesser degree ICO — have
/// no reliable signature, so the filename extension is the fallback. Without
/// it, opening a `.tga` fails outright.
fn reader_for<'a>(
    bytes: &'a [u8],
    hint: Option<&Path>,
) -> Result<image::ImageReader<std::io::Cursor<&'a [u8]>>, IoError> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| IoError::Decode(e.to_string()))?;

    if reader.format().is_none() {
        match hint.and_then(ImageFormat::from_path).and_then(|f| f.to_image_crate()) {
            Some(f) => reader.set_format(f),
            None => {
                let name = hint
                    .and_then(|p| p.extension())
                    .map(|e| e.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unrecognised data".into());
                return Err(IoError::Unsupported(name));
            }
        }
    }
    Ok(reader)
}

/// Encode and write, choosing the format from the file extension.
pub fn save(path: &Path, pixels: &PixelBuffer, quality: u8) -> Result<(), IoError> {
    let srgb = Profile::srgb();
    save_managed(path, pixels, quality, &srgb, &srgb)
}

/// Encode and write, converting out of `working` into `out`.
pub fn save_managed(
    path: &Path,
    pixels: &PixelBuffer,
    quality: u8,
    working: &Profile,
    out: &Profile,
) -> Result<(), IoError> {
    let format = ImageFormat::from_path(path)
        .ok_or_else(|| IoError::Unsupported(path.display().to_string()))?;
    let bytes = encode_managed(pixels, format, quality, working, out)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Encode to memory.
///
/// `quality` is only meaningful for JPEG. Formats without an alpha channel get
/// the image composited onto white first, because dropping alpha outright
/// turns transparent regions black.
///
/// Colours go out as sRGB and say so. [`encode_managed`] is the way to send
/// them somewhere else.
pub fn encode(pixels: &PixelBuffer, format: ImageFormat, quality: u8) -> Result<Vec<u8>, IoError> {
    let srgb = Profile::srgb();
    encode_managed(pixels, format, quality, &srgb, &srgb)
}

/// Encode, converting from the `working` space into `out` and saying in the
/// file which one that was.
///
/// An exported file that does not name its space is one that the next program
/// has to guess about, so the profile is embedded wherever the format has
/// somewhere to put it: PNG, JPEG, TIFF and WebP do, and BMP, TGA, GIF and ICO
/// do not, which is worth knowing before choosing one for a picture whose
/// colours matter.
///
/// If `out` is a CMYK profile the result is a TIFF of four inks, whatever
/// `format` said, because that is the only thing here that can hold them.
pub fn encode_managed(
    pixels: &PixelBuffer,
    format: ImageFormat,
    quality: u8,
    working: &Profile,
    out: &Profile,
) -> Result<Vec<u8>, IoError> {
    if out.space() == Space::Cmyk {
        let data = working
            .rgba8_to_inks(out, pixels.pixels(), RenderingIntent::RelativeColorimetric)
            .map_err(|e| IoError::Decode(e.to_string()))?;
        let inks = cmyk::Inks {
            width: pixels.width(),
            height: pixels.height(),
            data,
            deep: None,
        };
        return cmyk::write_tiff(&inks, out.bytes());
    }
    if out.space() != Space::Rgb {
        return Err(IoError::Unsupported(format!("exporting to {}", out.space().name())));
    }

    // Convert first, then flatten: compositing onto white has to happen in the
    // space the white belongs to, and that is the one being written.
    let converted;
    let source = if working.same_transform(out) {
        pixels
    } else {
        let mut copy = pixels.clone();
        working
            .convert_rgba8(out, copy.pixels_mut(), RenderingIntent::RelativeColorimetric)
            .map_err(|e| IoError::Decode(e.to_string()))?;
        converted = copy;
        &converted
    };

    let flattened;
    let source = if format.supports_alpha() {
        source
    } else {
        flattened = flatten_onto_white(source);
        &flattened
    };

    let buf: image::RgbaImage =
        image::ImageBuffer::from_raw(source.width(), source.height(), source.as_bytes().to_vec())
            .ok_or_else(|| IoError::Decode("pixel buffer had the wrong length".into()))?;

    let icc = out.bytes().to_vec();
    let mut cursor = std::io::Cursor::new(Vec::new());
    let (w, h) = (source.width(), source.height());
    let fail = |e: image::ImageError| IoError::Decode(e.to_string());

    match format {
        ImageFormat::Jpeg => {
            use image::ImageEncoder;
            let rgb = image::DynamicImage::ImageRgba8(buf).to_rgb8();
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut cursor,
                quality.clamp(1, 100),
            );
            let _ = enc.set_icc_profile(icc);
            enc.write_image(&rgb, w, h, image::ExtendedColorType::Rgb8).map_err(fail)?;
        }
        ImageFormat::Png => {
            use image::ImageEncoder;
            let mut enc = image::codecs::png::PngEncoder::new(&mut cursor);
            let _ = enc.set_icc_profile(icc);
            enc.write_image(&buf, w, h, image::ExtendedColorType::Rgba8).map_err(fail)?;
        }
        ImageFormat::Tiff => {
            use image::ImageEncoder;
            let mut enc = image::codecs::tiff::TiffEncoder::new(&mut cursor);
            let _ = enc.set_icc_profile(icc);
            enc.write_image(&buf, w, h, image::ExtendedColorType::Rgba8).map_err(fail)?;
        }
        other => {
            // The rest have nowhere to put a profile, so the pixels are simply
            // written in the space they were converted to.
            let f = other.to_image_crate().ok_or_else(|| {
                IoError::Unsupported(format!("{} encoding", other.display_name()))
            })?;
            image::DynamicImage::ImageRgba8(buf).write_to(&mut cursor, f).map_err(fail)?;
        }
    }
    Ok(cursor.into_inner())
}

/// Composite over white, for formats that cannot carry alpha.
fn flatten_onto_white(src: &PixelBuffer) -> PixelBuffer {
    let mut out = PixelBuffer::new(src.width(), src.height());
    for (dst, px) in out.pixels_mut().iter_mut().zip(src.pixels()) {
        let a = px.a as u32;
        let mix = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        *dst = Rgba8::opaque(mix(px.r), mix(px.g), mix(px.b));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PixelBuffer {
        let mut p = PixelBuffer::filled(8, 4, Rgba8::opaque(200, 100, 50));
        p.set(0, 0, Rgba8::new(10, 20, 30, 128));
        p.set(7, 3, Rgba8::TRANSPARENT);
        p
    }

    #[test]
    fn png_round_trips_exactly_including_alpha() {
        let src = sample();
        let bytes = encode(&src, ImageFormat::Png, 100).unwrap();
        let back = decode(&bytes, None).unwrap();
        assert_eq!(back.width(), 8);
        assert_eq!(back, src, "PNG must be lossless, alpha included");
    }

    #[test]
    fn jpeg_drops_alpha_onto_white_rather_than_black() {
        let mut src = PixelBuffer::filled(8, 8, Rgba8::TRANSPARENT);
        src.fill_rect(cshop_core::geom::IRect::at(0, 0, 4, 8), Rgba8::opaque(255, 0, 0));

        let bytes = encode(&src, ImageFormat::Jpeg, 95).unwrap();
        let back = decode(&bytes, None).unwrap();
        let clear = back.get(6, 4);
        assert!(
            clear.r > 240 && clear.g > 240 && clear.b > 240,
            "transparent areas should become white, got {clear:?}"
        );
        assert_eq!(clear.a, 255);
    }

    #[test]
    fn round_trip_through_every_writable_format() {
        let src = PixelBuffer::filled(4, 4, Rgba8::opaque(120, 130, 140));
        for f in ImageFormat::WRITABLE {
            let bytes = encode(&src, *f, 95).unwrap_or_else(|e| panic!("{f:?} encode: {e}"));
            let name = std::path::PathBuf::from(format!("x.{}", f.default_extension()));
            let back =
                decode(&bytes, Some(&name)).unwrap_or_else(|e| panic!("{f:?} decode: {e}"));
            assert_eq!(back.width(), 4, "{f:?} lost its dimensions");
            assert_eq!(back.height(), 4);
        }
    }

    #[test]
    fn signature_less_formats_are_found_by_extension() {
        // TGA carries no magic bytes, so without the filename hint the decoder
        // cannot identify it at all.
        let src = PixelBuffer::filled(4, 4, Rgba8::opaque(1, 2, 3));
        let bytes = encode(&src, ImageFormat::Tga, 100).unwrap();
        assert!(decode(&bytes, None).is_err());
        assert!(decode(&bytes, Some(Path::new("art.tga"))).is_ok());
    }

    #[test]
    fn a_misleading_extension_does_not_override_real_magic_bytes() {
        let bytes = encode(&sample(), ImageFormat::Png, 100).unwrap();
        let back = decode(&bytes, Some(Path::new("actually.tga"))).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn garbage_is_rejected_without_panicking() {
        assert!(decode(b"not an image at all", None).is_err());
        assert!(decode(&[], None).is_err());
    }

    #[test]
    fn truncated_png_is_an_error_not_a_panic() {
        let bytes = encode(&sample(), ImageFormat::Png, 100).unwrap();
        assert!(decode(&bytes[..bytes.len() / 2], None).is_err());
    }
}
