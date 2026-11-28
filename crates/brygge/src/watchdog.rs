//! RFC 022 D-1/D-1a (batch W0a, P-1(b)/(c)): the watchdog's safety-only skeleton. **No spawn exists
//! yet** — batch W0 adds `Watchdog`'s real, non-test form, from the design this batch's guards are
//! built to contain.
//!
//! **Incident 094:** a unit test that called the pre-incident `Watchdog::start()` directly, in-process,
//! spawned `std::env::current_exe()` as its "watchdog". Under `cargo test`, that is the test harness
//! itself, which has no notion of this crate's internal watchdog marker and simply ran its whole default
//! test set again — a fork bomb that hung the owner's machine twice. These two guards exist so the same
//! class of mistake is structurally impossible, however `Watchdog::start()` is written once W0 adds it:
//!
//! - [`may_spawn`] (P-1(b)): false whenever this process is already running as a watchdog (or any
//!   descendant of one), so a watchdog — or anything it in turn spawns — can never start another. That
//!   bounds the recursion depth at one, even if every other guard in the design fails;
//! - under `cfg(test)`, [`Watchdog::start`] (P-1(c)) is a function that cannot spawn anything: it
//!   contains no [`std::process::Command`] anywhere in a unit-test build, so no test can ever start a
//!   real process merely by calling the function it is testing.
//!
//! `may_spawn` has no caller yet in this batch — W0 wires it into the real `Watchdog::start()` (not
//! written until then) from the working tree this batch's safeguards are meant to protect.
// W0a, P-1(b)/(c): the guard exists before the spawn it will guard does. `may_spawn` has no caller in
// this batch; W0 wires it into `Watchdog::start`'s real, non-test form.
#![allow(dead_code)]

/// The environment variable that will mark brygge's own internal watchdog entry, once W0 adds it
/// (RFC 022 D-2). Read here only by [`may_spawn`].
const MARKER_ENV: &str = "BRYGGE_INTERNAL_WATCHDOG";

/// P-1(b): may this process start a watchdog? `false` whenever [`MARKER_ENV`] is already present in its
/// own environment — meaning this process either *is* a watchdog, or was started by one (the marker is
/// inherited by every child unless a caller deliberately clears it, which nothing in this crate does).
/// Either way, it must never start a watchdog of its own: that is what bounds the recursion this
/// incident's fork bomb needed to at most one level, independent of every other guard.
#[must_use]
fn may_spawn() -> bool {
    is_unmarked(std::env::var_os(MARKER_ENV))
}

/// The pure half of [`may_spawn`], over an explicit value instead of the real process environment.
/// `std::env::set_var`/`remove_var` are `unsafe` in this edition, and the workspace forbids `unsafe`
/// entirely, so a test cannot exercise both of [`may_spawn`]'s outcomes by mutating the real
/// environment — it exercises this pure function instead, which `may_spawn` is the only caller of.
fn is_unmarked(marker: Option<std::ffi::OsString>) -> bool {
    marker.is_none()
}

/// A placeholder for the future watchdog (RFC 022 D-1), until W0 adds its real, non-test form (P-1(a),
/// (d)). Holds nothing yet: there is nothing to hold until a real process exists behind it.
pub struct Watchdog {
    _private: (),
}

impl Watchdog {
    /// P-1(c): under a unit-test build, starting a watchdog is not merely refused at runtime — it does
    /// not exist as code. This function contains no [`std::process::Command`], so no amount of calling
    /// it from a test can ever spawn a process, regardless of what environment that test happens to run
    /// in. W0 adds the real, non-test form of this method separately; it does not replace this one, so a
    /// unit-test build of this crate keeps this exact behavior forever, not only until W0 lands.
    #[cfg(test)]
    pub fn start() -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "the watchdog is never started in a unit-test build",
        ))
    }
}

#[cfg(test)]
mod tests;
