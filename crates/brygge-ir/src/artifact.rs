//! The single-file IR artifact: manifest + canonical metadata + content-addressed blob store
//! (RFC 003 D-3/D-6, container re-cut by RFC 011 §2.3, compacted by RFC 018 D-1).
//!
//! Layout: `magic (8)` · `format (1)` · `version (uvarint × 3)` · `digest (32)` · then, for the contract
//! this build writes (0.3.0): metadata frames, the blob table (`count`, then `id(32) + raw_len` per
//! blob, in [`store_order`]), and the blob frames — see [`frame`] for the frame codec and the packing
//! rules. A frame is a run of raw bytes, stored either as they are or DEFLATE-compressed; **the digest
//! is SHA-256 of the *logical* (decompressed) stream** — `version bytes ‖ uvarint(meta_len) ‖ metadata
//! record ‖ uvarint(count) ‖ (id32 ‖ uvarint(raw_len))* ‖ every blob's bytes, in store order` — never a
//! frame header or a compressed byte, so the same content always digests the same way regardless of how
//! it was compressed (RFC 018 D-3). A 0.2.0 artifact (still read, never written — RFC 018 D-7, OQ-2) uses
//! the prior, unframed layout instead, where the digest is SHA-256 of the stored bytes directly; see
//! [`Decoded::digest`] for both definitions stated together.
//!
//! **Read order matters** (RFC 011 §2.3): magic, then format, then version, then the digest — all before
//! a single byte of metadata is parsed, so a pre-release or unsupported-contract artifact is refused
//! without ever attempting to read its (possibly incompatible) metadata shape. Reading is fully
//! bounds-checked and re-verifies the digest, every blob's content-address, and referential/canonical-
//! order integrity (RFC 011 §2.5) — a malformed or non-canonical artifact is a typed [`Error`], never a
//! panic.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};

use sha2::{Digest, Sha256};

use crate::canon::{self, CanonReader, CanonWriter};
use crate::content::{BlobId, ContentStore};
use crate::model::{AtomId, Ir, PathOp};
use crate::version::{self, ContractVersion};
use crate::{Error, Result};

pub(crate) mod frame;
mod sequential;

use frame::FrameLoc;

const MAGIC: &[u8; 8] = b"BRYGGEIR";
const PRE_RELEASE_FORMAT: u8 = 1;
const FORMAT: u8 = 2;

/// The result of decoding an artifact: the [`Ir`] plus how many fields from a newer contract minor were
/// skipped (RFC 011 §2.2 rule 9). The count is informational: a skipped field was non-critical, so it
/// cannot change what the artifact claims — `inspect` surfaces it as a note, never a warning.
#[derive(Debug, PartialEq, Eq)]
pub struct Decoded {
    /// The decoded IR.
    pub ir: Ir,
    /// How many non-critical fields with an id this build does not recognize were skipped.
    pub skipped_non_critical_fields: u64,
    /// The artifact's own contract version, as declared in its header (RFC 018 D-7): `0.2.0` or `0.3.0`
    /// for anything this build reads today.
    pub contract: ContractVersion,
    /// The artifact's own integrity digest, as checked by the reader (RFC 017 D-2). Not re-read from a
    /// fixed file offset — that would bypass the reader's own verification — but the exact digest the
    /// reader already computed and compared against the stored one before returning.
    ///
    /// **Two definitions, by contract** (RFC 018 D-3): for a 0.3.0 artifact, the SHA-256 of the
    /// *logical* (decompressed) stream — see the module doc. For a 0.2.0 artifact, the prior
    /// definition: the SHA-256 of the stored bytes directly, from the first version byte to the end of
    /// the input. Comparing a 0.2.0 and a 0.3.0 digest of the same content is meaningless; compare
    /// [`Ir`]s instead (`verify --against-source` does).
    pub digest: [u8; 32],
}

/// Serialize `ir` to a self-contained artifact, contract [`version::CURRENT`] (0.3.0 — RFC 018 D-1),
/// streaming the blob store straight from [`crate::content::ContentStore`]'s own bytes with no
/// intermediate copy (RFC 015 D-2). `out` must support [`Seek`]: the digest is written at its fixed
/// offset only after the body that it covers has already been written and hashed, then the cursor is
/// restored to the end, so the whole artifact — digest included — ends up in one streamed pass.
///
/// # Errors
/// [`Error::Io`] if `out` fails to write or seek.
pub fn write_to<W: Write + Seek>(ir: &Ir, out: &mut W) -> Result<()> {
    write_to_inner(ir, out, false)
}

