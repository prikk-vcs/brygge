//! A small hand-rolled argument parser for brygge's three-verb command surface (external design v0.3
//! CL-01…CL-08, handoff `cli-and-verify-handoff-v2.md`). No `clap`: the surface is small and brygge keeps
//! its dependency surface small (RFC 009).
//!
//! `decode`, `inspect`, `verify` — one noun per concept, nothing silently ignored. Every usage problem
//! (an unknown command, an unknown or missing flag, a missing value, a missing positional, a repeated
//! flag, or an option given to a source kind it does not apply to) is a parse error that `main` turns into
//! exit `USAGE` (2), naming the problem and pointing to `brygge <command> --help`.

use std::path::PathBuf;

/// Report rendering (CL-07): human by default, machine for CI gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// The human-facing form.
    Human,
    /// The stable, versioned, line-oriented form.
    Machine,
}

/// A source kind for `decode` (external design CL-01: the set is open by design).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Git (`brygge-decode-git`).
    Git,
    /// Mercurial (`brygge-decode-hg`).
    Hg,
    /// Subversion (`brygge-decode-svn`) — a repository directory or a dumpfile.
    Svn,
    /// CVS (`brygge-decode-cvs`) — a local repository directory of RCS `,v` files.
    Cvs,
}

impl SourceKind {
    /// The lowercase label used on the command line and in messages.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Hg => "hg",
            Self::Svn => "svn",
            Self::Cvs => "cvs",
        }
    }
}

/// A parsed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `decode <git|hg|svn|cvs> <source> --out <artifact> [--infer-renames | --reconstruct-refs] [--format …]`
    Decode {
        /// Which source decoder to use.
        kind: SourceKind,
        /// The source repository path (for svn: a repository directory or a dumpfile).
        source: PathBuf,
        /// Where to write the IR artifact. Required (CL-01: `decode` never runs without producing it).
        out: PathBuf,
        /// Turn on opt-in, always-marked rename inference (git/hg only).
        infer_renames: bool,
        /// Reconstruct branch/tag refs by convention or symbol, marking each `Derived` (svn/cvs only).
        reconstruct_refs: bool,
        /// Output format.
        format: Format,
    },
    /// `inspect <artifact> [--atoms] [--format …]`
    Inspect {
        /// The IR artifact to read.
        artifact: PathBuf,
        /// Append the per-atom listing (the reviewer's detail) to the default fidelity report.
        atoms: bool,
        /// Output format.
        format: Format,
    },
    /// `verify <artifact> [--against-source <source>] [--format …]`
    Verify {
        /// The IR artifact to check.
        artifact: PathBuf,
        /// If given, additionally re-derive from this source and confirm correspondence (VF-2).
        against_source: Option<PathBuf>,
        /// Output format.
        format: Format,
    },
    /// `--version`.
    Version,
    /// `--help`, `<command> --help`, or no arguments.
    Help,
}

/// The top-level usage text. Lists only commands that exist (CL-06).
pub const USAGE: &str = "\
brygge — carry version-control history into an intermediate representation (IR).

USAGE:
  brygge decode <git|hg|svn|cvs> <source> --out <artifact> [--infer-renames | --reconstruct-refs] [--format human|machine]
  brygge inspect <artifact> [--atoms] [--format human|machine]
  brygge verify <artifact> [--against-source <source>] [--format human|machine]
  brygge --version | --help | <command> --help

COMMANDS:
  decode   read a source into an IR artifact (git, hg, svn, or cvs in this build). --out is required.
           For svn, <source> is a repository directory (dumped read-only via `svnadmin dump`) or a
           dumpfile; for cvs, a local repository directory of RCS ,v files. --infer-renames (git/hg) and
           --reconstruct-refs (svn/cvs) each turn on an opt-in, always-marked derived layer; giving either
           to a source kind it does not apply to is a usage error
  inspect  print the fidelity report an artifact alone reproduces; --atoms adds the per-atom
           listing: epistemic status, source ids, derived parameters, and the loss boundary
  verify   the honesty checks any reader can run with no source (always); with --against-source, also
           re-derive from the source and confirm correspondence — the two results are reported
           separately, never merged

