//! The watchdog (RFC 022 D-1/D-2/D-3/D-4, refined D-1a): a clean stop without a signal handler, `std`
//! only, and no `unsafe` (the workspace forbids it).
//!
//! **Incident 094:** a pre-incident draft spawned [`std::env::current_exe`] from inside a function unit
//! tests called in-process. Under `cargo test`, `current_exe()` resolves to the *test harness*, which has
//! no notion of this crate's marker and simply reran its whole default test suite — a fork bomb that hung
//! the owner's machine twice. Batch W0 built the real, non-test [`Watchdog::start`] onto the guards W0a
//! already put in place (`may_spawn`, the `cfg(test)` `start`), plus the design-level guard that closes
//! the hole those two alone cannot: **only [`crate::main`] ever calls [`Watchdog::start`]**, never a
//! command function a unit test can call directly. This batch (W1) extends the same design to every
//! writing command, widens D-3 to directories, and adds D-4's leftover scan; nothing about (a)-(d) below
//! changes.
//!
//! - **(a) Only `main` starts a watchdog.** [`Guard`] is what every command function receives instead;
//!   a unit test always passes [`Guard::disabled`], which can never start a process no matter how the
//!   function under test calls it.
//! - **(b) No recursion, in any build:** [`may_spawn`] (unchanged from W0a).
//! - **(c) The test build cannot spawn at all:** the `cfg(test)` [`Watchdog::start`] (unchanged from
//!   W0a) — no [`std::process::Command`] exists in that build at all.
//! - **(d) The spawned program is checked before anything relies on it:** the `ready <pid>` handshake,
//!   proven at level 1 on a bare [`std::io::pipe`], with no process started ([`await_ready`]).
//!
//! **No environment variable or command-line option ever chooses which program [`Watchdog::start`]
//! spawns.** It is always, unconditionally, [`std::env::current_exe`] — adding a way to choose would add
//! a way to make brygge execute an arbitrary other program, against this project's own "safe and secure"
//! and "less is more" design (review 207, correction `1cf6df0`).
//!
//! **The protocol** (one line per message, `\n`-terminated, written by brygge to the watchdog's stdin):
//! `temp <path>` (a file), `tempdir <path>` (a directory, W1), or `done <path>` (either kind), where
//! `<path>` is percent-encoded with [`display::machine_value`]/[`display::machine_decode_bytes`] — "one
//! encoding serves both" the machine-output format and this protocol (RFC 022 D-1). A line that does not
//! parse is refused: skipped, and the state it would have changed is left untouched.
//!
//! **The marker** (`BRYGGE_INTERNAL_WATCHDOG=<pid>`) carries brygge's own process id — not a fixed
//! constant — because `std` has no portable way for a child to learn its parent's pid without `unsafe`.
//! D-3's removal-safety check matches a registered path's temporary-name suffix against exactly this pid.
//!
//! **D-3, widened (W1):** a registered path is now either a file or a directory, told apart at
//! registration time (the `temp`/`tempdir` verb above), never guessed from its name — the same name
//! scheme (`<name>.partial-<pid>-<n>`) is used by `encode git` for both a directory (the partial
//! repository) and a file (its report's temporary sibling), so the actual on-disk type is checked
//! against the *registered* kind, and either mismatch (a file where a directory was registered, or the
//! reverse) is refused, exactly as a symbolic link always is. **A directory is removed only once it has
//! stopped changing** ([`stops_changing`]): unchanged for at least one second, checked for at most
//! thirty — a tree still being written is left alone and named instead, never removed out from under a
//! write that is still in progress.
//!
//! **D-4 (W1):** before writing, `decode`, `encode git`, `transform` and the built-in `transform authors`
//! scan their output directory, one level deep, for brygge's own temporary forms carrying a *different*
//! process id, and name each one on stderr ([`scan_for_leftovers`]) — purely informational, and it never
//! deletes anything.
//!
//! When the pipe closes, brygge has ended, however it ended: Ctrl-C, `SIGTERM`, a closed terminal, a
//! panic, an abort, the out-of-memory killer, or `kill -9` of brygge itself. The watchdog then removes
//! every path still registered (D-3), then writes one sentence to stderr, and exits `0`.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, ErrorKind, IsTerminal, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::cli::Command;
use crate::{display, exit, output};

/// The environment variable that marks brygge's own internal watchdog entry. Its value is the decoding
/// process's own id (`std::process::id()`), used by [`maybe_run_internal_entry`]'s loop to validate a
/// registered path's temporary-name suffix (D-3) — never a fixed marker value, since the watchdog has no
/// other std-only way to learn which process started it.
const MARKER_ENV: &str = "BRYGGE_INTERNAL_WATCHDOG";

/// How long [`Watchdog::start`] waits for the spawned child's `ready <pid>` line before giving up on it
/// (D-1a(d)).
// R-6: only the non-test `start()` reads this; the `#[cfg(test)]` form (P-1(c)) never spawns anything
// and so never waits on a handshake, which makes this constant unused in a test build specifically.
#[cfg_attr(test, allow(dead_code))]
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

