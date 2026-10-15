//! Tokens and objects (ISO 32000-1 §7.2–7.3).
//!
//! Strings come back as bytes, literal and hex alike; names as bytes with
//! their `#xx` escapes undone. Everything is lenient the way real files need:
//! a stray delimiter is stepped over rather than fatal.

use std::rc::Rc;

/// A PDF object. Streams are shared, since the same one is reached from many
/// places and its bytes can be large.
#[derive(Debug, Clone, PartialEq)]
pub enum Object {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    String(Vec<u8>),
    Name(Vec<u8>),
    Array(Vec<Object>),
    Dict(Dict),
    Stream(Rc<Stream>),
    Ref(u32, u16),
}

impl Object {
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Object::Int(i) => Some(i as f64),
            Object::Real(r) => Some(r),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        self.as_f64().map(|v| v as f32)
    }

    pub fn as_int(&self) -> Option<i64> {
        match *self {
            Object::Int(i) => Some(i),
            Object::Real(r) if r.fract() == 0.0 => Some(r as i64),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<&[u8]> {
        match self {
            Object::Name(n) => Some(n),
            _ => None,
        }
    }

    pub fn is_name(&self, name: &[u8]) -> bool {
        self.as_name() == Some(name)
    }

    pub fn as_array(&self) -> Option<&[Object]> {
        match self {
            Object::Array(a) => Some(a),
            _ => None,
        }
    }

    /// The dictionary of a dictionary, or of a stream.
    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Object::Dict(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    pub fn as_stream(&self) -> Option<&Rc<Stream>> {
        match self {
            Object::Stream(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Object::String(s) => Some(s),
            _ => None,
        }
    }
}

/// A dictionary. Small enough in practice that a list beats a map.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dict(pub Vec<(Vec<u8>, Object)>);

impl Dict {
    pub fn get(&self, key: &[u8]) -> Option<&Object> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn contains(&self, key: &[u8]) -> bool {
        self.get(key).is_some()
    }

    pub fn insert(&mut self, key: &[u8], value: Object) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key.to_vec(), value)),
        }
    }
}

/// A stream: its dictionary and its bytes, still encoded.
#[derive(Debug, PartialEq)]
pub struct Stream {
    pub dict: Dict,
    pub raw: Vec<u8>,
}

/// One token. Operators and the delimiters come back as `Keyword`.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Int(i64),
    Real(f64),
    String(Vec<u8>),
    Name(Vec<u8>),
    Keyword(Vec<u8>),
    ArrayOpen,
    ArrayClose,
    DictOpen,
    DictClose,
}

