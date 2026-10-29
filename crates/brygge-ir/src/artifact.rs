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
use crate::model::{AtomId, ChangedPart, Ir, LossClass, PathOp};
use crate::status::EpistemicStatus;
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

    let mut hasher = Sha256::new();
    let digest_offset = out.stream_position().map_err(Error::io)?;
    out.write_all(&[0u8; 32]).map_err(Error::io)?;

    // The digest covers the *logical* stream (RFC 018 D-3): version, then the metadata's raw bytes,
    // then the blob table, then every blob's raw bytes in store order — never a frame header or a
    // compressed byte, so it never depends on how the content was compressed. Hashed by
    // `hash_header_and_table`/`hashed_blob_bytes`, the exact two functions [`digest_of`] also calls, so
    // the two can never diverge on what counts toward the digest.
    let (meta_bytes, order, lens) = hash_header_and_table(ir, version.as_bytes(), &mut hasher)?;

    write_frames(
        out,
        &meta_bytes,
        &frame::fixed_chunk_lens(meta_bytes.len() as u64),
        force_stored,
    )?;

    out.write_all(&canon::uvarint_value(order.len() as u64))
        .map_err(Error::io)?;
    for (id, len) in order.iter().zip(lens.iter()) {
        out.write_all(id.as_bytes()).map_err(Error::io)?;
        out.write_all(&canon::uvarint_value(*len))
            .map_err(Error::io)?;
    }

    let frame_lens = frame::canonical_frame_raw_lens(lens.iter().copied());
    out.write_all(&canon::uvarint_value(frame_lens.len() as u64))
        .map_err(Error::io)?;
    write_blob_frames(out, ir, &order, &frame_lens, &mut hasher, force_stored)?;

    let digest: [u8; 32] = hasher.finalize().into();
    let end = out.stream_position().map_err(Error::io)?;
    out.seek(SeekFrom::Start(digest_offset))
        .map_err(Error::io)?;
    out.write_all(&digest).map_err(Error::io)?;
    out.seek(SeekFrom::Start(end)).map_err(Error::io)?;
    Ok(())
}

/// `store_order(ir)`, plus each blob's own length (`ir.content.blob_len`, never a read of its bytes) —
/// and the one check that belongs with computing it: every blob `ir.content` holds must be named by
/// `order` (RFC 018 D-2's orphan check), since `store_order` only names a blob some op actually
/// references. Pure; shared by [`write_to_inner`] and [`digest_of`], so an orphan is caught the same
/// way, with the same message, regardless of which one is asking.
fn store_order_and_lens(ir: &Ir) -> Result<(Vec<BlobId>, Vec<u64>)> {
    let order = store_order(ir);
    if order.len() != ir.content.len() {
        return Err(Error::Invariant(
            match ir.content.ids().find(|id| !order.contains(id)) {
                Some(orphan) => {
                    format!("blob {orphan} is stored but referenced by no operation (orphan)")
                }
                None => "a blob is stored but referenced by no operation (orphan)".to_string(),
            },
        ));
    }
    let mut lens = Vec::with_capacity(order.len());
    for id in &order {
        let len = ir
            .content
            .blob_len(id)
            .ok_or_else(|| Error::Invariant(format!("blob {id} indexed but not readable")))?;
        lens.push(len);
    }
    Ok((order, lens))
}

/// The D-3 logical-stream digest's own header-and-table hashing (RFC 018 D-3): `version_bytes`, then
/// the metadata record, then the blob table — fed into `hasher` in exactly the order [`write_to`]
/// writes them, and returned (`meta_bytes`, `order`, `lens`) so the caller can go on to write or hash
/// what comes after (frames, for [`write_to_inner`]; every blob's own bytes, for [`digest_of`]) without
/// re-deriving any of it. Shared by both, so they cannot diverge on what the header and table
/// contribute to the digest.
fn hash_header_and_table(
    ir: &Ir,
    version_bytes: &[u8],
    hasher: &mut Sha256,
) -> Result<(Vec<u8>, Vec<BlobId>, Vec<u64>)> {
    hasher.update(version_bytes);
    let meta_bytes = ir.encode_metadata();
    hasher.update(canon::uvarint_value(meta_bytes.len() as u64));
    hasher.update(&meta_bytes);

    let (order, lens) = store_order_and_lens(ir)?;
    hasher.update(canon::uvarint_value(order.len() as u64));
    for (id, len) in order.iter().zip(lens.iter()) {
        hasher.update(id.as_bytes());
        hasher.update(canon::uvarint_value(*len));
    }
    Ok((meta_bytes, order, lens))
}

