//! Stream filters (ISO 32000-1 §7.4).
//!
//! The image codecs are not decoded here: decoding stops at the first of them
//! and hands back the bytes so far, with the codec's name, for the image code
//! to deal with — a JPEG goes to the JPEG decoder this crate already has.

use super::syntax::{Dict, Object};

/// What decoding a stream produced.
pub struct Decoded {
    pub data: Vec<u8>,
    /// An image codec the data is still encoded in, with its parameters.
    pub image_codec: Option<(Vec<u8>, Option<Dict>)>,
}

/// Bounds a decompression bomb: no single stream inflates past this.
const MAX_DECODED: usize = 512 << 20;

const IMAGE_CODECS: &[&[u8]] = &[b"DCTDecode", b"JPXDecode", b"CCITTFaxDecode", b"JBIG2Decode"];

fn full_name(name: &[u8]) -> &[u8] {
    match name {
        b"AHx" => b"ASCIIHexDecode",
        b"A85" => b"ASCII85Decode",
        b"LZW" => b"LZWDecode",
        b"Fl" => b"FlateDecode",
        b"RL" => b"RunLengthDecode",
        b"DCT" => b"DCTDecode",
        b"CCF" => b"CCITTFaxDecode",
        other => other,
    }
}

/// Run `raw` through the filters a stream's dictionary names. `resolve`
/// follows any indirect parameters.
pub fn decode_stream(
    dict: &Dict,
    raw: &[u8],
    resolve: &dyn Fn(&Object) -> Object,
) -> Result<Decoded, String> {
    let as_list = |o: Option<&Object>| -> Vec<Object> {
        match o.map(resolve) {
            None | Some(Object::Null) => Vec::new(),
            Some(Object::Array(a)) => a.iter().map(resolve).collect(),
            Some(other) => vec![other],
        }
    };
    let filters = as_list(dict.get(b"Filter").or_else(|| dict.get(b"F")));
    let parms = as_list(dict.get(b"DecodeParms").or_else(|| dict.get(b"DP")));
    let mut data = raw.to_vec();
    for (i, f) in filters.iter().enumerate() {
        let Some(name) = f.as_name() else { continue };
        let name = full_name(name);
        let p = parms.get(i).and_then(|p| p.as_dict()).cloned();
        if IMAGE_CODECS.contains(&name) {
            return Ok(Decoded { data, image_codec: Some((name.to_vec(), p)) });
        }
        data = decode(name, &data, p.as_ref())?;
    }
    Ok(Decoded { data, image_codec: None })
}

pub fn decode(name: &[u8], data: &[u8], parms: Option<&Dict>) -> Result<Vec<u8>, String> {
    let int = |k: &[u8], default: i64| parms.and_then(|p| p.get(k)).and_then(|v| v.as_int()).unwrap_or(default);
    match name {
        b"FlateDecode" => predict(flate(data)?, parms),
        b"LZWDecode" => predict(lzw(data, int(b"EarlyChange", 1) != 0)?, parms),
        b"ASCIIHexDecode" => Ok(ascii_hex(data)),
        b"ASCII85Decode" => ascii85(data),
        b"RunLengthDecode" => Ok(run_length(data)),
        b"Crypt" => Ok(data.to_vec()),
        other => Err(format!("unsupported filter /{}", String::from_utf8_lossy(other))),
    }
}

/// Inflate, keeping whatever comes out of a damaged or headerless stream.
pub fn flate(data: &[u8]) -> Result<Vec<u8>, String> {
    let (out, complete) = inflate(data, true);
    if complete || !out.is_empty() {
        return Ok(out);
    }
    let (out, _) = inflate(data, false);
    if out.is_empty() {
        return Err("FlateDecode: the data does not inflate".into());
    }
    Ok(out)
}