/// As [`write_to`], but every frame is `Codec::Stored` regardless of `compress_frame`'s own choice
/// (RFC 018 §6.2's digest-independence proof: the same `ir` written two ways gives two different files,
/// one digest, equal `Ir`s). Crate-private, test-only; adds no public API.
#[cfg(test)]
pub(crate) fn write_to_stored_only<W: Write + Seek>(ir: &Ir, out: &mut W) -> Result<()> {
    write_to_inner(ir, out, true)
}

fn write_to_inner<W: Write + Seek>(ir: &Ir, out: &mut W, force_stored: bool) -> Result<()> {
    out.write_all(MAGIC).map_err(Error::io)?;
    out.write_all(&[FORMAT]).map_err(Error::io)?;

    let mut version = CanonWriter::new();
    version.uvarint(u64::from(version::CURRENT.major));
    version.uvarint(u64::from(version::CURRENT.minor));
    version.uvarint(u64::from(version::CURRENT.patch));
    out.write_all(version.as_bytes()).map_err(Error::io)?;

    // The digest covers the *logical* stream (RFC 018 D-3): version, then the metadata's raw bytes,
    // then the blob table, then every blob's raw bytes in store order — never a frame header or a
    // compressed byte, so it never depends on how the content was compressed.
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    let digest_offset = out.stream_position().map_err(Error::io)?;
    out.write_all(&[0u8; 32]).map_err(Error::io)?;

    let meta_bytes = ir.encode_metadata();
    let meta_len = meta_bytes.len() as u64;
    hasher.update(canon::uvarint_value(meta_len));
    hasher.update(&meta_bytes);
    write_frames(
        out,
        &meta_bytes,
        &frame::fixed_chunk_lens(meta_len),
        force_stored,
    )?;

    let order = store_order(ir);
    if order.len() != ir.content.len() {
        // store_order only names a blob some op actually references (RFC 018 D-2); a blob present in
        // `ir.content` but named by none is an orphan — the same bug `check_structure`'s read-side
        // check exists to catch, caught here instead, before it is ever written (RFC 011 §2.5).
        return Err(Error::Invariant(
            match ir.content.ids().find(|id| !order.contains(id)) {
                Some(orphan) => {
                    format!("blob {orphan} is stored but referenced by no operation (orphan)")
                }
                None => "a blob is stored but referenced by no operation (orphan)".to_string(),
            },
        ));
    }
    let count_bytes = canon::uvarint_value(order.len() as u64);
    hasher.update(&count_bytes);
    out.write_all(&count_bytes).map_err(Error::io)?;
    let mut lens = Vec::with_capacity(order.len());
    for id in &order {
        let len = ir
            .content
            .blob_len(id)
            .ok_or_else(|| Error::Invariant(format!("blob {id} indexed but not readable")))?;
        hasher.update(id.as_bytes());
        out.write_all(id.as_bytes()).map_err(Error::io)?;
        let len_bytes = canon::uvarint_value(len);
        hasher.update(&len_bytes);
        out.write_all(&len_bytes).map_err(Error::io)?;
        lens.push(len);
    }

    let frame_lens = frame::canonical_frame_raw_lens(lens.iter().copied());
    out.write_all(&canon::uvarint_value(frame_lens.len() as u64))
        .map_err(Error::io)?;
    let mut blob_ids = order.iter();
    for &flen in &frame_lens {
        // One frame's raw bytes in memory at a time (RFC 015 D-3, RFC 018 §4): at most 1 MiB of packed
        // small blobs, or one large blob of its own.
        let mut buf = Vec::new();
        let mut collected = 0u64;
        while collected < flen {
            let id = blob_ids.next().ok_or_else(|| {
                Error::Invariant("the frame packing plan ran out of blobs".to_string())
            })?;
            let bytes = ir
                .content
                .get(id)?
                .ok_or_else(|| Error::Invariant(format!("blob {id} indexed but not readable")))?;
            hasher.update(&bytes);
            collected += bytes.len() as u64;
            buf.extend_from_slice(&bytes);
        }
        write_one_frame(out, &buf, force_stored)?;
    }

    let digest: [u8; 32] = hasher.finalize().into();
    let end = out.stream_position().map_err(Error::io)?;
    out.seek(SeekFrom::Start(digest_offset))
        .map_err(Error::io)?;
    out.write_all(&digest).map_err(Error::io)?;
    out.seek(SeekFrom::Start(end)).map_err(Error::io)?;
    Ok(())
}

