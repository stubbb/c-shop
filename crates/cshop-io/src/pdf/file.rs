//! The file's structure (ISO 32000-1 §7.5): where every object is, and the
//! page tree on top of that.
//!
//! Reads classic cross-reference tables, cross-reference streams, hybrid files,
//! object streams and incremental updates. When the table is missing or its
//! offsets are wrong — which happens to hand-edited and truncated files — the
//! objects are found by scanning for `n g obj` instead, and [`File::repaired`]
//! says so.

use super::filters;
use super::syntax::{Dict, Lexer, Object, Parser, Stream, Token};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug, Clone, Copy)]
enum Entry {
    Free,
    At(usize),
    /// In object stream `stream`, at position `index`.
    Packed { stream: u32, index: usize },
}

/// An object stream's decoded bytes, and where in them each object starts.
type ObjectStream = Rc<(Vec<u8>, HashMap<u32, usize>)>;

pub struct File {
    data: Vec<u8>,
    /// Offsets in the file count from the `%PDF` header, which is not always
    /// the first byte.
    base: usize,
    xref: HashMap<u32, Entry>,
    pub trailer: Dict,
    pub repaired: bool,
    cache: RefCell<HashMap<u32, Object>>,
    object_streams: RefCell<HashMap<u32, ObjectStream>>,
    scanned: RefCell<Option<HashMap<u32, usize>>>,
}

/// A page, with what it inherits from the tree above it already applied.
#[derive(Debug, Clone)]
pub struct Page {
    pub dict: Dict,
    pub resources: Dict,
    /// The visible area, `[x0 y0 x1 y1]` in default user space: the crop box
    /// clipped to the media box.
    pub bbox: [f32; 4],
    /// Clockwise rotation for display: 0, 90, 180 or 270.
    pub rotate: u32,
}

const MAX_OBJECT_NUMBER: i64 = 8_388_607;

impl File {
    pub fn open(data: Vec<u8>) -> Result<File, String> {
        let head = &data[..data.len().min(1024)];
        let base = head
            .windows(5)
            .position(|w| w == b"%PDF-")
            .ok_or("this is not a PDF: it has no %PDF- header")?;
        let mut file = File {
            data,
            base,
            xref: HashMap::new(),
            trailer: Dict::default(),
            repaired: false,
            cache: RefCell::new(HashMap::new()),
            object_streams: RefCell::new(HashMap::new()),
            scanned: RefCell::new(None),
        };
        if file.load_xref().is_err() || file.catalog().is_none() {
            file.rebuild()?;
        }
        if file.trailer.contains(b"Encrypt") {
            return Err("this PDF is encrypted, which is not supported".into());
        }
        Ok(file)
    }

    // ---- cross-reference --------------------------------------------------

    fn load_xref(&mut self) -> Result<(), String> {
        let tail_start = self.data.len().saturating_sub(2048);
        let tail = &self.data[tail_start..];
        let at = rfind(tail, b"startxref").ok_or("no startxref")?;
        let mut lexer = Lexer::new(tail, at + 9);
        let Some(Token::Int(mut offset)) = lexer.next() else { return Err("no xref offset".into()) };
        let mut seen = Vec::new();
        let mut first = true;
        while offset >= 0 && !seen.contains(&offset) && seen.len() < 512 {
            seen.push(offset);
            let trailer = self.read_section(offset as usize)?;
            if first {
                self.trailer = trailer.clone();
                first = false;
            } else {
                for (k, v) in &trailer.0 {
                    if !self.trailer.contains(k) {
                        self.trailer.insert(k, v.clone());
                    }
                }
            }
            offset = match trailer.get(b"Prev").and_then(|p| p.as_int()) {
                Some(p) => p,
                None => break,
            };
        }
        // Objects looked up while the table was half read (an indirect
        // /Length, say) may have been cached as missing.
        self.cache.borrow_mut().clear();
        self.object_streams.borrow_mut().clear();
        Ok(())
    }