/// Read one blob's bytes and hash them into `hasher` (RFC 018 D-3's blob-content hashing step) —
/// shared by [`write_blob_frames`]'s inner loop and [`digest_of`]'s own loop, so the two can never
/// diverge on what a blob's bytes contribute to the digest.
fn hashed_blob_bytes(ir: &Ir, id: &BlobId, hasher: &mut Sha256) -> Result<Vec<u8>> {
    let bytes = ir
        .content
        .get(id)?
        .ok_or_else(|| Error::Invariant(format!("blob {id} indexed but not readable")))?;
    hasher.update(&bytes);
    Ok(bytes)
}

/// The D-3 digest [`write_to`] would write for `ir`, without writing an artifact: streams the same
/// logical stream ([`hash_header_and_table`], then every blob's own bytes via [`hashed_blob_bytes`],
/// one blob held at a time, never the whole content store) into one hash and returns it. Batch T's own
/// use (RFC 018 D-5): a transform step's own `TransformRecord::consumed` is the digest of the artifact
/// it read, computed this way rather than by writing that artifact just to hash it back.
///
/// # Errors
/// Whatever [`write_to`] would itself error on while computing the same digest (an orphan blob, an
/// unreadable blob).
pub fn digest_of(ir: &Ir) -> Result<[u8; 32]> {
    let mut version = CanonWriter::new();
    version.uvarint(u64::from(version::CURRENT.major));
    version.uvarint(u64::from(version::CURRENT.minor));
    version.uvarint(u64::from(version::CURRENT.patch));

    let mut hasher = Sha256::new();
    let (_meta_bytes, order, _lens) = hash_header_and_table(ir, version.as_bytes(), &mut hasher)?;
    for id in &order {
        hashed_blob_bytes(ir, id, &mut hasher)?; // dropped immediately: one blob at a time
    }
    Ok(hasher.finalize().into())
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
    write_frame_body(out, codec, raw.len() as u64, &stored)
}

/// Write one frame's header and stored bytes, given an already-compressed `(codec, stored)` pair —
/// the part [`write_one_frame`] and [`write_blob_frames`]'s parallel path share.
fn write_frame_body<W: Write>(
    out: &mut W,
    codec: frame::Codec,
    raw_len: u64,
    stored: &[u8],
) -> Result<()> {
    out.write_all(&canon::uvarint_value(codec.to_u64()))
        .map_err(Error::io)?;
    out.write_all(&canon::uvarint_value(raw_len))
        .map_err(Error::io)?;
    out.write_all(&canon::uvarint_value(stored.len() as u64))
        .map_err(Error::io)?;
    out.write_all(stored).map_err(Error::io)?;
    Ok(())
}

/// The cap on raw bytes read from `ir.content` and held in memory at once while writing blob frames
/// (review 099 R-6): bounded, not unlimited, so parallel compression still respects RFC 015 D-3's
/// spirit (never the artifact's whole content at once) — just a wider window than one frame, wide
/// enough to give every worker real work.
const PARALLEL_WINDOW: u64 = 64 * 1024 * 1024;

