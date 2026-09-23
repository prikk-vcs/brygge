//! A minimal, bounds-checked reader for Mercurial **revlogv1** stores (RFC 005 D-1, Tier 2).
//!
//! Every revision read is **verified against its node**: [`Revlog::revision`] is the one place a
//! revision's full text is reconstructed, and it re-hashes that text the way Mercurial does
//! (`storageutil.hashrevisionsha1`) before returning it, so a store that presents content under a node it
//! does not hash to is refused rather than carried as a false link.
//!
//! A revlog is an index (`.i`) plus, when not *inline*, a data file (`.d`). Each 64-byte index entry
//! (big-endian) is `offset_flags(u64) · comp_len(u32) · uncomp_len(u32) · base_rev(i32) · link_rev(i32)
//! · p1(i32) · p2(i32) · node(20) · pad(12)`. A revision's stored chunk is either a full snapshot
//! (`base_rev == rev`) or a delta against `base_rev`; chunks are raw, zlib, or zstd by first byte. The
//! reader is fully bounds-checked: a malformed store is an [`Error`], never a panic (brygge reads an
//! untrusted store). Validated against `hg debugdata`/`hg debugindex` (see tests).

use std::io::Read as _;
use std::path::Path;

use crate::{Error, floor};

const ENTRY_LEN: usize = 64;
const FLAG_INLINE: u32 = 1 << 16;
/// Per-revision censored flag (`REVIDX_ISCENSORED`), refused by the floor.
const REVIDX_ISCENSORED: u16 = 1 << 15;
/// `REVIDX_ELLIPSIS` (narrow clones): Mercurial's own flag processor reports the hash as *not* matching
/// the text (`revlog.py` `ellipsisreadprocessor` returns `(text, False)`), so it cannot be verified.
const REVIDX_ELLIPSIS: u16 = 1 << 14;
/// `REVIDX_EXTSTORED` (large-file extensions): the revlog stores a pointer while the node hashes the
/// real text (`hgext/lfs/wrapper.py` `writetostore` returns `(rawtext, False)`), so the stored text is
/// not the hashed text.
const REVIDX_EXTSTORED: u16 = 1 << 13;
/// The flags that do not change what is hashed (`revlogutils/constants.py` `REVIDX_NEUTRAL_FLAGS`):
/// has-copies-info (1<<12), has-meta (1<<11) and the five delta-info flags (1<<10 … 1<<6). The stored raw
/// text of a revision carrying only these is exactly the text its node hashes.
const HASH_NEUTRAL_FLAGS: u16 =
    (1 << 12) | (1 << 11) | (1 << 10) | (1 << 9) | (1 << 8) | (1 << 7) | (1 << 6);

/// The null revision (`-1`): a missing parent or base.
pub const NULL_REV: i32 = -1;

/// Resource ceilings for reconstructing a revlog revision (RFC 010 D-4, CR-17). One place for every
/// revlog ceiling; tests construct a small instance instead of needing gigabytes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// The maximum size of one reconstructed revision text — checked on every decompressed chunk and on
    /// the output of every `mpatch` application, so a revlog decompression bomb (a small stored chunk
    /// that inflates to an enormous text) is refused rather than exhausting the host.
    pub(crate) max_revision_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_revision_bytes: 1024 * 1024 * 1024,
        }
    }
}

fn resource_limit(what: &str, max_revision_bytes: u64) -> Error {
    Error::ResourceLimit {
        what: what.to_string(),
        ceiling: format!("{max_revision_bytes} bytes"),
    }
}

fn read_err(msg: impl std::fmt::Display) -> Error {
    Error::Read(msg.to_string())
}

fn u32_be(buf: &[u8], at: usize) -> Result<u32, Error> {
    let s = buf
        .get(at..at + 4)
        .ok_or_else(|| read_err("revlog index truncated"))?;
    let a: [u8; 4] = s.try_into().map_err(|_| read_err("bad u32"))?;
    Ok(u32::from_be_bytes(a))
}

fn u64_be(buf: &[u8], at: usize) -> Result<u64, Error> {
    let s = buf
        .get(at..at + 8)
        .ok_or_else(|| read_err("revlog index truncated"))?;
    let a: [u8; 8] = s.try_into().map_err(|_| read_err("bad u64"))?;
    Ok(u64::from_be_bytes(a))
}

