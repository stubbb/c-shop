//! # cshop-io
//!
//! Image decoding and encoding.
//!
//! Everything here is pure Rust, so the build needs no system image libraries.
//! PSD — the format that actually matters for interoperability — gets its own
//! hand-written reader and writer in a later phase; this module covers the
//! flat formats.

pub mod bytes;
pub mod format;
pub mod project;
pub mod psd;

use cshop_core::color::Rgba8;
use cshop_core::pixels::PixelBuffer;
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
    let bytes = std::fs::read(path)?;
    let mut doc = decode_document(&bytes, Some(path))?;
    doc.name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| doc.name.clone());
    doc.path = Some(path.to_path_buf());
    Ok(doc)
}

/// Decode a layered document from memory.
pub fn decode_document(
    bytes: &[u8],
    hint: Option<&std::path::Path>,
) -> Result<cshop_core::document::Document, IoError> {
    if bytes.starts_with(b"CSHOP\0") {
        return project::read(bytes);
    }
    if bytes.starts_with(b"8BPS") {
        return psd::read(bytes);
    }
    // Not layered: one image becomes one background layer.
    let pixels = decode(bytes, hint)?;
    let (w, h) = (pixels.width(), pixels.height());
    let mut doc = cshop_core::document::Document::new(
        "Untitled",
        w,
        h,
        cshop_core::document::Background::Transparent,
    );
    doc.tree = Default::default();
    let id = doc.tree.alloc_id();
    let mut layer = cshop_core::layer::Layer::raster(id, "Background", pixels);
    layer.is_background = true;
    doc.tree.push(layer, None);
    doc.active = doc.tree.root().last().copied();
    doc.selected_layers = doc.active.into_iter().collect();
    doc.modified = false;
    Ok(doc)
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
pub fn decode(bytes: &[u8], hint: Option<&Path>) -> Result<PixelBuffer, IoError> {
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
    PixelBuffer::from_rgba_bytes(w, h, rgba.as_raw())
        .ok_or_else(|| IoError::Decode("decoder returned a malformed buffer".into()))
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
    let format = ImageFormat::from_path(path)
        .ok_or_else(|| IoError::Unsupported(path.display().to_string()))?;
    let bytes = encode(pixels, format, quality)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Encode to memory.
///
/// `quality` is only meaningful for JPEG. Formats without an alpha channel get
/// the image composited onto white first, because dropping alpha outright
/// turns transparent regions black.
pub fn encode(
    pixels: &PixelBuffer,
    format: ImageFormat,
    quality: u8,
) -> Result<Vec<u8>, IoError> {
    let flattened;
    let source = if format.supports_alpha() {
        pixels
    } else {
        flattened = flatten_onto_white(pixels);
        &flattened
    };

    let buf: image::RgbaImage =
        image::ImageBuffer::from_raw(source.width(), source.height(), source.as_bytes().to_vec())
            .ok_or_else(|| IoError::Decode("pixel buffer had the wrong length".into()))?;

    let mut out = std::io::Cursor::new(Vec::new());
    match format {
        ImageFormat::Jpeg => {
            let rgb = image::DynamicImage::ImageRgba8(buf).to_rgb8();
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut out,
                quality.clamp(1, 100),
            );
            enc.encode_image(&rgb).map_err(|e| IoError::Decode(e.to_string()))?;
        }
        other => {
            let f = other.to_image_crate().ok_or_else(|| {
                IoError::Unsupported(format!("{} encoding", other.display_name()))
            })?;
            image::DynamicImage::ImageRgba8(buf)
                .write_to(&mut out, f)
                .map_err(|e| IoError::Decode(e.to_string()))?;
        }
    }
    Ok(out.into_inner())
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
