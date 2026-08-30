//! Files whose pixels are ink rather than light.
//!
//! A CMYK file is not a picture of colours; it is an instruction to a press,
//! four numbers saying how much of each ink to lay down. What that looks like
//! depends entirely on the press, which is what the file's profile describes.
//! So there is exactly one correct way to open one — read the inks, read the
//! profile, and ask the profile what they come to — and one common wrong way,
//! which is to treat the four numbers as if they were already a colour. The
//! wrong way is what a file gets if it is opened by something that does not
//! know about profiles, and it is why print files so often arrive looking
//! flat and dark.
//!
//! Going the other way is the same trade in reverse, with one asymmetry worth
//! knowing: ink has no transparency, and paper is not black. See
//! [`cshop_core::profile`] for where that is dealt with.
//!
//! TIFF is the format here. CMYK JPEGs open — plenty of them exist — but
//! nothing writes one, because a four-component JPEG encoder is a great deal
//! of machinery for a format no press asks for any more.

use crate::IoError;

/// Ink samples straight out of a file, before any profile has spoken.
pub struct Inks {
    pub width: u32,
    pub height: u32,
    /// Four samples per pixel, in C M Y K order, with 0 meaning no ink.
    pub data: Vec<u8>,
    /// The same, at sixteen bits, when the file had them to give.
    pub deep: Option<Vec<u16>>,
}

impl Inks {
    fn check(self) -> Result<Inks, IoError> {
        let want = self.width as usize * self.height as usize * 4;
        if self.data.len() != want {
            return Err(IoError::Decode(format!(
                "{}x{} needs {want} ink samples, got {}",
                self.width,
                self.height,
                self.data.len()
            )));
        }
        Ok(self)
    }
}

/// Is this file made of ink? `None` means no, and says nothing about whether
/// it can be opened some other way.
pub fn is_separated(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        jpeg_components(bytes) == Some(4)
    } else if bytes.starts_with(b"II\x2a\x00") || bytes.starts_with(b"MM\x00\x2a") {
        matches!(tiff_colortype(bytes), Some(tiff::ColorType::CMYK(_)))
    } else {
        false
    }
}

/// Read the inks out.
pub fn read(bytes: &[u8]) -> Result<Inks, IoError> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        read_jpeg(bytes)
    } else {
        read_tiff(bytes)
    }
}

// --- JPEG ------------------------------------------------------------------

/// How many components a JPEG's frame header declares. Four means ink.
fn jpeg_components(bytes: &[u8]) -> Option<u8> {
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            at += 2;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let len = u16::from_be_bytes(bytes.get(at + 2..at + 4)?.try_into().ok()?) as usize;
        // Any start-of-frame: baseline, progressive, or one of the rarer ones,
        // but not the four markers in that range that mean something else.
        let is_sof = (0xC0..=0xCF).contains(&marker)
            && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            // length, precision, height, width, then the component count.
            return bytes.get(at + 4 + 5).copied();
        }
        at = at.checked_add(2 + len)?;
    }
    None
}

/// Adobe's `APP14` marker. Its presence is what says the samples are stored
/// inverted, which is a convention rather than anything the JPEG standard
/// asks for — and getting it wrong turns a photograph into its own negative.
fn jpeg_is_adobe(bytes: &[u8]) -> bool {
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return false;
        }
        let marker = bytes[at + 1];
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            at += 2;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            return false;
        }
        let Some(len) = bytes
            .get(at + 2..at + 4)
            .and_then(|b| b.try_into().ok())
            .map(u16::from_be_bytes)
            .map(usize::from)
        else {
            return false;
        };
        if marker == 0xEE && bytes.get(at + 4..at + 9) == Some(b"Adobe") {
            return true;
        }
        match at.checked_add(2 + len) {
            Some(next) => at = next,
            None => return false,
        }
    }
    false
}

fn read_jpeg(bytes: &[u8]) -> Result<Inks, IoError> {
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;

    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::CMYK);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(bytes), opts);
    let mut data = dec.decode().map_err(|e| IoError::Decode(e.to_string()))?;
    let (w, h) = dec
        .dimensions()
        .ok_or_else(|| IoError::Decode("the JPEG never declared a size".into()))?;

    if jpeg_is_adobe(bytes) {
        for v in &mut data {
            *v = 255 - *v;
        }
    }
    Inks { width: w as u32, height: h as u32, data, deep: None }.check()
}

// --- TIFF ------------------------------------------------------------------

fn tiff_colortype(bytes: &[u8]) -> Option<tiff::ColorType> {
    let mut dec = tiff::decoder::Decoder::new(std::io::Cursor::new(bytes)).ok()?;
    dec.colortype().ok()
}

fn read_tiff(bytes: &[u8]) -> Result<Inks, IoError> {
    let mut dec = tiff::decoder::Decoder::new(std::io::Cursor::new(bytes))
        .map_err(|e| IoError::Decode(e.to_string()))?;
    let (w, h) = dec.dimensions().map_err(|e| IoError::Decode(e.to_string()))?;
    if w > crate::MAX_DIMENSION || h > crate::MAX_DIMENSION {
        return Err(IoError::TooLarge(w, h, crate::MAX_DIMENSION));
    }
    let ct = dec.colortype().map_err(|e| IoError::Decode(e.to_string()))?;
    if !matches!(ct, tiff::ColorType::CMYK(_)) {
        return Err(IoError::Decode(format!("{ct:?} is not four inks")));
    }
    match dec.read_image().map_err(|e| IoError::Decode(e.to_string()))? {
        tiff::decoder::DecodingResult::U8(data) => {
            Inks { width: w, height: h, data, deep: None }.check()
        }
        tiff::decoder::DecodingResult::U16(deep) => {
            // Kept at full depth as well as narrowed, so a caller that can use
            // sixteen bits is not made to ask for them twice.
            let data = deep.iter().map(|&v| (v >> 8) as u8).collect();
            Inks { width: w, height: h, data, deep: Some(deep) }.check()
        }
        other => Err(IoError::Decode(format!(
            "ink samples of an unexpected width: {other:?}"
        ))),
    }
}

/// Write four inks as a TIFF, with the profile that says what they mean.
///
/// A CMYK file without its profile is guesswork for whoever opens it next, so
/// the profile is not optional here.
pub fn write_tiff(inks: &Inks, icc: &[u8]) -> Result<Vec<u8>, IoError> {
    use tiff::encoder::{colortype, TiffEncoder};
    use tiff::tags::Tag;

    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut enc = TiffEncoder::new(&mut out).map_err(|e| IoError::Decode(e.to_string()))?;
        let err = |e: tiff::TiffError| IoError::Decode(e.to_string());
        match &inks.deep {
            Some(deep) => {
                let mut img = enc
                    .new_image::<colortype::CMYK16>(inks.width, inks.height)
                    .map_err(err)?;
                img.encoder().write_tag(Tag::Unknown(34675), icc).map_err(err)?;
                img.write_data(deep).map_err(err)?;
            }
            None => {
                let mut img = enc
                    .new_image::<colortype::CMYK8>(inks.width, inks.height)
                    .map_err(err)?;
                img.encoder().write_tag(Tag::Unknown(34675), icc).map_err(err)?;
                img.write_data(&inks.data).map_err(err)?;
            }
        }
    }
    Ok(out.into_inner())
}