/// Write `bytes`, cut into `chunk_lens` (each a frame), as `uvarint(nframes) ‖ frame*`.
fn write_frames<W: Write>(
    out: &mut W,
    bytes: &[u8],
    chunk_lens: &[u64],
    force_stored: bool,
) -> Result<()> {
    out.write_all(&canon::uvarint_value(chunk_lens.len() as u64))
        .map_err(Error::io)?;
    let mut offset: usize = 0;
    for &len in chunk_lens {
        let len = usize::try_from(len).map_err(|e| Error::io_msg(e.to_string()))?;
        let chunk = bytes.get(offset..offset + len).ok_or_else(|| {
            Error::Invariant("chunk plan ran past the metadata bytes".to_string())
        })?;
        write_one_frame(out, chunk, force_stored)?;
        offset += len;
    }
    Ok(())
}

/// Write one frame: `uvarint(codec) ‖ uvarint(raw_len) ‖ uvarint(stored_len) ‖ stored bytes`.
fn write_one_frame<W: Write>(out: &mut W, raw: &[u8], force_stored: bool) -> Result<()> {
    let (codec, stored) = if force_stored {
        (frame::Codec::Stored, raw.to_vec())
    } else {
        frame::compress_frame(raw)
    };
    out.write_all(&canon::uvarint_value(codec.to_u64()))
        .map_err(Error::io)?;
    out.write_all(&canon::uvarint_value(raw.len() as u64))
        .map_err(Error::io)?;
    out.write_all(&canon::uvarint_value(stored.len() as u64))
        .map_err(Error::io)?;
    out.write_all(&stored).map_err(Error::io)?;
    Ok(())
}

/// Serialize `ir` to a self-contained artifact (contract [`version::CURRENT`]). Built on [`write_to`]
/// so the two encodings can never diverge.
///
/// # Panics
/// If `ir.content` is a file-backed [`crate::content::ContentStore`] (RFC 015 D-3) whose spill file
/// cannot be read — the store's own contract is that every indexed id reads back, so this signals a
/// bug in whatever built `ir`, not a normal failure. Every `ir.content` this crate or its decoders
/// build today is memory-backed, where reading never fails; `write_to` directly, which returns
/// [`Error::Io`] instead of panicking, is the better choice for an `ir` that might be file-backed.
#[must_use]
pub fn to_bytes(ir: &Ir) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    // A `Cursor<Vec<u8>>`'s own `Write`/`Seek` cannot fail — it grows to fit every write and accepts
    // any seek within what it has written. The one other way `write_to` can return `Err` is a failed
    // blob read, which is a contract violation in `ir` itself, not an I/O condition this function's
    // own in-memory destination could cause — see "Panics" above.
    #[allow(clippy::expect_used)]
    write_to(ir, &mut cursor).expect("ir.content's own contract: every indexed blob id reads back");
    cursor.into_inner()
}

/// The order a compact artifact stores its blobs in (RFC 018 D-2): each blob's position is set by its
/// **first use** — the earliest `Add`/`Modify`/`Replace` naming it, found by walking [`Ir::atoms`] in
/// their own canonical order and, within an atom, [`crate::model::ChangeAtom::ops`] in their own
/// canonical path order — and blobs are then ordered by that first use's **path** (as bytes), breaking
/// ties by the atom's index. Grouping by path, rather than by history, is what lets the writer's DEFLATE
/// frames compress a file's successive revisions together; it is also the order a bulk reader (an
/// encoder; [`crate::content::ContentStore::get`] in sequence) reads each compressed frame exactly once,
/// since a frame never holds blobs out of this order.
#[must_use]
pub fn store_order(ir: &Ir) -> Vec<BlobId> {
    let mut seen: BTreeSet<BlobId> = BTreeSet::new();
    let mut first_use: Vec<(Vec<u8>, usize, BlobId)> = Vec::new();
    for (atom_index, atom) in ir.atoms.iter().enumerate() {
        for op in &atom.ops {
            let (path, blob) = match op {
                PathOp::Add { path, blob, .. }
                | PathOp::Modify { path, blob, .. }
                | PathOp::Replace { path, blob, .. } => (path, *blob),
                PathOp::Delete { .. } => continue,
            };
            if seen.insert(blob) {
                first_use.push((path.clone().into_bytes(), atom_index, blob));
            }
        }
    }
    first_use.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    first_use.into_iter().map(|(_, _, id)| id).collect()
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
    let mut vr = CanonReader::new(after_format);
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

    if found.minor == 2 {
        from_bytes_v2(version_bytes, stored_digest, rest, found)
    } else {
        from_bytes_v3(version_bytes, stored_digest, rest, found)
    }
}