/// [`CREATE_NEW_PROCESS_GROUP`](https://learn.microsoft.com/windows/win32/procthread/process-creation-flags),
/// `0x00000200`: the watchdog gets its own console process group on Windows, so a Ctrl-C delivered to
/// brygge's own group never reaches it (RFC 022 D-1, D-7).
// R-6: like `HANDSHAKE_TIMEOUT`, only the non-test `start()` reads this; a Windows test build's
// `#[cfg(test)]` `start()` never spawns anything, so this is unused there specifically.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// D-3's "stops changing" rule (W1): a directory is removed only once it has looked unchanged for this
/// long, continuously.
const DIR_STABLE_IDLE: Duration = Duration::from_secs(1);
/// D-3's ceiling (W1): the watchdog never waits longer than this for one directory to stop changing — past
/// it, the directory is left alone and named instead of removed.
const DIR_STABLE_CEILING: Duration = Duration::from_secs(30);
/// How often the real wait polls the directory while waiting for it to stop changing.
const DIR_STABLE_POLL_INTERVAL: Duration = Duration::from_millis(200);

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

/// D-7/§1 (W1): does `cmd` need a clean-stop watchdog at all? `true` for exactly the four writing forms —
/// `decode`, `encode git`, `transform`, and the built-in `transform authors` (which shares `transform`'s
/// own temp-file convention and so carries the identical risk, even though it is its own [`Command`]
/// variant) — plus `verify` only when `--against-source` is given (the only case in which it re-derives
/// anything at all; `--against-target` alone re-decodes only through the same already-exempt spill file
/// `decode` itself uses, and registers nothing — see the request's evidence plan). Every other command
/// is read-only with respect to this process's own temporary artifacts and starts no watchdog: `inspect`,
/// `verify` without `--against-source`, `encode <target>` and `encode --list`, `transform --list`,
/// `--help`, and `--version`. A pure function of the already-parsed command, with no I/O of its own, so a
/// test can prove every arm without touching a real environment or filesystem.
#[must_use]
pub fn command_needs_watchdog(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::Decode { .. }
            | Command::EncodeGit { .. }
            | Command::Transform { .. }
            | Command::TransformAuthors { .. }
    ) || matches!(
        cmd,
        Command::Verify {
            against_source: Some(_),
            ..
        }
    )
}

/// What a command receives instead of ever being able to start a watchdog itself (D-1a(a)). Only
/// [`crate::main`] constructs the [`Active`](Guard::Active) form, from [`Watchdog::start`]; every unit
/// test passes [`Guard::disabled`], on which every method below is a no-op.
pub enum Guard {
    Active(Watchdog),
    Disabled,
}

impl Guard {
    /// What every unit test passes. Can never start a process, however the function under test calls it.
    // R-6: only `#[cfg(test)]` code ever constructs this (every unit test does, directly or through
    // `commands/tests.rs`); a non-test build never has a reason to build a `Guard` other than
    // `Watchdog::start`'s own `Active` form, so this is genuinely unused there.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn disabled() -> Self {
        Guard::Disabled
    }

    /// Tell the watchdog that the *file* at `path` now exists and, if brygge dies before
    /// [`release`](Self::release) is called for it, should be removed. A no-op on
    /// [`Disabled`](Guard::Disabled).
    pub fn register(&mut self, path: &Path) {
        if let Guard::Active(w) = self {
            w.register(path);
        }
    }

    /// As [`register`](Self::register), for a *directory* that should be removed as a whole, only once
    /// it has stopped changing (D-3, W1). A no-op on [`Disabled`](Guard::Disabled).
    pub fn register_dir(&mut self, path: &Path) {
        if let Guard::Active(w) = self {
            w.register_dir(path);
        }
    }

    /// Tell the watchdog that `path` (file or directory) is safely in its final place (or already
    /// removed) and no longer needs protecting. A no-op on [`Disabled`](Guard::Disabled).
    pub fn release(&mut self, path: &Path) {
        if let Guard::Active(w) = self {
            w.release(path);
        }
    }

    /// Close the pipe to the watchdog and wait for it to exit, so its wait (and any sentence it has to
    /// print) completes before the command's own exit code is returned. A no-op on
    /// [`Disabled`](Guard::Disabled).
    pub fn finish(self) {
        if let Guard::Active(w) = self {
            w.finish();
        }
    }
}

/// One running watchdog, holding the pipe brygge writes `register`/`release` messages to.
pub struct Watchdog {
    child: Child,
    stdin: ChildStdin,
    /// Set once a write to the watchdog has failed, so the "the watchdog is unreachable" note is printed
    /// at most once per run (D-1: losing the watchdog must never fail a command that would otherwise
    /// succeed, and must not spam stderr either).
    unreachable_reported: bool,
}

