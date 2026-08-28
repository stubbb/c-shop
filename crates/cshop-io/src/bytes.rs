//! Little-endian reading and writing, and the errors that come of trusting a
//! file too far.
//!
//! Every `read_*` is bounds-checked and returns an error rather than panicking:
//! a document format is an attack surface, and a truncated or hostile file
//! should be refused, not crash the editor.

use crate::IoError;

/// Append-only byte sink.
#[derive(Default)]
pub struct Writer {
    pub bytes: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer { bytes: Vec::new() }
    }

    pub fn u8(&mut self, v: u8) {
        self.bytes.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i32(&mut self, v: i32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }
    pub fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }
    pub fn raw(&mut self, v: &[u8]) {
        self.bytes.extend_from_slice(v);
    }

    /// A length-prefixed UTF-8 string.
    pub fn string(&mut self, v: &str) {
        self.u32(v.len() as u32);
        self.raw(v.as_bytes());
    }

    /// A length-prefixed blob.
    pub fn blob(&mut self, v: &[u8]) {
        self.u32(v.len() as u32);
        self.raw(v);
    }

    pub fn f32s(&mut self, v: &[f32]) {
        for x in v {
            self.f32(*x);
        }
    }
}

/// Cursor over a byte slice.
pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Move to an absolute position, refusing to go past the end.
    pub fn seek(&mut self, to: usize) -> Result<(), IoError> {
        if to > self.bytes.len() {
            return Err(IoError::Malformed("seek past the end of the file".into()));
        }
        self.pos = to;
        Ok(())
    }

    pub fn skip(&mut self, n: usize) -> Result<(), IoError> {
        self.seek(self.pos + n)
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], IoError> {
        if self.remaining() < n {
            return Err(IoError::Malformed(format!(
                "wanted {n} bytes but only {} are left",
                self.remaining()
            )));
        }
        let out = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, IoError> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, IoError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, IoError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, IoError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32, IoError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32, IoError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn bool(&mut self) -> Result<bool, IoError> {
        Ok(self.u8()? != 0)
    }

    pub fn string(&mut self) -> Result<String, IoError> {
        let n = self.u32()? as usize;
        let bytes = self.take(n)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| IoError::Malformed("a name was not valid UTF-8".into()))
    }

    pub fn blob(&mut self) -> Result<&'a [u8], IoError> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    pub fn f32s<const N: usize>(&mut self) -> Result<[f32; N], IoError> {
        let mut out = [0.0f32; N];
        for slot in out.iter_mut() {
            *slot = self.f32()?;
        }
        Ok(out)
    }

    // --- big-endian, which is what PSD uses -------------------------------
    pub fn be_u16(&mut self) -> Result<u16, IoError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn be_u32(&mut self) -> Result<u32, IoError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn be_i16(&mut self) -> Result<i16, IoError> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn be_i32(&mut self) -> Result<i32, IoError> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
}

impl Writer {
    // --- big-endian, which is what PSD uses -------------------------------
    pub fn be_u16(&mut self, v: u16) {
        self.bytes.extend_from_slice(&v.to_be_bytes());
    }
    pub fn be_u32(&mut self, v: u32) {
        self.bytes.extend_from_slice(&v.to_be_bytes());
    }
    pub fn be_i16(&mut self, v: i16) {
        self.bytes.extend_from_slice(&v.to_be_bytes());
    }
    pub fn be_i32(&mut self, v: i32) {
        self.bytes.extend_from_slice(&v.to_be_bytes());
    }

    /// Overwrite a four-byte big-endian length written earlier, once the
    /// section it covers has been built.
    pub fn patch_be_u32(&mut self, at: usize, v: u32) {
        self.bytes[at..at + 4].copy_from_slice(&v.to_be_bytes());
    }
}
