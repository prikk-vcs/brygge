//! The hand-rolled **canonical** binary codec: tagged records with a critical bit (RFC 011 D-1).
//!
//! One canonical form per value, so the same logical data always yields the same bytes — the basis of
//! determinism (`VF-1`), content-addressing, and the integrity digest. Deliberately dependency-free
//! (RFC 001 D-6). Every value is one of: a **primitive** (`uvarint`/`svarint`/raw bytes/`id32`), a
//! **record** (a `uvarint(n)` field count, then `n` fields in **strictly ascending tag order**), a
//! **list** (`uvarint(count)` then that many items), a **map** (`uvarint(count)` then that many
//! `(key, value)` text pairs, keys strictly ascending), or an **enum** (`uvarint(variant) ‖ record`). A
//! **field** is `uvarint(tag) ‖ uvarint(len) ‖ value (exactly len bytes)`, where `tag = (id << 1) |
//! critical`. Reading is fully bounds-checked and enforces every canonical rule (RFC 011 §2.2/D-1):
//! malformed or non-canonical input is a typed [`crate::Error`], never a panic (brygge reads untrusted
//! artifacts) and never silently accepted (an accepted non-canonical byte sequence would let two
//! different byte strings mean the same value, defeating content-addressing and the digest).

use crate::Error;

/// Appends canonical bytes to an in-memory buffer. Low-level: callers build a field's *value* bytes with
/// a fresh writer, then hand them to [`RecordWriter::field`], which frames them with a tag and length.
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

    /// Write an unsigned integer as minimal LEB128.
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

    /// Write a signed integer as zig-zag + minimal LEB128.
    pub fn svarint(&mut self, v: i64) {
        self.uvarint(zigzag(v));
    }

    /// Append raw bytes verbatim — **no** length prefix. Used to assemble a field's value (the field's
    /// own `tag`/`len` header, written by [`RecordWriter`], carries the length) and to concatenate
    /// already-encoded sub-values (a nested record's bytes, a list item's bytes).
    pub fn raw(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    /// Write a fixed 32-byte id (no length prefix — the schema fixes the width).
    pub fn raw32(&mut self, b: &[u8; 32]) {
        self.buf.extend_from_slice(b);
    }
}

/// Builds one record's field list, enforcing ascending tag order as fields are added, then frames it as
/// `uvarint(count) ‖ (uvarint(tag) ‖ uvarint(len) ‖ value)*`.
#[derive(Debug, Default)]
pub struct RecordWriter {
    fields: Vec<(u64, Vec<u8>)>,
    last_id: Option<u64>,
}

impl RecordWriter {
    /// A new, empty record.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add field `id` with pre-encoded `value` bytes. Every field in contract 0.2.0 is critical
    /// (`critical = true`); the parameter exists because the wire format allows non-critical fields
    /// (RFC 011 D-3), for a future minor version to add one. Fields must be added in ascending `id`
    /// order — every call site in this crate follows the schema tables' own field-id order, so this is
    /// asserted, not computed.
    pub fn field(&mut self, id: u64, critical: bool, value: Vec<u8>) {
        debug_assert!(
            self.last_id.is_none_or(|last| id > last),
            "record fields must be added in ascending id order (got {id} after {:?})",
            self.last_id
        );
        self.last_id = Some(id);
        let tag = (id << 1) | u64::from(critical);
        self.fields.push((tag, value));
    }

    /// Frame the record into `w`: `uvarint(count)` then each field's `tag ‖ len ‖ value`.
    pub fn finish_into(self, w: &mut CanonWriter) {
        w.uvarint(self.fields.len() as u64);
        for (tag, value) in self.fields {
            w.uvarint(tag);
            w.uvarint(value.len() as u64);
            w.raw(&value);
        }
    }

    /// Frame the record and return its bytes directly — for a record that is itself a field's value, or
    /// an enum's payload.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        self.finish_into(&mut w);
        w.into_bytes()
    }
}

/// Encode a `uvarint` value as a standalone field value (the field header carries no inner length; this
/// is just the varint bytes).
#[must_use]
pub fn uvarint_value(v: u64) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.uvarint(v);
    w.into_bytes()
}

/// Encode an `svarint` value as a standalone field value.
#[must_use]
pub fn svarint_value(v: i64) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.svarint(v);
    w.into_bytes()
}

