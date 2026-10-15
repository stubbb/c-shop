//! Fonts, as far as reading text needs them: bytes to codes, codes to
//! Unicode, codes to advance widths (ISO 32000-1 §9.5–9.10).
//!
//! Unicode comes from the font's /ToUnicode CMap first; for simple fonts,
//! then from the encoding (base encoding plus /Differences glyph names); for
//! composite fonts on a Uni*-UCS2/UTF16 CMap, from the code itself. What is
//! left is U+FFFD, so a loss shows rather than reading as plausible text.
//!
//! Glyph outlines are not used. The text becomes type set in an installed
//! face, so what is kept from the font is what that needs: the name, the
//! style, and the widths that say where each glyph went.

use super::file::File;
use super::syntax::{Dict, Lexer, Object, Token};
use std::collections::HashMap;

pub const REPLACEMENT: char = '\u{FFFD}';

/// The Adobe Glyph List names of the Latin text fonts (Annex D), plus the
/// ligatures and typographic symbols common in real files.
const AGL: &[(&str, u32)] = &[
    ("space", 0x20), ("exclam", 0x21), ("quotedbl", 0x22), ("numbersign", 0x23), ("dollar", 0x24),
    ("percent", 0x25), ("ampersand", 0x26), ("quotesingle", 0x27), ("quoteright", 0x2019),
    ("parenleft", 0x28), ("parenright", 0x29), ("asterisk", 0x2a), ("plus", 0x2b), ("comma", 0x2c),
    ("hyphen", 0x2d), ("period", 0x2e), ("slash", 0x2f), ("zero", 0x30), ("one", 0x31),
    ("two", 0x32), ("three", 0x33), ("four", 0x34), ("five", 0x35), ("six", 0x36), ("seven", 0x37),
    ("eight", 0x38), ("nine", 0x39), ("colon", 0x3a), ("semicolon", 0x3b), ("less", 0x3c),
    ("equal", 0x3d), ("greater", 0x3e), ("question", 0x3f), ("at", 0x40), ("bracketleft", 0x5b),
    ("backslash", 0x5c), ("bracketright", 0x5d), ("asciicircum", 0x5e), ("underscore", 0x5f),
    ("grave", 0x60), ("quoteleft", 0x2018), ("braceleft", 0x7b), ("bar", 0x7c),
    ("braceright", 0x7d), ("asciitilde", 0x7e), ("nbspace", 0xa0), ("exclamdown", 0xa1),
    ("cent", 0xa2), ("sterling", 0xa3), ("currency", 0xa4), ("yen", 0xa5), ("brokenbar", 0xa6),
    ("section", 0xa7), ("dieresis", 0xa8), ("copyright", 0xa9), ("ordfeminine", 0xaa),
    ("guillemotleft", 0xab), ("logicalnot", 0xac), ("sfthyphen", 0xad), ("registered", 0xae),
    ("macron", 0xaf), ("degree", 0xb0), ("plusminus", 0xb1), ("twosuperior", 0xb2),
    ("threesuperior", 0xb3), ("acute", 0xb4), ("mu", 0xb5), ("paragraph", 0xb6),
    ("periodcentered", 0xb7), ("cedilla", 0xb8), ("onesuperior", 0xb9), ("ordmasculine", 0xba),
    ("guillemotright", 0xbb), ("onequarter", 0xbc), ("onehalf", 0xbd), ("threequarters", 0xbe),
    ("questiondown", 0xbf), ("Agrave", 0xc0), ("Aacute", 0xc1), ("Acircumflex", 0xc2),
    ("Atilde", 0xc3), ("Adieresis", 0xc4), ("Aring", 0xc5), ("AE", 0xc6), ("Ccedilla", 0xc7),
    ("Egrave", 0xc8), ("Eacute", 0xc9), ("Ecircumflex", 0xca), ("Edieresis", 0xcb),
    ("Igrave", 0xcc), ("Iacute", 0xcd), ("Icircumflex", 0xce), ("Idieresis", 0xcf), ("Eth", 0xd0),
    ("Ntilde", 0xd1), ("Ograve", 0xd2), ("Oacute", 0xd3), ("Ocircumflex", 0xd4), ("Otilde", 0xd5),
    ("Odieresis", 0xd6), ("multiply", 0xd7), ("Oslash", 0xd8), ("Ugrave", 0xd9), ("Uacute", 0xda),
    ("Ucircumflex", 0xdb), ("Udieresis", 0xdc), ("Yacute", 0xdd), ("Thorn", 0xde),
    ("germandbls", 0xdf), ("agrave", 0xe0), ("aacute", 0xe1), ("acircumflex", 0xe2),
    ("atilde", 0xe3), ("adieresis", 0xe4), ("aring", 0xe5), ("ae", 0xe6), ("ccedilla", 0xe7),
    ("egrave", 0xe8), ("eacute", 0xe9), ("ecircumflex", 0xea), ("edieresis", 0xeb),
    ("igrave", 0xec), ("iacute", 0xed), ("icircumflex", 0xee), ("idieresis", 0xef), ("eth", 0xf0),
    ("ntilde", 0xf1), ("ograve", 0xf2), ("oacute", 0xf3), ("ocircumflex", 0xf4), ("otilde", 0xf5),
    ("odieresis", 0xf6), ("divide", 0xf7), ("oslash", 0xf8), ("ugrave", 0xf9), ("uacute", 0xfa),
    ("ucircumflex", 0xfb), ("udieresis", 0xfc), ("yacute", 0xfd), ("thorn", 0xfe),
    ("ydieresis", 0xff), ("dotlessi", 0x131), ("Lslash", 0x141), ("lslash", 0x142), ("OE", 0x152),
    ("oe", 0x153), ("Scaron", 0x160), ("scaron", 0x161), ("Ydieresis", 0x178), ("Zcaron", 0x17d),
    ("zcaron", 0x17e), ("florin", 0x192), ("circumflex", 0x2c6), ("caron", 0x2c7),
    ("breve", 0x2d8), ("dotaccent", 0x2d9), ("ring", 0x2da), ("ogonek", 0x2db), ("tilde", 0x2dc),
    ("hungarumlaut", 0x2dd), ("endash", 0x2013), ("emdash", 0x2014), ("quotesinglbase", 0x201a),
    ("quotedblleft", 0x201c), ("quotedblright", 0x201d), ("quotedblbase", 0x201e),
    ("dagger", 0x2020), ("daggerdbl", 0x2021), ("bullet", 0x2022), ("ellipsis", 0x2026),
    ("perthousand", 0x2030), ("guilsinglleft", 0x2039), ("guilsinglright", 0x203a),
    ("fraction", 0x2044), ("Euro", 0x20ac), ("trademark", 0x2122), ("minus", 0x2212),
    ("ff", 0xfb00), ("fi", 0xfb01), ("fl", 0xfb02), ("ffi", 0xfb03), ("ffl", 0xfb04),
    ("Delta", 0x2206), ("Omega", 0x2126), ("pi", 0x3c0), ("partialdiff", 0x2202),
    ("summation", 0x2211), ("product", 0x220f), ("radical", 0x221a), ("infinity", 0x221e),
    ("integral", 0x222b), ("approxequal", 0x2248), ("notequal", 0x2260), ("lessequal", 0x2264),
    ("greaterequal", 0x2265), ("lozenge", 0x25ca), ("arrowright", 0x2192), ("arrowleft", 0x2190),
    ("checkmark", 0x2713),
];

