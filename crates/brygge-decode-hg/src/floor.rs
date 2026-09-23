//! The Mercurial decoder's floor (RFC 005 D-4, CF-03): the owner-ratified list of features refused
//! rather than approximated. **One declared list** — every refusal site names its feature from here,
//! never a duplicated string literal — and the same list is what `decode()` records into provenance as
//! `params["floor"]`, so a reviewer reads from the artifact which floor applied (PR-5). Changing this
//! list is a reviewed code change, not a runtime knob: no caller can turn a floor item off.

pub(crate) const SUBREPO: &str = "subrepo";
pub(crate) const LARGEFILES: &str = "largefiles";
pub(crate) const LFS: &str = "lfs";
pub(crate) const CENSORED_REVISION: &str = "censored revision";
/// A repository with an unresolved merge in progress (`.hg/merge/state2` present) — corrections handoff
/// §1.3 (review 009 R-4): simpler and more honest than parsing mergestate to pin its local/other nodes.
pub(crate) const UNFINISHED_MERGE: &str = "unfinished merge";
/// A changelog extras key that is not valid UTF-8 — corrections handoff §3 (review 009 R-2).
pub(crate) const NON_UTF8_EXTRA_KEY: &str = "non-UTF-8 extra key";

/// Every refused feature, in the order this module declares them. This is the list recorded into
/// provenance (`params["floor"]`) and the list an architect review changes to change the floor.
pub(crate) const ALL: &[&str] = &[
    SUBREPO,
    LARGEFILES,
    LFS,
    CENSORED_REVISION,
    UNFINISHED_MERGE,
    NON_UTF8_EXTRA_KEY,
];

/// The floor as a single comma-joined string, for `params["floor"]` (PR-5).
pub(crate) fn joined() -> String {
    ALL.join(",")
}