/// [`from_bytes`] over a **contract 0.2.0** artifact: the prior, unframed layout (RFC 011 §2.3), kept
/// exactly as it has always read — the digest is the SHA-256 of the stored bytes directly, and blob
/// entries interleave `id(32) ‖ len ‖ bytes`, strictly ascending by id.
fn from_bytes_v2(
    version_bytes: &[u8],
    stored_digest: [u8; 32],
    rest: &[u8],
    found: ContractVersion,
) -> Result<Decoded> {
    let mut hasher = Sha256::new();
    hasher.update(version_bytes);
    hasher.update(rest);
    let computed: [u8; 32] = hasher.finalize().into();
    if computed != stored_digest {
        return Err(Error::DigestMismatch);
    }

    let mut r = CanonReader::new(rest);
    let meta_len = r.uvarint_len()?;
    let meta_bytes = r.raw(meta_len)?;
    let mut mr = CanonReader::new(meta_bytes);
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
        // Always the in-memory backing (`Ir::default()`'s `ContentStore::new()`): reading an artifact
        // back never fails to insert.
        ir.content.insert(data)?;
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
        contract: found,
        digest: stored_digest,
    })
}

/// [`from_bytes`] over a **contract 0.3.0** artifact (RFC 018 D-1): reconstructs the logical stream by
/// inflating every frame (a failure here is immediate — "a frame that does not inflate", the format
/// page's one exception to the digest-first rule, since there is no logical stream left to hash), then
/// hashes that reconstruction exactly as [`write_to`] does and compares it with `stored_digest` **before
/// trusting any of its structure** — matching [`from_bytes_v2`]'s own precedence. Only once the digest
/// matches are the packing rules, the blob table's order, and each blob's content address checked.
fn from_bytes_v3(
    version_bytes: &[u8],
    stored_digest: [u8; 32],
    rest: &[u8],
    found: ContractVersion,
) -> Result<Decoded> {
    let mut r = CanonReader::new(rest);

    let nframes_meta = r.uvarint()?;
    let mut meta_bytes = Vec::new();
    let mut observed_meta_lens =
        Vec::with_capacity(usize::try_from(nframes_meta).unwrap_or(0).min(1 << 20));
    for _ in 0..nframes_meta {
        let (raw, raw_len) = read_and_inflate_frame(&mut r)?;
        observed_meta_lens.push(raw_len);
        meta_bytes.extend_from_slice(&raw);
    }
    let meta_len = meta_bytes.len() as u64;

    let nblobs = r.uvarint()?;
    let mut table_ids = Vec::with_capacity(usize::try_from(nblobs).unwrap_or(0).min(1 << 20));
    let mut table_lens = Vec::with_capacity(usize::try_from(nblobs).unwrap_or(0).min(1 << 20));
    for _ in 0..nblobs {
        let id = BlobId(r.raw32()?);
        let len = r.uvarint()?;
        table_ids.push(id);
        table_lens.push(len);
    }

    let nframes_blob = r.uvarint()?;
    let mut blob_bytes = Vec::new();
    let mut observed_blob_lens =
        Vec::with_capacity(usize::try_from(nframes_blob).unwrap_or(0).min(1 << 20));
    for _ in 0..nframes_blob {
        let (raw, raw_len) = read_and_inflate_frame(&mut r)?;
        observed_blob_lens.push(raw_len);
        blob_bytes.extend_from_slice(&raw);
    }
    let trailing = !r.is_empty();

    // The digest, over exactly the logical stream `write_to` hashed: version, then meta_len + metadata,
    // then the blob table, then every blob's raw bytes in table order. Computed before any of the
    // structural checks below are trusted (today's digest-first precedence).
    let mut hasher = Sha256::new();
    hasher.update(version_bytes);
    hasher.update(canon::uvarint_value(meta_len));
    hasher.update(&meta_bytes);
    hasher.update(canon::uvarint_value(nblobs));
    for (id, len) in table_ids.iter().zip(table_lens.iter()) {
        hasher.update(id.as_bytes());
        hasher.update(canon::uvarint_value(*len));
    }
    hasher.update(&blob_bytes);
    let computed: [u8; 32] = hasher.finalize().into();
    if computed != stored_digest {
        return Err(Error::DigestMismatch);
    }

    if trailing {
        return Err(Error::NonCanonical(
            "trailing bytes after blob store".to_string(),
        ));
    }
    if observed_meta_lens != frame::fixed_chunk_lens(meta_len) {
        return Err(Error::NonCanonical(
            "metadata frames do not match the fixed 1 MiB chunking rule".to_string(),
        ));
    }
    if observed_blob_lens != frame::canonical_frame_raw_lens(table_lens.iter().copied()) {
        return Err(Error::NonCanonical(
            "blob frames do not match the packing rule".to_string(),
        ));
    }

    let mut mr = CanonReader::new(&meta_bytes);
    let mut ir = Ir::decode_metadata(&mut mr)?;
    if !mr.is_empty() {
        return Err(Error::NonCanonical(
            "trailing bytes in metadata".to_string(),
        ));
    }

    if table_ids != store_order(&ir) {
        return Err(Error::NonCanonical(
            "blob table is not in store order".to_string(),
        ));
    }

    let mut offset: usize = 0;
    for (id, len) in table_ids.iter().zip(table_lens.iter()) {
        let len = usize::try_from(*len)
            .map_err(|_| Error::Decode("blob length too large".to_string()))?;
        let slice = blob_bytes.get(offset..offset + len).ok_or_else(|| {
            Error::Decode("blob content shorter than the table declares".to_string())
        })?;
        if BlobId::of(slice) != *id {
            return Err(Error::Decode(
                "blob content does not match its id".to_string(),
            ));
        }
        ir.content.insert(slice.to_vec())?;
        offset += len;
    }

    check_structure(&ir)?;

    Ok(Decoded {
        ir,
        skipped_non_critical_fields: mr.skipped_non_critical_fields(),
        contract: found,
        digest: stored_digest,
    })
}

