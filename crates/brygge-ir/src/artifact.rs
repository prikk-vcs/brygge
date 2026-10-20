//! The single-file IR artifact: manifest + canonical metadata + content-addressed blob store
//! (RFC 003 D-3/D-6, container re-cut by RFC 011 §2.3).
//!
//! Layout: `magic (8)` · `format (1)` · `version (uvarint × 3)` · `digest (32)` · then the stored body:
//! `metadata: [len] Ir record` and the blob store (`count`, then `id(32) + [len]bytes` per blob, in
//! strictly ascending id order). The **digest** is SHA-256 over the stored bytes from the first version
//! byte to the end of the input, excluding the 32 digest bytes themselves — never a re-encoding, so
//! byte-for-byte tamper detection holds regardless of how the bytes were produced (`C-3b`, RFC 011 §9).
//! **Read order matters** (RFC 011 §2.3): magic, then format, then version, then the digest — all before
//! a single byte of metadata is parsed, so a pre-release or unsupported-contract artifact is refused
//! without ever attempting to read its (possibly incompatible) metadata shape. Reading is fully
//! bounds-checked and re-verifies the digest, every blob's content-address, and referential/canonical-
//! order integrity (RFC 011 §2.5) — a malformed or non-canonical artifact is a typed [`Error`], never a
//! panic.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Seek, SeekFrom, Write};

use sha2::{Digest, Sha256};

use crate::content::BlobId;
use crate::model::{AtomId, Ir, PathOp};
use crate::version::{self, ContractVersion};
use crate::{Error, Result};

const MAGIC: &[u8; 8] = b"BRYGGEIR";
const PRE_RELEASE_FORMAT: u8 = 1;
const FORMAT: u8 = 2;

fn io_err(e: std::io::Error) -> Error {
    Error::Io(e.to_string())
}

/// The result of decoding an artifact: the [`Ir`] plus how many fields from a newer contract minor were
/// skipped (RFC 011 §2.2 rule 9). The count is informational: a skipped field was non-critical, so it
/// cannot change what the artifact claims — `inspect` surfaces it as a note, never a warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// The decoded IR.
    pub ir: Ir,
    /// How many non-critical fields with an id this build does not recognize were skipped.
    pub skipped_non_critical_fields: u64,
}

/// Serialize `ir` to a self-contained artifact (contract [`version::CURRENT`]), streaming the blob
/// store straight from [`crate::content::ContentStore`]'s own bytes with no intermediate copy
/// (RFC 015 D-2). `out` must support [`Seek`]: the digest is written at its fixed offset only after
/// the body that it covers has already been written and hashed, then the cursor is restored to the
/// end, so the whole artifact — digest included — ends up in one streamed pass.
///
/// # Errors
/// [`Error::Io`] if `out` fails to write or seek.
pub fn write_to<W: Write + Seek>(ir: &Ir, out: &mut W) -> Result<()> {
    out.write_all(MAGIC).map_err(io_err)?;
    out.write_all(&[FORMAT]).map_err(io_err)?;

    let mut version = crate::canon::CanonWriter::new();
    version.uvarint(u64::from(version::CURRENT.major));
    version.uvarint(u64::from(version::CURRENT.minor));
    version.uvarint(u64::from(version::CURRENT.patch));
    out.write_all(version.as_bytes()).map_err(io_err)?;

    // The digest covers the stored bytes from the first version byte to the end — version, then
    // metadata, then blobs — never a re-encoding (RFC 011 §2.3/§9). It is written here as a
    // placeholder and overwritten once the hash below is complete.
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    let digest_offset = out.stream_position().map_err(io_err)?;
    out.write_all(&[0u8; 32]).map_err(io_err)?;

    let meta_bytes = ir.encode_metadata();
    let meta_len = crate::canon::uvarint_value(meta_bytes.len() as u64);
    hasher.update(&meta_len);
    out.write_all(&meta_len).map_err(io_err)?;
    hasher.update(&meta_bytes);
    out.write_all(&meta_bytes).map_err(io_err)?;

    let blob_count = crate::canon::uvarint_value(ir.content.len() as u64);
    hasher.update(&blob_count);
    out.write_all(&blob_count).map_err(io_err)?;
    for (id, bytes) in ir.content.iter_sorted() {
        hasher.update(id.as_bytes());
        out.write_all(id.as_bytes()).map_err(io_err)?;
        let blob_len = crate::canon::uvarint_value(bytes.len() as u64);
        hasher.update(&blob_len);
        out.write_all(&blob_len).map_err(io_err)?;
        hasher.update(bytes);
        out.write_all(bytes).map_err(io_err)?;
    }

    let digest: [u8; 32] = hasher.finalize().into();
    let end = out.stream_position().map_err(io_err)?;
    out.seek(SeekFrom::Start(digest_offset)).map_err(io_err)?;
    out.write_all(&digest).map_err(io_err)?;
    out.seek(SeekFrom::Start(end)).map_err(io_err)?;
    Ok(())
}

