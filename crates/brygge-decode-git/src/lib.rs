//! brygge's Git source decoder (RFC 004): read a local Git object database and produce a
//! [`brygge_ir::Ir`]. This is the sole crate that links `gix` (RFC 009 D-1); the IR, the honesty
//! machinery, and `verify --internal` link none of it. Placeholder pending the RFC 004 handoff.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// The decoder's version string, recorded into IR provenance (`PR-6`).
#[must_use]
pub fn decoder_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
