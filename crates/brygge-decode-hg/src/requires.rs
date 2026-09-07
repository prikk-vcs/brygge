//! The format-safety gate (RFC 005 §1, the "refuse rather than misread" spine).
//!
//! `.hg/requires` (and, under `share-safe`, `.hg/store/requires`) lists the format requirements a
//! repository relies on. This gate reads them and **refuses any requirement this build does not
//! implement** — a format brygge cannot read correctly is refused with a named reason, never guessed at.
//! It runs before a single revlog byte is parsed, so an unsupported store can never be half-read.

use crate::Error;

/// Requirements this build's revlog reader implements (or can safely ignore). Anything outside this set
/// is refused (either as a floor feature or as an unreadable format).
const SUPPORTED: &[&str] = &[
    "revlogv1",                // the format this reader implements
    "store",                   // objects under .hg/store (assumed; the only layout read)
    "fncache",                 // filelog path encoding — handled by the reader
    "dotencode",               // additional path encoding — handled by the reader
    "generaldelta",            // delta base is an arbitrary prior rev — handled
    "sparserevlog",            // affects delta-chain selection only; reading is unchanged
    "revlog-compression-zlib", // explicit zlib (also the default when unstated)
    "persistent-nodemap",      // an auxiliary index file; the revlog is read without it
    "share-safe",              // requires may live in .hg/store/requires; handled by the reader
];

/// Requirements that are a **floor refusal** (`FA-3`, RFC 005 D-4): the feature is understood but
/// deliberately out of scope. Each maps to a human label.
const FLOOR: &[(&str, &str)] = &[
    ("largefiles", "largefiles large-file storage"),
    ("lfs", "git-lfs-style large-file storage"),
];

/// Check a `.hg/requires` body. Returns `Ok(())` only if every requirement is in [`SUPPORTED`].
///
/// # Errors
/// [`Error::FloorRefusal`] for a known-but-out-of-scope feature ([`FLOOR`]); [`Error::UnsupportedFormat`]
/// for a format this reader does not implement (zstd/revlogv2/treemanifest/narrow) or any unknown
/// requirement — refused rather than misread.
pub fn check(requires_body: &str) -> Result<(), Error> {
    for line in requires_body.lines() {
        let req = line.trim();
        if req.is_empty() {
            continue;
        }
        if SUPPORTED.contains(&req) {
            continue;
        }
        if let Some((_, label)) = FLOOR.iter().find(|(k, _)| *k == req) {
            return Err(Error::FloorRefusal {
                feature: req.to_string(),
                reason: format!(
                    "{label} is refused rather than approximated (RFC 005 D-4, owner-ratified floor)"
                ),
            });
        }
        let reason = match req {
            "revlog-compression-zstd" => {
                "zstd revlog compression is not read by this build (zlib only for M2); \
                 refused rather than misread"
            }
            "revlogv2" | "changelogv2" | "exp-revlogv2.2" | "exp-revlogv2.1" | "exp-revlogv2.0" => {
                "revlogv2-family formats are a different on-disk layout this reader does not implement"
            }
            "treemanifest" => {
                "tree manifests are a different manifest layout this reader does not implement"
            }
            "narrowhg" | "narrowhg-lightweight" | "narrow" => {
                "narrow (partial) clones are not a complete history; refused rather than importing a \
                 truncated view as if whole (FA-1)"
            }
            _ => "unknown repository requirement; refused rather than misread (format-safety gate)",
        };
        return Err(Error::UnsupportedFormat {
            requirement: req.to_string(),
            reason: reason.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
