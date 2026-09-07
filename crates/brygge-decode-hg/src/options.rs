//! Decoder options (RFC 005 D-3, CF-01). Every option that can change the output is recorded into the IR
//! provenance (`PR-5`). The defaults are the maximally-honest ones: **no rename inference** — Mercurial's
//! *source-recorded* renames are carried as `Stated` regardless of this flag (SRC-H2); this flag governs
//! only whether brygge additionally *infers* renames hg did not record.

use std::collections::BTreeMap;

/// How the Mercurial decoder should behave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Infer renames hg did **not** record, and mark them `Derived` (parity with the Git decoder,
    /// RFC 004 D-3 / OQ-A). **Off by default.** Source-recorded (`hg mv`/`hg cp`) renames are always
    /// carried as `Stated` and are unaffected by this flag.
    pub detect_renames: bool,
    /// The similarity percentage recorded as the rename parameter when inference is on (exact-content
    /// only for now, so `100`).
    pub rename_threshold: u8,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            detect_renames: false,
            rename_threshold: 100,
        }
    }
}

impl Options {
    /// Render the options as canonical, sorted key→value strings for the IR provenance (`PR-5/CF-01`).
    #[must_use]
    pub fn as_params(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert(
            "detect_renames".to_string(),
            self.detect_renames.to_string(),
        );
        if self.detect_renames {
            m.insert(
                "rename_algorithm".to_string(),
                "exact-content-move".to_string(),
            );
            m.insert(
                "rename_threshold".to_string(),
                self.rename_threshold.to_string(),
            );
        }
        m
    }
}
