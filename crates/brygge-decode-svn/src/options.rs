//! Decoder options (RFC 006 D-4, CF-01). Every option that can change the output is recorded into the IR
//! provenance (`PR-5`). The default is the maximally-honest one: **branch/tag reconstruction off** — the
//! stated linear spine always imports; ref synthesis is an opt-in `Derived` layer (RFC 006 D-4).

use std::collections::BTreeMap;

/// The trunk/branches/tags layout convention used when reconstructing branch/tag refs (RFC 006 D-4/OQ-C).
/// The assumed convention is recorded as a derivation parameter (`PR-5`) on every reconstructed ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutPolicy {
    /// The trunk directory (repo-relative, no trailing slash), e.g. `"trunk"`.
    pub trunk: String,
    /// The branches container directory, e.g. `"branches"` (each child directory is one branch).
    pub branches: String,
    /// The tags container directory, e.g. `"tags"` (each child directory is one tag).
    pub tags: String,
}

impl Default for LayoutPolicy {
    fn default() -> Self {
        Self {
            trunk: "trunk".to_string(),
            branches: "branches".to_string(),
            tags: "tags".to_string(),
        }
    }
}

impl LayoutPolicy {
    /// A stable label for the convention, recorded in the derivation params (`PR-5`).
    #[must_use]
    pub fn label(&self) -> String {
        format!(
            "trunk={},branches={},tags={}",
            self.trunk, self.branches, self.tags
        )
    }

    /// Parse a [`label`](Self::label) back into a policy — for reproducing an import from its recorded
    /// provenance (`verify --against-source`). `None` if the label is not in `label` form.
    #[must_use]
    pub fn from_label(s: &str) -> Option<Self> {
        let (mut trunk, mut branches, mut tags) = (None, None, None);
        for part in s.split(',') {
            let (k, v) = part.split_once('=')?;
            match k {
                "trunk" => trunk = Some(v.to_string()),
                "branches" => branches = Some(v.to_string()),
                "tags" => tags = Some(v.to_string()),
                _ => {}
            }
        }
        Some(Self {
            trunk: trunk?,
            branches: branches?,
            tags: tags?,
        })
    }
}

/// How the Subversion decoder should behave.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    /// Reconstruct branch/tag refs from directory-copy convention, marking each `Derived` (RFC 006 D-4).
    /// **Off by default** — with it off, only the `Stated` linear spine (revisions + literal directory
    /// operations) is produced. Turning it on never yields an *unmarked* branch (INV-1): every
    /// reconstructed ref is `Derived`, and a convention-violating layout is recorded loudly, not guessed.
    pub reconstruct_refs: bool,
    /// The layout convention to assume when `reconstruct_refs` is on (RFC 006 OQ-C).
    pub layout: LayoutPolicy,
}

impl Options {
    /// Render the options as canonical, sorted key→value strings for the IR provenance (`PR-5/CF-01`).
    #[must_use]
    pub fn as_params(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert(
            "reconstruct_refs".to_string(),
            self.reconstruct_refs.to_string(),
        );
        if self.reconstruct_refs {
            m.insert("layout".to_string(), self.layout.label());
        }
        m
    }
}