/// Inflate as far as the data allows, and say whether it got to the end.
///
/// Driven by hand rather than through `decompress_to_vec`, whose error hands
/// back its whole working buffer — the inflated bytes and the zeros after
/// them, with no way to tell where one stops.
fn inflate(data: &[u8], zlib: bool) -> (Vec<u8>, bool) {
    use miniz_oxide::inflate::core::{decompress, inflate_flags, DecompressorOxide};
    use miniz_oxide::inflate::TINFLStatus;
    let mut flags = inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    if zlib {
        flags |= inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER;
    }
    let mut out = vec![0u8; data.len().saturating_mul(4).clamp(1024, MAX_DECODED)];
    let mut state = Box::<DecompressorOxide>::default();
    let mut input = data;
    let mut at = 0;
    loop {
        let (status, read, written) = decompress(&mut state, input, &mut out, at, flags);
        at += written;
        input = &input[read.min(input.len())..];
        match status {
            TINFLStatus::Done => {
                out.truncate(at);
                return (out, true);
            }
            TINFLStatus::HasMoreOutput if out.len() < MAX_DECODED => {
                let grown = out.len().saturating_mul(2).min(MAX_DECODED);
                out.resize(grown, 0);
            }
            // Truncated, corrupt, a bad checksum or too large: what came out
            // before the trouble is still good.
            _ => {
                out.truncate(at);
                return (out, false);
            }
        }
    }
}

/// PNG and TIFF predictors (§7.4.4.4).
pub fn predict(data: Vec<u8>, parms: Option<&Dict>) -> Result<Vec<u8>, String> {
    let int = |k: &[u8], default: i64| parms.and_then(|p| p.get(k)).and_then(|v| v.as_int()).unwrap_or(default);
    let predictor = int(b"Predictor", 1);
    if predictor <= 1 {
        return Ok(data);
    }
    let colors = int(b"Colors", 1).clamp(1, 32) as usize;
    let bpc = int(b"BitsPerComponent", 8).clamp(1, 16) as usize;
    let columns = int(b"Columns", 1).clamp(1, 1 << 24) as usize;
    let bpp = (colors * bpc).div_ceil(8).max(1);
    let row_len = (colors * bpc * columns).div_ceil(8);
    if predictor == 2 {
        if bpc != 8 {
            return Err("the TIFF predictor is only implemented for 8 bits".into());
        }
        let mut out = data;
        for row in out.chunks_mut(row_len) {
            for i in bpp..row.len() {
                row[i] = row[i].wrapping_add(row[i - bpp]);
            }
        }
        return Ok(out);
    }
    let mut out = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; row_len];
    for chunk in data.chunks(row_len + 1) {
        let kind = chunk[0];
        let mut row = chunk[1..].to_vec();
        row.resize(row_len, 0);
        for i in 0..row_len {
            let a = if i >= bpp { row[i - bpp] } else { 0 };
            let b = prev[i];
            let c = if i >= bpp { prev[i - bpp] } else { 0 };
            let pred = match kind {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => {
                    let p = a as i16 + b as i16 - c as i16;
                    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                other => return Err(format!("bad PNG row filter {other}")),
            };
            row[i] = row[i].wrapping_add(pred);
        }
        out.extend_from_slice(&row);
        prev = row;
    }
    Ok(out)
}

pub fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2);
    let mut high: Option<u8> = None;
    for &c in data {
        if c == b'>' {
            break;
        }
        if let Some(v) = super::syntax::hex_value(c) {
            match high.take() {
                Some(h) => out.push(h << 4 | v),
                None => high = Some(v),
            }
        }
    }
    if let Some(h) = high {
        out.push(h << 4);
    }
    out
}

