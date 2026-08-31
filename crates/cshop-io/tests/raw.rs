//! Reading raw files that describe themselves.
//!
//! There is no camera here to take a picture with, so the tests build DNGs of
//! their own: a header, the tags a developer needs, and sensor readings chosen
//! so the right answer is known exactly. That is a better test than a
//! photograph would be, because a photograph has no right answer to check
//! against.

use cshop_io::raw::{self, Develop};

/// Build a little-endian DNG with one uncompressed strip.
struct Dng {
    width: u32,
    height: u32,
    /// Row-major sensor readings.
    samples: Vec<u16>,
    /// Which colour is over each of the four photosites of the pattern.
    cfa: [u8; 4],
    black: u16,
    white: u16,
    /// The raw values a grey card produced.
    neutral: [f32; 3],
    /// XYZ to camera, which is the direction DNG states it in.
    matrix: [f32; 9],
}

impl Dng {
    fn new(width: u32, height: u32, samples: Vec<u16>) -> Dng {
        Dng {
            width,
            height,
            samples,
            cfa: [0, 1, 1, 2], // red, green / green, blue
            black: 0,
            white: 65535,
            neutral: [1.0, 1.0, 1.0],
            // The identity, so a camera reading is already XYZ and the test
            // can check the colour arithmetic without a camera in the way.
            matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn build(&self) -> Vec<u8> {
        // Layout: header, then the values that do not fit in an entry, then
        // the sensor data, then the directory.
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&[0x49, 0x49, 0x2A, 0x00]);
        out.extend_from_slice(&0u32.to_le_bytes()); // patched at the end

        let put = |out: &mut Vec<u8>, bytes: &[u8]| -> u32 {
            let at = out.len() as u32;
            out.extend_from_slice(bytes);
            at
        };

        let rational = |v: f32| {
            let d = 1_000_000u32;
            [((v * d as f32) as u32).to_le_bytes(), d.to_le_bytes()].concat()
        };
        let matrix_at = put(
            &mut out,
            &self.matrix.iter().flat_map(|&v| rational(v)).collect::<Vec<u8>>(),
        );
        let neutral_at = put(
            &mut out,
            &self.neutral.iter().flat_map(|&v| rational(v)).collect::<Vec<u8>>(),
        );
        // Four bytes or fewer live in the directory entry itself rather than
        // at an offset, which is the rule that catches everyone once.
        let cfa_inline = u32::from_le_bytes(self.cfa);
        let dim_inline = u32::from_le_bytes([2, 0, 2, 0]);
        let model_at = put(&mut out, b"Test Camera\0");
        let data_at = put(
            &mut out,
            &self.samples.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>(),
        );

        // The directory.
        let ifd_at = out.len() as u32;
        let mut entries: Vec<(u16, u16, u32, u32)> = vec![
            (256, 4, 1, self.width),                   // ImageWidth
            (257, 4, 1, self.height),                  // ImageLength
            (258, 3, 1, 16),                           // BitsPerSample
            (259, 3, 1, 1),                            // Compression: none
            (262, 3, 1, 32803),                        // PhotometricInterpretation: CFA
            (272, 2, 12, model_at),                    // Model
            (273, 4, 1, data_at),                      // StripOffsets
            (278, 4, 1, self.height),                  // RowsPerStrip
            (279, 4, 1, self.samples.len() as u32 * 2), // StripByteCounts
            (33421, 3, 2, dim_inline),                 // CFARepeatPatternDim
            (33422, 1, 4, cfa_inline),                 // CFAPattern
            (50706, 1, 4, u32::from_le_bytes([1, 4, 0, 0])), // DNGVersion
            (50714, 3, 1, self.black as u32),          // BlackLevel
            (50717, 3, 1, self.white as u32),          // WhiteLevel
            (50721, 10, 9, matrix_at),                 // ColorMatrix1
            (50728, 5, 3, neutral_at),                 // AsShotNeutral
        ];
        entries.sort_by_key(|e| e.0);

        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, kind, count, value) in &entries {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&kind.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            // A value small enough sits here; anything larger is an offset,
            // which is what these already are.
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&0u32.to_le_bytes()); // no next directory

        let bytes = ifd_at.to_le_bytes();
        out[4..8].copy_from_slice(&bytes);
        out
    }
}

