//! Resolving the SVN source into dumpstream bytes (RFC 006 D-1, Tier D).
//!
//! Two forms: a **dumpfile** the operator supplies (no subprocess), or a **local repository** brygge
//! dumps with a read-only `svnadmin dump` (a fixed argument vector, no shell, no network). A **remote /
//! URL source is refused** (INV-3): brygge never dumps over the network, and `svnrdump` is out of scope.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use crate::{Error, floor};

/// Resource ceilings for loading a dumpstream (RFC 010 D-4). One place for every SVN ceiling; tests
/// construct a small instance instead of needing gigabytes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// The maximum dumpstream size brygge will hold in memory (RFC 006 security review, T-8).
    /// Correctness first; streaming a larger dump is OQ-F (deferred). Refused, not truncated, above this.
    pub(crate) max_dump_bytes: usize,
    /// The maximum `svnadmin dump` stderr brygge holds, so a chatty or hostile subprocess writing
    /// endless stderr cannot exhaust the host either (CR-10). Only ever used for an error message.
    pub(crate) max_stderr_bytes: usize,
    /// The maximum bytes brygge reads from `svnadmin --version --quiet` (stdout and stderr each; review
    /// 010 R-5's "one place" for what was an inline 4 KiB literal), far past any real version string.
    pub(crate) max_svnadmin_version_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_dump_bytes: 8 * 1024 * 1024 * 1024,
            max_stderr_bytes: 64 * 1024,
            max_svnadmin_version_bytes: 4096,
        }
    }
}

/// The `svnadmin_version` provenance param never carries more than this many characters (review 010
/// R-5): a version string never needs more, and it bounds how much of a hostile `svnadmin --version`
/// output can reach the artifact even after neutralization.
const MAX_VERSION_LEN: usize = 128;

/// Keep only printable ASCII (`0x20`-`0x7e`) from `bytes`, capped at [`MAX_VERSION_LEN`] characters
/// (review 010 R-5): an identity-bearing provenance param must never carry a control character or other
/// non-printable byte, the same discipline `crate::props`/the CLI's `display` module apply elsewhere.
fn neutralize_version(bytes: &[u8]) -> String {
    bytes
        .iter()
        .filter(|&&b| (0x20..=0x7e).contains(&b))
        .map(|&b| b as char)
        .take(MAX_VERSION_LEN)
        .collect()
}

/// Where to read the SVN history from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A path to a fulltext SVN dumpstream file (`svnadmin dump` output). No subprocess is run.
    DumpFile(PathBuf),
    /// A path to a **local** SVN repository; brygge runs a read-only `svnadmin dump` on it.
    LocalRepo(PathBuf),
}

impl Source {
    /// Load the dumpstream bytes for this source.
    ///
    /// # Errors
    /// [`Error::Open`] if the file is unreadable, the repo path is a URL, or `svnadmin` fails;
    /// [`Error::ResourceLimit`] if the dumpstream exceeds the configured ceiling.
    pub fn load(&self) -> Result<Vec<u8>, Error> {
        self.load_with(&Limits::default())
    }

    pub(crate) fn load_with(&self, limits: &Limits) -> Result<Vec<u8>, Error> {
        match self {
            Self::DumpFile(path) => read_dumpfile_bounded(path, limits),
            Self::LocalRepo(path) => dump_local_repo(path, limits),
        }
    }
}

/// Run `svnadmin --version --quiet` (a fixed argv, no shell, no network, no repository touched) and
/// return its first line, neutralized (printable ASCII only, capped at [`MAX_VERSION_LEN`] — review 010
/// R-5) — the `svnadmin_version` provenance param (CR-07.6).
pub(crate) fn svnadmin_version() -> Result<String, Error> {
    let bound = Limits::default().max_svnadmin_version_bytes;
    let limits = Limits {
        max_dump_bytes: bound,
        max_stderr_bytes: bound,
        max_svnadmin_version_bytes: bound,
    };
    let mut cmd = Command::new("svnadmin");
    cmd.arg("--version").arg("--quiet");
    let out = run_and_capture(
        cmd,
        "svnadmin --version --quiet",
        "the svnadmin --version output",
        &limits,
    )?;
    version_from_output(&out)
}

/// The `svnadmin_version` param from `svnadmin --version --quiet`'s output: the neutralized first line,
/// never empty (review 010 F-2 — an empty result is `Error::Open`, not `""`).
fn version_from_output(out: &[u8]) -> Result<String, Error> {
    let first_line = out.split(|&b| b == b'\n').next().unwrap_or_default();
    let version = neutralize_version(first_line).trim().to_string();
    if version.is_empty() {
        return Err(Error::Open(
            "`svnadmin --version --quiet` printed no usable version".to_string(),
        ));
    }
    Ok(version)
}

fn over_limit(what: &str, max_bytes: usize) -> Error {
    Error::ResourceLimit {
        what: what.to_string(),
        ceiling: format!("{max_bytes} bytes"),
    }
}

