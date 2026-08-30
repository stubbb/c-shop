//! Finding the colour profile a file is carrying.
//!
//! Every container hides it somewhere different — a PNG chunk, a run of JPEG
//! application segments, a TIFF tag — and none of them is hard to read. What
//! makes this worth doing by hand rather than asking the decoder is that the
//! decoders disagree: a profile written by one program and read by another
//! goes missing often enough that a file can quietly lose the one thing that
//! says what its numbers mean. Scanning the container costs a few hundred
//! microseconds and never guesses.
//!
//! Everything here reads data from outside the program, so it is written to be
//! unfazed by rubbish: every offset is checked, nothing is trusted for a
//! length, and a malformed file yields `None` rather than a panic.

/// Refuse anything larger than this. Real profiles run to a few hundred
/// kilobytes; a claim far past that is a corrupt file or a hostile one, and
/// either way not worth the allocation.
const MAX_PROFILE: usize = 32 << 20;

/// The ICC profile embedded in an encoded image, if it has one.
pub fn embedded(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(bytes)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        jpeg(bytes)
    } else if bytes.starts_with(b"II\x2a\x00") || bytes.starts_with(b"MM\x00\x2a") {
        tiff(bytes)
    } else {
        None
    }
}

/// PNG: an `iCCP` chunk, whose payload is the profile deflated.
fn png(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut at = 8;
    while at + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
        let kind = bytes.get(at + 4..at + 8)?;
        let body = at + 8;
        let end = body.checked_add(len)?;
        if end > bytes.len() || len > MAX_PROFILE {
            return None;
        }
        if kind == b"iCCP" {
            let data = &bytes[body..end];
            // A latin-1 name, a null, then one byte of compression method.
            let nul = data.iter().position(|&b| b == 0)?;
            let method = *data.get(nul + 1)?;
            if method != 0 {
                return None; // deflate is the only method the format defines
            }
            return miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(
                data.get(nul + 2..)?,
                MAX_PROFILE,
            )
            .ok();
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return None; // the profile must precede the pixels
        }
        at = end.checked_add(4)?; // and the chunk's CRC
    }
    None
}

/// JPEG: `APP2` segments introduced by `ICC_PROFILE\0`, numbered because a
/// profile rarely fits in the 64K a segment allows.
fn jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    const TAG: &[u8] = b"ICC_PROFILE\0";
    let mut at = 2;
    let mut chunks: Vec<(u8, &[u8])> = Vec::new();
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            break;
        }
        let marker = bytes[at + 1];
        // Standalone markers carry no length.
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            at += 2;
            continue;
        }
        // Start of scan: the entropy-coded data follows and there are no more
        // headers worth walking.
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let len = u16::from_be_bytes(bytes.get(at + 2..at + 4)?.try_into().ok()?) as usize;
        if len < 2 {
            break;
        }
        let body = at + 4;
        let end = body.checked_add(len - 2)?;
        if end > bytes.len() {
            break;
        }
        if marker == 0xE2 {
            let data = &bytes[body..end];
            if data.len() > TAG.len() + 2 && data.starts_with(TAG) {
                let seq = data[TAG.len()];
                chunks.push((seq, &data[TAG.len() + 2..]));
            }
        }
        at = end;
    }
    if chunks.is_empty() {
        return None;
    }
    chunks.sort_by_key(|(seq, _)| *seq);
    let total: usize = chunks.iter().map(|(_, d)| d.len()).sum();
    if total > MAX_PROFILE {
        return None;
    }
    let mut out = Vec::with_capacity(total);
    for (_, d) in chunks {
        out.extend_from_slice(d);
    }
    Some(out)
}

/// TIFF: tag 34675 in the first directory.
fn tiff(bytes: &[u8]) -> Option<Vec<u8>> {
    const ICC_TAG: u16 = 34675;
    let big = bytes.starts_with(b"MM");
    let u16at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
        Some(if big { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) })
    };
    let u32at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    };

    let ifd = u32at(4)? as usize;
    let count = u16at(ifd)? as usize;
    for i in 0..count {
        let entry = ifd + 2 + i * 12;
        if u16at(entry)? != ICC_TAG {
            continue;
        }
        // Type is nominally UNDEFINED, but writers use BYTE too; either way
        // the samples are one byte each, so the count is the length.
        let len = u32at(entry + 4)? as usize;
        if len == 0 || len > MAX_PROFILE {
            return None;
        }
        // Four bytes or fewer live in the entry itself.
        let start = if len <= 4 { entry + 8 } else { u32at(entry + 8)? as usize };
        return bytes.get(start..start.checked_add(len)?).map(|s| s.to_vec());
    }
    None
}
