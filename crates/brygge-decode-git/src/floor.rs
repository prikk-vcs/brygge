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
/// A commit's extra-header **name** that is not valid UTF-8 (review 011 R-3c): gix does not enforce
/// git's own "headers are ASCII" convention, so a crafted commit can carry one. `Extra`/`Signature`
/// labels are text, so this is refused rather than lossily converted.
pub(crate) const NON_UTF8_COMMIT_HEADER_NAME: &str = "non-UTF-8 commit header name";
/// `extensions.objectFormat = sha256` (batch-2 handoff §1.1 investigation): this build's `gix`
/// dependency does not enable Cargo feature `sha256` (only `sha1`, gix's own default), so
/// `gix_hash::Kind::Sha256` does not exist in this binary and gix itself refuses such a repository's
/// config outright (confirmed empirically: `gix::open()` on a `git init --object-format=sha256`
/// repository fails in config validation, `gix-0.87.1/src/config/tree/sections/extensions.rs:33-35`'s
/// `#[cfg(feature = "sha256")]`-gated match arm being absent, surfacing as `gix::open::Error::Config`,
/// `gix-0.87.1/src/open/mod.rs:55-56`). Detected here, before `gix::open`, so the refusal is a named,
/// typed floor item rather than gix's raw internal config-error text.
pub(crate) const SHA256_OBJECT_FORMAT: &str = "SHA-256 object format";

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
    NON_UTF8_COMMIT_HEADER_NAME,
    SHA256_OBJECT_FORMAT,
];

/// The floor as a single comma-joined string, for `params["floor"]` (PR-5).
pub(crate) fn joined() -> String {
    ALL.join(",")
}