pub fn is_white(c: u8) -> bool {
    matches!(c, 0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ')
}

pub fn is_delimiter(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn is_regular(c: u8) -> bool {
    !is_white(c) && !is_delimiter(c)
}

pub struct Lexer<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub fn skip_white(&mut self) {
        let d = self.data;
        while self.pos < d.len() {
            let c = d[self.pos];
            if is_white(c) {
                self.pos += 1;
            } else if c == b'%' {
                while self.pos < d.len() && d[self.pos] != b'\n' && d[self.pos] != b'\r' {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn literal_string(&mut self) -> Vec<u8> {
        let d = self.data;
        let mut pos = self.pos + 1;
        let mut depth = 1;
        let mut out = Vec::new();
        while pos < d.len() {
            let c = d[pos];
            match c {
                b'\\' => {
                    pos += 1;
                    let Some(&e) = d.get(pos) else { break };
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut v: u32 = 0;
                            let mut n = 0;
                            while n < 3 && pos < d.len() && (b'0'..=b'7').contains(&d[pos]) {
                                v = v * 8 + (d[pos] - b'0') as u32;
                                pos += 1;
                                n += 1;
                            }
                            out.push(v as u8);
                            continue;
                        }
                        // A backslash at the end of a line continues it.
                        b'\r' => {
                            if d.get(pos + 1) == Some(&b'\n') {
                                pos += 1;
                            }
                        }
                        b'\n' => {}
                        // \( \) \\ and anything unknown: the character itself.
                        other => out.push(other),
                    }
                    pos += 1;
                }
                b'(' => {
                    depth += 1;
                    out.push(c);
                    pos += 1;
                }
                b')' => {
                    depth -= 1;
                    pos += 1;
                    if depth == 0 {
                        break;
                    }
                    out.push(c);
                }
                // Every end of line reads as \n.
                b'\r' => {
                    out.push(b'\n');
                    pos += if d.get(pos + 1) == Some(&b'\n') { 2 } else { 1 };
                }
                _ => {
                    out.push(c);
                    pos += 1;
                }
            }
        }
        self.pos = pos;
        out
    }

    fn hex_string(&mut self) -> Vec<u8> {
        let d = self.data;
        let mut pos = self.pos + 1;
        let mut out = Vec::new();
        let mut high: Option<u8> = None;
        while pos < d.len() && d[pos] != b'>' {
            if let Some(v) = hex_value(d[pos]) {
                match high.take() {
                    Some(h) => out.push(h << 4 | v),
                    None => high = Some(v),
                }
            }
            pos += 1;
        }
        if let Some(h) = high {
            out.push(h << 4); // an odd final digit is followed by an implied 0
        }
        self.pos = (pos + 1).min(d.len());
        out
    }
}

impl Iterator for Lexer<'_> {
    type Item = Token;

    /// The next token, or `None` at the end.
    fn next(&mut self) -> Option<Token> {
        self.skip_white();
        let d = self.data;
        let c = *d.get(self.pos)?;
        match c {
            b'/' => {
                self.pos += 1;
                let start = self.pos;
                while self.pos < d.len() && is_regular(d[self.pos]) {
                    self.pos += 1;
                }
                Some(Token::Name(unescape_name(&d[start..self.pos])))
            }
            b'(' => Some(Token::String(self.literal_string())),
            b'<' => {
                if d.get(self.pos + 1) == Some(&b'<') {
                    self.pos += 2;
                    Some(Token::DictOpen)
                } else {
                    Some(Token::String(self.hex_string()))
                }
            }
            b'>' => {
                if d.get(self.pos + 1) == Some(&b'>') {
                    self.pos += 2;
                    Some(Token::DictClose)
                } else {
                    self.pos += 1;
                    Some(Token::Keyword(b">".to_vec()))
                }
            }
            b'[' => {
                self.pos += 1;
                Some(Token::ArrayOpen)
            }
            b']' => {
                self.pos += 1;
                Some(Token::ArrayClose)
            }
            b'{' | b'}' | b')' => {
                self.pos += 1;
                Some(Token::Keyword(vec![c]))
            }
            _ => {
                let start = self.pos;
                while self.pos < d.len() && is_regular(d[self.pos]) {
                    self.pos += 1;
                }
                let word = &d[start..self.pos];
                Some(number(word).unwrap_or_else(|| Token::Keyword(word.to_vec())))
            }
        }
    }
}

pub fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn unescape_name(raw: &[u8]) -> Vec<u8> {
    if !raw.contains(&b'#') {
        return raw.to_vec();
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'#' {
            if let (Some(h), Some(l)) =
                (raw.get(i + 1).and_then(|&c| hex_value(c)), raw.get(i + 2).and_then(|&c| hex_value(c)))
            {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

/// An integer or real, if the word is one.
fn number(word: &[u8]) -> Option<Token> {
    let (sign, digits) = match word.first()? {
        b'+' => (1.0, &word[1..]),
        b'-' => (-1.0, &word[1..]),
        _ => (1.0, word),
    };
    if digits.is_empty() || !digits.iter().all(|c| c.is_ascii_digit() || *c == b'.') {
        return None;
    }
    let dots = digits.iter().filter(|&&c| c == b'.').count();
    if dots > 1 || digits == b"." {
        return None;
    }
    let text = std::str::from_utf8(digits).ok()?;
    if dots == 0 {
        // Too long for i64 still reads, as a real.
        return Some(match text.parse::<i64>() {
            Ok(v) => Token::Int(if sign < 0.0 { -v } else { v }),
            Err(_) => Token::Real(sign * text.parse::<f64>().ok()?),
        });
    }
    let text = if text.starts_with('.') { format!("0{text}") } else { text.to_string() };
    let text = if text.ends_with('.') { format!("{text}0") } else { text };
    Some(Token::Real(sign * text.parse::<f64>().ok()?))
}

/// Builds objects from tokens.
pub struct Parser<'a> {
    pub lexer: Lexer<'a>,
}

const MAX_NESTING: usize = 256;

impl<'a> Parser<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Self { lexer: Lexer::new(data, pos) }
    }

    /// The next object. `n g R` becomes a reference. A keyword where an
    /// object belongs comes back as `Err` with the keyword, so callers that
    /// expect one (`obj`, `stream`, an operator) can act on it.
    pub fn object(&mut self) -> Result<Object, Option<Vec<u8>>> {
        let tok = self.lexer.next().ok_or(None)?;
        self.build(tok, 0)
    }

    pub fn build(&mut self, tok: Token, depth: usize) -> Result<Object, Option<Vec<u8>>> {
        if depth > MAX_NESTING {
            return Ok(Object::Null);
        }
        Ok(match tok {
            Token::Int(i) => {
                if i >= 0 {
                    if let Some(r) = self.maybe_ref(i) {
                        return Ok(r);
                    }
                }
                Object::Int(i)
            }
            Token::Real(r) => Object::Real(r),
            Token::String(s) => Object::String(s),
            Token::Name(n) => Object::Name(n),
            Token::ArrayOpen => {
                let mut items = Vec::new();
                loop {
                    match self.lexer.next() {
                        None | Some(Token::ArrayClose) => break,
                        Some(t) => match self.build(t, depth + 1) {
                            Ok(o) => items.push(o),
                            Err(Some(k)) => items.push(keyword_object(&k)),
                            Err(None) => break,
                        },
                    }
                }
                Object::Array(items)
            }
            Token::DictOpen => {
                let mut dict = Dict::default();
                loop {
                    match self.lexer.next() {
                        None | Some(Token::DictClose) => break,
                        Some(Token::Name(key)) => {
                            let Some(t) = self.lexer.next() else { break };
                            if t == Token::DictClose {
                                break;
                            }
                            match self.build(t, depth + 1) {
                                // A null value is an absent key.
                                Ok(Object::Null) => {}
                                Ok(v) => dict.insert(&key, v),
                                Err(Some(k)) => {
                                    let v = keyword_object(&k);
                                    if v != Object::Null {
                                        dict.insert(&key, v);
                                    }
                                }
                                Err(None) => break,
                            }
                        }
                        // Junk where a key belongs is stepped over.
                        Some(_) => {}
                    }
                }
                Object::Dict(dict)
            }
            Token::Keyword(k) => match k.as_slice() {
                b"true" => Object::Bool(true),
                b"false" => Object::Bool(false),
                b"null" => Object::Null,
                _ => return Err(Some(k)),
            },
            Token::ArrayClose | Token::DictClose => return Err(Some(Vec::new())),
        })
    }

    fn maybe_ref(&mut self, num: i64) -> Option<Object> {
        let save = self.lexer.pos;
        if let Some(Token::Int(gen)) = self.lexer.next() {
            if gen >= 0 {
                if let Some(Token::Keyword(k)) = self.lexer.next() {
                    if k == b"R" {
                        return Some(Object::Ref(num as u32, gen as u16));
                    }
                }
            }
        }
        self.lexer.pos = save;
        None
    }
}

fn keyword_object(k: &[u8]) -> Object {
    match k {
        b"true" => Object::Bool(true),
        b"false" => Object::Bool(false),
        _ => Object::Null,
    }
}

/// Parse one object from bytes; a convenience for tests.
pub fn parse_one(data: &[u8]) -> Option<Object> {
    Parser::new(data, 0).object().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(data: &[u8]) -> Vec<u8> {
        match parse_one(data) {
            Some(Object::String(v)) => v,
            other => panic!("not a string: {other:?}"),
        }
    }

    #[test]
    fn literal_strings_unescape_and_balance() {
        assert_eq!(s(br"(a\(b\)c\\d)"), b"a(b)c\\d");
        assert_eq!(s(b"(nested (parens) stay)"), b"nested (parens) stay");
        assert_eq!(s(br"(\101\0533\7)"), b"A+3\x07");
        assert_eq!(s(b"(one\\\r\ntwo)"), b"onetwo");
        assert_eq!(s(b"(a\r\nb\rc)"), b"a\nb\nc");
        assert_eq!(s(br"(\q)"), b"q");
    }

    #[test]
    fn hex_strings_and_names() {
        assert_eq!(s(b"<48 65 6C\n6c 6F>"), b"Hello");
        assert_eq!(s(b"<901FA>"), b"\x90\x1f\xa0");
        assert_eq!(parse_one(b"/A#20B"), Some(Object::Name(b"A B".to_vec())));
        assert_eq!(parse_one(b"/"), Some(Object::Name(Vec::new())));
    }

    #[test]
    fn numbers_and_references() {
        let a = parse_one(b"[123 -17 +5 34.5 -.002 4. 1 0 R 0 0 612 792 7 0 R]").unwrap();
        assert_eq!(
            a,
            Object::Array(vec![
                Object::Int(123),
                Object::Int(-17),
                Object::Int(5),
                Object::Real(34.5),
                Object::Real(-0.002),
                Object::Real(4.0),
                Object::Ref(1, 0),
                Object::Int(0),
                Object::Int(0),
                Object::Int(612),
                Object::Int(792),
                Object::Ref(7, 0),
            ])
        );
    }

    #[test]
    fn dictionaries_drop_nulls_and_skip_junk() {
        let d = parse_one(b"<< /Type /Page /Gone null /A 1 ) /T true /Sub << /B (x) >> >>").unwrap();
        let d = d.as_dict().unwrap();
        assert!(d.get(b"Type").unwrap().is_name(b"Page"));
        assert!(!d.contains(b"Gone"));
        assert_eq!(d.get(b"A"), Some(&Object::Int(1)));
        assert_eq!(d.get(b"T"), Some(&Object::Bool(true)));
        assert_eq!(d.get(b"Sub").unwrap().as_dict().unwrap().get(b"B"), Some(&Object::String(b"x".to_vec())));
    }

    #[test]
    fn comments_are_whitespace() {
        let mut l = Lexer::new(b"% a comment\n42 % another\n/Name", 0);
        assert_eq!(l.next(), Some(Token::Int(42)));
        assert_eq!(l.next(), Some(Token::Name(b"Name".to_vec())));
        assert_eq!(l.next(), None);
    }
}
