//! Finding and running an external `brygge encode`/`brygge transform` plugin on `PATH` (RFC 017 D-3,
//! OQ-1; extended to `transform` by RFC 018 D-6, batch P — the same module, no copy: every rule below
//! is shared between the two kinds, parameterized only by [`Kind`]'s own prefix).
//!
//! brygge resolves the plugin's absolute path itself and spawns that path directly — never the bare
//! name through [`std::process::Command::new`]: on Windows, that would let the OS search the current
//! executable's own directory and system directories before `PATH`.
//!
//! The search walks `PATH` in order, skipping every empty and every relative entry (an empty entry means
//! the current directory on Unix; a relative entry would search whatever the current directory happens
//! to be, including a source repository). On Unix, a match is the first `<dir>/<prefix><name>` that is a
//! regular file (after following symlinks) with an execute bit. On Windows, a match is the first
//! `<dir>\<prefix><name>.exe` that is a regular file — `.exe` only, never `.bat` or `.cmd`, which run
//! through `cmd.exe` and its own argument-splitting rules.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Which plugin surface is being searched — sets the file-name prefix; every other rule (`PATH`-only
/// lookup, the execute bit, `.exe` only on Windows, the name grammar, no shell, `--list` and shadowing)
/// is shared between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `brygge-encode-<target>` (RFC 017 D-3).
    Encode,
    /// `brygge-transform-<name>` (RFC 018 D-6).
    Transform,
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Encode => "brygge-encode-",
            Self::Transform => "brygge-transform-",
        }
    }
}

/// The platform's expected plugin file name for `name`, under `kind`'s own prefix.
fn plugin_file_name(kind: Kind, name: &str) -> String {
    let prefix = kind.prefix();
    if cfg!(windows) {
        format!("{prefix}{name}.exe")
    } else {
        format!("{prefix}{name}")
    }
}

/// The target/transform name a candidate file name would serve under `kind`, if any: the part after
/// `kind`'s own prefix (and, on Windows, before a `.exe` suffix — a name without one, or with any other
/// suffix, names nothing).
fn name_from_file_name(kind: Kind, name: &str) -> Option<&str> {
    let base = if cfg!(windows) {
        name.strip_suffix(".exe")?
    } else {
        name
    };
    base.strip_prefix(kind.prefix())
}

/// `PATH`'s entries, in order, with every empty and every relative entry removed.
fn path_dirs() -> Vec<PathBuf> {
    path_dirs_from(std::env::var_os("PATH").as_deref())
}

/// As [`path_dirs`], over an explicit value instead of the real environment — so a test can give `PATH`
/// a crafted value (an empty entry, a relative entry) without mutating the process-wide environment,
/// which `cargo test`'s parallel threads share.
fn path_dirs_from(path_value: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    let Some(path) = path_value else {
        return Vec::new();
    };
    std::env::split_paths(path)
        .filter(|p| !p.as_os_str().is_empty() && p.is_absolute())
        .collect()
}

/// Whether `path` is a regular file (symlinks followed) brygge may run: on Unix, with at least one
/// execute bit set; elsewhere, being a regular file is all `plugin_file_name` and the `.exe` suffix
/// already guarantee (a `.bat`/`.cmd` never reaches this check at all — see [`target_from_file_name`]).
#[cfg(unix)]
fn is_runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_runnable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// The resolved, absolute path to `kind`'s own plugin named `name` on `PATH`, or `None` if it is not
/// there.
#[must_use]
pub fn find(kind: Kind, name: &str) -> Option<PathBuf> {
    find_in(kind, name, &path_dirs())
}

/// As [`find`], over an explicit, already-filtered list of directories (so a test can give a crafted
/// search order without touching the real `PATH`).
fn find_in(kind: Kind, name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let file_name = plugin_file_name(kind, name);
    dirs.iter()
        .map(|dir| dir.join(&file_name))
        .find(|p| is_runnable(p))
}

/// One plugin found on `PATH`, of whichever [`Kind`] was searched for.
pub struct Listed {
    /// Its target/transform name.
    pub name: String,
    /// Its resolved, absolute path.
    pub path: PathBuf,
    /// Whether an earlier `PATH` directory already has a plugin of the same name — [`find`] would
    /// never resolve to this one.
    pub shadowed: bool,
}

/// Every `kind`-prefixed plugin on `PATH`, by the same rules [`find`] uses, in `PATH` order (and,
/// within one directory, by name — a directory listing's own order is unspecified). A plugin whose
/// name an earlier directory already has is still listed, marked `shadowed`. No name of either kind is
/// reserved — a `transform` plugin and an `encode` plugin may share a name with no conflict, since
/// they are found under different prefixes.
#[must_use]
pub fn list(kind: Kind) -> Vec<Listed> {
    list_in(kind, &path_dirs())
}

/// As [`list`], over an explicit, already-filtered list of directories.
fn list_in(kind: Kind, dirs: &[PathBuf]) -> Vec<Listed> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut in_dir: Vec<(String, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(name) = name_from_file_name(kind, file_name) else {
                continue;
            };
            if !crate::cli::is_valid_target(name) || !is_runnable(&path) {
                continue;
            }
            in_dir.push((name.to_string(), path));
        }
        in_dir.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, path) in in_dir {
            let shadowed = !seen.insert(name.clone());
            out.push(Listed {
                name,
                path,
                shadowed,
            });
        }
    }
    out
}

/// How the plugin process ended.
pub enum Ran {
    /// It exited with this code.
    Exited(i32),
    /// It was killed by this signal (Unix only; never produced elsewhere).
    Signaled(i32),
    /// It could not even be started.
    CannotStart(std::io::Error),
}

/// Spawn `path` with `args`, inheriting stdin/stdout/stderr, and wait for it. No shell: `args` reach the
/// process exactly as given.
#[must_use]
pub fn run(path: &Path, args: &[OsString]) -> Ran {
    match std::process::Command::new(path).args(args).status() {
        Ok(status) => match status.code() {
            Some(code) => Ran::Exited(code),
            None => {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    Ran::Signaled(status.signal().unwrap_or(0))
                }
                #[cfg(not(unix))]
                {
                    Ran::Signaled(0)
                }
            }
        },
        Err(e) => Ran::CannotStart(e),
    }
}

/// The fixed name for a well-known low-numbered POSIX signal (identical on Linux and macOS); `None` for
/// anything else — brygge then reports the bare number, never a guess.
#[must_use]
pub fn signal_name(signal: i32) -> Option<&'static str> {
    Some(match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        10 => "SIGUSR1",
        11 => "SIGSEGV",
        12 => "SIGUSR2",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
