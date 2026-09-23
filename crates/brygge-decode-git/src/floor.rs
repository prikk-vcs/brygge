//! The Git decoder's floor (RFC 004 D-4, CF-03): the owner-ratified list of features refused rather than
//! approximated. **One declared list** — every refusal site names its feature from here, never a
//! duplicated string literal — and the same list is what `decode()` records into provenance as
//! `params["floor"]`, so a reviewer reads from the artifact which floor applied (PR-5). Changing this
//! list is a reviewed code change, not a runtime knob: no caller can turn a floor item off.

pub(crate) const SUBMODULE: &str = "submodule";
pub(crate) const REPLACE_REF: &str = "replace ref";
pub(crate) const GRAFTS: &str = "grafts";
pub(crate) const SHALLOW_CLONE: &str = "shallow clone";
pub(crate) const OBJECT_ALTERNATES: &str = "object alternates";
pub(crate) const REDIRECTED_GIT_DIRECTORY: &str = "redirected git directory";
pub(crate) const NON_UTF8_PATH: &str = "non-UTF-8 path";
pub(crate) const NON_UTF8_REF_NAME: &str = "non-UTF-8 ref name";

/// Every refused feature, in the order this module declares them. This is the list recorded into
/// provenance (`params["floor"]`) and the list an architect review changes to change the floor.
pub(crate) const ALL: &[&str] = &[
    SUBMODULE,
    REPLACE_REF,
    GRAFTS,
    SHALLOW_CLONE,
    OBJECT_ALTERNATES,
    REDIRECTED_GIT_DIRECTORY,
    NON_UTF8_PATH,
    NON_UTF8_REF_NAME,
];

/// The floor as a single comma-joined string, for `params["floor"]` (PR-5).
pub(crate) fn joined() -> String {
    ALL.join(",")
}