/// A sensor reading a flat field: every photosite gets the same fraction of
/// full scale, so a correct developer returns grey.
fn flat(width: u32, height: u32, level: u16) -> Vec<u16> {
    vec![level; (width * height) as usize]
}

#[test]
fn a_dng_is_recognised_and_a_plain_tiff_is_not() {
    let dng = Dng::new(8, 8, flat(8, 8, 30000)).build();
    assert!(raw::is_raw(&dng, None));
    // A TIFF header with no DNGVersion tag is a TIFF.
    let mut tiff = dng.clone();
    // Blank the DNGVersion tag number so it is no longer found.
    if let Some(at) = tiff.windows(2).position(|w| w == 50706u16.to_le_bytes()) {
        tiff[at] = 0;
        tiff[at + 1] = 0;
    }
    assert!(!raw::is_raw(&tiff, None));
}

#[test]
fn the_sensor_data_and_its_description_come_back() {
    let dng = Dng::new(16, 12, flat(16, 12, 12345)).build();
    let r = raw::read(&dng).expect("it should read");
    assert_eq!((r.width, r.height), (16, 12));
    assert_eq!(r.samples.len(), 16 * 12);
    assert_eq!(r.samples[0], 12345);
    assert_eq!(r.cfa, [0, 1, 1, 2]);
    assert_eq!(r.white, 65535.0);
    assert_eq!(r.camera.as_deref(), Some("Test Camera"));
}

/// A flat sensor reading should develop to a flat grey. If black, white or the
/// colour matrix are applied wrongly this comes out tinted, which is the
/// single most useful check there is.
#[test]
fn a_flat_field_develops_to_grey() {
    let dng = Dng::new(32, 24, flat(32, 24, 40000)).build();
    let r = raw::read(&dng).unwrap();
    let out = r.develop(Develop { gamma: false, ..Default::default() });

    // Away from the edges, where the interpolation has all its neighbours.
    for (x, y) in [(8, 8), (9, 8), (8, 9), (16, 12)] {
        let c = out.get(x, y);
        let spread = c.r.abs_diff(c.g).max(c.g.abs_diff(c.b)) as f32 / 65535.0;
        assert!(spread < 0.02, "({x}, {y}) came out tinted: {c:?}");
        let level = c.g as f32 / 65535.0;
        assert!((level - 40000.0 / 65535.0).abs() < 0.02, "and at the wrong level: {level}");
    }
}

/// Black is not zero on a real sensor, and subtracting it is what makes black
/// black rather than dark grey.
#[test]
fn the_black_level_is_subtracted() {
    let mut d = Dng::new(16, 16, flat(16, 16, 4096));
    d.black = 4096;
    d.white = 60000;
    let r = raw::read(&d.build()).unwrap();
    let out = r.develop(Develop { gamma: false, ..Default::default() });
    let c = out.get(8, 8);
    assert!(c.g < 300, "a reading at the black level is black, not {c:?}");
}

/// The white balance the camera recorded is what makes a grey card grey.
/// Without it a raw file is green, because sensors have twice as many green
/// photosites and a green-heavy response.
#[test]
fn the_recorded_white_balance_is_applied() {
    // A grey card, photographed: the sensor reads it unevenly, and
    // AsShotNeutral records exactly how. Feeding a *flat* field here would
    // prove nothing — a flat field is already neutral in the sensor's own
    // terms, and balancing it would rightly make it uneven.
    let (w, h) = (24u32, 24u32);
    let neutral = [2.0f32, 1.0, 0.5];
    let base = 20000.0f32;
    let mut samples = vec![0u16; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            // The pattern is red, green / green, blue.
            let channel = match (x % 2, y % 2) {
                (0, 0) => 0,
                (1, 1) => 2,
                _ => 1,
            };
            samples[(y * w + x) as usize] = (base * neutral[channel]) as u16;
        }
    }
    let mut d = Dng::new(w, h, samples);
    d.neutral = neutral;
    let bytes = d.build();
    let r = raw::read(&bytes).unwrap();

    // Measured with the colour matrix out of the way: it is doing real work
    // on a colour that is not neutral, and what is being checked here is
    // whether the balance made it neutral in the first place.
    let plain = Develop { gamma: false, colour_matrix: false, ..Default::default() };
    let with = r.develop(plain);
    let without = r.develop(Develop { white_balance: false, ..plain });

    let balance = |c: cshop_core::color::Rgba16| c.r as f32 / c.b.max(1) as f32;
    let (a, b) = (balance(with.get(12, 12)), balance(without.get(12, 12)));
    assert!((a - 1.0).abs() < 0.1, "balanced, red over blue should be about one: {a}");
    assert!(b > 2.0, "and unbalanced it should not be: {b}");
}