VOCABULARY: artifact, source, stated, derived, dropped, flagged, refused, Unverifiable.";

fn usage_error(command: &str, problem: impl std::fmt::Display) -> String {
    format!("{problem}\n\nrun `brygge {command} --help` for usage")
}

fn inapplicable_option(option: &str, kinds: &str, kind: SourceKind) -> String {
    format!("--{option} applies to {kinds}, not to {}", kind.label())
}

/// Parse arguments (those after the program name) into a [`Command`].
///
/// # Errors
/// Returns a usage message — the problem, then a pointer to `brygge <command> --help` — on a malformed or
/// unknown invocation. The caller exits `USAGE` (2) on this path.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut it = args.iter();
    let Some(first) = it.next() else {
        return Ok(Command::Help);
    };
    match first.as_str() {
        "--version" | "-V" => Ok(Command::Version),
        "--help" | "-h" => Ok(Command::Help),
        "decode" => parse_decode(args.get(1..).unwrap_or(&[])),
        "inspect" => parse_inspect(args.get(1..).unwrap_or(&[])),
        "verify" => parse_verify(args.get(1..).unwrap_or(&[])),
        "encode" => Err(
            "'encode' is not available yet: the prikk encoder follows prikk's import foundations \
             (see ROADMAP)"
                .to_string(),
        ),
        other => Err(format!(
            "unknown command '{other}'\n\nrun `brygge --help` for usage"
        )),
    }
}

fn wants_help(args: &[String]) -> bool {
    args.iter().any(|a| a == "--help" || a == "-h")
}

fn parse_format(rest: &mut std::slice::Iter<'_, String>, command: &str) -> Result<Format, String> {
    match rest.next().map(String::as_str) {
        Some("human") => Ok(Format::Human),
        Some("machine") => Ok(Format::Machine),
        Some(other) => Err(usage_error(
            command,
            format!("--format expects 'human' or 'machine', got '{other}'"),
        )),
        None => Err(usage_error(
            command,
            "--format needs a value ('human' or 'machine')",
        )),
    }
}

fn need_value(
    rest: &mut std::slice::Iter<'_, String>,
    flag: &str,
    command: &str,
) -> Result<PathBuf, String> {
    rest.next()
        .map(PathBuf::from)
        .ok_or_else(|| usage_error(command, format!("{flag} needs a value")))
}

fn parse_decode(args: &[String]) -> Result<Command, String> {
    if wants_help(args) {
        return Ok(Command::Help);
    }
    let mut it = args.iter();
    let kind = match it.next().map(String::as_str) {
        Some("git") => SourceKind::Git,
        Some("hg") => SourceKind::Hg,
        Some("svn") => SourceKind::Svn,
        Some("cvs") => SourceKind::Cvs,
        Some(other) => {
            return Err(usage_error(
                "decode",
                format!(
                    "source kind '{other}' is not supported (git, hg, svn, or cvs in this build)"
                ),
            ));
        }
        None => {
            return Err(usage_error(
                "decode",
                "decode needs a source kind and a source: decode <git|hg|svn|cvs> <source> --out <artifact>",
            ));
        }
    };
    let mut source: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut out_seen = false;
    let mut infer_renames = false;
    let mut infer_renames_seen = false;
    let mut reconstruct_refs = false;
    let mut reconstruct_refs_seen = false;
    let mut format = Format::Human;
    let mut format_seen = false;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--out" => {
                if out_seen {
                    return Err(usage_error("decode", "--out given more than once"));
                }
                out_seen = true;
                out = Some(need_value(&mut it, "--out", "decode")?);
            }
            "--infer-renames" => {
                if infer_renames_seen {
                    return Err(usage_error(
                        "decode",
                        "--infer-renames given more than once",
                    ));
                }
                infer_renames_seen = true;
                infer_renames = true;
            }
            "--reconstruct-refs" => {
                if reconstruct_refs_seen {
                    return Err(usage_error(
                        "decode",
                        "--reconstruct-refs given more than once",
                    ));
                }
                reconstruct_refs_seen = true;
                reconstruct_refs = true;
            }
            "--format" => {
                if format_seen {
                    return Err(usage_error("decode", "--format given more than once"));
                }
                format_seen = true;
                format = parse_format(&mut it, "decode")?;
            }
            other if other.starts_with('-') => {
                return Err(usage_error("decode", format!("unknown flag '{other}'")));
            }
            other => {
                if source.is_some() {
                    return Err(usage_error(
                        "decode",
                        format!("unexpected extra argument '{other}'"),
                    ));
                }
                source = Some(PathBuf::from(other));
            }
        }
    }
    let source = source
        .ok_or_else(|| usage_error("decode", "decode needs a source: <git|hg|svn|cvs> <source>"))?;
    let out = out.ok_or_else(|| usage_error("decode", "decode needs --out <artifact>"))?;

    if infer_renames && !matches!(kind, SourceKind::Git | SourceKind::Hg) {
        return Err(usage_error(
            "decode",
            inapplicable_option("infer-renames", "git, hg", kind),
        ));
    }
    if reconstruct_refs && !matches!(kind, SourceKind::Svn | SourceKind::Cvs) {
        return Err(usage_error(
            "decode",
            inapplicable_option("reconstruct-refs", "svn, cvs", kind),
        ));
    }

    Ok(Command::Decode {
        kind,
        source,
        out,
        infer_renames,
        reconstruct_refs,
        format,
    })
}

