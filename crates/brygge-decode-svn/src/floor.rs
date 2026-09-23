//! The Subversion decoder's floor (RFC 006 D-4, CF-03): the owner-ratified list of features refused
//! rather than approximated. **One declared list** — every refusal site names its feature from here,
//! never a duplicated string literal — and the same list is what `decode()` records into provenance as
//! `params["floor"]`, so a reviewer reads from the artifact which floor applied (PR-5). Changing this
//! list is a reviewed code change, not a runtime knob: no caller can turn a floor item off.

pub(crate) const REMOTE_SOURCE: &str = "remote-source";

/// Every refused feature, in the order this module declares them. This is the list recorded into
/// provenance (`params["floor"]`) and the list an architect review changes to change the floor.
/// `svn:externals` is not redeclared here — [`crate::props::SVN_EXTERNALS`] (the property-key constant
/// `check_externals` already refuses by) is the one source of truth for that string.
pub(crate) const ALL: &[&str] = &[crate::props::SVN_EXTERNALS, REMOTE_SOURCE];

/// The floor as a single comma-joined string, for `params["floor"]` (PR-5).
pub(crate) fn joined() -> String {
    ALL.join(",")
}