/// StandardEncoding above 0x7E (Annex D.2); below, ASCII with curly quotes.
const STANDARD_HIGH: &[(u8, &str)] = &[
    (161, "exclamdown"), (162, "cent"), (163, "sterling"), (164, "fraction"), (165, "yen"),
    (166, "florin"), (167, "section"), (168, "currency"), (169, "quotesingle"),
    (170, "quotedblleft"), (171, "guillemotleft"), (172, "guilsinglleft"), (173, "guilsinglright"),
    (174, "fi"), (175, "fl"), (177, "endash"), (178, "dagger"), (179, "daggerdbl"),
    (180, "periodcentered"), (182, "paragraph"), (183, "bullet"), (184, "quotesinglbase"),
    (185, "quotedblbase"), (186, "quotedblright"), (187, "guillemotright"), (188, "ellipsis"),
    (189, "perthousand"), (191, "questiondown"), (193, "grave"), (194, "acute"),
    (195, "circumflex"), (196, "tilde"), (197, "macron"), (198, "breve"), (199, "dotaccent"),
    (200, "dieresis"), (202, "ring"), (203, "cedilla"), (205, "hungarumlaut"), (206, "ogonek"),
    (207, "caron"), (208, "emdash"), (225, "AE"), (227, "ordfeminine"), (232, "Lslash"),
    (233, "Oslash"), (234, "OE"), (235, "ordmasculine"), (241, "ae"), (245, "dotlessi"),
    (248, "lslash"), (249, "oslash"), (250, "oe"), (251, "germandbls"),
];

