//! Decoder options (RFC 007 D-3/D-8, CF-01). Every option that can change the output is recorded into the
//! IR provenance (`PR-5`). The defaults are the maximally-honest ones: ref reconstruction **off**, a fixed
//! clustering window, and a confidence floor that flags/refuses only genuinely ambiguous changesets.

use std::collections::BTreeMap;

/// How the CVS decoder should behave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The changeset clustering window, in seconds: per-file revisions sharing `(author, log)` whose
    /// timestamps fall within this window are grouped into one changeset (SRC-C1, D-3). Recorded in
    /// provenance (PR-5).
    pub window_secs: u64,
    /// The confidence floor in `0..=100` (RFC 007 D-8/OQ-B): a reconstructed changeset scoring below it is
    /// loudly flagged/refused with a named reason rather than imported as if certain.
    pub confidence_floor: u8,
    /// Reconstruct tag/branch refs from per-file symbolic names, marking each `Derived` (RFC 007 D-5).
    /// **Off by default** — with it off, only the (derived) changeset spine and its faithful per-file ops
    /// are produced.
    pub reconstruct_refs: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            // A generous default window (cvs2svn's lineage uses a few minutes); recorded so a reader can
            // reproduce or review the grouping (PR-5).
            window_secs: 180,
            confidence_floor: 50,
            reconstruct_refs: false,
        }
    }
}

impl Options {
    /// Render the options as canonical, sorted key→value strings for the IR provenance (`PR-5/CF-01`).
    #[must_use]
    pub fn as_params(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert("window_secs".to_string(), self.window_secs.to_string());
        m.insert(
            "confidence_floor".to_string(),
            self.confidence_floor.to_string(),
        );
        m.insert(
            "reconstruct_refs".to_string(),
            self.reconstruct_refs.to_string(),
        );
        m.insert("cluster_keys".to_string(), "author,log".to_string());
        m
    }
}