    fn read_section(&mut self, offset: usize) -> Result<Dict, String> {
        if let Some(pos) = self.locate(offset, b"xref") {
            let trailer = self.read_table(pos)?;
            if let Some(stm) = trailer.get(b"XRefStm").and_then(|v| v.as_int()) {
                let _ = self.read_xref_stream(self.base + stm.max(0) as usize);
            }
            return Ok(trailer);
        }
        self.read_xref_stream(self.base + offset)
    }

    /// The keyword at `offset`, forgiving a few bytes of slack either way.
    fn locate(&self, offset: usize, keyword: &[u8]) -> Option<usize> {
        for base in [self.base, 0] {
            let pos = base + offset;
            let start = pos.saturating_sub(4).min(self.data.len());
            let end = (pos + 4 + keyword.len()).min(self.data.len());
            if let Some(i) = find(&self.data[start..end], keyword) {
                return Some(start + i);
            }
        }
        None
    }

    fn read_table(&mut self, pos: usize) -> Result<Dict, String> {
        let (trailer, entries) = parse_table(&self.data, pos + 4)?;
        for (num, entry) in entries {
            // An entry already present came from a later update, and wins.
            self.xref.entry(num).or_insert(entry);
        }
        Ok(trailer)
    }

    fn read_xref_stream(&mut self, pos: usize) -> Result<Dict, String> {
        let (_, obj) = self.read_indirect(pos).ok_or("no xref stream there")?;
        let stream = obj.as_stream().ok_or("no xref stream there")?.clone();
        if !stream.dict.get(b"Type").is_some_and(|t| t.is_name(b"XRef")) {
            return Err("not an xref stream".into());
        }
        for (num, kind, a, b) in self.xref_stream_entries(&stream)? {
            if self.xref.contains_key(&num) {
                continue;
            }
            let entry = match kind {
                0 => Entry::Free,
                1 => Entry::At(a as usize),
                2 => Entry::Packed { stream: a as u32, index: b as usize },
                _ => continue,
            };
            self.xref.insert(num, entry);
        }
        Ok(stream.dict.clone())
    }

    fn xref_stream_entries(&self, stream: &Stream) -> Result<Vec<(u32, u64, u64, u64)>, String> {
        let widths: Vec<usize> = stream
            .dict
            .get(b"W")
            .and_then(|w| w.as_array())
            .map(|w| w.iter().map(|v| v.as_int().unwrap_or(0).clamp(0, 8) as usize).collect())
            .filter(|w: &Vec<usize>| w.len() == 3)
            .ok_or("xref stream without a valid /W")?;
        let size = stream.dict.get(b"Size").and_then(|v| v.as_int()).unwrap_or(0);
        let index: Vec<i64> = match stream.dict.get(b"Index").and_then(|v| v.as_array()) {
            Some(a) => a.iter().filter_map(|v| v.as_int()).collect(),
            None => vec![0, size],
        };
        let data = self.stream_data(&stream.dict, &stream.raw)?;
        let row: usize = widths.iter().sum();
        let mut out = Vec::new();
        let mut i = 0;
        for pair in index.chunks_exact(2) {
            let (start, count) = (pair[0].clamp(0, MAX_OBJECT_NUMBER), pair[1].clamp(0, MAX_OBJECT_NUMBER));
            for num in start..start + count {
                if row == 0 || i + row > data.len() {
                    return Ok(out);
                }
                let mut fields = [0u64; 3];
                for (f, &w) in widths.iter().enumerate() {
                    fields[f] = data[i..i + w].iter().fold(0u64, |n, &b| n << 8 | b as u64);
                    i += w;
                }
                // A zero-width type field means type 1.
                let kind = if widths[0] == 0 { 1 } else { fields[0] };
                out.push((num as u32, kind, fields[1], fields[2]));
            }
        }
        Ok(out)
    }