/// WinAnsiEncoding's 0x80–0x9F, which is where it differs from Latin-1.
const WIN_ANSI_HIGH: [u32; 32] = [
    0x20AC, 0x2022, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
    0x2039, 0x0152, 0x2022, 0x017D, 0x2022, 0x2022, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
    0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x2022, 0x017E, 0x0178,
];

/// MacRomanEncoding's upper half.
const MAC_ROMAN_HIGH: [u32; 128] = [
    0xC4, 0xC5, 0xC7, 0xC9, 0xD1, 0xD6, 0xDC, 0xE1, 0xE0, 0xE2, 0xE4, 0xE3, 0xE5, 0xE7, 0xE9, 0xE8,
    0xEA, 0xEB, 0xED, 0xEC, 0xEE, 0xEF, 0xF1, 0xF3, 0xF2, 0xF4, 0xF6, 0xF5, 0xFA, 0xF9, 0xFB, 0xFC,
    0x2020, 0xB0, 0xA2, 0xA3, 0xA7, 0x2022, 0xB6, 0xDF, 0xAE, 0xA9, 0x2122, 0xB4, 0xA8, 0x2260,
    0xC6, 0xD8, 0x221E, 0xB1, 0x2264, 0x2265, 0xA5, 0xB5, 0x2202, 0x2211, 0x220F, 0x3C0, 0x222B,
    0xAA, 0xBA, 0x3A9, 0xE6, 0xF8, 0xBF, 0xA1, 0xAC, 0x221A, 0x192, 0x2248, 0x2206, 0xAB, 0xBB,
    0x2026, 0xA0, 0xC0, 0xC3, 0xD5, 0x152, 0x153, 0x2013, 0x2014, 0x201C, 0x201D, 0x2018, 0x2019,
    0xF7, 0x25CA, 0xFF, 0x178, 0x2044, 0x20AC, 0x2039, 0x203A, 0xFB01, 0xFB02, 0x2021, 0xB7,
    0x201A, 0x201E, 0x2030, 0xC2, 0xCA, 0xC1, 0xCB, 0xC8, 0xCD, 0xCE, 0xCF, 0xCC, 0xD3, 0xD4,
    0xF8FF, 0xD2, 0xDA, 0xDB, 0xD9, 0x131, 0x2C6, 0x2DC, 0xAF, 0x2D8, 0x2D9, 0x2DA, 0xB8, 0x2DD,
    0x2DB, 0x2C7,
];

fn agl(name: &str) -> Option<char> {
    AGL.iter().find(|(n, _)| *n == name).and_then(|(_, c)| char::from_u32(*c))
}