impl Watchdog {
    /// P-1(c): under a unit-test build, starting a watchdog is not merely refused at runtime — it does
    /// not exist as code. This function contains no [`std::process::Command`], so no amount of calling
    /// it from a test can ever spawn a process, regardless of what environment that test happens to run
    /// in. This exact behavior holds forever for a unit-test build of this crate, independent of the
    /// real, non-test form below.
    #[cfg(test)]
    pub fn start() -> std::io::Result<Self> {
        Err(std::io::Error::new(
            ErrorKind::Unsupported,
            "the watchdog is never started in a unit-test build",
        ))
    }

    /// Start a watchdog: the same binary, re-invoked through [`maybe_run_internal_entry`], in its own
    /// process group, with a pipe on its stdin and its stdout (for the handshake only). Its stderr is
    /// inherited, so its one sentence on a stop reaches the user exactly where every other brygge
    /// diagnostic does.
    ///
    /// D-1a(b): refuses at once, before any [`Command`] exists, when this process is itself a watchdog
    /// (or a descendant of one). D-1a(d): waits at most [`HANDSHAKE_TIMEOUT`] for the spawned child's
    /// first line to be exactly `ready <pid>` with `<pid>` equal to [`Child::id`]; anything else kills the
    /// child and returns an error instead, so `main` can go on without a watchdog.
    #[cfg(not(test))]
    pub fn start() -> std::io::Result<Self> {
        if !may_spawn() {
            return Err(std::io::Error::other(
                "refused: this process is itself a watchdog",
            ));
        }
        // No environment variable or option ever chooses this program (review 207, `1cf6df0`): it is
        // always, unconditionally, `current_exe()`.
        let mut cmd = std::process::Command::new(std::env::current_exe()?);
        cmd.env(MARKER_ENV, std::process::id().to_string())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
        }
        let mut child = cmd.spawn()?;
        let child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("the watchdog's stdout pipe was not created"))?;
        let expected = child.id();
        if let Err(e) = await_ready(child_stdout, expected, HANDSHAKE_TIMEOUT) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("the watchdog's stdin pipe was not created"))?;
        Ok(Self {
            child,
            stdin,
            unreachable_reported: false,
        })
    }

    /// Tell the watchdog that the file at `path` now exists and, if brygge dies before [`release`] is
    /// called for it, should be removed. R-4: when `path` cannot be registered at all (Windows only — see
    /// [`encode_path`]), nothing is sent, and the one-line notice is printed instead; the write itself is
    /// unaffected, only this one file's clean-stop safety net is missing.
    pub fn register(&mut self, path: &Path) {
        self.register_as("temp", path);
    }

    /// As [`register`](Self::register), for a directory (W1).
    pub fn register_dir(&mut self, path: &Path) {
        self.register_as("tempdir", path);
    }

    fn register_as(&mut self, verb: &str, path: &Path) {
        match registration_line_or_notice(verb, path) {
            Ok(line) => self.write_line(&line),
            Err(notice) => output::stderr(&notice),
        }
    }

    /// Tell the watchdog that `path` is safely in its final place (or already removed) and no longer
    /// needs protecting. A no-op for a `path` that [`register`](Self::register)/
    /// [`register_dir`](Self::register_dir) never managed to send in the first place (R-4): there is
    /// nothing registered to release.
    pub fn release(&mut self, path: &Path) {
        if let Some(encoded) = encode_path(path) {
            self.write_line(&format!("done {encoded}\n"));
        }
    }

    fn write_line(&mut self, line: &str) {
        if self.stdin.write_all(line.as_bytes()).is_err() && !self.unreachable_reported {
            self.unreachable_reported = true;
            output::stderr(
                "the watchdog is no longer reachable; this run continues without a clean-stop safety \
                 net, exactly as 0.10.0's did",
            );
        }
    }

    /// Close the pipe to the watchdog and wait for it to exit.
    pub fn finish(self) {
        drop(self.stdin);
        let mut child = self.child;
        let _ = child.wait();
    }
}

