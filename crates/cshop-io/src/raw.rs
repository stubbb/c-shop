//! Raw camera files that describe themselves.
//!
//! # Why only some of them
//!
//! A raw file is what the sensor measured: one number per photosite, each
//! behind a coloured filter, with none of the interpretation a picture needs.
//! Turning it into a photograph takes four things the file has to supply —
//! where the colour filters are, what counts as black and what as white, what
//! the light was, and how the camera's three primaries relate to real colour.
//!
//! Proprietary formats supply some of that and leave the rest to a database of
//! camera models maintained by hand, one entry per body, forever. That database
//! is the part of raw support that cannot be written, only accumulated.
//!
//! DNG needs no database, because it carries all four itself. So this reads
//! DNG — and the other raw formats that are DNG-shaped enough to carry the same
//! tags — and says plainly that it does not read the rest, rather than opening
//! them and guessing.
//!
//! # What happens to the numbers
//!
//! Subtract black, divide by white minus black, apply the white balance the
//! camera recorded, interpolate the two colours each photosite did not
//! measure, convert through the camera's own matrix into real colour, and
//! encode. Each step is somewhere a raw converter can differ from another; none
//! of them is optional.

use crate::IoError;
use cshop_core::color::Rgba8;
use cshop_core::pixels::{DeepBuffer, PixelBuffer};

/// Refuses anything larger, so a header claiming a gigapixel sensor asks for a
/// lot of memory rather than all of it.
const MAX_SIDE: u32 = 30_000;

/// What was read out of the file, before any of it is turned into a picture.
#[derive(Debug, Clone)]
pub struct Raw {
    pub width: u32,
    pub height: u32,
    /// One number per photosite, row-major, already at full precision.
    pub samples: Vec<u16>,
    /// The colour filter over each photosite in the repeating pattern, as
    /// indices into red, green, blue.
    pub cfa: [u8; 4],
    /// What the sensor reads with no light, per channel of the pattern.
    pub black: [f32; 4],
    /// What it reads when saturated.
    pub white: f32,
    /// The camera's neutral, as the multipliers that make grey grey.
    pub balance: [f32; 3],
    /// Camera primaries to XYZ, which is what turns three sensor numbers into
    /// a colour.
    pub to_xyz: [[f32; 3]; 3],
    /// What the camera called itself, when it said.
    pub camera: Option<String>,
}

/// How to develop a raw file.
#[derive(Debug, Clone, Copy)]
pub struct Develop {
    /// Apply the camera's recorded white balance. Off leaves the sensor's own
    /// response, which is green-heavy and looks it.
    pub white_balance: bool,
    /// Convert through the camera's matrix into sRGB. Off leaves the camera's
    /// own primaries, which are not any standard's.
    pub colour_matrix: bool,
    /// Encode for sRGB rather than leaving the numbers linear. Linear is what
    /// to ask for if the picture is going straight into further processing.
    pub gamma: bool,
}

impl Default for Develop {
    fn default() -> Self {
        Self { white_balance: true, colour_matrix: true, gamma: true }
    }
}

/// Whether these bytes look like a raw file this can read.
pub fn is_raw(bytes: &[u8], hint: Option<&std::path::Path>) -> bool {
    // A DNG is a TIFF, and so is a plain TIFF, so the signature alone is not
    // enough: the DNGVersion tag is what distinguishes them.
    if let Some(ext) = hint.and_then(|p| p.extension()).and_then(|e| e.to_str()) {
        if ext.eq_ignore_ascii_case("dng") {
            return true;
        }
    }
    Tiff::open(bytes).is_ok_and(|t| {
        t.ifds().iter().any(|ifd| t.entry(ifd, tag::DNG_VERSION).is_some())
    })
}