/// One parsed index entry.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Per-revision flags (`offset_flags & 0xFFFF`).
    pub flags: u16,
    /// The revision this one is a delta against; `== rev` means a full snapshot.
    pub base_rev: i32,
    /// The changeset revision that introduced this revision (used to resolve a copy source's true
    /// changeset when neither parent carries it directly — RFC 005 corrections handoff §2).
    pub link_rev: i32,
    /// First parent revision, or [`NULL_REV`].
    pub p1: i32,
    /// Second parent revision, or [`NULL_REV`].
    pub p2: i32,
    /// The 20-byte node id (opaque source identity, `PR-4`).
    pub node: [u8; 20],
    /// Absolute byte position of this revision's data chunk (in the `.i` when inline, else in the `.d`).
    data_pos: usize,
    /// Length of the stored (compressed) chunk.
    comp_len: usize,
    /// The full revision text length the index records for this revision (RFC 010 CR-17) — checked
    /// against the reconstructed text, never trusted as a pre-allocation size.
    uncomp_len: usize,
}

impl Entry {
    /// True when the censored flag is set (`REVIDX_ISCENSORED`).
    #[must_use]
    pub fn is_censored(&self) -> bool {
        self.flags & REVIDX_ISCENSORED != 0
    }
}

/// A parsed revlog: its index entries plus the buffers holding the data chunks.
pub struct Revlog {
    entries: Vec<Entry>,
    inline: bool,
    index_buf: Vec<u8>,
    data_buf: Vec<u8>,
}

impl Revlog {
    /// Open the revlog whose index is `index_path` (e.g. `.../00changelog.i`), reading its `.d` sibling
    /// when the revlog is not inline.
    ///
    /// # Errors
    /// [`Error::Read`] on I/O failure or a malformed/unsupported index (only revlogv1 is read; the
    /// format-safety gate refuses other formats before this is reached).
    pub fn open(index_path: &Path) -> Result<Self, Error> {
        let index_buf = std::fs::read(index_path)
            .map_err(|e| read_err(format!("{}: {e}", index_path.display())))?;
        if index_buf.is_empty() {
            return Ok(Self {
                entries: Vec::new(),
                inline: false,
                index_buf,
                data_buf: Vec::new(),
            });
        }
        let version = u32_be(&index_buf, 0)?;
        if version & 0xFFFF != 1 {
            return Err(read_err(format!(
                "unsupported revlog format {} (only revlogv1 is read)",
                version & 0xFFFF
            )));
        }
        let inline = version & FLAG_INLINE != 0;

        let data_buf = if inline {
            Vec::new()
        } else {
            let data_path = index_path.with_extension("d");
            match std::fs::read(&data_path) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(read_err(format!("{}: {e}", data_path.display()))),
            }
        };