/// Unicode for a glyph name, by the Adobe Glyph List specification's rules.
pub fn glyph_to_unicode(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    if let Some(c) = agl(name) {
        return Some(c.to_string());
    }
    let base = name.split('.').next().unwrap_or(name);
    if base.contains('_') {
        let parts: Option<Vec<String>> = base.split('_').map(glyph_to_unicode).collect();
        return parts.map(|p| p.concat());
    }
    if let Some(c) = agl(base) {
        return Some(c.to_string());
    }
    if let Some(hex) = base.strip_prefix("uni") {
        if !hex.is_empty() && hex.len() % 4 == 0 && hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            let units: Vec<u16> = (0..hex.len())
                .step_by(4)
                .map(|i| u16::from_str_radix(&hex[i..i + 4], 16).unwrap_or(0xFFFD))
                .collect();
            if units.iter().all(|u| !(0xD800..=0xDFFF).contains(u)) {
                return Some(String::from_utf16_lossy(&units));
            }
        }
    }
    if let Some(hex) = base.strip_prefix('u') {
        if (4..=6).contains(&hex.len()) && hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            if let Some(c) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                return Some(c.to_string());
            }
        }
    }
    let mut chars = base.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c.to_string()),
        _ => None,
    }
}

type Encoding = Vec<Option<String>>;

/// Codes that stand for the character of the same number.
fn ascii(t: &mut [Option<char>], codes: std::ops::Range<usize>) {
    for (c, slot) in t.iter_mut().enumerate().take(codes.end).skip(codes.start) {
        *slot = char::from_u32(c as u32);
    }
}

fn base_encoding(name: &[u8]) -> Option<Encoding> {
    let mut t: Vec<Option<char>> = vec![None; 256];
    match name {
        b"StandardEncoding" => {
            ascii(&mut t, 0x20..0x7F);
            t[0x27] = Some('\u{2019}');
            t[0x60] = Some('\u{2018}');
            for (code, name) in STANDARD_HIGH {
                t[*code as usize] = agl(name);
            }
        }
        b"WinAnsiEncoding" | b"PDFDocEncoding" => {
            ascii(&mut t, 0x20..0x100);
            for (i, &u) in WIN_ANSI_HIGH.iter().enumerate() {
                t[0x80 + i] = char::from_u32(u);
            }
        }
        b"MacRomanEncoding" => {
            ascii(&mut t, 0x20..0x80);
            for (i, &u) in MAC_ROMAN_HIGH.iter().enumerate() {
                t[0x80 + i] = char::from_u32(u);
            }
        }
        _ => return None,
    }
    Some(t.into_iter().map(|c| c.map(String::from)).collect())
}

// ---------------------------------------------------------------------------
// CMaps
// ---------------------------------------------------------------------------

