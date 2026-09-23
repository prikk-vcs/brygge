//! Read Mercurial's `.hg/store/obsstore` (obsolescence markers), format version 1, bounds-checked
//! (RFC 005 corrections handoff §1.2, following `obsolete.py`'s `_fm1readmarkers`). Only each marker's
//! **precursor node** is extracted — brygge does not interpret successor, parent, or metadata content,
//! but still walks past them byte-exactly to find the next record, checking every declared size against
//! the bytes actually present.
//!
//! Record layout (after a single leading version byte): `u32 size ‖ f64 date ‖ i16 tz ‖ u16 flags ‖
//! u8 numsuc ‖ u8 numpar ‖ u8 nummeta`, then the precursor node (20 bytes, or 32 when flag bit
//! `usingsha256` — bit value `2`, per `obsutil.py` (`bumpedfix = 1`, `usingsha256 = 2`) — is set), the
//! successor nodes, the parent nodes (omitted entirely when
//! `numpar == 3`, a sentinel meaning "not recorded", not a literal count), the metadata `(key_size,
//! value_size)` byte pairs, and finally the metadata bytes themselves. `size` is the record's own total
//! length, **including** the 4-byte size field, measured from the record's start — so `record_start +
//! size` is exactly where the next record begins. Verified against a real `obsstore` file produced by
//! `hg commit --amend` with core evolution enabled.

use std::collections::HashSet;
use std::path::Path;

use crate::Error;

/// The obsstore's own resource ceiling: checked against the file's size before it is read into memory,
/// so a hostile or corrupt obsstore cannot exhaust the host (RFC 010 D-4 style).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) max_obsstore_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_obsstore_bytes: 64 * 1024 * 1024,
        }
    }
}

const FORMAT_VERSION: u8 = 1;
/// `size(4) + date(8) + tz(2) + flags(2) + numsuc(1) + numpar(1) + nummeta(1)`.
const FIXED_HEADER_LEN: usize = 19;
/// `obsutil.py`: `bumpedfix = 1`, `usingsha256 = 2` (review 009 R-1 — this was `1 << 8` before).
const USING_SHA256: u16 = 2;
/// The sentinel `numpar` value meaning "parents were not recorded" — not literally three parents.
const NUMPAR_UNKNOWN: u8 = 3;

fn read_err(msg: impl std::fmt::Display) -> Error {
    Error::Read(format!("obsstore: {msg}"))
}

fn resource_limit(what: &str, ceiling: u64) -> Error {
    Error::ResourceLimit {
        what: what.to_string(),
        ceiling: format!("{ceiling} bytes"),
    }
}

fn u32_be(buf: &[u8], at: usize) -> Result<u32, Error> {
    let s = buf.get(at..at + 4).ok_or_else(|| read_err("truncated"))?;
    let a: [u8; 4] = s.try_into().map_err(|_| read_err("bad u32"))?;
    Ok(u32::from_be_bytes(a))
}

fn u16_be(buf: &[u8], at: usize) -> Result<u16, Error> {
    let s = buf.get(at..at + 2).ok_or_else(|| read_err("truncated"))?;
    let a: [u8; 2] = s.try_into().map_err(|_| read_err("bad u16"))?;
    Ok(u16::from_be_bytes(a))
}

fn byte_at(buf: &[u8], at: usize) -> Result<u8, Error> {
    buf.get(at).copied().ok_or_else(|| read_err("truncated"))
}

/// Read `store/obsstore` (absent means "no markers") and return the set of every marker's precursor
/// node. A 32-byte (SHA-256) precursor is read and skipped correctly but never inserted, since this
/// build's changelog nodes are always 20 bytes and could never match one.
///
/// # Errors
/// [`Error::UnsupportedFormat`] for any version other than 1; [`Error::Read`] on a malformed or
/// inconsistent record; [`Error::ResourceLimit`] if the file exceeds `limits.max_obsstore_bytes`.
pub(crate) fn precursor_nodes(store: &Path, limits: &Limits) -> Result<HashSet<[u8; 20]>, Error> {
    let path = store.join("obsstore");
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(e) => return Err(Error::Open(format!("cannot stat {}: {e}", path.display()))),
    };
    if meta.len() > limits.max_obsstore_bytes {
        return Err(resource_limit(
            "the obsstore file",
            limits.max_obsstore_bytes,
        ));
    }
    let data = std::fs::read(&path)
        .map_err(|e| Error::Open(format!("cannot read {}: {e}", path.display())))?;
    if data.is_empty() {
        return Ok(HashSet::new());
    }

    let version = *data.first().ok_or_else(|| read_err("empty file"))?;
    if version != FORMAT_VERSION {
        return Err(Error::UnsupportedFormat {
            requirement: format!("obsstore format {version}"),
            reason: "only obsstore format version 1 is implemented".to_string(),
        });
    }

    let len = data.len();
    let mut precursors = HashSet::new();
    let mut offset = 1usize;
    while offset < len {
        let record_start = offset;
        let size = usize::try_from(u32_be(&data, record_start)?)
            .map_err(|_| read_err("record size too large"))?;
        if size < FIXED_HEADER_LEN {
            return Err(read_err("record size smaller than its own fixed header"));
        }
        let record_end = record_start
            .checked_add(size)
            .ok_or_else(|| read_err("record size overflow"))?;
        if record_end > len {
            return Err(read_err("record extends past end of file"));
        }

        let flags = u16_be(&data, record_start + 14)?;
        let numsuc = byte_at(&data, record_start + 16)?;
        let numpar = byte_at(&data, record_start + 17)?;
        let nummeta = byte_at(&data, record_start + 18)?;
        let node_width: usize = if flags & USING_SHA256 != 0 { 32 } else { 20 };

        let mut pos = record_start + FIXED_HEADER_LEN;
        let precursor = data
            .get(pos..pos + node_width)
            .ok_or_else(|| read_err("truncated precursor node"))?;
        if node_width == 20 {
            let node: [u8; 20] = precursor
                .try_into()
                .map_err(|_| read_err("bad precursor node width"))?;
            precursors.insert(node);
        }
        pos += node_width;

        pos += usize::from(numsuc) * node_width; // successor nodes: skipped, not interpreted
        if numpar != NUMPAR_UNKNOWN {
            pos += usize::from(numpar) * node_width; // parent nodes: skipped, not interpreted
        }

        let meta_table_len = usize::from(nummeta) * 2;
        let meta_table = data
            .get(pos..pos + meta_table_len)
            .ok_or_else(|| read_err("truncated metadata size table"))?;
        let mut meta_bytes = 0usize;
        for pair in meta_table.chunks_exact(2) {
            let (Some(&k), Some(&v)) = (pair.first(), pair.get(1)) else {
                return Err(read_err("malformed metadata size table"));
            };
            meta_bytes += usize::from(k) + usize::from(v);
        }
        pos += meta_table_len;
        pos += meta_bytes;

        if pos != record_end {
            return Err(read_err("record contents do not match its declared size"));
        }
        offset = record_end;
    }

    Ok(precursors)
}

#[cfg(test)]
mod tests;
