//! The Mercurial decoder's floor: the owner-ratified list of features refused rather than approximated.
//! **One declared list** — every refusal site names its feature from here, never a duplicated string
//! literal — and the same list is what `decode()` records into provenance as `params["floor"]`, so a
//! reviewer reads from the artifact which floor applied. Changing this list is a reviewed code change,
//! not a runtime knob: no caller can turn a floor item off.
//!
//! Every feature is an **identifier**: lowercase ASCII, kebab-case, stable, and the same concept uses the
//! same identifier in every decoder (release-prep handoff §3). The human refusal message carries the
//! explanation and the remedy; the identifier is what machine output and provenance carry.

pub(crate) const SUBREPO: &str = "subrepo";
pub(crate) const LARGEFILES: &str = "largefiles";
pub(crate) const LFS: &str = "lfs";
pub(crate) const CENSORED_REVISION: &str = "censored-revision";
/// A repository with an unresolved merge in progress (`.hg/merge/state2` present): simpler and more
/// honest than parsing the merge state to pin its local and other nodes.
pub(crate) const UNFINISHED_MERGE: &str = "unfinished-merge";
/// A changelog extras key that is not valid UTF-8 (an `Extra` label must be text).
pub(crate) const NON_UTF8_EXTRA_KEY: &str = "non-utf8-extra-key";
/// A manifest path that is not valid UTF-8 (IR paths are text; no lossy conversion is offered).
pub(crate) const NON_UTF8_PATH: &str = "non-utf8-path";
/// A revision flagged as an *ellipsis* revision (narrow clones): Mercurial itself treats its hash as not
/// matching its text, so the stored node cannot be verified.
pub(crate) const ELLIPSIS_REVISION: &str = "ellipsis-revision";
/// A revision flagged as externally stored (large-file extensions): the revlog holds a pointer, not the
/// hashed text, so the stored node cannot be verified against what is stored.
pub(crate) const EXTERNAL_STORAGE_REVISION: &str = "external-storage-revision";
/// A revision carrying a flag bit Mercurial does not define: Mercurial itself refuses such a revision
/// ("incompatible revision flag"), and so does this reader.
pub(crate) const UNKNOWN_REVISION_FLAG: &str = "unknown-revision-flag";

/// Every refused feature, in the order this module declares them. This is the list recorded into
/// provenance (`params["floor"]`) and the list an architect review changes to change the floor.
pub(crate) const ALL: &[&str] = &[
    SUBREPO,
    LARGEFILES,
    LFS,
    CENSORED_REVISION,
    UNFINISHED_MERGE,
    NON_UTF8_EXTRA_KEY,
    NON_UTF8_PATH,
    ELLIPSIS_REVISION,
    EXTERNAL_STORAGE_REVISION,
    UNKNOWN_REVISION_FLAG,
];

/// The floor as a single comma-joined string, for `params["floor"]`.
pub(crate) fn joined() -> String {
    ALL.join(",")
}

#[cfg(test)]
mod tests;
