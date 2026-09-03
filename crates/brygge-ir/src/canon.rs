//! The hand-rolled **canonical** binary codec (RFC 003 D-1).
//!
//! One canonical form per value, so the same logical data always yields the same bytes — the basis of
//! determinism (`VF-1`), content-addressing, and the integrity digest. Deliberately dependency-free
//! (RFC 001 D-6): unsigned LEB128 varints, zig-zag varints for signed, length-prefixed byte strings,
//! fixed 32-byte ids written raw, single-byte enum tags, and `Vec`/map lengths written before their
//! (already-sorted) elements. The reader is fully bounds-checked: malformed input is an
//! [`Error::Decode`], never a panic (brygge reads untrusted history).

use crate::Error;

/// Appends canonical bytes to an in-memory buffer.
#[derive(Debug, Default)]
pub struct CanonWriter {
    buf: Vec<u8>,
}

impl CanonWriter {
    /// A new, empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume the writer and return the bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// The bytes written so far.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Write a single tag/discriminant byte.
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    /// Write an unsigned integer as LEB128.
    pub fn uvarint(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(byte);
                break;
            }
            self.buf.push(byte | 0x80);
        }
    }

    /// Write a signed integer as zig-zag + LEB128.
    pub fn ivarint(&mut self, v: i64) {
        self.uvarint(zigzag(v));
    }

    /// Write a length-prefixed byte string.
    pub fn bytes(&mut self, b: &[u8]) {
        self.uvarint(b.len() as u64);
        self.buf.extend_from_slice(b);
    }

    /// Write a length-prefixed UTF-8 string.
    pub fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }

    /// Write a fixed 32-byte id (no length prefix — the reader knows the width).
    pub fn raw32(&mut self, b: &[u8; 32]) {
        self.buf.extend_from_slice(b);
    }
}

/// Reads canonical bytes from a slice, bounds-checked.
#[derive(Debug)]
pub struct CanonReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> CanonReader<'a> {
    /// A reader over `buf`.
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// True when every byte has been consumed (used to reject trailing garbage).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| Error::Decode("length overflow".to_string()))?;
        let slice = self
            .buf
            .get(self.pos..end)
            .ok_or_else(|| Error::Decode("unexpected end of input".to_string()))?;
        self.pos = end;
        Ok(slice)
    }

    /// Read a single byte.
    pub fn u8(&mut self) -> Result<u8, Error> {
        Ok(*self
            .take(1)?
            .first()
            .ok_or_else(|| Error::Decode("unexpected end of input".to_string()))?)
    }

    /// Read an unsigned LEB128 integer (rejects overlong/overflowing encodings).
    pub fn uvarint(&mut self) -> Result<u64, Error> {
        let mut result: u64 = 0;
        let mut shift: u32 = 0;
        loop {
            let byte = self.u8()?;
            if shift >= 64 {
                return Err(Error::Decode("varint too long".to_string()));
            }
            result |= u64::from(byte & 0x7f)
                .checked_shl(shift)
                .ok_or_else(|| Error::Decode("varint overflow".to_string()))?;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        Ok(result)
    }

    /// Read a signed zig-zag + LEB128 integer.
    pub fn ivarint(&mut self) -> Result<i64, Error> {
        Ok(unzigzag(self.uvarint()?))
    }

    /// Read a length-prefixed byte string (with a sanity cap against absurd lengths).
    pub fn bytes(&mut self) -> Result<Vec<u8>, Error> {
        let len = self.uvarint()?;
        let len =
            usize::try_from(len).map_err(|_| Error::Decode("length too large".to_string()))?;
        if len > self.remaining() {
            return Err(Error::Decode("declared length exceeds input".to_string()));
        }
        Ok(self.take(len)?.to_vec())
    }

    /// Read a length-prefixed UTF-8 string.
    pub fn str(&mut self) -> Result<String, Error> {
        String::from_utf8(self.bytes()?).map_err(|_| Error::Decode("invalid UTF-8".to_string()))
    }

    /// Read a fixed 32-byte id.
    pub fn raw32(&mut self) -> Result<[u8; 32], Error> {
        let slice = self.take(32)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(slice);
        Ok(out)
    }
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

#[cfg(test)]
mod tests;