/// D-1a(d)'s wait, over any reader — so it is proven at level 1 on a bare [`std::io::pipe`] (stable since
/// 1.87, our MSRV), with **no process started**. Reads one line on a helper thread and waits for it with
/// [`mpsc::Receiver::recv_timeout`], so a reader that never produces a line (a hung or wrong program)
/// cannot block `start()` past `within`. Accepted only: `ready <pid>` where `<pid>` equals `expected`.
/// Anything else — a timeout, end of file, another line, or another id — is an error naming what was
/// seen; the one line after a call to this function that fails is `start()`'s `Child::kill`, proven by
/// review and by the accepting path on all three systems (handoff §2(d)).
fn await_ready(
    out: impl Read + Send + 'static,
    expected: u32,
    within: Duration,
) -> std::io::Result<()> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(out);
        let mut line = String::new();
        let result = match reader.read_line(&mut line) {
            Ok(0) => Err(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                "the watchdog closed its stdout before writing anything",
            )),
            Ok(_) => Ok(line),
            Err(e) => Err(e),
        };
        // The receiver may already be gone (a timeout already returned to the caller); nothing to do.
        let _ = tx.send(result);
    });
    match rx.recv_timeout(within) {
        Ok(Ok(line)) => {
            let trimmed = line.trim_end_matches(['\n', '\r']);
            match trimmed.split_once(' ') {
                Some(("ready", pid)) if pid.parse::<u32>() == Ok(expected) => Ok(()),
                _ => Err(std::io::Error::other(format!(
                    "the watchdog's first line was not accepted: {trimmed:?}"
                ))),
            }
        }
        Ok(Err(e)) => Err(std::io::Error::other(format!(
            "the watchdog did not answer: {e}"
        ))),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(std::io::Error::other(format!(
            "the watchdog did not answer within {within:?}"
        ))),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(std::io::Error::other(
            "the watchdog's reader thread ended without answering",
        )),
    }
}

/// What [`Watchdog::register_as`] should send for `path` under `verb` (`"temp"` or `"tempdir"`): the line
/// to write, or — only possible on Windows, where [`encode_path`] can fail — the one-line stderr notice
/// that this path cannot be protected. Split out (R-4) so the decision is testable without a real pipe or
/// child process, the same pure/impure split [`is_unmarked`]/[`may_spawn`] and [`classify_internal_entry`]
/// use.
fn registration_line_or_notice(verb: &str, path: &Path) -> Result<String, String> {
    match encode_path(path) {
        Some(encoded) => Ok(format!("{verb} {encoded}\n")),
        None => Err(format!(
            "the watchdog cannot protect {}: its path is not valid Unicode, so this run continues \
             without a clean-stop safety net for it",
            path.display()
        )),
    }
}

/// Percent-encode `path` with the same alphabet [`display::machine_bytes`] uses for every other
/// machine-format value (RFC 022 D-1: "one encoding serves both"), or `None` when `path` cannot be
/// encoded without loss (R-4; Windows only — see below).
///
/// On Unix this is exact over every byte, since a path is just bytes there, so it always succeeds.
#[cfg(unix)]
fn encode_path(path: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    Some(display::machine_bytes(path.as_os_str().as_bytes()))
}

/// On Windows, a path is UTF-16 and not every such path is valid Unicode (an unpaired surrogate, for
/// example). `to_string_lossy` would silently substitute replacement characters there, registering a
/// *different* name than the path actually on disk — a registration that can never be found again, with
/// no sign anything went wrong (R-4, review 208). `to_str` instead returns `None` for exactly that case,
/// so the caller can refuse to register it rather than register the wrong thing. Every temporary name
/// brygge itself ever constructs is plain ASCII, so this can only fail for a caller-supplied `--out`.
#[cfg(windows)]
fn encode_path(path: &Path) -> Option<String> {
    path.to_str().map(display::machine_value)
}

/// The inverse of [`encode_path`]. `None` for anything malformed (D-1: a line that does not parse is
/// refused, never guessed at).
#[cfg(unix)]
fn decode_path(s: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    display::machine_decode_bytes(s).map(|b| PathBuf::from(std::ffi::OsStr::from_bytes(&b)))
}

#[cfg(windows)]
fn decode_path(s: &str) -> Option<PathBuf> {
    let bytes = display::machine_decode_bytes(s)?;
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

/// Which of the two D-3 removal rules a registered path gets (W1): told apart at registration time by
/// the verb the protocol line carried (`temp` vs `tempdir`), never guessed from the path's name — see the
/// module doc's note on `encode git`'s name collision between its partial repository (a directory) and
/// its report's temporary sibling (a file), which share one naming scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
}