fn utf16(dst: &[u8]) -> String {
    if dst.len() % 2 == 1 {
        // Not UTF-16 at all: the bytes as code points.
        return dst.iter().map(|&b| b as char).collect();
    }
    let units: Vec<u16> = dst.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

fn be(bytes: &[u8]) -> u32 {
    bytes.iter().take(4).fold(0u32, |n, &b| n << 8 | b as u32)
}

/// A bfrange destination advanced by `k`, carrying across bytes.
fn offset(dst: &[u8], k: u32) -> Vec<u8> {
    let mut out = dst.to_vec();
    let mut carry = k as u64;
    for b in out.iter_mut().rev() {
        let v = *b as u64 + carry;
        *b = v as u8;
        carry = v >> 8;
        if carry == 0 {
            break;
        }
    }
    out
}

/// A CMap (§9.7.5) or a ToUnicode CMap (§9.10.3), read from its PostScript.
#[derive(Debug, Clone, Default)]
pub struct CMap {
    /// (byte length, low, high).
    codespace: Vec<(usize, Vec<u8>, Vec<u8>)>,
    unicode: HashMap<u32, String>,
    unicode_ranges: Vec<(u32, u32, Vec<u8>)>,
    cids: HashMap<u32, u32>,
    cid_ranges: Vec<(u32, u32, u32)>,
    pub identity: bool,
}

impl CMap {
    pub fn identity() -> CMap {
        CMap { codespace: vec![(2, vec![0, 0], vec![0xFF, 0xFF])], identity: true, ..Default::default() }
    }

    pub fn parse(data: &[u8]) -> CMap {
        let mut cmap = CMap::default();
        let mut lexer = Lexer::new(data, 0);
        let mut prev: Option<Token> = None;
        while let Some(tok) = lexer.next() {
            if let Token::Keyword(k) = &tok {
                match k.as_slice() {
                    b"begincodespacerange" => {
                        for g in groups(&mut lexer, b"endcodespacerange", 2) {
                            if let (Item::Tok(Token::String(lo)), Item::Tok(Token::String(hi))) = (&g[0], &g[1]) {
                                if lo.len() == hi.len() && !lo.is_empty() && lo.len() <= 4 {
                                    cmap.codespace.push((lo.len(), lo.clone(), hi.clone()));
                                }
                            }
                        }
                    }
                    b"beginbfchar" => {
                        for g in groups(&mut lexer, b"endbfchar", 2) {
                            match (&g[0], &g[1]) {
                                (Item::Tok(Token::String(src)), Item::Tok(Token::String(dst))) => {
                                    cmap.unicode.insert(be(src), utf16(dst));
                                }
                                (Item::Tok(Token::String(src)), Item::Tok(Token::Name(n))) => {
                                    if let Some(t) = glyph_to_unicode(&String::from_utf8_lossy(n)) {
                                        cmap.unicode.insert(be(src), t);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    b"beginbfrange" => {
                        for g in groups(&mut lexer, b"endbfrange", 3) {
                            let (Item::Tok(Token::String(lo)), Item::Tok(Token::String(hi))) = (&g[0], &g[1]) else {
                                continue;
                            };
                            let (lo, hi) = (be(lo), be(hi));
                            if hi < lo {
                                continue;
                            }
                            match &g[2] {
                                Item::Tok(Token::String(dst)) if !dst.is_empty() => {
                                    if hi - lo <= 256 {
                                        for k in 0..=hi - lo {
                                            cmap.unicode.insert(lo + k, utf16(&offset(dst, k)));
                                        }
                                    } else {
                                        cmap.unicode_ranges.push((lo, hi, dst.clone()));
                                    }
                                }
                                Item::List(dsts) => {
                                    for (k, dst) in dsts.iter().take((hi - lo + 1) as usize).enumerate() {
                                        cmap.unicode.insert(lo + k as u32, utf16(dst));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    b"begincidchar" => {
                        for g in groups(&mut lexer, b"endcidchar", 2) {
                            if let (Item::Tok(Token::String(src)), Item::Tok(Token::Int(cid))) = (&g[0], &g[1]) {
                                cmap.cids.insert(be(src), (*cid).max(0) as u32);
                            }
                        }
                    }
                    b"begincidrange" => {
                        for g in groups(&mut lexer, b"endcidrange", 3) {
                            if let (Item::Tok(Token::String(lo)), Item::Tok(Token::String(hi)), Item::Tok(Token::Int(cid))) =
                                (&g[0], &g[1], &g[2])
                            {
                                cmap.cid_ranges.push((be(lo), be(hi), (*cid).max(0) as u32));
                            }
                        }
                    }
                    b"usecmap" => {
                        if let Some(Token::Name(n)) = &prev {
                            if n == b"Identity-H" || n == b"Identity-V" {
                                cmap.identity = true;
                                if cmap.codespace.is_empty() {
                                    cmap.codespace = CMap::identity().codespace;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            prev = Some(tok);
        }
        cmap
    }

    pub fn to_unicode(&self, code: u32) -> Option<String> {
        if let Some(t) = self.unicode.get(&code) {
            return Some(t.clone());
        }
        self.unicode_ranges
            .iter()
            .find(|(lo, hi, _)| (*lo..=*hi).contains(&code))
            .map(|(lo, _, dst)| utf16(&offset(dst, code - lo)))
    }

    pub fn to_cid(&self, code: u32) -> u32 {
        if let Some(&c) = self.cids.get(&code) {
            return c;
        }
        if let Some((lo, _, first)) = self.cid_ranges.iter().find(|(lo, hi, _)| (*lo..=*hi).contains(&code)) {
            return first + code - lo;
        }
        if self.identity { code } else { 0 }
    }

    /// Cut a string into `(code, byte length)` by the codespace (§9.7.6.2).
    pub fn split(&self, data: &[u8], default_len: usize) -> Vec<(u32, usize)> {
        if self.codespace.is_empty() {
            return data.chunks(default_len).map(|c| (be(c), c.len())).collect();
        }
        let mut lengths: Vec<usize> = self.codespace.iter().map(|r| r.0).collect();
        lengths.sort_unstable();
        lengths.dedup();
        let mut out = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let fits = |n: usize| {
                let chunk = data.get(i..i + n)?;
                self.codespace
                    .iter()
                    .any(|(len, lo, hi)| *len == n && (0..n).all(|k| lo[k] <= chunk[k] && chunk[k] <= hi[k]))
                    .then_some(())
            };
            let n = lengths.iter().copied().find(|&n| fits(n).is_some()).unwrap_or(lengths[0]);
            let end = (i + n).min(data.len());
            out.push((be(&data[i..end]), n));
            i = end;
        }
        out
    }
}

/// One operand in a CMap section: a token, or an array of strings.
enum Item {
    Tok(Token),
    List(Vec<Vec<u8>>),
}

/// Read operands up to `end` in groups of `size`, an array counting as one.
fn groups(lexer: &mut Lexer, end: &[u8], size: usize) -> Vec<Vec<Item>> {
    let mut out = Vec::new();
    let mut group = Vec::new();
    while let Some(tok) = lexer.next() {
        if matches!(&tok, Token::Keyword(k) if k == end) {
            break;
        }
        let item = if tok == Token::ArrayOpen {
            // bfrange's other form: an array of destinations, one per code.
            let mut items = Vec::new();
            for t in lexer.by_ref() {
                if t == Token::ArrayClose {
                    break;
                }
                if let Token::String(s) = t {
                    items.push(s);
                }
            }
            Item::List(items)
        } else {
            Item::Tok(tok)
        };
        group.push(item);
        if group.len() == size {
            out.push(std::mem::take(&mut group));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

/// One decoded code: its text, its width in text space units (already
/// divided by 1000, or scaled by a Type 3 font matrix), and whether word
/// spacing applies to it — only a single-byte code 32 takes it (§9.3.3).
pub struct Code {
    pub text: String,
    pub width: f32,
    pub word_space: bool,
}

/// What the font says about how it looks, for choosing an installed face.
#[derive(Debug, Clone, Default)]
pub struct Description {
    /// The PostScript name without its subset prefix, e.g. `TimesNewRomanPS-BoldMT`.
    pub name: String,
    /// /FontFamily from the descriptor, when it has one.
    pub family: Option<String>,
    pub flags: u32,
    pub weight: Option<f32>,
    pub italic_angle: f32,
}

pub struct Font {
    pub description: Description,
    to_unicode: Option<CMap>,
    kind: Kind,
}

enum Kind {
    Simple {
        encoding: Encoding,
        widths: HashMap<u32, f32>,
        missing_width: f32,
        width_scale: f32,
    },
    Composite {
        cmap: CMap,
        unicode_codes: bool,
        widths: HashMap<u32, f32>,
        default_width: f32,
    },
}

impl Font {
    pub fn load(file: &File, dict: &Dict) -> Font {
        let r = |o: &Object| file.resolve(o);
        let get = |d: &Dict, k: &[u8]| d.get(k).map(r).unwrap_or(Object::Null);
        let base_font = get(dict, b"BaseFont").as_name().map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
        let name = match base_font.split_once('+') {
            Some((prefix, rest)) if prefix.len() == 6 && prefix.bytes().all(|c| c.is_ascii_uppercase()) => rest.to_string(),
            _ => base_font,
        };
        let subtype = get(dict, b"Subtype");
        let is_type0 = subtype.is_name(b"Type0");

        let descendant = if is_type0 {
            get(dict, b"DescendantFonts").as_array().and_then(|a| a.first()).map(r).and_then(|o| o.as_dict().cloned())
        } else {
            None
        };
        let descriptor = get(descendant.as_ref().unwrap_or(dict), b"FontDescriptor");
        let descriptor = descriptor.as_dict().cloned().unwrap_or_default();
        let flags = get(&descriptor, b"Flags").as_int().unwrap_or(0).max(0) as u32;
        let description = Description {
            name: name.clone(),
            family: get(&descriptor, b"FontFamily").as_bytes().map(super::file::decode_text),
            flags,
            weight: get(&descriptor, b"FontWeight").as_f32(),
            italic_angle: get(&descriptor, b"ItalicAngle").as_f32().unwrap_or(0.0),
        };

        let to_unicode = match get(dict, b"ToUnicode") {
            Object::Stream(s) => file.stream_data(&s.dict, &s.raw).ok().map(|d| CMap::parse(&d)),
            _ => None,
        };

        let kind = if is_type0 {
            let (cmap, cmap_name) = match get(dict, b"Encoding") {
                Object::Name(n) if n == b"Identity-H" || n == b"Identity-V" => (CMap::identity(), n),
                Object::Name(n) => (CMap::default(), n),
                Object::Stream(s) => {
                    let data = file.stream_data(&s.dict, &s.raw).unwrap_or_default();
                    let n = get(&s.dict, b"CMapName").as_name().map(|n| n.to_vec()).unwrap_or_default();
                    (CMap::parse(&data), n)
                }
                _ => (CMap::identity(), b"Identity-H".to_vec()),
            };
            let cmap_name = String::from_utf8_lossy(&cmap_name).into_owned();
            let cid = descendant.unwrap_or_default();
            let default_width = get(&cid, b"DW").as_f32().unwrap_or(1000.0);
            let w: Vec<Object> = get(&cid, b"W").as_array().map(|a| a.iter().map(r).collect()).unwrap_or_default();
            Kind::Composite {
                cmap,
                unicode_codes: cmap_name.starts_with("Uni") && (cmap_name.contains("UCS2") || cmap_name.contains("UTF16")),
                widths: cid_widths(&w, &r),
                default_width,
            }
        } else {
            let first = get(dict, b"FirstChar").as_int().unwrap_or(0).max(0) as u32;
            let mut widths = HashMap::new();
            if let Some(ws) = get(dict, b"Widths").as_array() {
                for (i, w) in ws.iter().enumerate() {
                    if let Some(w) = r(w).as_f32() {
                        widths.insert(first + i as u32, w);
                    }
                }
            }
            let mut missing_width = get(&descriptor, b"MissingWidth").as_f32().unwrap_or(0.0);
            if widths.is_empty() {
                // A standard 14 font without metrics: an even estimate,
                // monospaced for Courier, beats no width at all.
                missing_width = if name.contains("Courier") { 600.0 } else { 500.0 };
            }
            let width_scale = if subtype.is_name(b"Type3") {
                get(dict, b"FontMatrix").as_array().and_then(|m| m.first()).and_then(|v| r(v).as_f32()).unwrap_or(0.001)
            } else {
                0.001
            };
            let symbolic = flags & 4 != 0 && flags & 32 == 0;
            Kind::Simple { encoding: simple_encoding(file, &get(dict, b"Encoding"), symbolic, &name), widths, missing_width, width_scale }
        };
        Font { description, to_unicode, kind }
    }

    pub fn decode(&self, data: &[u8]) -> Vec<Code> {
        let unicode = |code: u32| self.to_unicode.as_ref().and_then(|m| m.to_unicode(code));
        match &self.kind {
            Kind::Simple { encoding, widths, missing_width, width_scale } => data
                .iter()
                .map(|&b| {
                    let code = b as u32;
                    let text = unicode(code).unwrap_or_else(|| match &encoding[b as usize] {
                        Some(t) => t.clone(),
                        None if (0x20..0x7F).contains(&b) => (b as char).to_string(),
                        None => REPLACEMENT.to_string(),
                    });
                    let width = widths.get(&code).copied().unwrap_or(*missing_width) * width_scale;
                    Code { text, width, word_space: b == 32 }
                })
                .collect(),
            Kind::Composite { cmap, unicode_codes, widths, default_width } => cmap
                .split(data, 2)
                .into_iter()
                .map(|(code, nbytes)| {
                    let text = unicode(code).unwrap_or_else(|| match char::from_u32(code) {
                        Some(c) if *unicode_codes => c.to_string(),
                        _ => REPLACEMENT.to_string(),
                    });
                    let cid = cmap.to_cid(code);
                    let width = widths.get(&cid).copied().unwrap_or(*default_width) * 0.001;
                    Code { text, width, word_space: code == 32 && nbytes == 1 }
                })
                .collect(),
        }
    }
}

fn simple_encoding(file: &File, enc: &Object, symbolic: bool, name: &str) -> Encoding {
    let (base_name, differences) = match enc {
        Object::Name(n) => (Some(n.clone()), Vec::new()),
        Object::Dict(d) => (
            d.get(b"BaseEncoding").and_then(|v| file.resolve(v).as_name().map(|n| n.to_vec())),
            file.resolve(d.get(b"Differences").unwrap_or(&Object::Null)).as_array().map(|a| a.to_vec()).unwrap_or_default(),
        ),
        _ => (None, Vec::new()),
    };
    let mut table = base_name.and_then(|n| base_encoding(&n)).unwrap_or_else(|| {
        if symbolic || name.starts_with("Symbol") || name.starts_with("ZapfDingbats") {
            vec![None; 256]
        } else {
            base_encoding(b"StandardEncoding").expect("built in")
        }
    });
    let mut code: i64 = 0;
    for item in differences {
        match file.resolve(&item) {
            Object::Int(c) => code = c,
            Object::Name(n) => {
                if (0..256).contains(&code) {
                    // g37 and friends name no character: show the loss, not a guess.
                    let text = glyph_to_unicode(&String::from_utf8_lossy(&n));
                    table[code as usize] = Some(text.unwrap_or_else(|| REPLACEMENT.to_string()));
                }
                code += 1;
            }
            _ => {}
        }
    }
    table
}

fn cid_widths(w: &[Object], r: &dyn Fn(&Object) -> Object) -> HashMap<u32, f32> {
    let mut widths = HashMap::new();
    let mut i = 0;
    while i + 1 < w.len() {
        let Some(first) = w[i].as_int().filter(|v| *v >= 0) else { break };
        match &w[i + 1] {
            Object::Array(list) => {
                for (k, v) in list.iter().enumerate() {
                    if let Some(v) = r(v).as_f32() {
                        widths.insert(first as u32 + k as u32, v);
                    }
                }
                i += 2;
            }
            last => {
                let (Some(last), Some(v)) = (last.as_int(), w.get(i + 2).and_then(|v| v.as_f32())) else { break };
                if last >= first && last - first < 70_000 {
                    for cid in first..=last {
                        widths.insert(cid as u32, v);
                    }
                }
                i += 3;
            }
        }
    }
    widths
}
