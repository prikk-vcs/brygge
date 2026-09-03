//! The single-file IR artifact: manifest + canonical metadata + content-addressed blob store
//! (RFC 003 D-3/D-6).
//!
//! Layout: `magic (8)` · `format (1)` · `digest (32)` · then a canonical body of `[len]meta` and the
//! blob store (`count`, then `id(32)+[len]bytes` per blob, in sorted-id order). The **digest** is
//! SHA-256 over the *identity* metadata (import time excluded — `ID-4`) followed by each blob's id and
//! bytes in sorted order, so a re-run at a different time yields the same digest while a tamper of any
//! identity-bearing byte is detected (`C-3b`). Reading is fully bounds-checked and re-verifies the
//! digest, every blob's content-address, and referential integrity — a malformed artifact is an
//! [`Error`], never a panic.

use sha2::{Digest, Sha256};

use crate::Error;
use crate::canon::{CanonReader, CanonWriter};
use crate::content::BlobId;
use crate::model::{Ir, PathOp};
use crate::version;

const MAGIC: &[u8; 8] = b"BRYGGEIR";
const FORMAT: u8 = 1;

/// The integrity digest: SHA-256 over the identity metadata (no import time) + the sorted blob store.
fn digest(ir: &Ir) -> [u8; 32] {
    let mut mw = CanonWriter::new();
    ir.encode_metadata(&mut mw, false);
    let mut hasher = Sha256::new();
    hasher.update(mw.as_bytes());
    for (id, bytes) in ir.content.iter_sorted() {
        hasher.update(id.as_bytes());
        hasher.update(bytes);
    }
    hasher.finalize().into()
}

/// Serialize `ir` to a self-contained artifact.
#[must_use]
pub fn to_bytes(ir: &Ir) -> Vec<u8> {
    let dig = digest(ir);

    let mut meta = CanonWriter::new();
    ir.encode_metadata(&mut meta, true); // stored form keeps import time (provenance)

    let mut body = CanonWriter::new();
    body.bytes(meta.as_bytes());
    body.uvarint(ir.content.len() as u64);
    for (id, bytes) in ir.content.iter_sorted() {
        body.raw32(id.as_bytes());
        body.bytes(bytes);
    }

    let mut out = Vec::with_capacity(8 + 1 + 32 + body.as_bytes().len());
    out.extend_from_slice(MAGIC);
    out.push(FORMAT);
    out.extend_from_slice(&dig);
    out.extend_from_slice(body.as_bytes());
    out
}

/// Parse an artifact, verifying its version, integrity digest, blob content-addresses, and referential
/// integrity.
///
/// # Errors
/// [`Error::Decode`] on malformed bytes, [`Error::UnsupportedContractMajor`] on an unknown contract
/// major, [`Error::DigestMismatch`] on a tamper/truncation.
pub fn from_bytes(bytes: &[u8]) -> Result<Ir, Error> {
    let magic = bytes
        .get(0..8)
        .ok_or_else(|| Error::Decode("too short for header".to_string()))?;
    if magic != MAGIC {
        return Err(Error::Decode("bad magic".to_string()));
    }
    let format = *bytes
        .get(8)
        .ok_or_else(|| Error::Decode("missing format byte".to_string()))?;
    if format != FORMAT {
        return Err(Error::Decode(format!("unknown artifact format {format}")));
    }
    let stored_digest: [u8; 32] = bytes
        .get(9..41)
        .ok_or_else(|| Error::Decode("missing digest".to_string()))?
        .try_into()
        .map_err(|_| Error::Decode("bad digest length".to_string()))?;
    let body = bytes
        .get(41..)
        .ok_or_else(|| Error::Decode("missing body".to_string()))?;

    let mut r = CanonReader::new(body);
    let meta = r.bytes()?;
    let mut mr = CanonReader::new(&meta);
    let mut ir = Ir::decode_metadata(&mut mr)?;
    if !mr.is_empty() {
        return Err(Error::Decode("trailing bytes in metadata".to_string()));
    }
    version::ensure_readable(ir.contract_version)?;

    let nblobs = r.uvarint()?;
    for _ in 0..nblobs {
        let id = BlobId(r.raw32()?);
        let data = r.bytes()?;
        if BlobId::of(&data) != id {
            return Err(Error::Decode(
                "blob content does not match its id".to_string(),
            ));
        }
        ir.content.insert(data);
    }
    if !r.is_empty() {
        return Err(Error::Decode("trailing bytes after blob store".to_string()));
    }

    // Referential integrity: every op's blob must be present.
    for atom in &ir.atoms {
        for op in &atom.ops {
            if let PathOp::Add { blob, .. } | PathOp::Modify { blob, .. } = op {
                if !ir.content.contains(blob) {
                    return Err(Error::Decode(
                        "an operation references a blob missing from the store".to_string(),
                    ));
                }
            }
        }
    }

    if digest(&ir) != stored_digest {
        return Err(Error::DigestMismatch);
    }
    Ok(ir)
}

#[cfg(test)]
mod tests;