/// Encode a `list<T>` field value: `uvarint(count)` then each item's already-encoded bytes, concatenated.
#[must_use]
pub fn list_value(items: impl ExactSizeIterator<Item = Vec<u8>>) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.uvarint(items.len() as u64);
    for item in items {
        w.raw(&item);
    }
    w.into_bytes()
}

/// Encode a `map` field value: `uvarint(count)` then `(klen ‖ key ‖ vlen ‖ value)*`. The caller passes
/// entries already in ascending key order (a `BTreeMap`'s own iteration order).
#[must_use]
pub fn map_value<'a>(entries: impl ExactSizeIterator<Item = (&'a str, &'a str)>) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.uvarint(entries.len() as u64);
    for (k, v) in entries {
        w.uvarint(k.len() as u64);
        w.raw(k.as_bytes());
        w.uvarint(v.len() as u64);
        w.raw(v.as_bytes());
    }
    w.into_bytes()
}

/// Reads canonical bytes from a slice, bounds-checked, enforcing every canonical rule on the way
/// (RFC 011 §2.2). Tracks how many non-critical fields it skipped, across the whole decode
/// ([`Decoded::skipped_non_critical_fields`](crate::artifact::Decoded)).
#[derive(Debug)]
pub struct CanonReader<'a> {
    buf: &'a [u8],
    pos: usize,
    /// Fields with an unrecognized id and `critical = 0`, skipped rather than rejected.
    skipped_non_critical_fields: u64,
}

impl<'a> CanonReader<'a> {
    /// A reader over `buf`.
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Self {
            buf,
            pos: 0,
            skipped_non_critical_fields: 0,
        }
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