        let entries = if inline {
            Self::parse_inline(&index_buf)?
        } else {
            Self::parse_split(&index_buf)?
        };
        Ok(Self {
            entries,
            inline,
            index_buf,
            data_buf,
        })
    }

    /// Parse one 64-byte index entry at `at`, given its revision number (rev 0 carries the version word
    /// in its high bytes, so its offset is forced to zero).
    fn parse_entry(buf: &[u8], at: usize, data_pos: usize) -> Result<Entry, Error> {
        let offset_flags = u64_be(buf, at)?;
        let flags = (offset_flags & 0xFFFF) as u16;
        let comp_len = u32_be(buf, at + 8)? as usize;
        let uncomp_len = u32_be(buf, at + 12)? as usize;
        let base_rev = u32_be(buf, at + 16)? as i32;
        let link_rev = u32_be(buf, at + 20)? as i32;
        let p1 = u32_be(buf, at + 24)? as i32;
        let p2 = u32_be(buf, at + 28)? as i32;
        let node_slice = buf
            .get(at + 32..at + 52)
            .ok_or_else(|| read_err("revlog index truncated at node"))?;
        let node: [u8; 20] = node_slice.try_into().map_err(|_| read_err("bad node"))?;
        Ok(Entry {
            flags,
            base_rev,
            link_rev,
            p1,
            p2,
            node,
            uncomp_len,
            data_pos,
            comp_len,
        })
    }

    /// Non-inline: the `.i` is a packed array of 64-byte entries; data lives in the `.d` at each entry's
    /// own offset. We read the `.d` sequentially (offset == running sum of comp_len), which is how a
    /// revlog is laid out, avoiding reliance on the version-polluted rev-0 offset field.
    fn parse_split(index_buf: &[u8]) -> Result<Vec<Entry>, Error> {
        if index_buf.len() % ENTRY_LEN != 0 {
            return Err(read_err("revlog index size is not a multiple of 64"));
        }
        let count = index_buf.len() / ENTRY_LEN;
        let mut entries = Vec::with_capacity(count);
        let mut data_pos = 0usize;
        for rev in 0..count {
            let entry = Self::parse_entry(index_buf, rev * ENTRY_LEN, data_pos)?;
            data_pos = data_pos
                .checked_add(entry.comp_len)
                .ok_or_else(|| read_err("revlog data offset overflow"))?;
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Inline: each 64-byte entry is immediately followed by its own data chunk in the `.i`.
    fn parse_inline(index_buf: &[u8]) -> Result<Vec<Entry>, Error> {
        let mut entries = Vec::new();
        let mut pos = 0usize;
        while pos < index_buf.len() {
            let data_pos = pos
                .checked_add(ENTRY_LEN)
                .ok_or_else(|| read_err("revlog inline overflow"))?;
            let entry = Self::parse_entry(index_buf, pos, data_pos)?;
            pos = data_pos
                .checked_add(entry.comp_len)
                .ok_or_else(|| read_err("revlog inline overflow"))?;
            if pos > index_buf.len() {
                return Err(read_err("revlog inline data runs past end of index"));
            }
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Number of revisions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the revlog has no revisions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry for revision `rev`, if present.
    #[must_use]
    pub fn entry(&self, rev: usize) -> Option<&Entry> {
        self.entries.get(rev)
    }

    /// The raw stored chunk bytes for revision `rev`.
    fn chunk(&self, rev: usize) -> Result<&[u8], Error> {
        let e = self
            .entries
            .get(rev)
            .ok_or_else(|| read_err("revision out of range"))?;
        let buf = if self.inline {
            &self.index_buf
        } else {
            &self.data_buf
        };
        buf.get(e.data_pos..e.data_pos + e.comp_len)
            .ok_or_else(|| read_err("revlog chunk out of range"))
    }

    /// Reconstruct the full revision text for `rev` (following the delta chain to a snapshot), under
    /// the default [`Limits`].
    ///
    /// # Errors
    /// [`Error::Read`] on a malformed chunk, an unknown compression marker, a bad delta, a cyclic /
    /// over-long base chain, or a reconstructed length that does not match the index (RFC 010 CR-17);
    /// [`Error::FloorRefusal`] on a censored, ellipsis, externally stored or unknown-flag revision;
    /// [`Error::Read`] if the reconstructed text does not hash to the node; [`Error::ResourceLimit`] if a
    /// decompressed chunk or a patched text exceeds the ceiling.
    pub fn revision(&self, rev: usize) -> Result<Vec<u8>, Error> {
        self.revision_with(rev, &Limits::default())
    }

    pub(crate) fn revision_with(&self, rev: usize, limits: &Limits) -> Result<Vec<u8>, Error> {
        let e = self
            .entries
            .get(rev)
            .ok_or_else(|| read_err("revision out of range"))?;
        check_flags(e)?;

        // Build the base chain from `rev` back to a full snapshot, guarding against cycles/over-length.
        let mut chain = Vec::new();
        let mut cur = rev;
        loop {
            chain.push(cur);
            let entry = self
                .entries
                .get(cur)
                .ok_or_else(|| read_err("base chain out of range"))?;
            let base = entry.base_rev;
            if base < 0 || base as usize == cur {
                break; // full snapshot
            }
            let base = base as usize;
            if base >= cur || chain.len() > self.entries.len() {
                return Err(read_err("revlog base chain is cyclic or over-long"));
            }
            cur = base;
        }
        chain.reverse();

        let mut text: Vec<u8> = Vec::new();
        for (i, &r) in chain.iter().enumerate() {
            let raw = decompress(self.chunk(r)?, limits)?;
            text = if i == 0 {
                raw // the snapshot
            } else {
                mpatch(&text, &raw, limits)?
            };
            // RFC 010 CR-17: every step's reconstructed text — the snapshot and each intermediate delta
            // application — must equal the length the index recorded for *that* revision. A mismatch is
            // a malformed store (index/data disagree), not a resource ceiling.
            let step_entry = self
                .entries
                .get(r)
                .ok_or_else(|| read_err("base chain out of range"))?;
            if text.len() != step_entry.uncomp_len {
                return Err(read_err("revision length does not match the index"));
            }
        }
        self.verify_node(rev, &text)?;
        Ok(text)
    }

    /// The node of parent revision `parent` (`NULL_REV` → twenty zero bytes, Mercurial's null id).
    fn parent_node(&self, parent: i32) -> Result<[u8; 20], Error> {
        if parent == NULL_REV {
            return Ok([0u8; 20]);
        }
        usize::try_from(parent)
            .ok()
            .and_then(|p| self.entries.get(p))
            .map(|e| e.node)
            .ok_or_else(|| read_err("revlog parent revision out of range"))
    }

    /// Verify that `text` (revision `rev`'s reconstructed raw text) hashes to the entry's node, exactly as
    /// Mercurial's `storageutil.hashrevisionsha1` does: SHA-1 over the two parent nodes in ascending order
    /// and then the raw text. Null is all zeros, so `nullid ‖ p1 ‖ text` (Mercurial's p2-is-null branch) is
    /// the same computation as the ascending-order rule. The hash is collision-detecting, as Mercurial's
    /// own is (`utils/hashutil.py` uses sha1dc).
    fn verify_node(&self, rev: usize, text: &[u8]) -> Result<(), Error> {
        let e = self
            .entries
            .get(rev)
            .ok_or_else(|| read_err("revision out of range"))?;
        let p1 = self.parent_node(e.p1)?;
        let p2 = self.parent_node(e.p2)?;
        let (lo, hi) = if p1 <= p2 { (p1, p2) } else { (p2, p1) };
        check_revision_hash(&e.node, &lo, &hi, text)
    }
}

/// The verdict for one revision: `text` hashed as Mercurial does (collision-detecting SHA-1 over the two
/// parent nodes, already in ascending order, then the raw text) must equal `node`. Factored out of
/// [`Revlog::verify_node`] so the collision and mismatch paths can be exercised without a store.
fn check_revision_hash(
    node: &[u8; 20],
    lo: &[u8; 20],
    hi: &[u8; 20],
    text: &[u8],
) -> Result<(), Error> {
    use sha1_checked::{CollisionResult, Digest, Sha1};

    let mut hasher = Sha1::new();
    Digest::update(&mut hasher, lo);
    Digest::update(&mut hasher, hi);
    Digest::update(&mut hasher, text);
    let result = hasher.try_finalize();
    if result.has_collision() {
        return Err(read_err(format!(
            "revision {} triggers SHA-1 collision detection (crafted store)",
            crate::util::hex(node)
        )));
    }
    let CollisionResult::Ok(digest) = result else {
        return Err(read_err(
            "SHA-1 collision detection reported an unexpected state",
        ));
    };
    if digest.as_slice() != node.as_slice() {
        return Err(read_err(format!(
            "revision {} does not match its content (corrupt or crafted store)",
            crate::util::hex(node)
        )));
    }
    Ok(())
}

/// Refuse a revision whose flags mean its stored text is not the text its node hashes, or that this reader
/// cannot interpret. Mirrors Mercurial's own `flagutil._processflagsfunc` (`revlogutils/flagutil.py`):
/// an unknown flag is refused there too ("incompatible revision flag").
fn check_flags(e: &Entry) -> Result<(), Error> {
    if e.is_censored() {
        return Err(Error::FloorRefusal {
            feature: floor::CENSORED_REVISION.to_string(),
            reason: "a censored revision's content was deliberately removed; refused rather than \
                     importing a hole as if it were content"
                .to_string(),
        });
    }
    if e.flags & REVIDX_ELLIPSIS != 0 {
        return Err(Error::FloorRefusal {
            feature: floor::ELLIPSIS_REVISION.to_string(),
            reason:
                "an ellipsis revision (from a narrow clone) does not hash to its text, so its node \
                     cannot be verified; refused rather than carrying an unverifiable identifier"
                    .to_string(),
        });
    }
    if e.flags & REVIDX_EXTSTORED != 0 {
        return Err(Error::FloorRefusal {
            feature: floor::EXTERNAL_STORAGE_REVISION.to_string(),
            reason: "an externally stored revision holds a pointer, not the text its node hashes, so its \
                     node cannot be verified; refused rather than carrying an unverifiable identifier"
                .to_string(),
        });
    }
    let unknown =
        e.flags & !(REVIDX_ISCENSORED | REVIDX_ELLIPSIS | REVIDX_EXTSTORED | HASH_NEUTRAL_FLAGS);
    if unknown != 0 {
        return Err(Error::FloorRefusal {
            feature: floor::UNKNOWN_REVISION_FLAG.to_string(),
            reason: format!(
                "a revision carries flag bits ({unknown:#06x}) that Mercurial does not define; refused \
                 rather than misread"
            ),
        });
    }
    Ok(())
}

/// Decompress one revlog chunk by its leading marker byte (verified against real stores):
/// `0x00` raw (byte 0 included) · `u` raw (byte 0 dropped) · `x` zlib · `0x28` zstd frame. Bounded at
/// `limits.max_revision_bytes` + 1 for zlib and zstd alike (RFC 010 CR-17): a compression bomb — a small
/// stored chunk that inflates to an enormous text — is refused, not allocated.
fn decompress(chunk: &[u8], limits: &Limits) -> Result<Vec<u8>, Error> {
    let Some(&first) = chunk.first() else {
        return Ok(Vec::new());
    };
    match first {
        0x00 => Ok(chunk.to_vec()),
        b'u' => Ok(chunk.get(1..).unwrap_or(&[]).to_vec()),
        b'x' => {
            let mut out = Vec::new();
            flate2::read::ZlibDecoder::new(chunk)
                .take(limits.max_revision_bytes + 1)
                .read_to_end(&mut out)
                .map_err(|e| read_err(format!("zlib: {e}")))?;
            if out.len() as u64 > limits.max_revision_bytes {
                return Err(resource_limit(
                    "a decompressed revision",
                    limits.max_revision_bytes,
                ));
            }
            Ok(out)
        }
        0x28 => {
            let mut dec = ruzstd::StreamingDecoder::new(chunk)
                .map_err(|e| read_err(format!("zstd init: {e}")))?;
            let mut out = Vec::new();
            (&mut dec)
                .take(limits.max_revision_bytes + 1)
                .read_to_end(&mut out)
                .map_err(|e| read_err(format!("zstd: {e}")))?;
            if out.len() as u64 > limits.max_revision_bytes {
                return Err(resource_limit(
                    "a decompressed revision",
                    limits.max_revision_bytes,
                ));
            }
            Ok(out)
        }
        other => Err(read_err(format!(
            "unknown revlog compression marker 0x{other:02x}"
        ))),
    }
}

/// Apply a Mercurial delta (`bdiff`/`mpatch` format) to `base`: a sequence of hunks, each
/// `start(u32be) end(u32be) len(u32be) data[len]`, replacing `base[start..end]` with `data`. The output
/// is bounded at `limits.max_revision_bytes` (RFC 010 CR-17), the same ceiling as decompression.
fn mpatch(base: &[u8], delta: &[u8], limits: &Limits) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(base.len());
    let mut pos = 0usize; // consumed position in base
    let mut i = 0usize; // position in delta
    while i < delta.len() {
        let start = u32_be(delta, i)? as usize;
        let end = u32_be(delta, i + 4)? as usize;
        let len = u32_be(delta, i + 8)? as usize;
        i += 12;
        let data = delta
            .get(i..i + len)
            .ok_or_else(|| read_err("delta hunk data truncated"))?;
        if start < pos || end > base.len() || start > end {
            return Err(read_err("delta hunk out of order or out of range"));
        }
        let keep = base
            .get(pos..start)
            .ok_or_else(|| read_err("delta base slice out of range"))?;
        out.extend_from_slice(keep);
        out.extend_from_slice(data);
        pos = end;
        i += len;
    }
    let tail = base
        .get(pos..)
        .ok_or_else(|| read_err("delta base tail out of range"))?;
    out.extend_from_slice(tail);
    if out.len() as u64 > limits.max_revision_bytes {
        return Err(resource_limit(
            "a patched revision",
            limits.max_revision_bytes,
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
