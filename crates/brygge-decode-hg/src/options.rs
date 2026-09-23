//! Decoder options. Every option that can change the output is recorded into the IR provenance.
//!
//! The Mercurial decoder currently has **no** options: Mercurial records its own copies and renames, so
//! they are carried as `Stated`, and nothing is ever inferred. The type stays so a caller's
//! `Options::default()` keeps working when an option is added, and so provenance has one place that
//! renders them.

use std::collections::BTreeMap;

/// How the Mercurial decoder should behave. There is nothing to configure yet; construct it with
/// [`Options::default`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {}

impl Options {
    /// Render the options as canonical, sorted key→value strings for the IR provenance. Empty while
    /// there are no options; the decoder adds its own `floor` entry.
    #[must_use]
    pub fn as_params(&self) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
}