    /// Rebuild the object table and trailer by scanning the file.
    fn rebuild(&mut self) -> Result<(), String> {
        self.repaired = true;
        self.xref.clear();
        self.cache.borrow_mut().clear();
        for (num, pos) in scan_objects(&self.data) {
            self.xref.insert(num, Entry::At(pos - self.base));
        }
        let mut trailer = Dict::default();
        let mut from = 0;
        while let Some(i) = find(&self.data[from..], b"trailer") {
            let at = from + i + 7;
            if let Ok(Object::Dict(d)) = Parser::new(&self.data, at).object() {
                for (k, v) in d.0 {
                    trailer.insert(&k, v);
                }
            }
            from = at;
        }
        // Files without a `trailer` keep its keys in their xref streams, which
        // also index the objects packed into object streams.
        let nums: Vec<u32> = self.xref.keys().copied().collect();
        for num in nums {
            let obj = self.get(num);
            let Some(s) = obj.as_stream() else { continue };
            if !s.dict.get(b"Type").is_some_and(|t| t.is_name(b"XRef")) {
                continue;
            }
            for key in [&b"Root"[..], b"Info", b"ID", b"Encrypt"] {
                if let (Some(v), false) = (s.dict.get(key), trailer.contains(key)) {
                    trailer.insert(key, v.clone());
                }
            }
            if let Ok(entries) = self.xref_stream_entries(s) {
                for (n, kind, a, b) in entries {
                    if kind == 2 {
                        self.xref.entry(n).or_insert(Entry::Packed { stream: a as u32, index: b as usize });
                    }
                }
            }
        }
        self.trailer = trailer;
        self.cache.borrow_mut().clear();
        if self.catalog().is_none() {
            // No usable /Root: take the first catalog in the file.
            let mut nums: Vec<u32> = self.xref.keys().copied().collect();
            nums.sort_unstable();
            for num in nums {
                if self.get(num).as_dict().is_some_and(|d| d.get(b"Type").is_some_and(|t| t.is_name(b"Catalog"))) {
                    self.trailer.insert(b"Root", Object::Ref(num, 0));
                    break;
                }
            }
        }
        self.trailer.0.retain(|(k, _)| k != b"Prev" && k != b"XRefStm");
        if self.catalog().is_none() {
            return Err("this PDF has no document catalog, even after scanning it".into());
        }
        Ok(())
    }

    // ---- objects ----------------------------------------------------------

    /// Parse `num gen obj … endobj` at `pos`.
    fn read_indirect(&self, pos: usize) -> Option<(u32, Object)> {
        let mut p = Parser::new(&self.data, pos);
        let (Some(Token::Int(num)), Some(Token::Int(_)), Some(Token::Keyword(k))) =
            (p.lexer.next(), p.lexer.next(), p.lexer.next())
        else {
            return None;
        };
        if k != b"obj" || num < 0 {
            return None;
        }
        let obj = p.object().unwrap_or(Object::Null);
        if let (Object::Dict(d), Some(Token::Keyword(k))) = (&obj, p.lexer.next()) {
            if k == b"stream" {
                let raw = self.stream_bytes(d, p.lexer.pos);
                return Some((num as u32, Object::Stream(Rc::new(Stream { dict: d.clone(), raw }))));
            }
        }
        Some((num as u32, obj))
    }

    fn stream_bytes(&self, dict: &Dict, mut pos: usize) -> Vec<u8> {
        let data = &self.data;
        if data.get(pos..pos + 2) == Some(b"\r\n") {
            pos += 2;
        } else if matches!(data.get(pos), Some(b'\n' | b'\r')) {
            pos += 1;
        }
        // /Length, unless it is wrong, which is common enough to check.
        let length = match dict.get(b"Length") {
            // A loop back to this same object resolves to null; see `get`.
            Some(Object::Ref(n, _)) => self.get(*n).as_int(),
            Some(o) => o.as_int(),
            None => None,
        };
        if let Some(len) = length.filter(|l| *l >= 0).map(|l| l as usize) {
            let end = pos.saturating_add(len);
            if end <= data.len() {
                let mut after = end;
                while after < data.len() && super::syntax::is_white(data[after]) {
                    after += 1;
                }
                if data[after..].starts_with(b"endstream") {
                    return data[pos..end].to_vec();
                }
            }
        }
        let mut end = find(&data[pos.min(data.len())..], b"endstream").map_or(data.len(), |i| pos + i);
        if end >= pos + 2 && &data[end - 2..end] == b"\r\n" {
            end -= 2;
        } else if end > pos && matches!(data[end - 1], b'\n' | b'\r') {
            end -= 1;
        }
        data[pos.min(end)..end].to_vec()
    }