/// Read a raw file's sensor data and everything needed to interpret it.
pub fn read(bytes: &[u8]) -> Result<Raw, IoError> {
    let tiff = Tiff::open(bytes)?;
    let ifds = tiff.ifds();
    if !ifds.iter().any(|ifd| tiff.entry(ifd, tag::DNG_VERSION).is_some()) {
        return Err(IoError::Unsupported(
            "this is a raw file, but not one that describes itself. Only DNG carries the \
             colour matrix, black and white levels and filter pattern needed to develop \
             it; the rest need a database of camera models, which this does not have."
                .into(),
        ));
    }

    // The raw image is usually in a SubIFD rather than the first one, because
    // the first holds the small preview a camera puts there for its screen.
    let raw_ifd = ifds
        .iter()
        .find(|ifd| {
            matches!(tiff.u32s(ifd, tag::PHOTOMETRIC).first(), Some(&32803 | &34892))
        })
        .cloned()
        .ok_or_else(|| {
            IoError::Decode("this DNG has no sensor data in it, only previews".into())
        })?;
    let meta = ifds.first().cloned().unwrap_or_else(|| raw_ifd.clone());

    let width = *tiff.u32s(&raw_ifd, tag::WIDTH).first().unwrap_or(&0);
    let height = *tiff.u32s(&raw_ifd, tag::HEIGHT).first().unwrap_or(&0);
    if width == 0 || height == 0 {
        return Err(IoError::Decode("this DNG's sensor data has no size".into()));
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(IoError::TooLarge(width, height, MAX_SIDE));
    }

    let bits = *tiff.u32s(&raw_ifd, tag::BITS).first().unwrap_or(&16) as u16;
    let compression = *tiff.u32s(&raw_ifd, tag::COMPRESSION).first().unwrap_or(&1);
    let samples = decode_samples(&tiff, &raw_ifd, width, height, bits, compression)?;

    // The filter pattern, as indices into red, green, blue. A pattern that is
    // not two by two is a sensor this cannot demosaic.
    let dims = tiff.u32s(&raw_ifd, tag::CFA_DIM);
    let pattern = tiff.bytes_of(&raw_ifd, tag::CFA_PATTERN);
    let cfa = match (dims.as_slice(), pattern.len()) {
        ([2, 2], 4) => [pattern[0], pattern[1], pattern[2], pattern[3]],
        // Some writers omit the dimensions and give four bytes anyway.
        (_, 4) => [pattern[0], pattern[1], pattern[2], pattern[3]],
        _ => {
            return Err(IoError::Unsupported(
                "this sensor's colour filter is not the usual two-by-two pattern".into(),
            ))
        }
    };
    if cfa.iter().any(|&c| c > 2) {
        return Err(IoError::Unsupported(
            "this sensor has a filter colour that is not red, green or blue".into(),
        ));
    }

    let full = ((1u32 << bits.min(16)) - 1) as f32;
    let black_values = tiff.floats(&raw_ifd, tag::BLACK_LEVEL);
    let black = match black_values.len() {
        0 => [0.0; 4],
        1 => [black_values[0]; 4],
        _ => {
            let mut b = [0.0f32; 4];
            for (i, slot) in b.iter_mut().enumerate() {
                *slot = *black_values.get(i).unwrap_or(&black_values[0]);
            }
            b
        }
    };
    let white = tiff.floats(&raw_ifd, tag::WHITE_LEVEL).first().copied().unwrap_or(full);

    // AsShotNeutral is the camera's neutral *as a divisor*: the raw values a
    // grey card produced. The multipliers that make it grey are its reciprocal.
    let neutral = tiff.floats(&meta, tag::AS_SHOT_NEUTRAL);
    let balance = if neutral.len() >= 3 && neutral.iter().all(|&v| v > 1e-6) {
        let g = neutral[1];
        [g / neutral[0], 1.0, g / neutral[2]]
    } else {
        [1.0, 1.0, 1.0]
    };

    let to_xyz = camera_to_xyz(&tiff, &meta);
    let camera = tiff
        .string(&meta, tag::MODEL)
        .map(|m| match tiff.string(&meta, tag::MAKE) {
            Some(make) if !m.starts_with(&make) => format!("{make} {m}"),
            _ => m,
        });

    Ok(Raw {
        width,
        height,
        samples,
        cfa,
        black,
        white,
        balance,
        to_xyz,
        camera,
    })
}

/// Read a raw file and develop it into a picture.
pub fn read_developed(bytes: &[u8], how: Develop) -> Result<PixelBuffer, IoError> {
    Ok(read(bytes)?.develop(how).to_eight())
}

impl Raw {
    /// The sensor's reading at one photosite, black subtracted and scaled so
    /// that white is one.
    #[inline]
    fn level(&self, x: u32, y: u32) -> f32 {
        let i = (y as usize) * self.width as usize + x as usize;
        let raw = self.samples.get(i).copied().unwrap_or(0) as f32;
        let black = self.black[self.filter(x, y) as usize % 4];
        let range = (self.white - black).max(1.0);
        ((raw - black) / range).clamp(0.0, 1.0)
    }

    /// Which colour sits over a photosite: 0 red, 1 green, 2 blue.
    #[inline]
    fn colour_at(&self, x: u32, y: u32) -> u8 {
        self.cfa[self.filter(x, y) as usize]
    }

    #[inline]
    fn filter(&self, x: u32, y: u32) -> u8 {
        ((y % 2) * 2 + (x % 2)) as u8
    }