    /// How many non-critical fields with an unrecognized id have been skipped so far.
    #[must_use]
    pub fn skipped_non_critical_fields(&self) -> u64 {
        self.skipped_non_critical_fields
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

    /// Read a single byte (a variant tag, a fixed enum discriminant).
    pub fn u8(&mut self) -> Result<u8, Error> {
        Ok(*self
            .take(1)?
            .first()
            .ok_or_else(|| Error::Decode("unexpected end of input".to_string()))?)
    }

    /// Read an unsigned LEB128 integer, rejecting an overlong (non-minimal) or overflowing encoding
    /// (RFC 011 §2.2 rule 5): the standard LEB128 minimality check — the terminating byte is never
    /// `0x00` unless it is also the first byte (i.e. the value fits in one byte and is exactly that
    /// byte).
    pub fn uvarint(&mut self) -> Result<u64, Error> {
        let mut result: u64 = 0;
        let mut shift: u32 = 0;
        let mut nbytes: u32 = 0;
        let last_byte;
        loop {
            let byte = self.u8()?;
            nbytes += 1;
            if shift >= 64 {
                return Err(Error::Decode("varint too long".to_string()));
            }
            result |= u64::from(byte & 0x7f)
                .checked_shl(shift)
                .ok_or_else(|| Error::Decode("varint overflow".to_string()))?;
            if byte & 0x80 == 0 {
                last_byte = byte;
                break;
            }
            shift += 7;
        }
        if last_byte == 0 && nbytes > 1 {
            return Err(Error::NonCanonical("overlong varint".to_string()));
        }
        Ok(result)
    }

    /// Read a signed zig-zag + LEB128 integer.
    pub fn svarint(&mut self) -> Result<i64, Error> {
        Ok(unzigzag(self.uvarint()?))
    }

    /// Read exactly `n` raw bytes (no internal length prefix — the caller already knows `n`, from a
    /// field's own header length).
    pub fn raw(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if n > self.remaining() {
            return Err(Error::Decode("declared length exceeds input".to_string()));
        }
        self.take(n)
    }

    /// Read a fixed 32-byte id.
    pub fn raw32(&mut self) -> Result<[u8; 32], Error> {
        let slice = self.take(32)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(slice);
        Ok(out)
    }

    /// Read a `uvarint(count)` then call `read_item` that many times, collecting the results — the
    /// shared shape of every `list<T>` field.
    pub fn list<T>(
        &mut self,
        mut read_item: impl FnMut(&mut Self) -> Result<T, Error>,
    ) -> Result<Vec<T>, Error> {
        let n = self.uvarint()?;
        let n = usize::try_from(n).map_err(|_| Error::Decode("list too long".to_string()))?;
        let mut out = Vec::with_capacity(n.min(1 << 20));
        for _ in 0..n {
            out.push(read_item(self)?);
        }
        Ok(out)
    }

    /// Read a `map` value: `uvarint(count)` then that many `(key, value)` text pairs, rejecting
    /// non-ascending or duplicate keys (RFC 011 §2.1).
    pub fn map(&mut self) -> Result<std::collections::BTreeMap<String, String>, Error> {
        let n = self.uvarint()?;
        let mut out = std::collections::BTreeMap::new();
        let mut last_key: Option<String> = None;
        for _ in 0..n {
            let klen = self.uvarint_len()?;
            let key = self.text(klen)?;
            let vlen = self.uvarint_len()?;
            let value = self.text(vlen)?;
            if last_key.as_deref().is_some_and(|last| key.as_str() <= last) {
                return Err(Error::NonCanonical(
                    "map keys are not strictly ascending".to_string(),
                ));
            }
            last_key = Some(key.clone());
            out.insert(key, value);
        }
        Ok(out)
    }

    /// Read a `uvarint` length and check it against the remaining input, returning it as `usize`.
    pub fn uvarint_len(&mut self) -> Result<usize, Error> {
        let len = self.uvarint()?;
        let len =
            usize::try_from(len).map_err(|_| Error::Decode("length too large".to_string()))?;
        if len > self.remaining() {
            return Err(Error::Decode("declared length exceeds input".to_string()));
        }
        Ok(len)
    }

    /// Read exactly `len` bytes as UTF-8 text.
    pub fn text(&mut self, len: usize) -> Result<String, Error> {
        String::from_utf8(self.raw(len)?.to_vec())
            .map_err(|_| Error::Decode("invalid UTF-8".to_string()))
    }

    /// Read exactly `len` raw bytes and return them owned.
    pub fn bytes_exact(&mut self, len: usize) -> Result<Vec<u8>, Error> {
        Ok(self.raw(len)?.to_vec())
    }

    /// Read one record: `uvarint(n)` fields, in strictly ascending tag order (RFC 011 §2.2 rule 4).
    /// `handle(reader, id, len)` is called once per field and must return `Ok(true)` having consumed
    /// **exactly** `len` bytes if it recognized `id`, or `Ok(false)` having consumed nothing if it did
    /// not. This driver then enforces the exact-length invariant, and applies the critical/skip rule
    /// (RFC 011 §2.2 rule 9) to every field `handle` declined.
    ///
    /// # Errors
    /// [`Error::NonCanonical`] on out-of-order or duplicate tags, or a length mismatch;
    /// [`Error::UnknownCriticalField`] on an unrecognized field with `critical = 1`.
    pub fn record_fields(
        &mut self,
        record_name: &'static str,
        mut handle: impl FnMut(&mut Self, u64, usize) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        let n = self.uvarint()?;
        let mut last_id: Option<u64> = None;
        for _ in 0..n {
            let raw_tag = self.uvarint()?;
            let critical = raw_tag & 1 == 1;
            let id = raw_tag >> 1;
            if last_id.is_some_and(|last| id <= last) {
                return Err(Error::NonCanonical(format!(
                    "{record_name}: field tags are not strictly ascending"
                )));
            }
            last_id = Some(id);
            let len = self.uvarint_len()?;
            let start = self.pos;
            let end = start
                .checked_add(len)
                .ok_or_else(|| Error::Decode("field length overflow".to_string()))?;
            let recognized = handle(self, id, len)?;
            if recognized {
                if self.pos != end {
                    return Err(Error::NonCanonical(format!(
                        "{record_name}: field {id} value length mismatch"
                    )));
                }
            } else {
                if self.pos != start {
                    return Err(Error::NonCanonical(format!(
                        "{record_name}: field {id} was not consumed cleanly"
                    )));
                }
                if critical {
                    return Err(Error::UnknownCriticalField {
                        record: record_name,
                        tag: id,
                    });
                }
                self.pos = end;
                self.skipped_non_critical_fields += 1;
            }
        }
        Ok(())
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
