//! The content-addressed blob store (RFC 001 D-1, RFC 003 D-2/D-3; the file backing: RFC 015 D-3).
//!
//! File bytes are stored once per [`BlobId`] (SHA-256 of the bytes) — dedup, integrity, and determinism
//! for free. Path operations in the model reference blobs by id; the store owns the bytes.
//!
//! **Two backings, one surface.** The default, [`ContentStore::new`], keeps every blob in memory, as
//! before. [`ContentStore::spilling`] keeps them in a private spill file instead, holding only an
//! index (`BlobId` → offset, length) in memory. Both answer the same calls — `get`/`iter_sorted` read
//! **owned, fallible** bytes either way (RFC 015 D-3: a file-backed blob cannot be lent as `&[u8]`), so
//! no caller branches on which backing it has. A blob already present (the same id) is never written
//! twice, in either backing.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{Error, Result};

mod spill;

/// A content address: the SHA-256 of a blob's bytes (RFC 003 D-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobId(pub [u8; 32]);

impl BlobId {
    /// Compute the id of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    /// The 32 raw bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// A lowercase hex rendering, for inspection and reports.
    #[must_use]
    pub fn to_hex(&self) -> String {
        to_hex(&self.0)
    }
}

impl std::fmt::Display for BlobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Where a decode's content lives while building (RFC 015 D-3), chosen by the caller of a decoder — not
/// a decoder option: it cannot change the output, and review 076 ruled it must never appear in the IR
/// or its provenance (RFC 015 D-3a), where every recorded option is one that could. A decoder exposes a
/// `decode_with_storage` alongside its memory-backed `decode` shorthand.
#[derive(Debug, Clone, Default)]
pub enum Storage {
    /// Keep every blob in memory, as [`ContentStore::new`] does.
    #[default]
    Memory,
    /// Spill blobs to a private file in this directory, as [`ContentStore::spilling`] does.
    Spill(std::path::PathBuf),
}

/// A seekable, readable artifact source behind [`Backing::Artifact`] (RFC 015 D-4): a real file in
/// production ([`ContentStore::from_artifact_source`]'s usual caller, [`crate::artifact::from_file`]),
/// or an owned in-memory `Cursor<Vec<u8>>` in a fast test ([`crate::artifact::read_from`]). Boxed so
/// `Backing` stays a plain enum with no generic parameter to thread through `ContentStore`/`Ir`/every
/// caller; `'static` (the implicit default for a `dyn` bound with no lifetime given) so the box can
/// outlive the call that built it.
pub(crate) trait ArtifactSource: Read + Seek + std::fmt::Debug {}
impl<T: Read + Seek + std::fmt::Debug> ArtifactSource for T {}

/// Where a blob's bytes live: in memory, at an offset/length in the spill file, or at an offset/length
/// in an already content-digested artifact (RFC 015 D-4).
#[derive(Debug)]
enum Backing {
    Memory(BTreeMap<BlobId, Vec<u8>>),
    /// `RefCell`, not `&mut`: `get`/`iter_sorted` read (seek and read on the shared file handle)
    /// through `&self`, so a caller never needs mutable access just to read a blob back — the same
    /// shape the memory backing already offers. Single-threaded use only (brygge-ir makes no
    /// cross-thread claim for `Ir`/`ContentStore`); a borrow conflict would be a decoder bug, not
    /// input-dependent, so a panic on a held borrow is acceptable here.
    File {
        file: RefCell<std::fs::File>,
        index: BTreeMap<BlobId, (u64, u64)>,
        /// The spill file's directory (RFC 015 D-3a), kept only to name it in a write/read error —
        /// never read back from, and never part of equality or any other observable behavior.
        dir: PathBuf,
    },
    /// RFC 015 D-4: a blob's bytes live at `(offset, length)` in a **contract 0.2.0** artifact the
    /// streaming reader already verified once, in one sequential pass, without keeping a copy of any
    /// blob past that pass. Every later [`ContentStore::get`] re-hashes what it reads back and compares
    /// it with the `BlobId` — the one thing standing between a caller and bytes the source no longer
    /// actually holds ([`Error::ArtifactChanged`]).
    Artifact {
        source: RefCell<Box<dyn ArtifactSource>>,
        index: BTreeMap<BlobId, (u64, u64)>,
    },
    /// RFC 018 D-1/D-4: a blob's bytes live in a **contract 0.3.0** artifact's compressed blob frames.
    /// `index` maps a blob to (which frame, its offset in that frame's raw/decompressed bytes, its
    /// length); `frames` lists every blob frame the verifying pass found, in file order. `get` inflates
    /// the owning frame (bounded as the format page states) and re-hashes the blob slice, the same
    /// [`Error::ArtifactChanged`] check as [`Backing::Artifact`]. `cache` keeps the last few
    /// decompressed multi-blob frames (RFC 018 D-4's own bound: at most 8, at most 8 MiB) so that
    /// reading several blobs out of the same frame does not re-inflate it each time; a frame holding a
    /// single blob is never cached (nothing in it would be reused).
    ArtifactCompact {
        source: RefCell<Box<dyn ArtifactSource>>,
        frames: Vec<crate::artifact::frame::FrameLoc>,
        index: BTreeMap<BlobId, (usize, u64, u64)>,
        cache: RefCell<std::collections::VecDeque<(usize, Vec<u8>)>>,
    },
}