/// Write every blob frame the plan (`frame_lens`) describes, reading blobs from `ir.content` in
/// `order` and compressing up to [`PARALLEL_WINDOW`] worth of them at a time, across up to
/// `available_parallelism().min(8)` threads (review 099 R-6: DEFLATE at level 6 is real, serial CPU
/// work over every byte written; a repository with gigabytes of blob content pays for that many times
/// over unless the frames are compressed in parallel). Each frame is still compressed entirely on its
/// own (never split across workers) and written in exactly the order [`frame::canonical_frame_raw_lens`]
/// gives — the same bytes a fully serial writer would produce, just produced faster. A frame whose own
/// raw length already exceeds the window is compressed alone, after whatever is queued ahead of it.
fn write_blob_frames<W: Write>(
    out: &mut W,
    ir: &Ir,
    order: &[BlobId],
    frame_lens: &[u64],
    hasher: &mut Sha256,
    force_stored: bool,
) -> Result<()> {
    let workers = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .min(8);

    let mut blob_ids = order.iter();
    let mut pending: Vec<Vec<u8>> = Vec::new();
    let mut pending_total: u64 = 0;

    for &flen in frame_lens {
        // One frame's raw bytes, assembled here (RFC 015 D-3, RFC 018 §4): at most 1 MiB of packed
        // small blobs, or one large blob of its own.
        let mut buf = Vec::new();
        let mut collected = 0u64;
        while collected < flen {
            let id = blob_ids.next().ok_or_else(|| {
                Error::Invariant("the frame packing plan ran out of blobs".to_string())
            })?;
            let bytes = hashed_blob_bytes(ir, id, hasher)?;
            collected += bytes.len() as u64;
            buf.extend_from_slice(&bytes);
        }

        let buf_len = buf.len() as u64;
        if buf_len >= PARALLEL_WINDOW {
            flush_pending(out, &mut pending, force_stored, workers)?;
            pending_total = 0;
            write_one_frame(out, &buf, force_stored)?;
            continue;
        }
        if pending_total + buf_len > PARALLEL_WINDOW && !pending.is_empty() {
            flush_pending(out, &mut pending, force_stored, workers)?;
            pending_total = 0;
        }
        pending_total += buf_len;
        pending.push(buf);
    }
    flush_pending(out, &mut pending, force_stored, workers)
}

/// Compress every buffer in `pending` (in parallel, deterministically — see
/// [`compress_many_parallel`]) and write each resulting frame, in order, then clear `pending`.
fn flush_pending<W: Write>(
    out: &mut W,
    pending: &mut Vec<Vec<u8>>,
    force_stored: bool,
    workers: usize,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let compressed = if force_stored {
        pending
            .iter()
            .map(|b| (frame::Codec::Stored, b.clone()))
            .collect()
    } else {
        compress_many_parallel(pending, workers)
    };
    for (raw, (codec, stored)) in pending.iter().zip(compressed.iter()) {
        write_frame_body(out, *codec, raw.len() as u64, stored)?;
    }
    pending.clear();
    Ok(())
}

/// Compress every buffer in `buffers`, each entirely on its own thread's work (never split across
/// workers), across up to `workers` threads — deterministic and byte-identical to compressing every
/// buffer serially, in the same order: `std::thread::scope` blocks until every worker finishes, each
/// worker owns a disjoint, contiguous slice of both the input and the output (no shared mutable state,
/// no lock), and the result is assembled back in the original index order regardless of which worker
/// finished which chunk first or when.
fn compress_many_parallel(buffers: &[Vec<u8>], workers: usize) -> Vec<(frame::Codec, Vec<u8>)> {
    let n = buffers.len();
    if n == 0 {
        return Vec::new();
    }
    let workers = workers.clamp(1, n);
    let chunk_size = n.div_ceil(workers);
    let mut results: Vec<(frame::Codec, Vec<u8>)> =
        (0..n).map(|_| (frame::Codec::Stored, Vec::new())).collect();
    std::thread::scope(|scope| {
        for (bufs, outs) in buffers
            .chunks(chunk_size)
            .zip(results.chunks_mut(chunk_size))
        {
            scope.spawn(move || {
                for (buf, slot) in bufs.iter().zip(outs.iter_mut()) {
                    *slot = frame::compress_frame(buf);
                }
            });
        }
    });
    results
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

    check_structure(&ir, found)?;

    Ok(Decoded {
        ir,
        skipped_non_critical_fields: mr.skipped_non_critical_fields()
            + r.skipped_non_critical_fields(),
        contract: found,
        digest: stored_digest,
    })
}