/// Read one frame's header and stored bytes from `r`, inflate it (bounded, [`frame::inflate_exact`]),
/// and return its raw bytes alongside the declared `raw_len` (kept separately so a caller can check the
/// observed frame-length sequence against the packing rules without re-deriving it from `raw.len()`).
fn read_and_inflate_frame(r: &mut CanonReader<'_>) -> Result<(Vec<u8>, u64)> {
    let codec = frame::Codec::from_u64(r.uvarint()?)?;
    let raw_len = r.uvarint()?;
    let stored_len = r.uvarint_len()?;
    let stored = r.raw(stored_len)?;
    let raw = frame::inflate_exact(codec, stored, raw_len)?;
    Ok((raw, raw_len))
}

/// Parse an artifact by reading `r` forward exactly once (RFC 015 D-4): every byte past the stored
/// digest is read (and, for a 0.3.0 artifact, inflated) as the pass advances, and a blob's bytes are
/// never kept beyond the one frame that is verifying them. The resulting [`Ir`]'s content store
/// re-reads a blob from `r` — re-hashing it against its [`BlobId`] — only if something later actually
/// asks for it ([`crate::content`]'s `Backing::Artifact`/`Backing::ArtifactCompact`); nothing is copied
/// up front. `r` must support [`Seek`]: once, up front, to learn its total length (the bound every
/// declared length is checked against), and again later, by the content store, for each on-demand read
/// — this is also why `r` must outlive the call (`'static`): it is kept, not discarded, once the pass
/// succeeds.
///
/// **The digest always wins**, except when a frame does not inflate at all (RFC 018: there is then no
/// logical stream left to hash, so that failure is reported on its own — see [`from_bytes_v3`]'s doc).
/// Otherwise, if the body is also malformed, the error reported is [`Error::DigestMismatch`] whenever
/// the digest itself does not match — the same precedence [`from_bytes`] already has.
///
/// # Errors
/// As [`from_bytes`]; [`Error::ArtifactChanged`] is never returned by this call itself (it can only
/// come from a later [`crate::content::ContentStore::get`] on the `Ir` this returns).
pub fn read_from<R: Read + Seek + std::fmt::Debug + 'static>(mut r: R) -> Result<Decoded> {
    let total = r.seek(SeekFrom::End(0)).map_err(Error::io)?;
    r.seek(SeekFrom::Start(0)).map_err(Error::io)?;
    let mut sr = sequential::SeqReader::new(r, total);

    let magic = sr.raw(8, "header")?;
    if magic != MAGIC {
        return Err(Error::Decode("bad magic".to_string()));
    }
    let format = sr.raw_u8("format byte")?;
    if format == PRE_RELEASE_FORMAT {
        return Err(Error::PreReleaseArtifact);
    }
    if format != FORMAT {
        return Err(Error::Decode(format!("unknown artifact format {format}")));
    }

    // The version gate runs before the digest is even read (as today): an unsupported contract is
    // refused on its own terms, never folded into the digest/structure precedence below. Read
    // unhashed here (via the plain, not `_hashed`, uvarint): both v2 and v3 hash the version bytes
    // themselves, reconstructed canonically, below.
    let major = u32::try_from(sr.uvarint_raw()?)
        .map_err(|_| Error::Decode("version major out of range".to_string()))?;
    let minor = u32::try_from(sr.uvarint_raw()?)
        .map_err(|_| Error::Decode("version minor out of range".to_string()))?;
    let patch = u32::try_from(sr.uvarint_raw()?)
        .map_err(|_| Error::Decode("version patch out of range".to_string()))?;
    let found = ContractVersion::new(major, minor, patch);
    version::ensure_readable(found)?;

    let stored_digest: [u8; 32] = sr
        .raw(32, "digest")?
        .try_into()
        .map_err(|_| Error::Decode("bad digest length".to_string()))?;

    let mut version_writer = CanonWriter::new();
    version_writer.uvarint(u64::from(major));
    version_writer.uvarint(u64::from(minor));
    version_writer.uvarint(u64::from(patch));
    let version_bytes = version_writer.into_bytes();

    if minor == 2 {
        read_from_v2(sr, &version_bytes, stored_digest, found)
    } else {
        read_from_v3(sr, &version_bytes, stored_digest, found)
    }
}