/// Read a dumpfile into memory, refusing before allocation if its size already exceeds
/// `limits.max_dump_bytes` (CR-10) — checked from filesystem metadata, not after a full read. Reading
/// itself is still bounded with `take(max_dump_bytes + 1)` in case the file grows after the metadata
/// check.
fn read_dumpfile_bounded(path: &Path, limits: &Limits) -> Result<Vec<u8>, Error> {
    let meta = std::fs::metadata(path)
        .map_err(|e| Error::Open(format!("cannot stat dumpfile {}: {e}", path.display())))?;
    if meta.len() > limits.max_dump_bytes as u64 {
        return Err(over_limit("the dumpstream", limits.max_dump_bytes));
    }
    let file = std::fs::File::open(path)
        .map_err(|e| Error::Open(format!("cannot read dumpfile {}: {e}", path.display())))?;
    let mut bytes = Vec::new();
    file.take(limits.max_dump_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Open(format!("cannot read dumpfile {}: {e}", path.display())))?;
    if bytes.len() as u64 > limits.max_dump_bytes as u64 {
        return Err(over_limit("the dumpstream", limits.max_dump_bytes));
    }
    Ok(bytes)
}

/// Run a read-only `svnadmin dump` on a local repository, capturing the fulltext dumpstream from stdout.
fn dump_local_repo(path: &Path, limits: &Limits) -> Result<Vec<u8>, Error> {
    // A URL is not a local repository; brygge does not dump over the network (INV-3).
    let shown = path.to_string_lossy();
    if shown.contains("://") {
        return Err(Error::FloorRefusal {
            feature: floor::REMOTE_SOURCE.to_string(),
            reason: "brygge dumps only a local repository; a URL/remote source is refused, and \
                     `svnrdump` (network) is out of scope"
                .to_string(),
        });
    }
    // `svnadmin dump <repo> --quiet` writes a fulltext dumpstream to stdout (no `--deltas`), progress to
    // stderr (suppressed by --quiet, but still captured, bounded, for a failure message). Fixed argv, no
    // shell. It performs no network I/O and runs no repository hooks.
    let mut cmd = Command::new("svnadmin");
    cmd.arg("dump").arg(path).arg("--quiet");
    run_and_capture(
        cmd,
        &format!("svnadmin dump {}", path.display()),
        "the dumpstream",
        limits,
    )
}

/// Spawn `command`, capturing stdout (bounded at `limits.max_dump_bytes` + 1) while draining stderr
/// (bounded at `limits.max_stderr_bytes` + 1) concurrently on a second thread (CR-10): a full stderr
/// pipe must not be able to deadlock a child that is still writing a large amount to stdout, or vice
/// versa. On a stdout overflow, the child is killed rather than left to keep writing into a pipe nobody
/// drains.
pub(crate) fn run_and_capture(
    mut command: Command,
    program_desc: &str,
    overflow_what: &str,
    limits: &Limits,
) -> Result<Vec<u8>, Error> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Open(format!("failed to run `{program_desc}`: {e}")))?;

    let Some(child_stdout) = child.stdout.take() else {
        return Err(Error::Open(format!("`{program_desc}` has no stdout pipe")));
    };
    let Some(child_stderr) = child.stderr.take() else {
        return Err(Error::Open(format!("`{program_desc}` has no stderr pipe")));
    };

    let max_stderr_bytes = limits.max_stderr_bytes;
    let stderr_thread = thread::spawn(move || {
        let mut child_stderr = child_stderr;
        let mut buf = Vec::new();
        // Keep only the first `max_stderr_bytes` (bounded memory), but keep reading to true EOF and
        // discard the rest (2026-09-23 review 005, R-1): dropping the read end early would close the
        // pipe while the child may still be writing, killing it with SIGPIPE/EPIPE mid-dump rather than
        // letting it finish normally.
        let _ = (&mut child_stderr)
            .take(max_stderr_bytes as u64)
            .read_to_end(&mut buf);
        let _ = std::io::copy(&mut child_stderr, &mut std::io::sink());
        buf
    });

    let mut stdout = Vec::new();
    let read_outcome = child_stdout
        .take(limits.max_dump_bytes as u64 + 1)
        .read_to_end(&mut stdout);
    let overflowed = stdout.len() as u64 > limits.max_dump_bytes as u64;
    if overflowed {
        let _ = child.kill();
    }

    let status = child
        .wait()
        .map_err(|e| Error::Open(format!("failed to wait on `{program_desc}`: {e}")))?;
    let stderr = stderr_thread.join().unwrap_or_default();

    read_outcome
        .map_err(|e| Error::Open(format!("failed to read `{program_desc}` output: {e}")))?;
    if overflowed {
        return Err(over_limit(overflow_what, limits.max_dump_bytes));
    }
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        return Err(Error::Open(format!(
            "`{program_desc}` failed: {}",
            stderr.trim()
        )));
    }
    Ok(stdout)
}

#[cfg(test)]
mod tests;