/// Checks a metadata frame's `raw_len` against RFC 018 D-1's fixed-chunking rule — **before** it is
/// inflated (review 099 R-1): every frame but the last must be exactly [`frame::FRAME_CAP`] (1 MiB);
/// the last (`index + 1 == nframes`) at most that. `nframes` is already known (it is read before any
/// frame), so "which is last" needs no lookahead.
fn check_meta_frame_plan(index: u64, nframes: u64, raw_len: u64) -> Result<()> {
    let is_last = index + 1 == nframes;
    let ok = if is_last {
        raw_len <= frame::FRAME_CAP
    } else {
        raw_len == frame::FRAME_CAP
    };
    if ok {
        Ok(())
    } else {
        Err(Error::NonCanonical(format!(
            "metadata frame {index} of {nframes}: raw_len {raw_len} does not match the fixed 1 MiB \
             chunking rule"
        )))
    }
}

/// Walks a blob frame's decompressed bytes against the blob boundaries the blob table already
/// declared, hashing each blob's own bytes as they stream past (never holding more than the current
/// blob's running hash state — RFC 015 D-4's bound, carried over compression) and remembering which
/// table positions, if any, do not match their declared id. Checked only **after** the whole artifact's
/// digest has matched (review 099 R-1/precedence: a content mismatch is a structural finding, deferred
/// the same way every other one here is — the pass must finish hashing everything first).
struct BlobBoundaryWalker<'a> {
    table_ids: &'a [BlobId],
    table_lens: &'a [u64],
    end: usize,
    current: usize,
    remaining: u64,
    hasher: Sha256,
    mismatches: Vec<usize>,
}

impl<'a> BlobBoundaryWalker<'a> {
    fn new(table_ids: &'a [BlobId], table_lens: &'a [u64], range: std::ops::Range<usize>) -> Self {
        let mut w = Self {
            table_ids,
            table_lens,
            end: range.end.min(table_lens.len()),
            current: range.start,
            remaining: 0,
            hasher: Sha256::new(),
            mismatches: Vec::new(),
        };
        w.start_current();
        w
    }

    fn start_current(&mut self) {
        self.remaining = self.table_lens.get(self.current).copied().unwrap_or(0);
        self.hasher = Sha256::new();
    }

    fn finish_current(&mut self) {
        let digest: [u8; 32] = std::mem::replace(&mut self.hasher, Sha256::new())
            .finalize()
            .into();
        if self
            .table_ids
            .get(self.current)
            .is_some_and(|id| id.0 != digest)
        {
            self.mismatches.push(self.current);
        }
        self.current += 1;
        if self.current < self.end {
            self.start_current();
        }
    }

    fn feed(&mut self, mut chunk: &[u8]) {
        loop {
            while self.current < self.end && self.remaining == 0 {
                self.finish_current();
            }
            if chunk.is_empty() || self.current >= self.end {
                return;
            }
            let take = (self.remaining as usize).min(chunk.len());
            let Some((head, tail)) = chunk.split_at_checked(take) else {
                return;
            };
            self.hasher.update(head);
            self.remaining = self.remaining.saturating_sub(take as u64);
            chunk = tail;
        }
    }

    /// Called once this frame's own bytes are exhausted: finish any blob(s) that end exactly at the
    /// frame's own end (including a trailing empty blob), and return the mismatches found.
    fn finish(mut self) -> Vec<usize> {
        while self.current < self.end && self.remaining == 0 {
            self.finish_current();
        }
        self.mismatches
    }
}