/// [`read_from`] over a **contract 0.2.0** artifact: unchanged from before RFC 018, just relocated —
/// streams forward, hashing as it reads (`sr`'s own hashed methods), and builds the legacy
/// `(offset, length)` artifact backing.
fn read_from_v2<R: Read + Seek + std::fmt::Debug + 'static>(
    mut sr: sequential::SeqReader<R>,
    version_bytes: &[u8],
    stored_digest: [u8; 32],
    found: ContractVersion,
) -> Result<Decoded> {
    sr.hash_raw(version_bytes);
    match read_body_v2(&mut sr) {
        Ok(mut body) if sr.at_end() => {
            if sr.digest() != stored_digest {
                return Err(Error::DigestMismatch);
            }
            body.ir.content = ContentStore::from_artifact_source(sr.into_reader(), body.index);
            check_structure(&body.ir)?;
            Ok(Decoded {
                ir: body.ir,
                skipped_non_critical_fields: body.skipped,
                contract: found,
                digest: stored_digest,
            })
        }
        Ok(_) => tiebreak(
            &mut sr,
            stored_digest,
            Error::NonCanonical("trailing bytes after blob store".to_string()),
        ),
        Err(e @ Error::Io(_)) => Err(e),
        Err(e) => tiebreak(&mut sr, stored_digest, e),
    }
}

/// Finish the pass after a structural problem (an outright error, or clean parse with bytes left over):
/// drain whatever is left, unread, to the true end of the source, so the digest it was checked against
/// covers everything the writer covered either way, then prefer reporting a digest mismatch — matching
/// [`from_bytes_v2`]'s own precedence — and fall back to `structural_err` only when the digest actually
/// matches despite it.
fn tiebreak<R: Read>(
    sr: &mut sequential::SeqReader<R>,
    stored_digest: [u8; 32],
    structural_err: Error,
) -> Result<Decoded> {
    sr.drain_to_end_hashed()?;
    if sr.digest() != stored_digest {
        Err(Error::DigestMismatch)
    } else {
        Err(structural_err)
    }
}

/// [`read_body_v2`]'s result: the parsed metadata, the blob directory (`BlobId` → its `(offset, length)`
/// in the source), and the skipped-non-critical-field count — named fields instead of a tuple so the
/// signature below stays simple enough for clippy's `type_complexity` to pass over.
struct BodyV2 {
    ir: Ir,
    index: BTreeMap<BlobId, (u64, u64)>,
    skipped: u64,
}

