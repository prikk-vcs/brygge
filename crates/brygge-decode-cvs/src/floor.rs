//! The CVS decoder's floor (RFC 007 D-4, CF-03): the owner-ratified list of features refused rather than
//! approximated. **One declared list** — every refusal site names its feature from here, never a
//! duplicated string literal — and the same list is what `decode()` records into provenance as
//! `params["floor"]`, so a reviewer reads from the artifact which floor applied (PR-5). Changing this
//! list is a reviewed code change, not a runtime knob: no caller can turn a floor item off.

pub(crate) const REMOTE_SOURCE: &str = "remote-source";
pub(crate) const WHOLE_IMPORT_UNDER_CONFIDENCE_FLOOR: &str = "whole-import-under-confidence-floor";
/// The same repo-relative path exists as both `dir/f,v` (live) and `dir/Attic/f,v` (mainline-deleted) —
/// an inconsistent repository (corrections handoff §2.4/CR-08.3).
pub(crate) const PATH_IN_ATTIC_AND_LIVE: &str = "path-in-attic-and-live";
/// A symlink anywhere under the repository root, file or directory (corrections handoff §2.4/CR-08.4):
/// brygge reads only the repository it is given.
pub(crate) const SYMLINK_IN_REPOSITORY: &str = "symlink-in-repository";
/// A path component that is not valid UTF-8 (corrections handoff §2.4/CR-03): no lossy conversion
/// anywhere on a path.
pub(crate) const NON_UTF8_PATH: &str = "non-utf8-path";
/// A symbol (tag or branch) name that is not valid UTF-8 (corrections handoff §2.4, review 008 R-4): no
/// lossy conversion anywhere identity- or reference-bearing.
pub(crate) const NON_UTF8_SYMBOL_NAME: &str = "non-utf8-symbol-name";
/// A file whose default (vendor) `branch` is set, but which also has trunk revisions after that branch's
/// branch point (`cvs admin -b`) — its main line is ambiguous, and brygge refuses to guess (corrections
/// handoff §2.4, review 008 R-5).
pub(crate) const DEFAULT_BRANCH_WITH_LATER_TRUNK: &str = "default-branch-with-later-trunk";

/// Every refused feature, in the order this module declares them. This is the list recorded into
/// provenance (`params["floor"]`) and the list an architect review changes to change the floor.
pub(crate) const ALL: &[&str] = &[
    REMOTE_SOURCE,
    WHOLE_IMPORT_UNDER_CONFIDENCE_FLOOR,
    PATH_IN_ATTIC_AND_LIVE,
    SYMLINK_IN_REPOSITORY,
    NON_UTF8_PATH,
    NON_UTF8_SYMBOL_NAME,
    DEFAULT_BRANCH_WITH_LATER_TRUNK,
];

/// The floor as a single comma-joined string, for `params["floor"]` (PR-5).
pub(crate) fn joined() -> String {
    ALL.join(",")
}