/// The pattern says which photosite measured which colour, and reading it
/// backwards swaps red and blue — a mistake that produces a picture which
/// looks fine until you notice the sky is orange.
#[test]
fn the_filter_pattern_decides_which_colour_is_which() {
    // A sensor where only the photosites in the pattern's first position have
    // signal. With RGGB that is red; with BGGR it is blue.
    let (w, h) = (16u32, 16u32);
    let mut samples = vec![0u16; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            if x % 2 == 0 && y % 2 == 0 {
                samples[(y * w + x) as usize] = 60000;
            }
        }
    }
    let read_with = |cfa: [u8; 4]| {
        let mut d = Dng::new(w, h, samples.clone());
        d.cfa = cfa;
        let r = raw::read(&d.build()).unwrap();
        let out = r.develop(Develop { gamma: false, colour_matrix: false, ..Default::default() });
        out.get(8, 8)
    };
    let rggb = read_with([0, 1, 1, 2]);
    let bggr = read_with([2, 1, 1, 0]);
    assert!(rggb.r > rggb.b, "red-first should give red: {rggb:?}");
    assert!(bggr.b > bggr.r, "and blue-first, blue: {bggr:?}");
}

/// A raw format that does not describe itself has to be refused with a reason,
/// not opened and guessed at.
#[test]
fn a_raw_file_without_the_tags_says_what_is_missing() {
    // A TIFF-shaped file with no DNG tags at all.
    let mut bytes = vec![0x49, 0x49, 0x2A, 0x00];
    bytes.extend_from_slice(&8u32.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    let err = raw::read(&bytes).unwrap_err();
    let text = format!("{err}");
    assert!(text.contains("describes itself"), "{text}");
    assert!(text.contains("database"), "and says why: {text}");
}

#[test]
fn something_that_is_not_a_raw_file_is_refused() {
    assert!(raw::read(b"not a file").is_err());
    assert!(!raw::is_raw(b"not a file", None));
}

/// Sixteen bits out, because narrowing on the way out of a raw converter
/// throws away the reason for using one.
#[test]
fn developing_gives_back_sixteen_bits() {
    let dng = Dng::new(16, 16, flat(16, 16, 30000)).build();
    let out = raw::read(&dng).unwrap().develop(Develop::default());
    let levels: std::collections::HashSet<u16> =
        out.pixels().iter().map(|p| p.g).collect();
    assert!(!levels.is_empty());
    // A deep buffer, not a widened eight-bit one: the values are not all
    // multiples of 257.
    assert!(out.pixels().iter().any(|p| p.g % 257 != 0) || levels.len() > 1);
}

// --- Lossless JPEG ---------------------------------------------------------

/// A minimal lossless-JPEG encoder, written here so the decoder has something
/// to decode. There is no camera to take a picture with, and a stream built
/// from a known set of samples is a better test than a photograph would be:
/// every value has a right answer.
mod ljpeg {
    /// A canonical Huffman table over the seventeen magnitudes. Fifteen codes
    /// of four bits and two of five, which is exactly full — the lengths have
    /// to satisfy Kraft's equality or the table is not a prefix code.
    fn table() -> ([u8; 16], Vec<u8>) {
        let mut counts = [0u8; 16];
        counts[3] = 15; // fifteen codes of length four
        counts[4] = 2; // and two of length five
        (counts, (0u8..=16).collect())
    }

    /// The canonical code for each symbol: codes assigned in order, shortest
    /// first, exactly as the decoder reconstructs them.
    fn codes() -> Vec<(u32, u32)> {
        let (counts, values) = table();
        let mut out = vec![(0u32, 0u32); values.len()];
        let (mut code, mut i) = (0u32, 0usize);
        for length in 1..=16u32 {
            for _ in 0..counts[(length - 1) as usize] {
                out[i] = (code, length);
                code += 1;
                i += 1;
            }
            code <<= 1;
        }
        out
    }

    struct Bits {
        out: Vec<u8>,
        acc: u32,
        have: u32,
    }

    impl Bits {
        fn push(&mut self, value: u32, length: u32) {
            for i in (0..length).rev() {
                self.acc = (self.acc << 1) | ((value >> i) & 1);
                self.have += 1;
                if self.have == 8 {
                    let b = self.acc as u8;
                    self.out.push(b);
                    // A byte that looks like a marker is followed by a stuffed
                    // zero, which is how the data says it is not one.
                    if b == 0xFF {
                        self.out.push(0x00);
                    }
                    self.acc = 0;
                    self.have = 0;
                }
            }
        }

        fn flush(&mut self) {
            while self.have != 0 {
                self.push(1, 1);
            }
        }
    }

    /// Encode one component, sixteen bits, predictor 1 (the value to the left).
    pub fn encode(width: u32, height: u32, samples: &[u16], precision: u8) -> Vec<u8> {
        let (counts, values) = table();
        let codes = codes();
        let mut out: Vec<u8> = vec![0xFF, 0xD8];

        // Start of frame, lossless.
        out.extend_from_slice(&[0xFF, 0xC3]);
        out.extend_from_slice(&(11u16).to_be_bytes());
        out.push(precision);
        out.extend_from_slice(&(height as u16).to_be_bytes());
        out.extend_from_slice(&(width as u16).to_be_bytes());
        out.push(1); // one component
        out.extend_from_slice(&[0, 0x11, 0]);

        // The table.
        out.extend_from_slice(&[0xFF, 0xC4]);
        out.extend_from_slice(&((2 + 1 + 16 + values.len()) as u16).to_be_bytes());
        out.push(0); // class 0, id 0
        out.extend_from_slice(&counts);
        out.extend_from_slice(&values);

        // Start of scan.
        out.extend_from_slice(&[0xFF, 0xDA]);
        out.extend_from_slice(&(8u16).to_be_bytes());
        out.push(1);
        out.extend_from_slice(&[0, 0]); // component 0, table 0
        out.push(1); // predictor 1
        out.push(0); // end (unused in lossless)
        out.push(0); // point transform

        let mut bits = Bits { out: Vec::new(), acc: 0, have: 0 };
        let start = 1i32 << (precision - 1);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let value = samples[y * width as usize + x] as i32;
                let prediction = if y == 0 && x == 0 {
                    start
                } else if x == 0 {
                    samples[(y - 1) * width as usize] as i32
                } else {
                    samples[y * width as usize + x - 1] as i32
                };
                let diff = value - prediction;
                let magnitude = if diff == 0 {
                    0u32
                } else {
                    32 - (diff.unsigned_abs()).leading_zeros()
                };
                let (code, length) = codes[magnitude as usize];
                bits.push(code, length);
                if magnitude > 0 {
                    // Negative differences are coded as the value plus the
                    // range minus one, which is what puts the sign in the top
                    // bit without a sign bit.
                    let encoded = if diff >= 0 {
                        diff as u32
                    } else {
                        (diff + (1 << magnitude) - 1) as u32
                    };
                    bits.push(encoded, magnitude);
                }
            }
        }
        bits.flush();
        out.extend_from_slice(&bits.out);
        out.extend_from_slice(&[0xFF, 0xD9]);
        out
    }
}