/// Serialize `ir` to a self-contained artifact (contract [`version::CURRENT`]). Built on [`write_to`]
/// so the two encodings can never diverge.
#[must_use]
pub fn to_bytes(ir: &Ir) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    // A `Cursor<Vec<u8>>` grows to fit every write and accepts any seek within what it has written —
    // there is no OS resource to fail, so `write_to` cannot return `Err` here.
    #[allow(clippy::expect_used)]
    write_to(ir, &mut cursor).expect("writing to an in-memory Vec cannot fail");
    cursor.into_inner()
}

/// Parse an artifact, verifying its version, integrity digest, blob content-addresses, and every
/// canonical-order/referential-integrity rule in RFC 011 §2.5.
///
/// # Errors
/// [`Error::Decode`]/[`Error::NonCanonical`] on malformed or non-canonical bytes,
/// [`Error::PreReleaseArtifact`] on a pre-0.2.0 artifact, [`Error::UnsupportedContract`] on a contract
/// this build does not read, [`Error::DigestMismatch`] on a tamper/truncation.
pub fn from_bytes(bytes: &[u8]) -> Result<Decoded> {
    let magic = bytes
        .get(0..8)
        .ok_or_else(|| Error::Decode("too short for header".to_string()))?;
    if magic != MAGIC {
        return Err(Error::Decode("bad magic".to_string()));
    }
    let format = *bytes
        .get(8)
        .ok_or_else(|| Error::Decode("missing format byte".to_string()))?;
    if format == PRE_RELEASE_FORMAT {
        // The metadata is never parsed for a pre-release artifact — the wire shape underneath changed
        // too much to read safely (RFC 011 §2.3).
        return Err(Error::PreReleaseArtifact);
    }
    if format != FORMAT {
        return Err(Error::Decode(format!("unknown artifact format {format}")));
    }

    let after_format = bytes
        .get(9..)
        .ok_or_else(|| Error::Decode("missing version".to_string()))?;
    let mut vr = crate::canon::CanonReader::new(after_format);
    let major = u32::try_from(vr.uvarint()?)
        .map_err(|_| Error::Decode("version major out of range".to_string()))?;
    let minor = u32::try_from(vr.uvarint()?)
        .map_err(|_| Error::Decode("version minor out of range".to_string()))?;
    let patch = u32::try_from(vr.uvarint()?)
        .map_err(|_| Error::Decode("version patch out of range".to_string()))?;
    let found = ContractVersion::new(major, minor, patch);
    version::ensure_readable(found)?;
    let version_len = after_format.len() - vr.remaining();

    let digest_start = 9 + version_len;
    let stored_digest: [u8; 32] = bytes
        .get(digest_start..digest_start + 32)
        .ok_or_else(|| Error::Decode("missing digest".to_string()))?
        .try_into()
        .map_err(|_| Error::Decode("bad digest length".to_string()))?;
    let rest_start = digest_start + 32;
    let rest = bytes
        .get(rest_start..)
        .ok_or_else(|| Error::Decode("missing body".to_string()))?;

    let version_bytes = bytes
        .get(9..digest_start)
        .ok_or_else(|| Error::Decode("missing version bytes".to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(version_bytes);
    hasher.update(rest);
    let computed: [u8; 32] = hasher.finalize().into();
    if computed != stored_digest {
        return Err(Error::DigestMismatch);
    }

    let mut r = crate::canon::CanonReader::new(rest);
    let meta_len = r.uvarint_len()?;
    let meta_bytes = r.raw(meta_len)?;
    let mut mr = crate::canon::CanonReader::new(meta_bytes);
    let mut ir = Ir::decode_metadata(&mut mr)?;
    if !mr.is_empty() {
        return Err(Error::NonCanonical(
            "trailing bytes in metadata".to_string(),
        ));
    }

    let nblobs = r.uvarint()?;
    let mut last_blob_id: Option<BlobId> = None;
    for _ in 0..nblobs {
        let id = BlobId(r.raw32()?);
        if last_blob_id.is_some_and(|last| id <= last) {
            return Err(Error::NonCanonical(
                "blob store ids are not strictly ascending".to_string(),
            ));
        }
        last_blob_id = Some(id);
        let len = r.uvarint_len()?;
        let data = r.bytes_exact(len)?;
        if BlobId::of(&data) != id {
            return Err(Error::Decode(
                "blob content does not match its id".to_string(),
            ));
        }
        ir.content.insert(data);
    }
    if !r.is_empty() {
        return Err(Error::NonCanonical(
            "trailing bytes after blob store".to_string(),
        ));
    }

    check_structure(&ir)?;

    Ok(Decoded {
        ir,
        skipped_non_critical_fields: mr.skipped_non_critical_fields()
            + r.skipped_non_critical_fields(),
    })
}

/// The structural checks RFC 011 §2.5 asks a reader to enforce beyond each record's own shape: unique
/// atom ids, canonical atom order, every parent/`from_atom` naming an earlier atom, every ref target
/// known, and blob referential integrity in both directions (no missing blob, no orphan).
fn check_structure(ir: &Ir) -> Result<()> {
    let ids: Vec<AtomId> = ir.atoms.iter().map(|a| a.id).collect();
    let mut position: BTreeMap<AtomId, usize> = BTreeMap::new();
    for (i, id) in ids.iter().enumerate() {
        if position.insert(*id, i).is_some() {
            return Err(Error::NonCanonical(format!("duplicate atom id {id}")));
        }
    }

    for (i, atom) in ir.atoms.iter().enumerate() {
        for parent in &atom.parents {
            match position.get(parent) {
                Some(&pi) if pi < i => {}
                _ => {
                    return Err(Error::NonCanonical(format!(
                        "atom {} names a parent that is not an earlier atom",
                        atom.id
                    )));
                }
            }
        }
        for copy in &atom.copies {
            match position.get(&copy.from_atom) {
                Some(&pi) if pi < i => {}
                _ => {
                    return Err(Error::NonCanonical(format!(
                        "atom {}'s copy of {:?} names a from_atom that is not an earlier atom",
                        atom.id, copy.from
                    )));
                }
            }
        }
    }

    let canonical = crate::builder::canonical_order_ids(&ir.atoms)
        .ok_or_else(|| Error::NonCanonical("atom graph contains a cycle".to_string()))?;
    if canonical != ids {
        return Err(Error::NonCanonical(
            "atoms are not in canonical topological order".to_string(),
        ));
    }

    for rf in &ir.refs {
        if !position.contains_key(&rf.target) {
            return Err(Error::NonCanonical(format!(
                "ref {:?} targets an unknown atom",
                rf.name
            )));
        }
    }

    let mut referenced: BTreeSet<BlobId> = BTreeSet::new();
    for atom in &ir.atoms {
        for op in &atom.ops {
            if let PathOp::Add { blob, .. }
            | PathOp::Modify { blob, .. }
            | PathOp::Replace { blob, .. } = op
            {
                if !ir.content.contains(blob) {
                    return Err(Error::Decode(
                        "an operation references a blob missing from the store".to_string(),
                    ));
                }
                referenced.insert(*blob);
            }
        }
    }
    for (id, _) in ir.content.iter_sorted() {
        if !referenced.contains(id) {
            return Err(Error::NonCanonical(format!(
                "blob {id} is stored but referenced by no operation (orphan)"
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mutation;