/// The metadata and blob-section body, shared by [`read_from_v2`]'s success and tiebreak paths: parses
/// exactly like [`from_bytes_v2`]'s own body loop, but streamed — a blob's bytes pass through
/// [`sequential::SeqReader::blob_body_hash`] and are never collected.
fn read_body_v2<R: Read>(sr: &mut sequential::SeqReader<R>) -> Result<BodyV2> {
    let meta_len = usize::try_from(sr.uvarint_len_hashed()?)
        .map_err(|_| Error::Decode("length too large".to_string()))?;
    let meta_bytes = sr.hashed(meta_len, "metadata")?;
    let mut mr = CanonReader::new(&meta_bytes);
    let ir = Ir::decode_metadata(&mut mr)?;
    if !mr.is_empty() {
        return Err(Error::NonCanonical(
            "trailing bytes in metadata".to_string(),
        ));
    }
    let skipped = mr.skipped_non_critical_fields();

    let nblobs = sr.uvarint_hashed()?;
    let mut last_blob_id: Option<BlobId> = None;
    let mut index = BTreeMap::new();
    for _ in 0..nblobs {
        let raw_id: [u8; 32] = sr
            .hashed(32, "blob id")?
            .try_into()
            .map_err(|_| Error::Decode("bad blob id length".to_string()))?;
        let id = BlobId(raw_id);
        if last_blob_id.is_some_and(|last| id <= last) {
            return Err(Error::NonCanonical(
                "blob store ids are not strictly ascending".to_string(),
            ));
        }
        last_blob_id = Some(id);
        let len = sr.uvarint_len_hashed()?;
        let offset = sr.position();
        let actual = sr.blob_body_hash(len)?;
        if actual != id.0 {
            return Err(Error::Decode(
                "blob content does not match its id".to_string(),
            ));
        }
        index.insert(id, (offset, len));
    }
    Ok(BodyV2 { ir, index, skipped })
}