/// One parsed protocol line (module doc). `None` for anything else: an unknown verb, a missing path, or
/// a path that does not decode.
fn parse_line(line: &str) -> Option<Message> {
    let (verb, rest) = line.split_once(' ')?;
    let path = decode_path(rest)?;
    match verb {
        "temp" => Some(Message::Temp(path, Kind::File)),
        "tempdir" => Some(Message::Temp(path, Kind::Dir)),
        "done" => Some(Message::Done(path)),
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Message {
    Temp(PathBuf, Kind),
    Done(PathBuf),
}

/// D-2: if this process is brygge's own internal watchdog entry, run its loop to completion and return
/// the exit code `main` should use (never returns `None` in that case, and never reaches ordinary command
/// dispatch). If it is not (the marker is absent), returns `None` so `main` can go on to parse ordinary
/// arguments — the only way this function can affect a normal invocation at all is by checking one
/// environment variable. Called from `main`, **before** argument parsing.
///
/// This is brygge's own internal entry point, not part of the command surface: it is reached only when
/// [`MARKER_ENV`] is already set in the process's own environment, which nothing but [`Watchdog::start`]
/// ever sets, on the one child it spawns — never from a shell, never documented as a command, and never
/// reachable through `--help`. It is the one site `tools/check-self-exec.sh` lists for this crate.
#[must_use]
pub fn maybe_run_internal_entry() -> Option<i32> {
    // R-3: `var_os`, never `var`. `var(...).ok()` turns a *present but non-UTF-8* marker into `None`,
    // indistinguishable from the marker being absent at all — which would fall through to ordinary
    // command dispatch with the marker still set in the environment, the exact bug this guards against.
    // `var_os` never fails on non-UTF-8: it just hands back the raw value for
    // `classify_internal_entry` to judge.
    let marker = std::env::var_os(MARKER_ENV)?;
    match classify_internal_entry(&marker, std::io::stdin().is_terminal()) {
        Ok(pid) => Some(run_watchdog_loop(pid)),
        Err(code) => {
            output::stderr("this is an internal entry of brygge, not a command");
            Some(code)
        }
    }
}

/// The pure half of [`maybe_run_internal_entry`] (D-2): given the marker's value and whether stdin is a
/// terminal, decide whether to run the loop (`Ok(pid)`) or refuse (`Err(exit code)`). Testable without
/// mutating the real environment or a real terminal (`std::env::set_var` is `unsafe` in this edition,
/// which the workspace forbids entirely) — the same pure/impure split [`is_unmarked`]/[`may_spawn`] use.
///
/// Takes an [`std::ffi::OsStr`] rather than `&str` (R-3) so a marker that is present but not valid UTF-8
/// can be represented and tested at all: [`std::ffi::OsStr::to_str`] returns `None` for one, and that is
/// treated exactly like a UTF-8 string that merely is not decimal — refused, never treated as absent.
fn classify_internal_entry(marker: &std::ffi::OsStr, stdin_is_terminal: bool) -> Result<u32, i32> {
    // D-2/R-3: a marker that is not a decimal process id (whether because it is not UTF-8 at all, or
    // because it is UTF-8 but not decimal), or a terminal stdin, is refused outright — never guessed
    // at, and never run with a fallback pid that could coincidentally match something real.
    match marker.to_str().and_then(|s| s.trim().parse::<u32>().ok()) {
        Some(pid) if !stdin_is_terminal => Ok(pid),
        _ => Err(exit::USAGE),
    }
}

/// The watchdog's own loop (D-2): write `ready <own pid>` first, then read registration lines until the
/// pipe closes (brygge has ended, one way or another), then apply D-3 to whatever is still registered,
/// print the one sentence if anything was registered at all, and exit `0` — the watchdog's own exit
/// status is never read by anyone, so there is nothing to report it to. **Contains no [`Command`]: a
/// watchdog never starts anything.**
fn run_watchdog_loop(pid: u32) -> i32 {
    let own_pid = std::process::id();
    // The parent may already be gone (or stdout otherwise unusable); either way there is nothing
    // further to hand back on stdout, but stdin may still carry registrations to honor, so the loop
    // goes on regardless of this write's outcome.
    let _ = output::stdout(&format!("ready {own_pid}\n"));
    let stdin = std::io::stdin();
    let lines = stdin.lock().lines().map_while(Result::ok);
    let (removed, not_removed, malformed) = apply_registrations(
        lines,
        pid,
        |p: &Path| std::fs::remove_file(p),
        remove_dir_once_stable,
    );
    // R-2: a skipped `done` is harmless, but a skipped `temp`/`tempdir` leaves a path the watchdog was
    // never told to protect, with no visible sign anything went wrong — so a malformed line is counted
    // and reported, never merely dropped in silence.
    if let Some(notice) = malformed_registrations_notice(malformed) {
        output::stderr(&notice);
    }
    if !removed.is_empty() || !not_removed.is_empty() {
        output::stderr(&sentence(&removed, &not_removed));
    }
    exit::CLEAN
}

/// R-2's one line, or `None` when there was nothing to report (never prints `0 registration line(s)`
/// — the message exists only to call out a real brygge bug, and zero malformed lines is the ordinary,
/// unremarkable case).
fn malformed_registrations_notice(malformed: usize) -> Option<String> {
    if malformed == 0 {
        None
    } else {
        Some(format!(
            "watchdog: {malformed} registration line(s) could not be read and were ignored"
        ))
    }
}

/// The real [`Dir`](Kind::Dir) removal function `run_watchdog_loop` hands to [`apply_registrations`]:
/// waits for `path` to stop changing ([`stops_changing`], over the real clock and a real recursive
/// snapshot — [`wait_for_dir_to_stop_changing`]), then removes it with [`std::fs::remove_dir_all`], which
/// does not follow a symbolic link found *inside* the tree (fixed in Rust 1.58.1, below this project's
/// MSRV, so no extra guard is needed for that here — the registered path itself is checked separately, by
/// [`why_not_removable`], before this is ever called).
fn remove_dir_once_stable(path: &Path) -> DirOutcome {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return DirOutcome::AlreadyGone,
        Err(e) => return DirOutcome::Error(e.to_string()),
        Ok(_) => {}
    }
    if !wait_for_dir_to_stop_changing(path) {
        return DirOutcome::StillChanging;
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => DirOutcome::Removed,
        Err(e) if e.kind() == ErrorKind::NotFound => DirOutcome::AlreadyGone,
        Err(e) => DirOutcome::Error(e.to_string()),
    }
}

/// What trying to remove a registered directory came to — the production
/// [`remove_dir_once_stable`]'s own return type, and what a test can inject directly into
/// [`apply_registrations`] without any real waiting or filesystem state.
#[derive(Debug, PartialEq, Eq)]
enum DirOutcome {
    /// Removed, just now.
    Removed,
    /// Already gone before this run even looked (not a failure — there is simply nothing left to do).
    AlreadyGone,
    /// Still changing after the full ceiling; left in place.
    StillChanging,
    /// Some other I/O error, named.
    Error(String),
}

/// A cheap, best-effort summary of a directory tree's contents, recursive but never following a symbolic
/// link found inside it (the same "never followed" rule [`why_not_removable`] applies to the registered
/// path itself): the number of entries, their total byte length, and the newest modification time seen.
/// Equality is all [`stops_changing`] needs — two equal snapshots mean "no visible change since the last
/// check" — so this never needs to be exact or complete, only consistent from one call to the next.
/// Best-effort throughout: an entry or a subdirectory that cannot be read (removed mid-scan, or a
/// permission problem) is simply skipped, never treated as a reason to fail the check.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct DirSnapshot {
    entries: u64,
    total_bytes: u64,
    newest_mtime: Option<std::time::SystemTime>,
}