fn parse_inspect(args: &[String]) -> Result<Command, String> {
    if wants_help(args) {
        return Ok(Command::Help);
    }
    let mut it = args.iter();
    let mut artifact: Option<PathBuf> = None;
    let mut atoms = false;
    let mut atoms_seen = false;
    let mut format = Format::Human;
    let mut format_seen = false;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--atoms" => {
                if atoms_seen {
                    return Err(usage_error("inspect", "--atoms given more than once"));
                }
                atoms_seen = true;
                atoms = true;
            }
            "--format" => {
                if format_seen {
                    return Err(usage_error("inspect", "--format given more than once"));
                }
                format_seen = true;
                format = parse_format(&mut it, "inspect")?;
            }
            other if other.starts_with('-') => {
                return Err(usage_error("inspect", format!("unknown flag '{other}'")));
            }
            other => {
                if artifact.is_some() {
                    return Err(usage_error(
                        "inspect",
                        format!("unexpected extra argument '{other}'"),
                    ));
                }
                artifact = Some(PathBuf::from(other));
            }
        }
    }
    let artifact = artifact.ok_or_else(|| usage_error("inspect", "inspect needs an <artifact>"))?;
    Ok(Command::Inspect {
        artifact,
        atoms,
        format,
    })
}

fn parse_verify(args: &[String]) -> Result<Command, String> {
    if wants_help(args) {
        return Ok(Command::Help);
    }
    let mut it = args.iter();
    let mut artifact: Option<PathBuf> = None;
    let mut against_source: Option<PathBuf> = None;
    let mut against_source_seen = false;
    let mut format = Format::Human;
    let mut format_seen = false;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--against-source" => {
                if against_source_seen {
                    return Err(usage_error(
                        "verify",
                        "--against-source given more than once",
                    ));
                }
                against_source_seen = true;
                against_source = Some(need_value(&mut it, "--against-source", "verify")?);
            }
            "--format" => {
                if format_seen {
                    return Err(usage_error("verify", "--format given more than once"));
                }
                format_seen = true;
                format = parse_format(&mut it, "verify")?;
            }
            other if other.starts_with('-') => {
                return Err(usage_error("verify", format!("unknown flag '{other}'")));
            }
            other => {
                if artifact.is_some() {
                    return Err(usage_error(
                        "verify",
                        format!("unexpected extra argument '{other}'"),
                    ));
                }
                artifact = Some(PathBuf::from(other));
            }
        }
    }
    let artifact = artifact.ok_or_else(|| usage_error("verify", "verify needs an <artifact>"))?;
    Ok(Command::Verify {
        artifact,
        against_source,
        format,
    })
}

#[cfg(test)]
mod tests;