/// [`read_from`] over a **contract 0.3.0** artifact (RFC 018 D-1): streams forward exactly once,
/// inflating each frame transiently to feed the logical-stream digest (a metadata frame's bytes are
/// kept — metadata has always been held whole — but a blob frame's decompressed bytes are hashed and
/// then dropped; only its `FrameLoc` and each blob's `(frame, offset, length)` are kept). So the pass's
/// own memory is bounded by one frame at a time, not the artifact's total content — RFC 015 D-4's bound,
/// carried over compression (one open question this batch leaves for review: a single very large blob's
/// own frame is still inflated whole by [`frame::inflate_exact`], rather than in chunks through the
/// hash, since `miniz_oxide`'s safe chunked/windowed inflate was judged too large a hand-rolled surface
/// to add correctly in this batch — see the review request).
fn read_from_v3<R: Read + Seek + std::fmt::Debug + 'static>(
    mut sr: sequential::SeqReader<R>,
    version_bytes: &[u8],
    stored_digest: [u8; 32],
    found: ContractVersion,
) -> Result<Decoded> {
    let mut hasher = Sha256::new();
    hasher.update(version_bytes);

    let nframes_meta = sr.uvarint_raw()?;
    let mut meta_bytes = Vec::new();
    let mut observed_meta_lens =
        Vec::with_capacity(usize::try_from(nframes_meta).unwrap_or(0).min(1 << 20));
    for _ in 0..nframes_meta {
        let (raw, raw_len) = read_and_inflate_frame_streaming(&mut sr)?;
        observed_meta_lens.push(raw_len);
        meta_bytes.extend_from_slice(&raw);
    }
    let meta_len = meta_bytes.len() as u64;
    hasher.update(canon::uvarint_value(meta_len));
    hasher.update(&meta_bytes);

    let nblobs = sr.uvarint_raw()?;
    hasher.update(canon::uvarint_value(nblobs));
    let mut table_ids = Vec::with_capacity(usize::try_from(nblobs).unwrap_or(0).min(1 << 20));
    let mut table_lens = Vec::with_capacity(usize::try_from(nblobs).unwrap_or(0).min(1 << 20));
    for _ in 0..nblobs {
        let id_bytes: [u8; 32] = sr
            .raw(32, "blob id")?
            .try_into()
            .map_err(|_| Error::Decode("bad blob id length".to_string()))?;
        hasher.update(id_bytes);
        let len = sr.uvarint_raw()?;
        hasher.update(canon::uvarint_value(len));
        table_ids.push(BlobId(id_bytes));
        table_lens.push(len);
    }

    let nframes_blob = sr.uvarint_raw()?;
    let mut frames: Vec<FrameLoc> =
        Vec::with_capacity(usize::try_from(nframes_blob).unwrap_or(0).min(1 << 20));
    let mut observed_blob_lens =
        Vec::with_capacity(usize::try_from(nframes_blob).unwrap_or(0).min(1 << 20));
    for _ in 0..nframes_blob {
        let codec = frame::Codec::from_u64(sr.uvarint_raw()?)?;
        let raw_len = sr.uvarint_raw()?;
        let stored_len = sr.uvarint_len_raw()?;
        let stored_offset = sr.position();
        let stored = sr.raw(
            usize::try_from(stored_len)
                .map_err(|_| Error::Decode("frame too large".to_string()))?,
            "blob frame",
        )?;
        let raw = frame::inflate_exact(codec, &stored, raw_len)?;
        hasher.update(&raw);
        observed_blob_lens.push(raw_len);
        frames.push(FrameLoc {
            stored_offset,
            codec,
            raw_len,
            stored_len,
        });
    }

    let trailing = !sr.at_end();
    let computed: [u8; 32] = hasher.finalize().into();
    if computed != stored_digest {
        return Err(Error::DigestMismatch);
    }
    if trailing {
        return Err(Error::NonCanonical(
            "trailing bytes after blob store".to_string(),
        ));
    }
    if observed_meta_lens != frame::fixed_chunk_lens(meta_len) {
        return Err(Error::NonCanonical(
            "metadata frames do not match the fixed 1 MiB chunking rule".to_string(),
        ));
    }
    if observed_blob_lens != frame::canonical_frame_raw_lens(table_lens.iter().copied()) {
        return Err(Error::NonCanonical(
            "blob frames do not match the packing rule".to_string(),
        ));
    }

    let mut mr = CanonReader::new(&meta_bytes);
    let mut ir = Ir::decode_metadata(&mut mr)?;
    if !mr.is_empty() {
        return Err(Error::NonCanonical(
            "trailing bytes in metadata".to_string(),
        ));
    }
    if table_ids != store_order(&ir) {
        return Err(Error::NonCanonical(
            "blob table is not in store order".to_string(),
        ));
    }

    // Each blob's (frame, offset-in-the-frame's-raw-bytes, length), assigned by replaying the same
    // packing plan the frames were just checked against — the frame boundaries and the blob boundaries
    // coincide exactly, by that same check.
    let mut index: BTreeMap<BlobId, (usize, u64, u64)> = BTreeMap::new();
    let mut frame_index = 0usize;
    let mut offset_in_frame = 0u64;
    for (id, &len) in table_ids.iter().zip(table_lens.iter()) {
        let frame_raw_len = frames
            .get(frame_index)
            .ok_or_else(|| Error::Invariant("blob table ran past the blob frames".to_string()))?
            .raw_len;
        if offset_in_frame >= frame_raw_len {
            frame_index += 1;
            offset_in_frame = 0;
        }
        index.insert(*id, (frame_index, offset_in_frame, len));
        offset_in_frame += len;
    }

    let skipped = mr.skipped_non_critical_fields();
    ir.content = ContentStore::from_artifact_source_compact(sr.into_reader(), frames, index);
    check_structure(&ir)?;

    Ok(Decoded {
        ir,
        skipped_non_critical_fields: skipped,
        contract: found,
        digest: stored_digest,
    })
}

/// As [`read_and_inflate_frame`], reading from the sequential, unhashed [`sequential::SeqReader`]
/// instead of a [`CanonReader`] (RFC 018's streaming path).
fn read_and_inflate_frame_streaming<R: Read>(
    sr: &mut sequential::SeqReader<R>,
) -> Result<(Vec<u8>, u64)> {
    let codec = frame::Codec::from_u64(sr.uvarint_raw()?)?;
    let raw_len = sr.uvarint_raw()?;
    let stored_len = sr.uvarint_len_raw()?;
    let stored = sr.raw(
        usize::try_from(stored_len).map_err(|_| Error::Decode("frame too large".to_string()))?,
        "metadata frame",
    )?;
    let raw = frame::inflate_exact(codec, &stored, raw_len)?;
    Ok((raw, raw_len))
}

/// [`read_from`], over a real file (RFC 015 D-4) — the entry point `verify`/`inspect` use: the file is
/// opened once and kept open by the resulting [`Ir`]'s content store for its own later, on-demand reads.
///
/// # Errors
/// [`Error::Io`] if `path` cannot be opened; otherwise as [`read_from`].
pub fn from_file(path: &std::path::Path) -> Result<Decoded> {
    let file = std::fs::File::open(path).map_err(Error::io)?;
    read_from(file)
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
    for id in ir.content.ids() {
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