fn dir_snapshot(root: &Path) -> DirSnapshot {
    let mut snap = DirSnapshot::default();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            snap.entries += 1;
            if meta.file_type().is_symlink() {
                // Never followed: its own target is not this tree's content.
            } else if meta.is_dir() {
                stack.push(path);
            } else {
                snap.total_bytes += meta.len();
            }
            if let Ok(m) = meta.modified() {
                snap.newest_mtime = Some(snap.newest_mtime.map_or(m, |cur| cur.max(m)));
            }
        }
    }
    snap
}

/// D-3's "stops changing" rule (handoff §1/§2), over an injected clock and snapshot so a test can prove
/// every one of its three clauses — "a tree that changes is not removed and is named", "a tree still for
/// one second is removed", "the thirty-second ceiling" — with **no real wall-clock seconds spent at all**.
///
/// `initial` is the first `(timestamp_ms, snapshot)` sample, already taken; `poll` is called for every
/// sample after that. In production ([`wait_for_dir_to_stop_changing`]), `poll` sleeps for one tick and
/// takes a fresh [`DirSnapshot`]; a test instead pops from a fixed, pre-built sequence of fake timestamps
/// and fake snapshots, so no real time passes and no real directory is touched. `S`'s equality is all this
/// needs: two equal snapshots mean "no visible change between the two samples".
///
/// Removes (returns `true`) once the snapshot has read the same for at least `idle_ms` of elapsed time,
/// continuously; gives up (returns `false`) once `ceiling_ms` has elapsed since `initial`'s own timestamp,
/// whichever comes first.
fn stops_changing<S: PartialEq>(
    initial: (u64, S),
    mut poll: impl FnMut() -> (u64, S),
    idle_ms: u64,
    ceiling_ms: u64,
) -> bool {
    let (start, mut last) = initial;
    let mut last_changed_at = start;
    let mut now = start;
    loop {
        if now.saturating_sub(last_changed_at) >= idle_ms {
            return true;
        }
        if now.saturating_sub(start) >= ceiling_ms {
            return false;
        }
        let (t, snap) = poll();
        now = t;
        if snap != last {
            last = snap;
            last_changed_at = now;
        }
    }
}

/// The real, impure wrapper around [`stops_changing`]: a real [`Instant`]-based clock (converted to
/// milliseconds since this wait began) and a real, recursive [`dir_snapshot`] of `path`, polled every
/// [`DIR_STABLE_POLL_INTERVAL`], for at most [`DIR_STABLE_CEILING`], requiring [`DIR_STABLE_IDLE`] of
/// continuous quiet before it is willing to call `path` safe to remove.
fn wait_for_dir_to_stop_changing(path: &Path) -> bool {
    let clock_start = Instant::now();
    let now_ms = || {
        u64::try_from(Instant::now().duration_since(clock_start).as_millis()).unwrap_or(u64::MAX)
    };
    let initial = (now_ms(), dir_snapshot(path));
    stops_changing(
        initial,
        || {
            thread::sleep(DIR_STABLE_POLL_INTERVAL);
            (now_ms(), dir_snapshot(path))
        },
        u64::try_from(DIR_STABLE_IDLE.as_millis()).unwrap_or(u64::MAX),
        u64::try_from(DIR_STABLE_CEILING.as_millis()).unwrap_or(u64::MAX),
    )
}