    /// Object `num`, or null when there is no such object.
    pub fn get(&self, num: u32) -> Object {
        if let Some(o) = self.cache.borrow().get(&num) {
            return o.clone();
        }
        // Mark it in progress, so a damaged file whose objects refer to each
        // other in a loop resolves the loop to null instead of overflowing.
        self.cache.borrow_mut().insert(num, Object::Null);
        let obj = match self.xref.get(&num).copied() {
            None | Some(Entry::Free) => Object::Null,
            Some(Entry::At(offset)) => self.load_at(num, offset),
            Some(Entry::Packed { stream, index }) => self.load_packed(num, stream, index),
        };
        self.cache.borrow_mut().insert(num, obj.clone());
        obj
    }

    fn load_at(&self, num: u32, offset: usize) -> Object {
        if let Some((got, obj)) = self.read_indirect(self.base + offset) {
            if got == num {
                return obj;
            }
        }
        // The offset is wrong: find the object by scanning instead.
        let mut scanned = self.scanned.borrow_mut();
        let table = scanned.get_or_insert_with(|| scan_objects(&self.data).into_iter().collect());
        match table.get(&num) {
            Some(&pos) => self.read_indirect(pos).map_or(Object::Null, |(_, o)| o),
            None => Object::Null,
        }
    }

    fn load_packed(&self, num: u32, stream: u32, _index: usize) -> Object {
        let cached = self.object_streams.borrow().get(&stream).cloned();
        let entry = match cached {
            Some(e) => e,
            None => {
                let mut table = HashMap::new();
                let mut data = Vec::new();
                if let Object::Stream(s) = self.get(stream) {
                    data = self.stream_data(&s.dict, &s.raw).unwrap_or_default();
                    let n = s.dict.get(b"N").and_then(|v| v.as_int()).unwrap_or(0).clamp(0, 1 << 20);
                    let first = s.dict.get(b"First").and_then(|v| v.as_int()).unwrap_or(0).max(0) as usize;
                    let mut lexer = Lexer::new(&data, 0);
                    for _ in 0..n {
                        match (lexer.next(), lexer.next()) {
                            (Some(Token::Int(obj)), Some(Token::Int(off))) if obj >= 0 && off >= 0 => {
                                table.insert(obj as u32, first + off as usize);
                            }
                            _ => break,
                        }
                    }
                }
                let e = Rc::new((data, table));
                self.object_streams.borrow_mut().insert(stream, e.clone());
                e
            }
        };
        match entry.1.get(&num) {
            Some(&pos) => Parser::new(&entry.0, pos).object().unwrap_or(Object::Null),
            None => Object::Null,
        }
    }

    /// Follow references until a direct object.
    pub fn resolve(&self, obj: &Object) -> Object {
        let mut obj = obj.clone();
        for _ in 0..32 {
            match obj {
                Object::Ref(n, _) => obj = self.get(n),
                other => return other,
            }
        }
        Object::Null
    }

    /// The value under `key`, resolved.
    pub fn get_in(&self, dict: &Dict, key: &[u8]) -> Object {
        dict.get(key).map_or(Object::Null, |v| self.resolve(v))
    }

    /// A stream's bytes, decoded through every filter but the image codecs.
    pub fn stream_data(&self, dict: &Dict, raw: &[u8]) -> Result<Vec<u8>, String> {
        let decoded = filters::decode_stream(dict, raw, &|o| self.resolve(o))?;
        Ok(decoded.data)
    }

    // ---- document level ---------------------------------------------------

    pub fn catalog(&self) -> Option<Dict> {
        match self.get_in(&self.trailer, b"Root") {
            Object::Dict(d) => Some(d),
            _ => None,
        }
    }

