//! The Subversion decoder's floor (RFC 006 D-4, CF-03): the owner-ratified list of features refused
//! rather than approximated. **One declared list** — every refusal site names its feature from here,
//! never a duplicated string literal — and the same list is what `decode()` records into provenance as
//! `params["floor"]`, so a reviewer reads from the artifact which floor applied (PR-5). Changing this
//! list is a reviewed code change, not a runtime knob: no caller can turn a floor item off.

/// `svn:externals` on any node: it references other repositories, which brygge does not follow. The
/// identifier is deliberately not the property's own name (`svn:externals`, see
/// [`crate::props::SVN_EXTERNALS`]): a floor feature is a lowercase kebab-case identifier.
pub(crate) const EXTERNALS: &str = "svn-externals";
/// A URL or other remote source: brygge dumps only a local repository. Spelled identically in the CVS
/// decoder, since it is the same concept.
pub(crate) const REMOTE_SOURCE: &str = "remote-source";

/// Every refused feature, in the order this module declares them. This is the list recorded into
/// provenance (`params["floor"]`) and the list an architect review changes to change the floor.
pub(crate) const ALL: &[&str] = &[EXTERNALS, REMOTE_SOURCE];

/// The floor as a single comma-joined string, for `params["floor"]` (PR-5).
pub(crate) fn joined() -> String {
    ALL.join(",")
}

#[cfg(test)]
mod tests;
