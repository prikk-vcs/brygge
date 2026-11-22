//! Decoder options (RFC 004 D-3, CF-01). Every option that can change the output is recorded into the
//! IR provenance (`PR-5`), so the result stays reproducible and the judgement reviewable.

use std::collections::BTreeMap;

/// How the decoder should behave. The defaults are the maximally-honest ones: **no rename inference**
/// (RFC 004 D-3), so a default import is entirely source-*Stated*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Infer renames from identical content and mark them **Derived** (RFC 004 D-3). **Off by default.**
    /// When on, a delete+add of the *same blob* within one commit also emits a marked
    /// `Derived(InferredRename)` hint — beside, never replacing, the literal delete+add. CLI flag
    /// `--infer-renames` (handoff `cli-and-verify-handoff-v2.md`).
    pub infer_renames: bool,
    /// The similarity percentage recorded as the rename parameter. M1 detects only exact-content moves,
    /// so this is `100`; it exists so the recorded parameter is explicit and future thresholds fit.
    pub rename_threshold: u8,
    /// Ref name prefixes to carry **besides** `refs/heads/*` and `refs/tags/*` (CR-05's own namespace
    /// policy, otherwise unchanged): each becomes an `Other`-kind IR ref, named by the part of the ref
    /// name after the prefix, and contributes a walk tip like any other carried ref. **Empty by
    /// default**, and not exposed on brygge's CLI — this exists for RFC 019 D-6's own check
    /// (`brygge encode git`'s internal comparison and `verify --against-target git`), which needs to see
    /// a commit reachable only from `refs/brygge/other/*` (a `--branch-lines` dead/earlier-life root) to
    /// compare it, not to change what an ordinary `decode git` import carries. A prefix must end with
    /// `/`, or it matches no ref (never silently widened to a substring match).
    pub carry_extra_namespaces: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            infer_renames: false,
            rename_threshold: 100,
            carry_extra_namespaces: Vec::new(),
        }
    }
}

impl Options {
    /// Render the options as canonical, sorted key→value strings for the IR provenance (`PR-5/CF-01`).
    #[must_use]
    pub fn as_params(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert("infer_renames".to_string(), self.infer_renames.to_string());
        if self.infer_renames {
            m.insert(
                "rename_algorithm".to_string(),
                "exact-content-move".to_string(),
            );
            m.insert(
                "rename_threshold".to_string(),
                self.rename_threshold.to_string(),
            );
        }
        if !self.carry_extra_namespaces.is_empty() {
            m.insert(
                "carry_extra_namespaces".to_string(),
                self.carry_extra_namespaces.join(","),
            );
        }
        m
    }
}