/// A DNG whose sensor data is compressed the way a camera actually writes it.
fn compressed_dng(width: u32, height: u32, samples: &[u16]) -> Vec<u8> {
    let stream = ljpeg::encode(width, height, samples, 16);
    let mut d = Dng::new(width, height, Vec::new());
    d.cfa = [0, 1, 1, 2];
    let mut out = d.build();

    // Replace the strip with the compressed stream and say so. Simpler than a
    // second builder: find the uncompressed entry and rewrite it.
    let ifd_at = u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(out[ifd_at..ifd_at + 2].try_into().unwrap()) as usize;
    let data_at = out.len() as u32;
    out.extend_from_slice(&stream);
    for i in 0..count {
        let e = ifd_at + 2 + i * 12;
        let tag = u16::from_le_bytes(out[e..e + 2].try_into().unwrap());
        let value = match tag {
            259 => Some(7u32),                  // Compression: lossless JPEG
            273 => Some(data_at),               // StripOffsets
            279 => Some(stream.len() as u32),   // StripByteCounts
            _ => None,
        };
        if let Some(v) = value {
            out[e + 8..e + 12].copy_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// The compression a DNG actually uses is not the JPEG anyone means by JPEG:
/// no transform, no quantisation, no loss. Every sample has to come back
/// exactly.
#[test]
fn lossless_jpeg_data_comes_back_sample_for_sample() {
    let (w, h) = (32u32, 16u32);
    let mut samples = vec![0u16; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            // A pattern with runs, jumps and both signs of difference.
            let v = ((x * 977 + y * 3391) % 60000) as u16;
            samples[(y * w + x) as usize] = v;
        }
    }
    let dng = compressed_dng(w, h, &samples);
    let r = raw::read(&dng).expect("it should read");
    assert_eq!(r.samples.len(), samples.len());
    assert_eq!(r.samples, samples, "lossless has to mean lossless");
}

#[test]
fn a_compressed_dng_develops_like_an_uncompressed_one() {
    let (w, h) = (24u32, 24u32);
    let samples = vec![40000u16; (w * h) as usize];
    let compressed = raw::read(&compressed_dng(w, h, &samples)).unwrap();
    let plain = raw::read(&Dng::new(w, h, samples).build()).unwrap();

    let a = compressed.develop(Develop::default());
    let b = plain.develop(Develop::default());
    assert_eq!(a.pixels(), b.pixels(), "how it was stored should not change the picture");
}

/// A compression this cannot read must say so, not produce noise.
#[test]
fn an_unknown_compression_is_refused_with_its_number() {
    let (w, h) = (8u32, 8u32);
    let mut dng = Dng::new(w, h, flat(w, h, 1000)).build();
    let ifd_at = u32::from_le_bytes(dng[4..8].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(dng[ifd_at..ifd_at + 2].try_into().unwrap()) as usize;
    for i in 0..count {
        let e = ifd_at + 2 + i * 12;
        if u16::from_le_bytes(dng[e..e + 2].try_into().unwrap()) == 259 {
            dng[e + 8..e + 12].copy_from_slice(&34892u32.to_le_bytes());
        }
    }
    let err = format!("{}", raw::read(&dng).unwrap_err());
    assert!(err.contains("34892"), "it should say which: {err}");
}

/// A raw file opens as a document, sixteen bits deep — which is the reason for
/// shooting raw, and would be thrown away by narrowing here.
#[test]
fn a_raw_file_opens_as_a_deep_document() {
    let (w, h) = (32u32, 24u32);
    let dng = Dng::new(w, h, flat(w, h, 45000)).build();
    let doc = cshop_io::decode_document(&dng, Some(std::path::Path::new("shot.dng")))
        .expect("it should open");
    assert_eq!((doc.width, doc.height), (w, h));
    assert_eq!(doc.depth(), 16, "raw comes in deep, not narrowed");

    let layer = doc.tree.get(doc.tree.iter_all()[0]).unwrap();
    assert_eq!(layer.name, "Raw");
    let Some(cshop_core::layer::Surface::Sixteen(px)) = layer.surface() else {
        panic!("it should be a sixteen-bit surface");
    };
    // A flat field develops to a flat grey, all the way through the document.
    let c = px.get(16, 12);
    assert!(c.r.abs_diff(c.g) < 2000 && c.g.abs_diff(c.b) < 2000, "{c:?}");
}