/// One of brygge's own temporary-name forms, parsed from a bare file name (never a full path — both
/// [`why_not_removable`] and [`scan_for_leftovers`] call this only after `file_name()`), returning the
/// process id the name itself carries, or `None` for anything that merely *resembles* one of these forms
/// without actually matching it (D-4: "never names a path that only resembles a form").
///
/// Recognizes every writing command's own convention, as the actual code builds them (never a guessed or
/// invented name):
/// - `decode`'s/`verify`'s atomic-write temp artifact (`commands.rs::atomic_write`):
///   `.<name>.brygge-tmp-<pid>`;
/// - `transform`'s (and the built-in `transform authors`'s) own scratch output
///   (`commands.rs::create_transform_temp`) — a **file**, despite the handoff's own checklist grouping it
///   with the directory forms; grounded here against the real code, not the checklist's label:
///   `.brygge-transform-<pid>-<nanos>-<attempt>`;
/// - `encode git`'s partial output repository (a directory) or its report's temporary sibling (a file) —
///   the two share one scheme, and are told apart only by what is registered and what is actually on
///   disk, never by the name alone: `<name>.partial-<pid>-<attempt>`.
fn classify_temp_name(name: &str) -> Option<u32> {
    parse_atomic_write_name(name)
        .or_else(|| parse_transform_scratch_name(name))
        .or_else(|| parse_encode_git_partial_name(name))
}

fn parse_decimal(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u32>().ok()
}

fn parse_atomic_write_name(name: &str) -> Option<u32> {
    let body = name.strip_prefix('.')?;
    let (base, pid_str) = body.rsplit_once(".brygge-tmp-")?;
    if base.is_empty() {
        return None;
    }
    parse_decimal(pid_str)
}

fn parse_transform_scratch_name(name: &str) -> Option<u32> {
    let rest = name.strip_prefix(".brygge-transform-")?;
    let mut parts = rest.split('-');
    let pid_str = parts.next()?;
    let nanos_str = parts.next()?;
    let attempt_str = parts.next()?;
    if parts.next().is_some() {
        return None; // exactly three numeric groups — pid, nanos, attempt — nothing more.
    }
    if nanos_str.is_empty() || !nanos_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if attempt_str.is_empty() || !attempt_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    parse_decimal(pid_str)
}

fn parse_encode_git_partial_name(name: &str) -> Option<u32> {
    let (base, rest) = name.rsplit_once(".partial-")?;
    if base.is_empty() {
        return None;
    }
    let (pid_str, attempt_str) = rest.split_once('-')?;
    if attempt_str.is_empty() || !attempt_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    parse_decimal(pid_str)
}

/// D-3: may the watchdog remove `path`, which it must still believe is registered by `pid` as `kind`?
/// `None` means yes; `Some(reason)` names why not — and `path` is left exactly as it was.
///
/// W1: `kind` is told apart from the registered verb, never guessed — `path`'s name is still checked
/// against [`classify_temp_name`] (defense in depth: a registration this process did not actually send
/// can never be acted on just because it arrived on the pipe), and then its *actual* on-disk type is
/// checked against `kind`: a registered directory that is actually a file, or the reverse, is refused —
/// exactly as a symbolic link always is.
fn why_not_removable(path: &Path, pid: u32, kind: Kind) -> Option<String> {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return Some("its name is not valid Unicode".to_string());
    };
    if classify_temp_name(name) != Some(pid) {
        return Some(format!(
            "{name} is not one of brygge's own temporary names for process {pid}"
        ));
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Some("a symbolic link, never followed".to_string())
        }
        Ok(meta) => match kind {
            Kind::File if meta.is_file() => None,
            Kind::File => Some("not a regular file".to_string()),
            Kind::Dir if meta.is_dir() => None,
            Kind::Dir => Some("not a directory".to_string()),
        },
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => Some(format!("its state cannot be checked ({e})")),
    }
}