    /// Every page, in order, with inherited attributes applied.
    pub fn pages(&self) -> Vec<Page> {
        let mut out = Vec::new();
        let Some(catalog) = self.catalog() else { return out };
        let root = self.get_in(&catalog, b"Pages");
        let mut seen: Vec<u32> = Vec::new();
        self.walk(&root, &Dict::default(), &mut seen, &mut out, 0);
        out
    }

    fn walk(&self, node: &Object, inherited: &Dict, seen: &mut Vec<u32>, out: &mut Vec<Page>, depth: usize) {
        let Some(d) = node.as_dict() else { return };
        if depth > 64 || out.len() > 100_000 {
            return;
        }
        let mut inherited = inherited.clone();
        for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
            if let Some(v) = d.get(key) {
                inherited.insert(key, self.resolve(v));
            }
        }
        let kids = self.get_in(d, b"Kids");
        let is_tree = d.get(b"Type").is_some_and(|t| t.is_name(b"Pages"))
            || (kids.as_array().is_some() && !d.get(b"Type").is_some_and(|t| t.is_name(b"Page")));
        if is_tree {
            for kid in kids.as_array().unwrap_or(&[]) {
                // Cycles in a damaged tree: each node is visited once.
                if let Object::Ref(n, _) = kid {
                    if seen.contains(n) {
                        continue;
                    }
                    seen.push(*n);
                }
                self.walk(&self.resolve(kid), &inherited, seen, out, depth + 1);
            }
            return;
        }
        let rect = |key: &[u8]| -> Option<[f32; 4]> {
            let a = inherited.get(key)?.as_array()?.iter().map(|v| self.resolve(v).as_f32()).collect::<Option<Vec<f32>>>()?;
            (a.len() == 4).then(|| [a[0].min(a[2]), a[1].min(a[3]), a[0].max(a[2]), a[1].max(a[3])])
        };
        let media = rect(b"MediaBox").filter(|r| r[2] > r[0] && r[3] > r[1]).unwrap_or([0.0, 0.0, 612.0, 792.0]);
        let bbox = match rect(b"CropBox") {
            Some(c) => {
                let r = [c[0].max(media[0]), c[1].max(media[1]), c[2].min(media[2]), c[3].min(media[3])];
                if r[2] > r[0] && r[3] > r[1] { r } else { media }
            }
            None => media,
        };
        let rotate = inherited.get(b"Rotate").and_then(|v| v.as_int()).unwrap_or(0).rem_euclid(360) as u32 / 90 * 90;
        let resources = match inherited.get(b"Resources") {
            Some(Object::Dict(r)) => r.clone(),
            _ => Dict::default(),
        };
        out.push(Page { dict: d.clone(), resources, bbox, rotate });
    }

    /// The page's content: every content stream, decoded and joined.
    pub fn contents(&self, page: &Page) -> Vec<u8> {
        let contents = self.get_in(&page.dict, b"Contents");
        let parts: Vec<Object> = match contents {
            Object::Array(a) => a.iter().map(|o| self.resolve(o)).collect(),
            other => vec![other],
        };
        let mut out = Vec::new();
        for part in parts {
            if let Object::Stream(s) = part {
                if let Ok(d) = self.stream_data(&s.dict, &s.raw) {
                    out.extend_from_slice(&d);
                    out.push(b'\n');
                }
            }
        }
        out
    }

    /// A text string from the document information dictionary.
    pub fn info(&self, key: &[u8]) -> Option<String> {
        let info = self.get_in(&self.trailer, b"Info");
        let v = self.get_in(info.as_dict()?, key);
        v.as_bytes().map(decode_text).filter(|s| !s.trim().is_empty())
    }
}

