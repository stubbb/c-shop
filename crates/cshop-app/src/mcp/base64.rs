//! Base64, because an image has to travel inside a JSON string.
//!
//! Standard alphabet with padding, which is what the protocol's image blocks
//! specify. Encoding only — nothing here ever reads base64 back.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        // Pack the chunk into 24 bits, short chunks padded with zero bits.
        let b = |i: usize| *chunk.get(i).unwrap_or(&0) as u32;
        let packed = (b(0) << 16) | (b(1) << 8) | b(2);
        let sextet = |shift: u32| ALPHABET[((packed >> shift) & 0x3f) as usize] as char;

        out.push(sextet(18));
        out.push(sextet(12));
        // The last one or two characters carry no data when the chunk was
        // short, and are written as padding rather than as zeroes.
        out.push(if chunk.len() > 1 { sextet(6) } else { '=' });
        out.push(if chunk.len() > 2 { sextet(0) } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn the_padding_cases_are_right() {
        // The three chunk lengths, which is where every base64 bug lives.
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn the_high_bytes_use_the_whole_alphabet() {
        assert_eq!(encode(&[0xff, 0xff, 0xff]), "////");
        assert_eq!(encode(&[0xfb, 0xff, 0xbf]), "+/+/");
        assert_eq!(encode(&[0x00, 0x00, 0x00]), "AAAA");
    }
}