pub fn ascii85(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut body: Vec<u8> = data.iter().copied().filter(|c| !super::syntax::is_white(*c)).collect();
    if body.starts_with(b"<~") {
        body.drain(..2);
    }
    if let Some(end) = body.windows(2).position(|w| w == b"~>") {
        body.truncate(end);
    }
    let mut out = Vec::with_capacity(body.len() * 4 / 5);
    let mut group: Vec<u32> = Vec::with_capacity(5);
    for c in body {
        if c == b'z' && group.is_empty() {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        if !(b'!'..=b'u').contains(&c) {
            return Err(format!("ASCII85Decode: bad character {:?}", c as char));
        }
        group.push((c - b'!') as u32);
        if group.len() == 5 {
            let n = group.iter().fold(0u64, |n, &v| n * 85 + v as u64) as u32;
            out.extend_from_slice(&n.to_be_bytes());
            group.clear();
        }
    }
    if !group.is_empty() {
        let keep = group.len() - 1;
        group.resize(5, 84);
        let n = group.iter().fold(0u64, |n, &v| n * 85 + v as u64) as u32;
        out.extend_from_slice(&n.to_be_bytes()[..keep]);
    }
    Ok(out)
}

pub fn lzw(data: &[u8], early_change: bool) -> Result<Vec<u8>, String> {
    let early = early_change as usize;
    let mut out = Vec::new();
    let mut table: Vec<Vec<u8>> = (0..=255u8).map(|b| vec![b]).collect();
    table.push(Vec::new()); // 256: clear
    table.push(Vec::new()); // 257: end of data
    let mut width = 9;
    let mut prev: Option<Vec<u8>> = None;
    let mut bits: u32 = 0;
    let mut nbits = 0;
    for &byte in data {
        bits = (bits << 8) | byte as u32;
        nbits += 8;
        while nbits >= width {
            nbits -= width;
            let code = ((bits >> nbits) & ((1 << width) - 1)) as usize;
            bits &= (1 << nbits) - 1;
            if code == 256 {
                table.truncate(258);
                width = 9;
                prev = None;
                continue;
            }
            if code == 257 {
                return Ok(out);
            }
            let entry = if code < table.len() {
                let e = table[code].clone();
                if let Some(p) = &prev {
                    let mut n = p.clone();
                    n.push(e[0]);
                    table.push(n);
                }
                e
            } else if let (Some(p), true) = (&prev, code == table.len()) {
                let mut e = p.clone();
                e.push(p[0]);
                table.push(e.clone());
                e
            } else {
                return Err(format!("LZWDecode: bad code {code}"));
            };
            out.extend_from_slice(&entry);
            if out.len() > MAX_DECODED {
                return Err("LZWDecode: the stream decodes to too much".into());
            }
            prev = Some(entry);
            if table.len() + early >= (1 << width) && width < 12 {
                width += 1;
            }
        }
    }
    Ok(out)
}

pub fn run_length(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let n = data[i] as usize;
        if n == 128 {
            break;
        }
        if n < 128 {
            let end = (i + 2 + n).min(data.len());
            out.extend_from_slice(&data[(i + 1).min(end)..end]);
            i += 2 + n;
        } else if let Some(&b) = data.get(i + 1) {
            out.extend(std::iter::repeat_n(b, 257 - n));
            i += 2;
        } else {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lzw_example_from_the_specification() {
        // ISO 32000-1 §7.4.4.2: "-----A---B" encodes to these nine bytes.
        let encoded = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        assert_eq!(lzw(&encoded, true).unwrap(), b"-----A---B");
    }

    #[test]
    fn ascii85_hex_and_run_length() {
        assert_eq!(ascii85(b"<~87cURD]j7B\nEbo7~>").unwrap(), b"Hello world");
        assert_eq!(ascii85(b"z~>").unwrap(), [0, 0, 0, 0]);
        assert_eq!(ascii_hex(b"48 65 6c\n6C 6f>junk"), b"Hello");
        assert_eq!(ascii_hex(b"7>"), [0x70]);
        assert_eq!(run_length(b"\x02abc\xfdz\x80ignored"), b"abczzzz");
    }

    #[test]
    fn png_up_predictor() {
        // Two rows of three bytes, the second stored as its difference from the first.
        let encoded = [2u8, 10, 20, 30, 2, 1, 1, 1];
        let mut parms = Dict::default();
        parms.insert(b"Predictor", Object::Int(12));
        parms.insert(b"Columns", Object::Int(3));
        assert_eq!(predict(encoded.to_vec(), Some(&parms)).unwrap(), [10, 20, 30, 11, 21, 31]);
    }

    #[test]
    fn flate_keeps_what_inflates_from_a_truncated_stream() {
        let data = b"BT /F1 12 Tf (hello) Tj ET\n".repeat(200);
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&data, 6);
        let got = flate(&packed[..packed.len() / 2]).unwrap();
        assert!(!got.is_empty() && data.starts_with(&got));
    }
}