/// A set of blobs keyed by content address, in one of two backings (above). **Equality** is the set of
/// blob ids, not a byte comparison: a `BlobId` is the SHA-256 of its bytes, so two stores with the same
/// id set hold the same content (collision-resistant hashing; `Ir`'s own `PartialEq`, used by
/// `verify --against-source`, relies on exactly this). Iterates in sorted `BlobId` order (deterministic
/// — `VF-1`).
#[derive(Debug)]
pub struct ContentStore {
    backing: Backing,
}

impl Default for ContentStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for ContentStore {
    fn eq(&self, other: &Self) -> bool {
        self.ids().eq(other.ids())
    }
}
impl Eq for ContentStore {}

impl ContentStore {
    /// An empty, in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            backing: Backing::Memory(BTreeMap::new()),
        }
    }

    /// An empty store backed by a private spill file created fresh in `dir` (RFC 015 D-3/D-3a). The
    /// file is removed from `dir` immediately after opening (Unix) or opened with delete-on-close
    /// (Windows): it is never left behind in `dir`, however the process ends. `dir` must already exist.
    ///
    /// # Errors
    /// [`Error::Io`] if `dir` does not exist or is not a directory, or the spill file cannot be
    /// created.
    pub fn spilling(dir: &Path) -> Result<Self> {
        let file = spill::create(dir)?;
        Ok(Self {
            backing: Backing::File {
                file: RefCell::new(file),
                index: BTreeMap::new(),
                dir: dir.to_path_buf(),
            },
        })
    }

    /// An empty store in the given [`Storage`]: [`ContentStore::new`] for [`Storage::Memory`],
    /// [`ContentStore::spilling`] for [`Storage::Spill`].
    ///
    /// # Errors
    /// [`Error::Io`], as [`ContentStore::spilling`].
    pub fn for_storage(storage: &Storage) -> Result<Self> {
        match storage {
            Storage::Memory => Ok(Self::new()),
            Storage::Spill(dir) => Self::spilling(dir),
        }
    }

    /// A store backed by an already-verified artifact (RFC 015 D-4): `index` maps each blob to its
    /// `(offset, length)` in `source`, as the streaming reader's own sequential pass found them. Not
    /// public: only [`crate::artifact::read_from`]/[`crate::artifact::from_file`] build one, right after
    /// that pass, never before it.
    pub(crate) fn from_artifact_source(
        source: impl ArtifactSource + 'static,
        index: BTreeMap<BlobId, (u64, u64)>,
    ) -> Self {
        Self {
            backing: Backing::Artifact {
                source: RefCell::new(Box::new(source)),
                index,
            },
        }
    }

    /// A store backed by an already-verified **contract 0.3.0** artifact's compressed blob frames
    /// (RFC 018 D-4). As [`ContentStore::from_artifact_source`], but over [`Backing::ArtifactCompact`]:
    /// only [`crate::artifact::read_from`]/[`crate::artifact::from_file`] build one, right after the
    /// verifying pass, never before it.
    pub(crate) fn from_artifact_source_compact(
        source: impl ArtifactSource + 'static,
        frames: Vec<crate::artifact::frame::FrameLoc>,
        index: BTreeMap<BlobId, (usize, u64, u64)>,
    ) -> Self {
        Self {
            backing: Backing::ArtifactCompact {
                source: RefCell::new(Box::new(source)),
                frames,
                index,
                cache: RefCell::new(std::collections::VecDeque::new()),
            },
        }
    }

    /// Insert `bytes`, returning its content address. Inserting identical bytes twice is idempotent —
    /// the second insert neither writes nor allocates again.
    ///
    /// # Errors
    /// [`Error::Io`] if the blob cannot be appended to the spill file (for example, the disk is full).
    pub fn insert(&mut self, bytes: Vec<u8>) -> Result<BlobId> {
        let id = BlobId::of(&bytes);
        match &mut self.backing {
            Backing::Memory(blobs) => {
                blobs.entry(id).or_insert(bytes);
            }
            Backing::File { file, index, dir } => {
                if let std::collections::btree_map::Entry::Vacant(e) = index.entry(id) {
                    let mut f = file.borrow_mut();
                    let name = |e: std::io::Error| {
                        Error::io_msg(format!(
                            "cannot write to the spill file in {}: {e}",
                            dir.display()
                        ))
                    };
                    let offset = f.seek(SeekFrom::End(0)).map_err(name)?;
                    f.write_all(&bytes).map_err(name)?;
                    e.insert((offset, bytes.len() as u64));
                }
            }
            // An artifact-backed store is read-only (RFC 015 D-4): it represents a file the streaming
            // reader already verified, not an in-progress build — `IrBuilder` never holds one of these
            // (it is only ever produced by `artifact::read_from`/`from_file`, after the pass, as the
            // finished `Ir.content`), so this arm exists only to keep the match exhaustive.
            Backing::Artifact { .. } | Backing::ArtifactCompact { .. } => {
                return Err(Error::Invariant(
                    "cannot insert into an artifact-backed content store (read-only)".to_string(),
                ));
            }
        }
        Ok(id)
    }

    /// The bytes for `id`, if present.
    ///
    /// # Errors
    /// [`Error::Io`] if `id` is present but its bytes could not be read back from the spill file (for
    /// example, a truncated file).
    pub fn get(&self, id: &BlobId) -> Result<Option<Vec<u8>>> {
        match &self.backing {
            Backing::Memory(blobs) => Ok(blobs.get(id).cloned()),
            Backing::File { file, index, dir } => {
                let Some(&(offset, len)) = index.get(id) else {
                    return Ok(None);
                };
                let mut f = file.borrow_mut();
                let name = |e: std::io::Error| {
                    Error::io_msg(format!(
                        "cannot read the spill file in {}: {e}",
                        dir.display()
                    ))
                };
                f.seek(SeekFrom::Start(offset)).map_err(name)?;
                let mut buf =
                    vec![0u8; usize::try_from(len).map_err(|e| Error::io_msg(e.to_string()))?];
                f.read_exact(&mut buf).map_err(name)?;
                Ok(Some(buf))
            }
            Backing::Artifact { source, index } => {
                let Some(&(offset, len)) = index.get(id) else {
                    return Ok(None);
                };
                let mut s = source.borrow_mut();
                s.seek(SeekFrom::Start(offset)).map_err(Error::io)?;
                let mut buf =
                    vec![0u8; usize::try_from(len).map_err(|e| Error::io_msg(e.to_string()))?];
                s.read_exact(&mut buf).map_err(Error::io)?;
                // RFC 015 D-4: the verifying pass hashed these bytes once and moved on without keeping
                // them; this is the only re-check standing between a caller and bytes the source no
                // longer actually holds at that offset.
                if BlobId::of(&buf) != *id {
                    return Err(Error::ArtifactChanged(format!(
                        "blob {id} no longer matches its content; the artifact changed after it was \
                         verified"
                    )));
                }
                Ok(Some(buf))
            }
            Backing::ArtifactCompact {
                source,
                frames,
                index,
                cache,
            } => {
                let Some(&(frame_index, offset, len)) = index.get(id) else {
                    return Ok(None);
                };
                let frame = frames.get(frame_index).ok_or_else(|| {
                    Error::ArtifactChanged(format!("blob {id}'s frame no longer exists"))
                })?;
                let raw = if len == frame.raw_len {
                    // A single-blob frame: nothing to cache for a later blob (RFC 018 D-4).
                    crate::artifact::frame::inflate_frame(&mut *source.borrow_mut(), frame)?
                } else {
                    let mut c = cache.borrow_mut();
                    if let Some((_, raw)) = c.iter().find(|(idx, _)| *idx == frame_index) {
                        raw.clone()
                    } else {
                        let raw = crate::artifact::frame::inflate_frame(
                            &mut *source.borrow_mut(),
                            frame,
                        )?;
                        if c.len() >= 8 {
                            c.pop_front();
                        }
                        c.push_back((frame_index, raw.clone()));
                        raw
                    }
                };
                let start = usize::try_from(offset).map_err(|e| Error::io_msg(e.to_string()))?;
                let end = start + usize::try_from(len).map_err(|e| Error::io_msg(e.to_string()))?;
                let buf = raw
                    .get(start..end)
                    .ok_or_else(|| {
                        Error::ArtifactChanged(format!(
                            "blob {id}'s frame no longer holds enough bytes"
                        ))
                    })?
                    .to_vec();
                if BlobId::of(&buf) != *id {
                    return Err(Error::ArtifactChanged(format!(
                        "blob {id} no longer matches its content; the artifact changed after it was \
                         verified"
                    )));
                }
                Ok(Some(buf))
            }
        }
    }

    /// `id`'s length, without reading its bytes (RFC 018 D-1): the writer needs every blob's length to
    /// plan the blob table and the frame packing before it reads any blob's content.
    pub(crate) fn blob_len(&self, id: &BlobId) -> Option<u64> {
        match &self.backing {
            Backing::Memory(blobs) => blobs.get(id).map(|b| b.len() as u64),
            Backing::File { index, .. } | Backing::Artifact { index, .. } => {
                index.get(id).map(|&(_, len)| len)
            }
            Backing::ArtifactCompact { index, .. } => index.get(id).map(|&(_, _, len)| len),
        }
    }

    /// True when `id` is present.
    #[must_use]
    pub fn contains(&self, id: &BlobId) -> bool {
        match &self.backing {
            Backing::Memory(blobs) => blobs.contains_key(id),
            Backing::File { index, .. } | Backing::Artifact { index, .. } => index.contains_key(id),
            Backing::ArtifactCompact { index, .. } => index.contains_key(id),
        }
    }

    /// The number of distinct blobs.
    #[must_use]
    pub fn len(&self) -> usize {
        match &self.backing {
            Backing::Memory(blobs) => blobs.len(),
            Backing::File { index, .. } | Backing::Artifact { index, .. } => index.len(),
            Backing::ArtifactCompact { index, .. } => index.len(),
        }
    }

    /// True when the store holds no blobs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total content bytes across all blobs (for the fidelity report). Known from the index alone —
    /// reading no blob back.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        match &self.backing {
            Backing::Memory(blobs) => blobs.values().map(|b| b.len() as u64).sum(),
            Backing::File { index, .. } | Backing::Artifact { index, .. } => {
                index.values().map(|&(_, len)| len).sum()
            }
            Backing::ArtifactCompact { index, .. } => index.values().map(|&(_, _, len)| len).sum(),
        }
    }

    /// Every blob id, in sorted order — the canonical order for serialization and the digest. Reading
    /// a blob's bytes is a separate [`ContentStore::get`] per id (RFC 015 D-3: at most one blob is ever
    /// in memory at a time this way).
    pub fn ids(&self) -> impl Iterator<Item = &BlobId> {
        match &self.backing {
            Backing::Memory(blobs) => Either::Left(blobs.keys()),
            Backing::File { index, .. } | Backing::Artifact { index, .. } => {
                Either::Right(Either::Left(index.keys()))
            }
            Backing::ArtifactCompact { index, .. } => Either::Right(Either::Right(index.keys())),
        }
    }

    /// Keep only the blobs `keep` accepts, dropping the rest from the **index** — no blob already
    /// written is copied, re-hashed, or moved (RFC 015 D-2: `IrBuilder::finish` used to prune by
    /// building a second store with `insert`, a full copy of every referenced blob while the old store
    /// was still alive). For a file-backed store, the unreferenced bytes stay physically in the spill
    /// file — harmless, since the whole file is temporary and never read except by id through the
    /// index — but are no longer indexed, so `write_to` never streams them. An artifact-backed store is
    /// never pruned (it is already finished, never passed to `IrBuilder::finish`); the arm exists only
    /// to keep the match exhaustive.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&BlobId) -> bool) {
        match &mut self.backing {
            Backing::Memory(blobs) => blobs.retain(|id, _| keep(id)),
            Backing::File { index, .. } | Backing::Artifact { index, .. } => {
                index.retain(|id, _| keep(id));
            }
            Backing::ArtifactCompact { index, .. } => {
                index.retain(|id, _| keep(id));
            }
        }
    }
}

/// Two iterator shapes with one static type, so [`ContentStore::ids`] can return either backing's
/// `keys()` without boxing (both yield `&BlobId` in the same sorted order; a `BTreeMap`'s `keys()` is
/// already that).
enum Either<L, R> {
    Left(L),
    Right(R),
}

impl<'a, L, R> Iterator for Either<L, R>
where
    L: Iterator<Item = &'a BlobId>,
    R: Iterator<Item = &'a BlobId>,
{
    type Item = &'a BlobId;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Left(l) => l.next(),
            Self::Right(r) => r.next(),
        }
    }
}

/// Lowercase-hex a byte slice without a dependency (and without indexing).
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        // Writing to a String is infallible; the discarded Result carries only `fmt::Error`.
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests;