/// The testable core of [`run_watchdog_loop`], over any line source and any removal functions (so a test
/// can observe exactly what would have been removed without touching the real filesystem, or can plant a
/// fake failure, or a fake "still changing" directory outcome, with no real waiting). Replays `lines` into
/// the registered-paths list exactly as the real loop does (a malformed line is refused: skipped, nothing
/// it named is touched, but counted — R-2), then applies D-3 to whatever is still registered when the
/// source ends, returning the paths actually removed, the paths left alone with why (both in registration
/// order), and how many lines could not be read at all.
fn apply_registrations(
    lines: impl Iterator<Item = String>,
    pid: u32,
    mut remove_file: impl FnMut(&Path) -> std::io::Result<()>,
    mut remove_dir: impl FnMut(&Path) -> DirOutcome,
) -> (Vec<PathBuf>, Vec<(PathBuf, String)>, usize) {
    let mut registered: Vec<(PathBuf, Kind)> = Vec::new();
    let mut malformed = 0usize;
    for line in lines {
        match parse_line(&line) {
            Some(Message::Temp(path, kind)) => registered.push((path, kind)),
            Some(Message::Done(path)) => registered.retain(|(p, _)| p != &path),
            // R-2: a malformed line is still refused — skipped, nothing it named is touched — but it
            // can only come from a brygge bug (brygge itself always writes well-formed lines), so it is
            // counted instead of vanishing without a trace.
            None => malformed += 1,
        }
    }

    let mut removed = Vec::new();
    let mut not_removed = Vec::new();
    for (path, kind) in registered {
        match why_not_removable(&path, pid, kind) {
            Some(reason) => not_removed.push((path, reason)),
            None => match kind {
                Kind::File => match remove_file(&path) {
                    Ok(()) => removed.push(path),
                    Err(e) if e.kind() == ErrorKind::NotFound => {}
                    Err(e) => not_removed.push((path, e.to_string())),
                },
                Kind::Dir => match remove_dir(&path) {
                    DirOutcome::Removed => removed.push(path),
                    DirOutcome::AlreadyGone => {}
                    DirOutcome::StillChanging => not_removed.push((
                        path,
                        format!(
                            "still changing after waiting up to {}s; left in place",
                            DIR_STABLE_CEILING.as_secs()
                        ),
                    )),
                    DirOutcome::Error(e) => not_removed.push((path, e)),
                },
            },
        }
    }
    (removed, not_removed, malformed)
}

/// D-2/D-3's one sentence. `removed`/`not_removed` are each in registration order; every path is rendered
/// through [`display::human`], the same neutralization every other path or source-derived string gets
/// before it reaches a terminal.
fn sentence(removed: &[PathBuf], not_removed: &[(PathBuf, String)]) -> String {
    let fmt_path = |p: &Path| display::human(&p.display().to_string()).into_owned();
    let mut s = String::from("interrupted:");
    if !removed.is_empty() {
        let list = removed
            .iter()
            .map(|p| fmt_path(p))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(s, " removed {list}");
    }
    if !not_removed.is_empty() {
        if !removed.is_empty() {
            s.push(';');
        }
        let list = not_removed
            .iter()
            .map(|(p, reason)| format!("{} ({reason})", fmt_path(p)))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(s, " not removed: {list}");
    }
    s.push_str("; nothing was written");
    s
}

/// What [`scan_for_leftovers`] found at one path: enough to render [`leftover_notice`] without any
/// further filesystem access, so the message itself stays testable as a pure function (the same
/// pure/impure split every other rendering function in this module uses).
enum LeftoverKind {
    File { size: u64 },
    Dir,
}

/// D-4 (handoff §1/§2): before writing, scan `dir` — the command's own output directory — one level
/// deep, never recursively, for brygge's own temporary forms ([`classify_temp_name`]) that carry a
/// process id other than `own_pid` (this run's own), and name each one on stderr. **Purely
/// informational: this never deletes anything, under any circumstance.** `dir` not existing yet, or not
/// being readable, is not an error here — there is simply nothing to scan.
pub fn scan_for_leftovers(dir: &Path, own_pid: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        let Some(found_pid) = classify_temp_name(name_str) else {
            continue;
        };
        if found_pid == own_pid {
            continue;
        }
        let path = entry.path();
        let kind = match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => LeftoverKind::Dir,
            Ok(meta) => LeftoverKind::File { size: meta.len() },
            Err(_) => continue, // gone between the listing and the stat; nothing to report.
        };
        output::stderr(&leftover_notice(&path, found_pid, &kind));
    }
}

/// The pure half of [`scan_for_leftovers`]'s one message: given a path, the pid it carries, and whether
/// it is a file (with its size) or a directory, render the exact D-4 line. `path` is rendered through
/// [`display::human`], the same neutralization every other path gets before it reaches a terminal.
fn leftover_notice(path: &Path, pid: u32, kind: &LeftoverKind) -> String {
    let raw = path.display().to_string();
    let shown = display::human(&raw);
    match kind {
        LeftoverKind::File { size } => {
            let rm = if cfg!(windows) {
                format!("del {shown}")
            } else {
                format!("rm {shown}")
            };
            format!(
                "found {shown} ({size}), a temporary file of brygge process {pid}; if no brygge is \
                 running, remove it with: {rm}"
            )
        }
        LeftoverKind::Dir => {
            let rm = if cfg!(windows) {
                format!("rmdir /s {shown}")
            } else {
                format!("rm -r {shown}")
            };
            format!(
                "found {shown}, a temporary directory of brygge process {pid}; if no brygge is \
                 running, remove it with: {rm}"
            )
        }
    }
}

#[cfg(test)]
mod tests;
