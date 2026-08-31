//! PDF, out only.
//!
//! # Why one direction
//!
//! Writing a PDF that holds a picture is a page, an image object and a
//! content stream that draws it: a few hundred bytes of structure round the
//! image, all of it written here. Reading one is a different proposition —
//! object streams, cross-reference streams, a dozen filters, embedded fonts,
//! and a page description language whose whole job is to be general. That is a
//! project rather than a feature, and pretending otherwise by reading the
//! easy tenth of it would produce a reader that opens some files and silently
//! mangles others.
//!
//! So: a document goes out as a PDF page at its own size, and PDFs do not come
//! in. The menu says which.
//!
//! # What the page holds
//!
//! One image, the composite, at the document's pixel size mapped onto a page
//! measured in points at the document's own resolution — so a 300 dpi document
//! makes a page the size it would print at, rather than a page of the size it
//! happens to be in pixels.

use crate::IoError;
use cshop_core::pixels::PixelBuffer;

/// Write a flattened document as a single-page PDF.
///
/// `dpi` decides how large the page is: pixels divided by dots per inch, times
/// seventy-two points to the inch.
pub fn write(composite: &PixelBuffer, dpi: f32) -> Result<Vec<u8>, IoError> {
    let (w, h) = (composite.width(), composite.height());
    if w == 0 || h == 0 {
        return Err(IoError::Unsupported("there is nothing to write".into()));
    }
    let dpi = if dpi.is_finite() && dpi > 1.0 { dpi } else { 72.0 };
    let page_w = w as f32 * 72.0 / dpi;
    let page_h = h as f32 * 72.0 / dpi;

    // Straight RGB, with any transparency composited onto white. PDF can hold
    // an alpha channel as a soft mask, and a picture that arrives on a page
    // over nothing is a picture over the paper — which is white.
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let c = composite.get(x, y);
            let a = c.a as u32;
            let over = |v: u8| ((v as u32 * a + 255 * (255 - a)) / 255) as u8;
            rgb.extend_from_slice(&[over(c.r), over(c.g), over(c.b)]);
        }
    }
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&rgb, 7);

    let mut out: Vec<u8> = Vec::with_capacity(compressed.len() + 1024);
    let mut offsets: Vec<usize> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");
    // A comment of high bytes, which is how a PDF tells anything transferring
    // it that it is binary and must not be line-ending converted.
    out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");

    let object = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(out.len());
        let n = offsets.len();
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };

    object(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    object(
        &mut out,
        &mut offsets,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    );
    object(
        &mut out,
        &mut offsets,
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] \
             /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>",
            page_w, page_h
        )
        .as_bytes(),
    );

    // The content stream: scale the unit image up to the page and draw it.
    let content = format!("q\n{page_w:.2} 0 0 {page_h:.2} 0 0 cm\n/Im0 Do\nQ\n");
    object(
        &mut out,
        &mut offsets,
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        )
        .as_bytes(),
    );

    offsets.push(out.len());
    let image_obj = offsets.len();
    out.extend_from_slice(format!("{image_obj} 0 obj\n").as_bytes());
    out.extend_from_slice(
        format!(
            "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} \
             /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode \
             /Length {} >>\nstream\n",
            compressed.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&compressed);
    out.extend_from_slice(b"\nendstream\nendobj\n");

    // The cross-reference table, which is what makes the objects findable.
    let xref_at = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", offsets.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for at in &offsets {
        out.extend_from_slice(format!("{at:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cshop_core::color::Rgba8;

    #[test]
    fn a_page_is_written_and_says_it_is_a_pdf() {
        let px = PixelBuffer::filled(64, 48, Rgba8::opaque(200, 40, 40));
        let bytes = write(&px, 72.0).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert!(bytes.ends_with(b"%%EOF\n"));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Type /Catalog"));
        assert!(text.contains("/MediaBox [0 0 64.00 48.00]"), "a page at 72 dpi is its pixels");
        assert!(text.contains("/Width 64 /Height 48"));
        assert!(text.contains("startxref"));
    }

    /// The page is measured in points at the document's own resolution, so a
    /// 300 dpi picture makes a page the size it would print at.
    #[test]
    fn the_page_is_the_size_it_would_print_at() {
        let px = PixelBuffer::filled(300, 600, Rgba8::WHITE);
        let text = String::from_utf8_lossy(&write(&px, 300.0).unwrap()).into_owned();
        // 300 px at 300 dpi is one inch, which is 72 points.
        assert!(text.contains("/MediaBox [0 0 72.00 144.00]"), "{}", &text[..400]);
    }

    /// A picture over nothing is a picture over the paper, and the paper is
    /// white. Leaving transparency as black would be a surprise on a press.
    #[test]
    fn transparency_lands_on_white_rather_than_black() {
        let mut px = PixelBuffer::new(2, 1);
        px.set(0, 0, Rgba8::new(0, 0, 0, 0));
        px.set(1, 0, Rgba8::opaque(0, 0, 0));
        let bytes = write(&px, 72.0).unwrap();
        // Pull the image stream back out and inflate it. Found by its filter
        // rather than by counting streams: "endstream" contains "stream", so
        // counting finds the end of the content stream first.
        let filter = bytes
            .windows(20)
            .position(|w| w == b"/Filter /FlateDecode")
            .expect("the image says how it is compressed");
        let start = bytes[filter..]
            .windows(7)
            .position(|w| w == b"stream\n")
            .map(|i| filter + i + 7)
            .expect("and its data follows");
        let end = bytes[start..]
            .windows(10)
            .position(|w| w == b"\nendstream")
            .map(|i| start + i)
            .expect("and ends");
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&bytes[start..end]).unwrap();
        assert_eq!(&raw[..3], &[255, 255, 255], "the transparent pixel is paper");
        assert_eq!(&raw[3..6], &[0, 0, 0], "and the opaque one is still black");
    }

    #[test]
    fn a_picture_of_nothing_is_refused() {
        assert!(write(&PixelBuffer::new(0, 0), 72.0).is_err());
    }
}