/// A classic table's entries and the trailer after them.
fn parse_table(data: &[u8], pos: usize) -> Result<(Dict, Vec<(u32, Entry)>), String> {
    let mut lexer = Lexer::new(data, pos);
    let mut entries = Vec::new();
    loop {
        match lexer.next() {
            Some(Token::Keyword(k)) if k == b"trailer" => {
                return match Parser::new(data, lexer.pos).object() {
                    Ok(Object::Dict(d)) => Ok((d, entries)),
                    _ => Err("the trailer is not a dictionary".into()),
                };
            }
            Some(Token::Int(start)) => {
                let Some(Token::Int(count)) = lexer.next() else {
                    return Err("bad xref subsection".into());
                };
                if !(0..=MAX_OBJECT_NUMBER).contains(&start) || !(0..=MAX_OBJECT_NUMBER).contains(&count) {
                    return Err("xref subsection out of range".into());
                }
                for num in start..start + count {
                    let (Some(Token::Int(off)), Some(Token::Int(_)), Some(Token::Keyword(kind))) =
                        (lexer.next(), lexer.next(), lexer.next())
                    else {
                        return Err(format!("bad xref entry for object {num}"));
                    };
                    if kind == b"n" && off > 0 {
                        entries.push((num as u32, Entry::At(off as usize)));
                    } else if kind == b"f" {
                        entries.push((num as u32, Entry::Free));
                    }
                }
            }
            _ => return Err("bad xref table".into()),
        }
    }
}

/// A PDF text string: UTF-16BE or UTF-8 by byte order mark, otherwise
/// PDFDocEncoding (Latin-1 with its own 0x18–0x1F and 0x80–0xA0).
pub fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    const HIGH: [u32; 33] = [
        0x2022, 0x2020, 0x2021, 0x2026, 0x2014, 0x2013, 0x0192, 0x2044, 0x2039, 0x203A, 0x2212,
        0x2030, 0x201E, 0x201C, 0x201D, 0x2018, 0x2019, 0x201A, 0x2122, 0xFB01, 0xFB02, 0x0141,
        0x0152, 0x0160, 0x0178, 0x017D, 0x0131, 0x0142, 0x0153, 0x0161, 0x017E, 0xFFFD, 0x20AC,
    ];
    const LOW: [u32; 8] = [0x02D8, 0x02C7, 0x02C6, 0x02D9, 0x02DD, 0x02DB, 0x02DA, 0x02DC];
    bytes
        .iter()
        .map(|&b| {
            let c = match b {
                0x18..=0x1F => LOW[(b - 0x18) as usize],
                0x80..=0xA0 => HIGH[(b - 0x80) as usize],
                _ => b as u32,
            };
            char::from_u32(c).unwrap_or('\u{FFFD}')
        })
        .collect()
}

/// Every `n g obj` in the file, the last definition of each winning.
fn scan_objects(data: &[u8]) -> Vec<(u32, usize)> {
    let mut found: HashMap<u32, usize> = HashMap::new();
    let mut i = 0;
    while let Some(k) = find(&data[i..], b"obj") {
        let at = i + k;
        i = at + 3;
        // Not part of `endobj`, and followed by a delimiter.
        if at >= 3 && &data[at - 3..at] == b"end" {
            continue;
        }
        if data.get(at + 3).is_some_and(|&c| !super::syntax::is_white(c) && !super::syntax::is_delimiter(c)) {
            continue;
        }
        // Walk back over: whitespace, generation digits, whitespace, number digits.
        let mut j = at;
        let back = |pred: &dyn Fn(u8) -> bool, j: &mut usize| -> usize {
            let end = *j;
            while *j > 0 && pred(data[*j - 1]) {
                *j -= 1;
            }
            end - *j
        };
        if back(&super::syntax::is_white, &mut j) == 0 {
            continue;
        }
        if back(&|c: u8| c.is_ascii_digit(), &mut j) == 0 {
            continue;
        }
        if back(&super::syntax::is_white, &mut j) == 0 {
            continue;
        }
        let num_end = j;
        if back(&|c: u8| c.is_ascii_digit(), &mut j) == 0 {
            continue;
        }
        if j > 0 && data[j - 1].is_ascii_digit() {
            continue;
        }
        if let Some(num) = std::str::from_utf8(&data[j..num_end]).ok().and_then(|s| s.parse::<u32>().ok()) {
            found.insert(num, j);
        }
    }
    found.into_iter().collect()
}

pub fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).rposition(|w| w == needle)
}
