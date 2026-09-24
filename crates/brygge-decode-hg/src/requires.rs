//! The format-safety gate (RFC 005 §1, the "refuse rather than misread" spine).
//!
//! `.hg/requires` (and, under `share-safe`, `.hg/store/requires`) lists the format requirements a
//! repository relies on. This gate reads them and **refuses any requirement this build does not
//! implement** — a format brygge cannot read correctly is refused with a named reason, never guessed at.
//! It runs before a single revlog byte is parsed, so an unsupported store can never be half-read.

use crate::{Error, floor};

/// Requirements this build's revlog reader implements (or can safely ignore). Anything outside this set
/// is refused (either as a floor feature or as an unreadable format).
const SUPPORTED: &[&str] = &[
    "revlogv1",                // the format this reader implements
    "store",                   // objects under .hg/store (the only layout read)
    "fncache",      // filelog path encoding; required: a store without it is refused below
    "dotencode",    // leading `.`/space path encoding; read from here (RFC 013 D-1)
    "generaldelta", // delta base is an arbitrary prior rev — handled
    "sparserevlog", // affects delta-chain selection only; reading is unchanged
    "revlog-compression-zlib", // explicit zlib (also the default when unstated)
    "revlog-compression-zstd", // zstd chunks — read via the pure-Rust ruzstd decoder
    "persistent-nodemap", // an auxiliary index file; the revlog is read without it
    "share-safe",   // requires may live in .hg/store/requires; handled by the reader
];

/// Requirements that are a **floor refusal** (`FA-3`, RFC 005 D-4): the feature is understood but
/// deliberately out of scope. Each maps to a human label.
const FLOOR: &[(&str, &str)] = &[
    (floor::LARGEFILES, "largefiles large-file storage"),
    (floor::LFS, "git-lfs-style large-file storage"),
];

/// How this repository's filelog file names are encoded (RFC 013 D-1), read from its requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreEncoding {
    /// `dotencode`: a leading `.` or space in a path component is encoded (`~2e` / `~20`). Every repository
    /// since Mercurial 1.7 has it; a `fncache` repository without it does not.
    pub dotencode: bool,
}

/// Check a `.hg/requires` body. Returns the store encoding only if every requirement is in [`SUPPORTED`] and
/// the store is an `fncache` store.
///
/// # Errors
/// [`Error::FloorRefusal`] for a known-but-out-of-scope feature ([`FLOOR`]); [`Error::UnsupportedFormat`]
/// for a format this reader does not implement (zstd/revlogv2/treemanifest/narrow) or any unknown
/// requirement, and for a store **without `fncache`** (Mercurial before 1.1, 2008, whose file names are
/// encoded differently) — refused rather than misread.
pub fn check(requires_body: &str) -> Result<StoreEncoding, Error> {
    let mut fncache = false;
    let mut dotencode = false;
    for line in requires_body.lines() {
        let req = line.trim();
        if req.is_empty() {
            continue;
        }
        if SUPPORTED.contains(&req) {
            fncache |= req == "fncache";
            dotencode |= req == "dotencode";
            continue;
        }
        if let Some((_, label)) = FLOOR.iter().find(|(k, _)| *k == req) {
            return Err(Error::FloorRefusal {
                feature: req.to_string(),
                reason: format!("{label} is refused rather than approximated"),
            });
        }
        let reason = match req {
            "revlogv2" | "changelogv2" | "exp-revlogv2.2" | "exp-revlogv2.1" | "exp-revlogv2.0" => {
                "revlogv2-family formats are a different on-disk layout this reader does not implement"
            }
            "treemanifest" => {
                "tree manifests are a different manifest layout this reader does not implement"
            }
            "narrowhg" | "narrowhg-lightweight" | "narrow" => {
                "narrow (partial) clones are not a complete history; refused rather than importing a \
                 truncated view as if whole"
            }
            _ => "unknown repository requirement; refused rather than misread",
        };
        return Err(Error::UnsupportedFormat {
            requirement: req.to_string(),
            reason: reason.to_string(),
        });
    }
    if !fncache {
        return Err(Error::UnsupportedFormat {
            requirement: "store-without-fncache".to_string(),
            reason: "a store without `fncache` (Mercurial before 1.1) encodes its file names differently \
                     from every later one, which this reader does not implement; refused rather than \
                     misread. Convert a copy with a current Mercurial (`hg clone --pull` writes a current store), \
                     then decode the copy"
                .to_string(),
        });
    }
    Ok(StoreEncoding { dotencode })
}

#[cfg(test)]
mod tests;