    /// Turn the sensor's readings into a picture.
    ///
    /// At sixteen bits, because that is what the sensor gave and narrowing on
    /// the way out of a raw converter throws away the whole reason for using
    /// one.
    pub fn develop(&self, how: Develop) -> DeepBuffer {
        let (w, h) = (self.width, self.height);
        let mut out = DeepBuffer::new(w, h);
        if w < 2 || h < 2 {
            return out;
        }

        // Bilinear demosaic with the green channel done first and the red and
        // blue interpolated against it. Interpolating each channel on its own
        // puts colour fringes on every edge, because the three channels are
        // sampled in different places and an edge lands between them; using
        // green — which is sampled twice as often and carries most of the
        // detail — as the reference is what keeps the fringes down.
        let green = self.interpolate_green();

        let balance = if how.white_balance { self.balance } else { [1.0; 3] };
        let matrix = how.colour_matrix.then(|| xyz_to_srgb(self.to_xyz));

        for y in 0..h {
            for x in 0..w {
                let g = green[(y as usize) * w as usize + x as usize];
                let (r, b) = self.interpolate_chroma(x, y, &green, g);
                let mut c = [r * balance[0], g * balance[1], b * balance[2]];
                if let Some(m) = matrix {
                    c = apply(m, c);
                }
                let encode = |v: f32| {
                    let v = v.clamp(0.0, 1.0);
                    let v = if how.gamma { cshop_core::color::linear_to_srgb(v) } else { v };
                    (v * 65535.0 + 0.5) as u16
                };
                out.set(
                    x as i32,
                    y as i32,
                    cshop_core::color::Rgba16::new(
                        encode(c[0]),
                        encode(c[1]),
                        encode(c[2]),
                        65535,
                    ),
                );
            }
        }
        out
    }

    /// Green everywhere: measured where there is a green photosite, and the
    /// average of its neighbours where there is not.
    fn interpolate_green(&self) -> Vec<f32> {
        let (w, h) = (self.width, self.height);
        let mut green = vec![0.0f32; (w as usize) * (h as usize)];
        for y in 0..h {
            for x in 0..w {
                let i = (y as usize) * w as usize + x as usize;
                if self.colour_at(x, y) == 1 {
                    green[i] = self.level(x, y);
                    continue;
                }
                let (mut sum, mut n) = (0.0f32, 0.0f32);
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    sum += self.level(nx as u32, ny as u32);
                    n += 1.0;
                }
                green[i] = if n > 0.0 { sum / n } else { 0.0 };
            }
        }
        green
    }

    /// Red and blue at one pixel, interpolated as differences from green.
    ///
    /// The difference between a colour and green varies far more slowly across
    /// a picture than either does on its own — that is what "colour changes
    /// slowly, brightness changes fast" means numerically — so interpolating
    /// the difference and adding green back is markedly better than
    /// interpolating the colour.
    fn interpolate_chroma(&self, x: u32, y: u32, green: &[f32], g: f32) -> (f32, f32) {
        let (w, h) = (self.width, self.height);
        let here = self.colour_at(x, y);
        let mut got = [None, None];

        if here == 0 {
            got[0] = Some(self.level(x, y));
        } else if here == 2 {
            got[1] = Some(self.level(x, y));
        }

        for (want, slot) in got.iter_mut().enumerate() {
            if slot.is_some() {
                continue;
            }
            let target = if want == 0 { 0u8 } else { 2 };
            let (mut sum, mut n) = (0.0f32, 0.0f32);
            // Two rings: the diagonals reach the opposite colour on a Bayer
            // grid, the sides reach it from a green site.
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let (nx, ny) = (nx as u32, ny as u32);
                    if self.colour_at(nx, ny) != target {
                        continue;
                    }
                    let ng = green[(ny as usize) * w as usize + nx as usize];
                    sum += self.level(nx, ny) - ng;
                    n += 1.0;
                }
            }
            *slot = Some(if n > 0.0 { (sum / n + g).clamp(0.0, 1.0) } else { g });
        }
        (got[0].unwrap_or(g), got[1].unwrap_or(g))
    }
}

/// The camera's primaries in XYZ, from whichever colour matrix the file has.
///
/// DNG states the matrix the other way round — XYZ to camera — under one or
/// two illuminants. The second is the daylight one where both are present,
/// which is the better default for a picture with no other information.
fn camera_to_xyz(tiff: &Tiff, ifd: &Ifd) -> [[f32; 3]; 9usize.pow(0) * 3] {
    let read = |t: u16| {
        let v = tiff.floats(ifd, t);
        (v.len() >= 9).then(|| {
            [
                [v[0], v[1], v[2]],
                [v[3], v[4], v[5]],
                [v[6], v[7], v[8]],
            ]
        })
    };
    let xyz_to_cam = read(tag::COLOR_MATRIX_2).or_else(|| read(tag::COLOR_MATRIX_1));
    match xyz_to_cam.and_then(invert3) {
        Some(m) => m,
        // No matrix: treat the camera's primaries as sRGB's, which is wrong
        // and visible, and better than a black picture.
        None => invert3(SRGB_TO_XYZ).unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]),
    }
}