/// [`from_bytes`] over a **contract 0.3.0** artifact (RFC 018 D-1): reconstructs the logical stream by
/// inflating every frame (a failure here is immediate — "a frame that does not inflate", the format
/// page's one exception to the digest-first rule, since there is no logical stream left to hash), then
/// hashes that reconstruction exactly as [`write_to`] does and compares it with `stored_digest` **before
/// trusting any of its structure** — matching [`from_bytes_v2`]'s own precedence. Only once the digest
/// matches are the packing rules, the blob table's order, and each blob's content address checked.
///
/// **Review 099 R-1:** every frame's `raw_len` is checked against the packing plan *before* it is
/// inflated (the blob table is read first, so the plan is known before the first blob frame), and every
/// inflate streams through a fixed 64 KiB buffer ([`frame::inflate_streamed`]) rather than allocating a
/// buffer sized by the frame's own claim — `from_bytes`'s own contract still materializes every blob's
/// real bytes in the end (it returns a fully in-memory `Ir`), so this does not bound its *total* memory
/// to the artifact's size the way [`read_from`] does; it bounds *each frame's own inflate* to 64 KiB,
/// and refuses a plan-inconsistent frame before wasting an inflate on it at all.
fn from_bytes_v3(
    version_bytes: &[u8],
    stored_digest: [u8; 32],
    rest: &[u8],
    found: ContractVersion,
) -> Result<Decoded> {
    let mut r = CanonReader::new(rest);
    let mut hasher = Sha256::new();
    hasher.update(version_bytes);

    let nframes_meta = r.uvarint()?;
    let mut meta_bytes = Vec::new();
    for i in 0..nframes_meta {
        let codec = frame::Codec::from_u64(r.uvarint()?)?;
        let raw_len = r.uvarint()?;
        check_meta_frame_plan(i, nframes_meta, raw_len)?;
        let stored_len = r.uvarint_len()?;
        let stored = r.raw(stored_len)?;
        frame::check_frame_bounds(codec, stored.len() as u64, raw_len)?;
        frame::inflate_streamed(codec, stored, raw_len, |chunk| {
            meta_bytes.extend_from_slice(chunk);
            Ok(())
        })?;
    }
    let meta_len = meta_bytes.len() as u64;
    hasher.update(canon::uvarint_value(meta_len));
    hasher.update(&meta_bytes);

    let nblobs = r.uvarint()?;
    hasher.update(canon::uvarint_value(nblobs));
    let mut table_ids = Vec::with_capacity(usize::try_from(nblobs).unwrap_or(0).min(1 << 20));
    let mut table_lens = Vec::with_capacity(usize::try_from(nblobs).unwrap_or(0).min(1 << 20));
    for _ in 0..nblobs {
        let id = BlobId(r.raw32()?);
        let len = r.uvarint()?;
        hasher.update(id.as_bytes());
        hasher.update(canon::uvarint_value(len));
        table_ids.push(id);
        table_lens.push(len);
    }

    // Each blob frame's own decompressed bytes, kept (one `Vec` per frame) until the digest and every
    // structural check pass — `from_bytes`'s own contract returns a fully in-memory `Ir`, so it must
    // materialize every blob's real bytes somewhere; keeping them here, produced by the one bounded
    // (64 KiB at a time) inflate the verifying pass already did, means no byte is ever inflated twice.
    let plan = frame::canonical_frame_plan(&table_lens);
    let nframes_blob = r.uvarint()?;
    if nframes_blob != plan.len() as u64 {
        return Err(Error::NonCanonical(format!(
            "{nframes_blob} blob frame(s), the packing rule expects {}",
            plan.len()
        )));
    }
    let mut mismatches = Vec::new();
    let mut frame_buffers: Vec<(std::ops::Range<usize>, Vec<u8>)> = Vec::new();
    for (frame_index, (expected_raw_len, range)) in plan.into_iter().enumerate() {
        let codec = frame::Codec::from_u64(r.uvarint()?)?;
        let raw_len = r.uvarint()?;
        if raw_len != expected_raw_len {
            return Err(Error::NonCanonical(format!(
                "blob frame {frame_index}: raw_len {raw_len} does not match the packing rule's \
                 {expected_raw_len}"
            )));
        }
        let stored_len = r.uvarint_len()?;
        let stored = r.raw(stored_len)?;
        frame::check_frame_bounds(codec, stored.len() as u64, raw_len)?;
        let mut walker = BlobBoundaryWalker::new(&table_ids, &table_lens, range.clone());
        let mut buf = Vec::new();
        frame::inflate_streamed(codec, stored, raw_len, |chunk| {
            hasher.update(chunk);
            walker.feed(chunk);
            buf.extend_from_slice(chunk);
            Ok(())
        })?;
        mismatches.extend(walker.finish());
        frame_buffers.push((range, buf));
    }
    let trailing = !r.is_empty();

    let computed: [u8; 32] = hasher.finalize().into();
    if computed != stored_digest {
        return Err(Error::DigestMismatch);
    }

    if trailing {
        return Err(Error::NonCanonical(
            "trailing bytes after blob store".to_string(),
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
    if !mismatches.is_empty() {
        return Err(Error::Decode(
            "blob content does not match its id".to_string(),
        ));
    }

    // Every blob's content was hashed against its declared id above (no mismatch, checked just before
    // this point) — safe to insert now, slicing each blob's own bytes out of the frame buffer that
    // produced them, in store order.
    for (range, buf) in frame_buffers {
        let mut offset: usize = 0;
        for i in range {
            let len = usize::try_from(*table_lens.get(i).unwrap_or(&0))
                .map_err(|_| Error::Decode("blob length too large".to_string()))?;
            let slice = buf.get(offset..offset + len).ok_or_else(|| {
                Error::Invariant("blob frame shorter than its own blobs".to_string())
            })?;
            ir.content.insert(slice.to_vec())?;
            offset += len;
        }
    }

    check_structure(&ir, found)?;

    Ok(Decoded {
        ir,
        skipped_non_critical_fields: mr.skipped_non_critical_fields(),
        contract: found,
        digest: stored_digest,
    })
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
            check_structure(&body.ir, found)?;
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
/// inflating each frame through a fixed 64 KiB buffer ([`frame::inflate_streamed`]) to feed the
/// logical-stream digest — metadata's bytes are kept (metadata has always been held whole, and each of
/// its frames is capped at 1 MiB by the fixed-chunking rule, checked before it is inflated); a blob
/// frame's decompressed bytes are hashed, chunk by chunk, against both the overall digest and whichever
/// blob(s) the chunk belongs to ([`BlobBoundaryWalker`]), and then dropped — only the frame's own
/// `FrameLoc` and each blob's `(frame, offset, length)` are kept. So the pass's own memory is bounded by
/// 64 KiB of inflate output at a time, not by any frame's declared size, however large (review 099 R-1 —
/// the gap this closes: a `raw_len` is checked against the packing plan, known from the blob table read
/// before it, *before* that frame is inflated at all, and no allocation anywhere in this pass is sized
/// by an unchecked `raw_len`).
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
    for i in 0..nframes_meta {
        let codec = frame::Codec::from_u64(sr.uvarint_raw()?)?;
        let raw_len = sr.uvarint_raw()?;
        check_meta_frame_plan(i, nframes_meta, raw_len)?;
        let stored_len = sr.uvarint_len_raw()?;
        let stored = sr.raw(
            usize::try_from(stored_len)
                .map_err(|_| Error::Decode("frame too large".to_string()))?,
            "metadata frame",
        )?;
        frame::check_frame_bounds(codec, stored.len() as u64, raw_len)?;
        frame::inflate_streamed(codec, &stored, raw_len, |chunk| {
            meta_bytes.extend_from_slice(chunk);
            Ok(())
        })?;
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

    let plan = frame::canonical_frame_plan(&table_lens);
    let nframes_blob = sr.uvarint_raw()?;
    if nframes_blob != plan.len() as u64 {
        return Err(Error::NonCanonical(format!(
            "{nframes_blob} blob frame(s), the packing rule expects {}",
            plan.len()
        )));
    }
    let mut frames: Vec<FrameLoc> = Vec::with_capacity(plan.len());
    let mut index: BTreeMap<BlobId, (usize, u64, u64)> = BTreeMap::new();
    let mut mismatches = Vec::new();
    for (frame_index, (expected_raw_len, range)) in plan.into_iter().enumerate() {
        let codec = frame::Codec::from_u64(sr.uvarint_raw()?)?;
        let raw_len = sr.uvarint_raw()?;
        if raw_len != expected_raw_len {
            return Err(Error::NonCanonical(format!(
                "blob frame {frame_index}: raw_len {raw_len} does not match the packing rule's \
                 {expected_raw_len}"
            )));
        }
        let stored_len = sr.uvarint_len_raw()?;
        let stored_offset = sr.position();
        let stored = sr.raw(
            usize::try_from(stored_len)
                .map_err(|_| Error::Decode("frame too large".to_string()))?,
            "blob frame",
        )?;
        frame::check_frame_bounds(codec, stored.len() as u64, raw_len)?;
        let mut walker = BlobBoundaryWalker::new(&table_ids, &table_lens, range.clone());
        frame::inflate_streamed(codec, &stored, raw_len, |chunk| {
            hasher.update(chunk);
            walker.feed(chunk);
            Ok(())
        })?;
        mismatches.extend(walker.finish());
        frames.push(FrameLoc {
            stored_offset,
            codec,
            raw_len,
            stored_len,
        });
        let mut offset_in_frame = 0u64;
        for i in range {
            let id = *table_ids
                .get(i)
                .ok_or_else(|| Error::Invariant("blob table index out of range".to_string()))?;
            let len = *table_lens.get(i).unwrap_or(&0);
            index.insert(id, (frame_index, offset_in_frame, len));
            offset_in_frame += len;
        }
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
    if !mismatches.is_empty() {
        return Err(Error::Decode(
            "blob content does not match its id".to_string(),
        ));
    }

    let skipped = mr.skipped_non_critical_fields();
    ir.content = ContentStore::from_artifact_source_compact(sr.into_reader(), frames, index);
    check_structure(&ir, found)?;

    Ok(Decoded {
        ir,
        skipped_non_critical_fields: skipped,
        contract: found,
        digest: stored_digest,
    })
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
/// known, blob referential integrity in both directions (no missing blob, no orphan), and — RFC 018
/// D-5 — the transform chain's own cross-record rules (step bounds, vocabulary, and that contract 0.2.0
/// never carries any of it).
fn check_structure(ir: &Ir, contract: ContractVersion) -> Result<()> {
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

    if contract.minor == 2 {
        reject_transform_extensions(ir)?;
    } else {
        check_transform_chain(ir)?;
    }

    Ok(())
}

/// RFC 018 D-5: contract 0.2.0 has none of the batch-T extensions — `Ir` field 6 (`transforms`),
/// `EpistemicStatus` variant 2 (`Transformed`), `LossClass` variant 3 (`Transform`), or `DropRecord`
/// field 4 (`step`). The shared metadata decoder ([`Ir::decode_metadata`] and everything it calls) does
/// not itself know which contract it is reading (RFC 011 §2.4's record decoders are contract-oblivious
/// by design), so a 0.2.0 artifact that smuggles any of these in via hand-crafted bytes decodes to a
/// structurally plausible `Ir` — this is the one place that must notice and refuse it.
fn reject_transform_extensions(ir: &Ir) -> Result<()> {
    if !ir.transforms.is_empty() {
        return Err(Error::NonCanonical(
            "contract 0.2.0 does not have Ir field 6 (transforms)".to_string(),
        ));
    }
    for atom in &ir.atoms {
        if matches!(atom.status, EpistemicStatus::Transformed { .. }) {
            return Err(Error::NonCanonical(
                "contract 0.2.0 does not have EpistemicStatus variant 2 (Transformed)".to_string(),
            ));
        }
        for op in &atom.ops {
            if matches!(op.status(), EpistemicStatus::Transformed { .. }) {
                return Err(Error::NonCanonical(
                    "contract 0.2.0 does not have EpistemicStatus variant 2 (Transformed)"
                        .to_string(),
                ));
            }
        }
        for copy in &atom.copies {
            if matches!(copy.status, EpistemicStatus::Transformed { .. }) {
                return Err(Error::NonCanonical(
                    "contract 0.2.0 does not have EpistemicStatus variant 2 (Transformed)"
                        .to_string(),
                ));
            }
        }
    }
    for rf in &ir.refs {
        if matches!(rf.status, EpistemicStatus::Transformed { .. }) {
            return Err(Error::NonCanonical(
                "contract 0.2.0 does not have EpistemicStatus variant 2 (Transformed)".to_string(),
            ));
        }
    }
    for drop in &ir.loss.dropped {
        if matches!(drop.class, LossClass::Transform) {
            return Err(Error::NonCanonical(
                "contract 0.2.0 does not have LossClass variant 3 (Transform)".to_string(),
            ));
        }
        if drop.step.is_some() {
            return Err(Error::NonCanonical(
                "contract 0.2.0 does not have DropRecord field 4 (step)".to_string(),
            ));
        }
    }
    Ok(())
}

/// Which kind of record a status belongs to (RFC 018 D-5): sets which `changed` vocabulary applies,
/// and — for [`StatusContext::Copy`] — that `Transformed` may not appear there at all (D-5 names only
/// an atom's, an op's, and a ref's own status as valid locations).
#[derive(Debug, Clone, Copy)]
enum StatusContext {
    Atom,
    Op,
    Ref,
    Copy,
}

impl StatusContext {
    fn vocab_ok(self, part: ChangedPart) -> bool {
        match self {
            Self::Atom => part.valid_on_atom(),
            Self::Op => part.valid_on_op(),
            Self::Ref => part.valid_on_ref(),
            Self::Copy => false,
        }
    }
}

/// Walk one status's own `Transformed`/`prior` chain **iteratively** (RFC 018 D-5 explicitly asks for
/// this, not recursion: the chain is already bounded to 64 at decode time, but this walk makes no
/// assumption of its own about that), checking each layer's `step` against the chain's actual length and
/// its `changed` against `context`'s vocabulary (or refusing outright, for [`StatusContext::Copy`], where
/// `Transformed` is never a valid location at all).
fn check_status_chain(
    status: &EpistemicStatus,
    context: StatusContext,
    nsteps: usize,
) -> Result<()> {
    let mut current = status;
    loop {
        let EpistemicStatus::Transformed {
            step,
            changed,
            prior,
        } = current
        else {
            return Ok(());
        };
        if matches!(context, StatusContext::Copy) {
            return Err(Error::NonCanonical(
                "Transformed may not appear on a copy record's own status".to_string(),
            ));
        }
        let step_usize = usize::try_from(*step).unwrap_or(usize::MAX);
        if step_usize >= nsteps {
            return Err(Error::NonCanonical(format!(
                "a Transformed status's step {step} is not less than the chain's length {nsteps}"
            )));
        }
        if changed.iter().any(|c| !context.vocab_ok(*c)) {
            return Err(Error::Decode(
                "a Transformed status's changed uses a part outside its vocabulary".to_string(),
            ));
        }
        current = prior;
    }
}

/// RFC 018 D-5's structural rules for a contract that has the transform chain (0.3.0 and later): every
/// `Transformed`'s own step is in range and uses the right vocabulary for where it sits, `Transformed`
/// never appears on a copy record, and every `Transform`-class drop's step is in range (its presence and
/// correlation with `class` was already checked at decode time — [`crate::model`]'s own `DropRecord`
/// decoder).
fn check_transform_chain(ir: &Ir) -> Result<()> {
    let nsteps = ir.transforms.len();
    for atom in &ir.atoms {
        check_status_chain(&atom.status, StatusContext::Atom, nsteps)?;
        for op in &atom.ops {
            check_status_chain(op.status(), StatusContext::Op, nsteps)?;
        }
        for copy in &atom.copies {
            check_status_chain(&copy.status, StatusContext::Copy, nsteps)?;
        }
    }
    for rf in &ir.refs {
        check_status_chain(&rf.status, StatusContext::Ref, nsteps)?;
    }
    for drop in &ir.loss.dropped {
        if let Some(step) = drop.step {
            let step_usize = usize::try_from(step).unwrap_or(usize::MAX);
            if step_usize >= nsteps {
                return Err(Error::NonCanonical(format!(
                    "a Transform drop's step {step} is not less than the chain's length {nsteps}"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mutation;
