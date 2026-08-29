//! JSON, by hand.
//!
//! Written rather than pulled in for the same reason the project format and
//! the PSD codec are: the whole of JSON is a page of grammar, and a parser
//! that faces the network is somewhere the failure modes want to be visible
//! rather than trusted. It is also the only way to keep this crate's
//! dependency list at nothing, which is the point of the editor generally.
//!
//! Objects keep their insertion order. JSON does not require it, but a report
//! a person reads by eye is worth more when its fields do not shuffle.

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn object(fields: Vec<(&str, Json)>) -> Json {
        Json::Object(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    /// A string field, which is what nearly every lookup here wants.
    pub fn str_field(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Json::as_str)
    }

    pub fn write(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out);
        out
    }

    fn write_into(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Number(n) => {
                // JSON has no infinity and no NaN. Rather than emit something
                // no parser will read back, say null — the alternative is a
                // response the caller cannot decode at all.
                if n.is_finite() {
                    if *n == n.trunc() && n.abs() < 1e15 {
                        let _ = write!(out, "{}", *n as i64);
                    } else {
                        let _ = write!(out, "{n}");
                    }
                } else {
                    out.push_str("null");
                }
            }
            Json::String(s) => escape_into(s, out),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_into(out);
                }
                out.push(']');
            }
            Json::Object(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    escape_into(k, out);
                    out.push(':');
                    v.write_into(out);
                }
                out.push('}');
            }
        }
    }
}

impl From<bool> for Json {
    fn from(v: bool) -> Json {
        Json::Bool(v)
    }
}

impl From<f64> for Json {
    fn from(v: f64) -> Json {
        Json::Number(v)
    }
}

impl From<u32> for Json {
    fn from(v: u32) -> Json {
        Json::Number(v as f64)
    }
}

impl From<usize> for Json {
    fn from(v: usize) -> Json {
        Json::Number(v as f64)
    }
}

impl From<&str> for Json {
    fn from(v: &str) -> Json {
        Json::String(v.to_string())
    }
}

impl From<String> for Json {
    fn from(v: String) -> Json {
        Json::String(v)
    }
}

fn escape_into(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Everything below a space has to be escaped; the rest may go
            // through as UTF-8, which every JSON reader accepts.
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// How deeply nested a document may be.
///
/// This parser recurses, so without a limit a few kilobytes of `[[[[…` from
/// the network would overflow the stack. That is the whole reason for it.
const MAX_DEPTH: usize = 64;

pub fn parse(source: &str) -> Result<Json, String> {
    let bytes: Vec<char> = source.chars().collect();
    let mut p = Parser { c: &bytes, at: 0, depth: 0 };
    p.skip_whitespace();
    let value = p.value()?;
    p.skip_whitespace();
    if p.at != p.c.len() {
        return Err(format!("trailing text after the value, at character {}", p.at));
    }
    Ok(value)
}

struct Parser<'a> {
    c: &'a [char],
    at: usize,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.c.get(self.at).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.at += 1;
        }
    }

    fn expect(&mut self, want: char) -> Result<(), String> {
        if self.peek() == Some(want) {
            self.at += 1;
            Ok(())
        } else {
            Err(format!("expected {want:?} at character {}", self.at))
        }
    }

    fn literal(&mut self, word: &str) -> Result<(), String> {
        for want in word.chars() {
            self.expect(want)?;
        }
        Ok(())
    }

    fn value(&mut self) -> Result<Json, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(format!("nested more than {MAX_DEPTH} deep"));
        }
        let out = match self.peek() {
            None => Err("the document ends where a value was expected".to_string()),
            Some('n') => self.literal("null").map(|_| Json::Null),
            Some('t') => self.literal("true").map(|_| Json::Bool(true)),
            Some('f') => self.literal("false").map(|_| Json::Bool(false)),
            Some('"') => self.string().map(Json::String),
            Some('[') => self.array(),
            Some('{') => self.object(),
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(format!("{c:?} does not start a value, at character {}", self.at)),
        };
        self.depth -= 1;
        out
    }

    fn array(&mut self) -> Result<Json, String> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(']') {
            self.at += 1;
            return Ok(Json::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(',') => self.at += 1,
                Some(']') => {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                _ => return Err(format!("expected ',' or ']' at character {}", self.at)),
            }
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.expect('{')?;
        let mut fields = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some('}') {
            self.at += 1;
            return Ok(Json::Object(fields));
        }
        loop {
            self.skip_whitespace();
            let key = self.string()?;
            self.skip_whitespace();
            self.expect(':')?;
            self.skip_whitespace();
            let value = self.value()?;
            fields.push((key, value));
            self.skip_whitespace();
            match self.peek() {
                Some(',') => self.at += 1,
                Some('}') => {
                    self.at += 1;
                    return Ok(Json::Object(fields));
                }
                _ => return Err(format!("expected ',' or '}}' at character {}", self.at)),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err("the document ends inside a string".to_string());
            };
            self.at += 1;
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let Some(esc) = self.peek() else {
                        return Err("the document ends inside an escape".to_string());
                    };
                    self.at += 1;
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => out.push(self.unicode_escape()?),
                        other => return Err(format!("\\{other} is not an escape")),
                    }
                }
                c if (c as u32) < 0x20 => {
                    return Err(format!("a raw control character at {}", self.at - 1))
                }
                c => out.push(c),
            }
        }
    }

    /// A `\uXXXX` escape, including the surrogate pair that any character
    /// outside the basic plane arrives as.
    fn unicode_escape(&mut self) -> Result<char, String> {
        let first = self.hex4()?;
        // A high surrogate is only half a character; the low half must follow.
        if (0xD800..0xDC00).contains(&first) {
            if self.peek() != Some('\\') {
                return Err("a high surrogate with nothing after it".to_string());
            }
            self.at += 1;
            self.expect('u')?;
            let second = self.hex4()?;
            if !(0xDC00..0xE000).contains(&second) {
                return Err("a high surrogate followed by something else".to_string());
            }
            let combined = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
            return char::from_u32(combined).ok_or_else(|| "not a character".to_string());
        }
        char::from_u32(first).ok_or_else(|| format!("\\u{first:04x} is not a character"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut value = 0u32;
        for _ in 0..4 {
            let Some(c) = self.peek() else {
                return Err("the document ends inside a \\u escape".to_string());
            };
            let digit = c.to_digit(16).ok_or_else(|| format!("{c:?} is not a hex digit"))?;
            value = value * 16 + digit;
            self.at += 1;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.at;
        if self.peek() == Some('-') {
            self.at += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.at += 1;
        }
        if self.peek() == Some('.') {
            self.at += 1;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.at += 1;
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.at += 1;
            if matches!(self.peek(), Some('+' | '-')) {
                self.at += 1;
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.at += 1;
            }
        }
        let text: String = self.c[start..self.at].iter().collect();
        text.parse::<f64>()
            .map(Json::Number)
            .map_err(|_| format!("{text:?} is not a number"))
    }
}