const SRGB_TO_XYZ: [[f32; 3]; 3] = [
    [0.412_456_4, 0.357_576_1, 0.180_437_5],
    [0.212_672_9, 0.715_152_2, 0.072_175_0],
    [0.019_333_9, 0.119_192, 0.950_304_1],
];

/// Camera primaries straight to sRGB, which is the product of two matrices the
/// picture would otherwise go through one at a time.
fn xyz_to_srgb(cam_to_xyz: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let Some(xyz_to_rgb) = invert3(SRGB_TO_XYZ) else {
        return cam_to_xyz;
    };
    let mut out = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| xyz_to_rgb[i][k] * cam_to_xyz[k][j]).sum();
        }
    }
    // The rows are normalised so that a neutral camera reading comes out
    // neutral. Without it the picture takes a cast from whatever the matrix
    // happened to sum to.
    for row in out.iter_mut() {
        let sum: f32 = row.iter().sum();
        if sum.abs() > 1e-6 {
            for v in row.iter_mut() {
                *v /= sum;
            }
        }
    }
    out
}

fn apply(m: [[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * c[0] + m[0][1] * c[1] + m[0][2] * c[2],
        m[1][0] * c[0] + m[1][1] * c[1] + m[1][2] * c[2],
        m[2][0] * c[0] + m[2][1] * c[1] + m[2][2] * c[2],
    ]
}

fn invert3(m: [[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let c = |a: usize, b: usize, x: usize, y: usize| m[a][b] * m[x][y];
    Some([
        [
            (c(1, 1, 2, 2) - c(1, 2, 2, 1)) / det,
            (c(0, 2, 2, 1) - c(0, 1, 2, 2)) / det,
            (c(0, 1, 1, 2) - c(0, 2, 1, 1)) / det,
        ],
        [
            (c(1, 2, 2, 0) - c(1, 0, 2, 2)) / det,
            (c(0, 0, 2, 2) - c(0, 2, 2, 0)) / det,
            (c(0, 2, 1, 0) - c(0, 0, 1, 2)) / det,
        ],
        [
            (c(1, 0, 2, 1) - c(1, 1, 2, 0)) / det,
            (c(0, 1, 2, 0) - c(0, 0, 2, 1)) / det,
            (c(0, 0, 1, 1) - c(0, 1, 1, 0)) / det,
        ],
    ])
}

/// A tiny preview of the sensor data, for showing the file exists before it is
/// developed.
pub fn thumbnail(raw: &Raw, longest: u32) -> PixelBuffer {
    let scale = (longest as f32 / raw.width.max(raw.height) as f32).min(1.0);
    let (w, h) = (
        ((raw.width as f32 * scale) as u32).max(1),
        ((raw.height as f32 * scale) as u32).max(1),
    );
    let mut out = PixelBuffer::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let sx = (x as f32 / scale) as u32;
            let sy = (y as f32 / scale) as u32;
            let v = (raw.level(sx.min(raw.width - 1), sy.min(raw.height - 1)) * 255.0) as u8;
            out.set(x as i32, y as i32, Rgba8::opaque(v, v, v));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The TIFF a DNG is
// ---------------------------------------------------------------------------

mod tag {
    pub const WIDTH: u16 = 256;
    pub const HEIGHT: u16 = 257;
    pub const BITS: u16 = 258;
    pub const COMPRESSION: u16 = 259;
    pub const PHOTOMETRIC: u16 = 262;
    pub const MAKE: u16 = 271;
    pub const MODEL: u16 = 272;
    pub const STRIP_OFFSETS: u16 = 273;
    pub const STRIP_COUNTS: u16 = 279;
    pub const ROWS_PER_STRIP: u16 = 278;
    pub const TILE_WIDTH: u16 = 322;
    pub const TILE_LENGTH: u16 = 323;
    pub const TILE_OFFSETS: u16 = 324;
    pub const TILE_COUNTS: u16 = 325;
    pub const SUB_IFDS: u16 = 330;
    pub const CFA_DIM: u16 = 33421;
    pub const CFA_PATTERN: u16 = 33422;
    pub const DNG_VERSION: u16 = 50706;
    pub const BLACK_LEVEL: u16 = 50714;
    pub const WHITE_LEVEL: u16 = 50717;
    pub const COLOR_MATRIX_1: u16 = 50721;
    pub const COLOR_MATRIX_2: u16 = 50722;
    pub const AS_SHOT_NEUTRAL: u16 = 50728;
}

#[derive(Debug, Clone)]
struct Entry {
    kind: u16,
    count: u32,
    /// Where the values are, resolved: an entry small enough to fit sits in
    /// the directory itself rather than pointing elsewhere.
    at: usize,
}

/// One image file directory: the tags for one image in the file.
#[derive(Debug, Clone, Default)]
struct Ifd {
    entries: std::collections::HashMap<u16, Entry>,
}

struct Tiff<'a> {
    bytes: &'a [u8],
    big_endian: bool,
}

impl<'a> Tiff<'a> {
    fn open(bytes: &'a [u8]) -> Result<Tiff<'a>, IoError> {
        let big_endian = match bytes.get(..4) {
            Some([0x49, 0x49, 0x2A, 0x00]) => false,
            Some([0x4D, 0x4D, 0x00, 0x2A]) => true,
            _ => return Err(IoError::Unsupported("this is not a TIFF-shaped file".into())),
        };
        Ok(Tiff { bytes, big_endian })
    }

    fn u16_at(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.bytes.get(at..at + 2)?.try_into().ok()?;
        Some(if self.big_endian { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) })
    }

    fn u32_at(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.bytes.get(at..at + 4)?.try_into().ok()?;
        Some(if self.big_endian { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    }

    /// Every directory in the file, including the sub-directories a DNG puts
    /// its sensor data in.
    fn ifds(&self) -> Vec<Ifd> {
        let mut out = Vec::new();
        let mut seen = 0usize;
        let mut at = self.u32_at(4).unwrap_or(0) as usize;
        // A chain of directories, each pointing at the next. Bounded: a file
        // that points at itself would otherwise be read forever.
        while at != 0 && seen < 64 {
            let Some(ifd) = self.read_ifd(at) else { break };
            let next = self.u32_at(at + 2 + ifd.entries.len() * 12).unwrap_or(0) as usize;
            // Sub-directories first, so the sensor data is found before the
            // preview that usually precedes it.
            for sub in self.u32s(&ifd, tag::SUB_IFDS) {
                if let Some(child) = self.read_ifd(sub as usize) {
                    out.push(child);
                }
            }
            out.push(ifd);
            seen += 1;
            at = next;
        }
        out
    }

    fn read_ifd(&self, at: usize) -> Option<Ifd> {
        let count = self.u16_at(at)? as usize;
        if count > 4096 {
            return None;
        }
        let mut entries = std::collections::HashMap::with_capacity(count);
        for i in 0..count {
            let e = at + 2 + i * 12;
            let tag = self.u16_at(e)?;
            let kind = self.u16_at(e + 2)?;
            let n = self.u32_at(e + 4)?;
            let size = type_size(kind) as u64 * n as u64;
            // Four bytes or fewer live in the entry; anything larger is a
            // pointer to where they really are.
            let values_at = if size <= 4 {
                e + 8
            } else {
                self.u32_at(e + 8)? as usize
            };
            if values_at >= self.bytes.len() && size > 0 {
                continue;
            }
            entries.insert(tag, Entry { kind, count: n, at: values_at });
        }
        Some(Ifd { entries })
    }

    fn entry<'b>(&self, ifd: &'b Ifd, tag: u16) -> Option<&'b Entry> {
        ifd.entries.get(&tag)
    }

    fn u32s(&self, ifd: &Ifd, tag: u16) -> Vec<u32> {
        let Some(e) = self.entry(ifd, tag) else { return Vec::new() };
        let stride = type_size(e.kind) as usize;
        (0..e.count.min(65_536) as usize)
            .filter_map(|i| {
                let at = e.at + i * stride;
                match e.kind {
                    1 | 2 | 6 | 7 => self.bytes.get(at).map(|&b| b as u32),
                    3 | 8 => self.u16_at(at).map(|v| v as u32),
                    4 | 9 => self.u32_at(at),
                    _ => None,
                }
            })
            .collect()
    }

    fn floats(&self, ifd: &Ifd, tag: u16) -> Vec<f32> {
        let Some(e) = self.entry(ifd, tag) else { return Vec::new() };
        let stride = type_size(e.kind) as usize;
        (0..e.count.min(4096) as usize)
            .filter_map(|i| {
                let at = e.at + i * stride;
                match e.kind {
                    // Rational and signed rational: a pair of integers, which
                    // is how DNG states every matrix and level it has.
                    5 => {
                        let n = self.u32_at(at)? as f32;
                        let d = self.u32_at(at + 4)? as f32;
                        (d != 0.0).then_some(n / d)
                    }
                    10 => {
                        let n = self.u32_at(at)? as i32 as f32;
                        let d = self.u32_at(at + 4)? as i32 as f32;
                        (d != 0.0).then_some(n / d)
                    }
                    3 => self.u16_at(at).map(|v| v as f32),
                    4 => self.u32_at(at).map(|v| v as f32),
                    1 => self.bytes.get(at).map(|&b| b as f32),
                    11 => {
                        let b: [u8; 4] = self.bytes.get(at..at + 4)?.try_into().ok()?;
                        Some(if self.big_endian {
                            f32::from_be_bytes(b)
                        } else {
                            f32::from_le_bytes(b)
                        })
                    }
                    _ => None,
                }
            })
            .collect()
    }

    fn bytes_of(&self, ifd: &Ifd, tag: u16) -> Vec<u8> {
        let Some(e) = self.entry(ifd, tag) else { return Vec::new() };
        self.bytes
            .get(e.at..e.at + e.count.min(4096) as usize)
            .map(|s| s.to_vec())
            .unwrap_or_default()
    }

    fn string(&self, ifd: &Ifd, tag: u16) -> Option<String> {
        let raw = self.bytes_of(ifd, tag);
        let text = String::from_utf8_lossy(&raw);
        let trimmed = text.trim_end_matches('\0').trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }
}

fn type_size(kind: u16) -> u32 {
    match kind {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 1,
    }
}

/// The sensor's numbers, however the file stored them.
fn decode_samples(
    tiff: &Tiff,
    ifd: &Ifd,
    width: u32,
    height: u32,
    bits: u16,
    compression: u32,
) -> Result<Vec<u16>, IoError> {
    let pixels = width as usize * height as usize;
    let mut out = vec![0u16; pixels];

    // Strips or tiles: DNG allows both, and a compressed file is usually
    // tiled, because a tile can be decompressed on its own.
    let tiled = tiff.entry(ifd, tag::TILE_OFFSETS).is_some();
    let (offsets, counts) = if tiled {
        (tiff.u32s(ifd, tag::TILE_OFFSETS), tiff.u32s(ifd, tag::TILE_COUNTS))
    } else {
        (tiff.u32s(ifd, tag::STRIP_OFFSETS), tiff.u32s(ifd, tag::STRIP_COUNTS))
    };
    if offsets.is_empty() {
        return Err(IoError::Decode("this DNG says where its data is not".into()));
    }
    let (tile_w, tile_h) = if tiled {
        (
            *tiff.u32s(ifd, tag::TILE_WIDTH).first().unwrap_or(&width),
            *tiff.u32s(ifd, tag::TILE_LENGTH).first().unwrap_or(&height),
        )
    } else {
        (
            width,
            *tiff.u32s(ifd, tag::ROWS_PER_STRIP).first().unwrap_or(&height),
        )
    };
    if tile_w == 0 || tile_h == 0 {
        return Err(IoError::Decode("this DNG's tiles have no size".into()));
    }
    let across = width.div_ceil(tile_w);

    for (i, &offset) in offsets.iter().enumerate() {
        let length = counts.get(i).copied().unwrap_or(0) as usize;
        let Some(chunk) = tiff.bytes.get(offset as usize..) else { continue };
        let chunk = &chunk[..length.min(chunk.len())];
        let (tx, ty) = if tiled {
            ((i as u32 % across) * tile_w, (i as u32 / across) * tile_h)
        } else {
            (0, i as u32 * tile_h)
        };

        let values: Vec<u16> = match compression {
            1 => unpack(chunk, bits, tiff.big_endian),
            7 | 0x8005 => lossless_jpeg(chunk)?,
            other => {
                return Err(IoError::Unsupported(format!(
                    "this DNG's sensor data is compressed in a way this cannot read \
                     (method {other}); DNGs written uncompressed or with lossless JPEG \
                     are read"
                )))
            }
        };

        // A tile at the right or bottom edge is padded to a whole tile, so its
        // rows are wider than what belongs in the picture.
        let this_w = tile_w.min(width.saturating_sub(tx));
        let this_h = tile_h.min(height.saturating_sub(ty));
        for row in 0..this_h {
            for col in 0..this_w {
                let from = (row * tile_w + col) as usize;
                let Some(&v) = values.get(from) else { continue };
                let to = (ty + row) as usize * width as usize + (tx + col) as usize;
                if let Some(slot) = out.get_mut(to) {
                    *slot = v;
                }
            }
        }
    }
    Ok(out)
}

/// Unpack samples that were stored without compression.
///
/// Twelve and fourteen bits a sample are packed across byte boundaries, which
/// is why this is not a cast.
fn unpack(bytes: &[u8], bits: u16, big_endian: bool) -> Vec<u16> {
    match bits {
        16 => bytes
            .chunks_exact(2)
            .map(|c| {
                if big_endian {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect(),
        8 => bytes.iter().map(|&b| b as u16).collect(),
        n if n > 0 && n < 16 => {
            let mut out = Vec::with_capacity(bytes.len() * 8 / n as usize);
            let (mut acc, mut have) = (0u32, 0u32);
            for &b in bytes {
                acc = (acc << 8) | b as u32;
                have += 8;
                while have >= n as u32 {
                    have -= n as u32;
                    out.push(((acc >> have) & ((1 << n) - 1)) as u16);
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Lossless JPEG
// ---------------------------------------------------------------------------

/// Decode the lossless JPEG a DNG's sensor data is usually stored in.
///
/// This is not the JPEG anyone means by JPEG. That one is process 1 or 2: a
/// cosine transform, quantisation, and loss. This is process 14, from the same
/// standard and almost nothing else in common — no transform, no quantisation,
/// and no loss. Each sample is predicted from its neighbours and only the
/// difference is Huffman-coded, which is why a raw file is large and exact
/// where a photograph is small and approximate.
///
/// Raw files use it because it is the only lossless mode of a standard every
/// camera already had an encoder for.
fn lossless_jpeg(bytes: &[u8]) -> Result<Vec<u16>, IoError> {
    let mut at = 0usize;
    let mut tables: Vec<Option<Huffman>> = vec![None, None, None, None];
    let mut frame: Option<Frame> = None;

    while at + 1 < bytes.len() {
        if bytes[at] != 0xFF {
            at += 1;
            continue;
        }
        let marker = bytes[at + 1];
        at += 2;
        match marker {
            // Padding and the start of the file.
            0xFF | 0xD8 => continue,
            0xD9 => break,
            _ => {}
        }
        let Some(length) = be16(bytes, at) else { break };
        let segment = bytes.get(at + 2..at + length as usize).unwrap_or(&[]);
        match marker {
            // Start of frame, lossless, Huffman.
            0xC3 => frame = Some(read_frame(segment)?),
            0xC4 => read_tables(segment, &mut tables),
            0xDA => {
                let frame = frame
                    .as_ref()
                    .ok_or_else(|| IoError::Decode("this stream has no frame header".into()))?;
                let scan_end = at + length as usize;
                return decode_scan(segment, &bytes[scan_end..], frame, &tables);
            }
            _ => {}
        }
        at += length as usize;
    }
    Err(IoError::Decode("this lossless JPEG stream has no scan in it".into()))
}

fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

#[derive(Debug)]
struct Frame {
    precision: u8,
    width: u32,
    height: u32,
    /// One per component; DNG uses one or two, the second being a way of
    /// splitting a Bayer row into two half-width components.
    components: Vec<u8>,
}

fn read_frame(segment: &[u8]) -> Result<Frame, IoError> {
    let precision = *segment.first().unwrap_or(&16);
    let height = be16(segment, 1).unwrap_or(0) as u32;
    let width = be16(segment, 3).unwrap_or(0) as u32;
    let n = *segment.get(5).unwrap_or(&1) as usize;
    if width == 0 || height == 0 || n == 0 || n > 4 {
        return Err(IoError::Decode("this lossless JPEG frame makes no sense".into()));
    }
    let components = (0..n)
        .map(|i| *segment.get(6 + i * 3).unwrap_or(&(i as u8)))
        .collect();
    Ok(Frame { precision, width, height, components })
}

/// A canonical Huffman table: the code lengths and what they mean.
#[derive(Debug, Clone, Default)]
struct Huffman {
    /// Indexed by code length, the first code of that length and where its
    /// values start.
    first_code: [i32; 17],
    first_index: [i32; 17],
    max_code: [i32; 17],
    values: Vec<u8>,
}

impl Huffman {
    fn build(counts: &[u8; 16], values: Vec<u8>) -> Huffman {
        let mut h = Huffman { values, ..Default::default() };
        let (mut code, mut index) = (0i32, 0i32);
        for length in 1..=16usize {
            h.first_code[length] = code;
            h.first_index[length] = index;
            let n = counts[length - 1] as i32;
            code += n;
            index += n;
            h.max_code[length] = code - 1;
            if n == 0 {
                // No codes of this length: nothing can match it.
                h.max_code[length] = -1;
            }
            code <<= 1;
        }
        h
    }
}

fn read_tables(mut segment: &[u8], tables: &mut [Option<Huffman>]) {
    while segment.len() > 17 {
        let id = (segment[0] & 0x0F) as usize;
        let mut counts = [0u8; 16];
        counts.copy_from_slice(&segment[1..17]);
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        let Some(values) = segment.get(17..17 + total) else { return };
        if id < tables.len() {
            tables[id] = Some(Huffman::build(&counts, values.to_vec()));
        }
        segment = &segment[17 + total..];
    }
}

/// Reads bits out of an entropy-coded stream, skipping the stuffed zero that
/// follows every `FF` — which is how a byte that looks like a marker is
/// carried inside data that must not contain one.
struct Bits<'a> {
    bytes: &'a [u8],
    at: usize,
    acc: u32,
    have: u32,
}

impl<'a> Bits<'a> {
    fn new(bytes: &'a [u8]) -> Bits<'a> {
        Bits { bytes, at: 0, acc: 0, have: 0 }
    }

    fn bit(&mut self) -> u32 {
        if self.have == 0 {
            let mut b = self.bytes.get(self.at).copied().unwrap_or(0);
            self.at += 1;
            if b == 0xFF {
                match self.bytes.get(self.at) {
                    Some(0x00) => self.at += 1,
                    // A real marker: the data has ended, and zeroes from here
                    // are as good an answer as any.
                    _ => b = 0,
                }
            }
            self.acc = b as u32;
            self.have = 8;
        }
        self.have -= 1;
        (self.acc >> self.have) & 1
    }

    fn bits(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit();
        }
        v
    }

    fn decode(&mut self, table: &Huffman) -> u8 {
        let mut code = 0i32;
        for length in 1..=16usize {
            code = (code << 1) | self.bit() as i32;
            if table.max_code[length] >= code && code >= table.first_code[length] {
                let i = table.first_index[length] + (code - table.first_code[length]);
                return table.values.get(i as usize).copied().unwrap_or(0);
            }
        }
        0
    }
}

/// The difference a Huffman symbol stands for.
///
/// The symbol is a *magnitude*: how many bits the difference needs. The bits
/// themselves follow, and the top one says which side of zero it is on — the
/// standard's way of coding a signed value in as few bits as it takes.
fn extend(value: u32, magnitude: u8) -> i32 {
    if magnitude == 0 {
        return 0;
    }
    let v = value as i32;
    if v < (1 << (magnitude - 1)) {
        v - (1 << magnitude) + 1
    } else {
        v
    }
}

fn decode_scan(
    header: &[u8],
    data: &[u8],
    frame: &Frame,
    tables: &[Option<Huffman>],
) -> Result<Vec<u16>, IoError> {
    let n = *header.first().unwrap_or(&1) as usize;
    let n = n.clamp(1, frame.components.len().max(1));
    // Which Huffman table each component uses, and the predictor.
    let table_of: Vec<usize> = (0..n)
        .map(|i| (*header.get(2 + i * 2).unwrap_or(&0) >> 4) as usize)
        .collect();
    let predictor = *header.get(1 + n * 2).unwrap_or(&1);
    let point_transform = *header.get(3 + n * 2).unwrap_or(&0) as u32;

    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut out = vec![0u16; w * h * n];
    let mut bits = Bits::new(data);
    // The value every row and the first row start from: half the range, which
    // is the standard's rule for "nothing to predict from".
    let start = 1i32 << (frame.precision.clamp(1, 16) - 1 - point_transform.min(15) as u8);
    let mut previous = vec![0i32; w * n];

    for y in 0..h {
        let mut left = vec![0i32; n];
        for x in 0..w {
            for c in 0..n {
                let table = tables
                    .get(table_of[c])
                    .and_then(|t| t.as_ref())
                    .ok_or_else(|| {
                        IoError::Decode("this stream names a Huffman table it does not have".into())
                    })?;
                let magnitude = bits.decode(table);
                let diff = extend(bits.bits(magnitude as u32), magnitude);

                // The prediction. Only the first two are used by raw writers;
                // the rest are here because the standard has them and a file
                // that used one would otherwise decode to noise.
                let a = left[c];
                let b = previous[x * n + c];
                let cc = if x > 0 { previous[(x - 1) * n + c] } else { 0 };
                let prediction = if y == 0 && x == 0 {
                    start
                } else if y == 0 {
                    a
                } else if x == 0 {
                    b
                } else {
                    match predictor {
                        1 => a,
                        2 => b,
                        3 => cc,
                        4 => a + b - cc,
                        5 => a + ((b - cc) >> 1),
                        6 => b + ((a - cc) >> 1),
                        7 => (a + b) >> 1,
                        _ => a,
                    }
                };
                let value = prediction + diff;
                left[c] = value;
                out[(y * w + x) * n + c] = ((value << point_transform) as u32 & 0xFFFF) as u16;
            }
        }
        for x in 0..w {
            for c in 0..n {
                previous[x * n + c] = out[(y * w + x) * n + c] as i32 >> point_transform;
            }
        }
    }
    Ok(out)
}
