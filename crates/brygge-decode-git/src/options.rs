//! Decoder options (RFC 004 D-3, CF-01). Every option that can change the output is recorded into the
//! IR provenance (`PR-5`), so the result stays reproducible and the judgement reviewable.

use std::collections::BTreeMap;

/// How the decoder should behave. The defaults are the maximally-honest ones: **no rename inference**
/// (RFC 004 D-3), so a default import is entirely source-*Stated*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Infer renames from identical content and mark them **Derived** (RFC 004 D-3). **Off by default.**
    /// When on, a delete+add of the *same blob* within one commit also emits a marked
    /// `Derived(InferredRename)` hint — beside, never replacing, the literal delete+add.
    pub detect_renames: bool,
    /// The similarity percentage recorded as the rename parameter. M1 detects only exact-content moves,
    /// so this is `100`; it exists so the recorded parameter is explicit and future thresholds fit.
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
