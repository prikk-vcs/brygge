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
    /// `decode <git|hg|svn|cvs> <source> --out <artifact> [--infer-renames | --reconstruct-refs] [--spill-dir <DIR>] [--format …]`
    Decode {
        /// Which source decoder to use.
        kind: SourceKind,
        /// The source repository path (for svn: a repository directory or a dumpfile).
        source: PathBuf,
        /// Where to write the IR artifact. Required (CL-01: `decode` never runs without producing it).
        out: PathBuf,
        /// Turn on opt-in, always-marked rename inference (git only; Mercurial records its renames).
        infer_renames: bool,
        /// Reconstruct branch/tag refs by convention or symbol, marking each `Derived` (svn/cvs only).
        reconstruct_refs: bool,
        /// Where to keep file content while decoding, instead of in memory (RFC 015 D-3/D-3a). Must
        /// already exist and be a directory; `None` means the output artifact's own directory.
        spill_dir: Option<PathBuf>,
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
    /// `verify <artifact> [--against-source <source>] [--spill-dir <DIR>] [--format …]`
    Verify {
        /// The IR artifact to check.
        artifact: PathBuf,
        /// If given, additionally re-derive from this source and confirm correspondence (VF-2).
        against_source: Option<PathBuf>,
        /// Where `--against-source`'s re-decode keeps file content while decoding, instead of in
        /// memory (RFC 015 D-4, as `decode`'s own `--spill-dir`). Must already exist and be a
        /// directory; `None` means the artifact's own directory. Meaningless, and rejected, without
        /// `--against-source`.
        spill_dir: Option<PathBuf>,
        /// Output format.
        format: Format,
    },
    /// `encode <target> <artifact> [--format …] [-- <plugin arguments>…]` (RFC 017 D-3).
    Encode {
        /// The plugin's target name (`[a-z0-9][a-z0-9-]*`, at most 64 bytes; never `prikk`, reserved).
        target: String,
        /// The IR artifact to encode.
        artifact: PathBuf,
        /// Everything after `--`, verbatim, passed to the plugin unchanged.
        plugin_args: Vec<String>,
        /// Output format, for the verify-first report and the banner (OQ-4).
        format: Format,
    },
    /// `encode --list [--format …]` (RFC 017 D-3).
    EncodeList {
        /// Output format.
        format: Format,
    },
    /// `transform <name> <artifact> --out <file> [--format …] [-- <plugin arguments>…]` (RFC 018 D-6).
    Transform {
        /// The plugin's own name (`[a-z0-9][a-z0-9-]*`, at most 64 bytes — the same grammar `encode`'s
        /// target uses).
        name: String,
        /// The IR artifact to transform.
        artifact: PathBuf,
        /// Where to write the transformed artifact. Required: `transform` never runs a plugin without
        /// somewhere to put what it produces.
        out: PathBuf,
        /// Everything after `--`, verbatim, passed to the plugin unchanged.
        plugin_args: Vec<String>,
        /// Output format, for the verify-first report and the banner.
        format: Format,
    },
    /// `transform --list [--format …]` (RFC 018 D-6).
    TransformList {
        /// Output format.
        format: Format,
    },
    /// `--version`.
    Version,
    /// `--help`, `<command> --help`, or no arguments.
    Help,
}

/// The target name reserved for the built-in encoder (Track B1, RFC 008): never looked up on `PATH`.
pub const PRIKK_TARGET: &str = "prikk";

/// Today's message for `encode prikk` (kept until Track B1 lands): a usage error, not a lookup failure.
pub const PRIKK_NOT_AVAILABLE: &str = "'encode prikk' is not available yet: the prikk encoder follows \
                                        prikk's import foundations (see ROADMAP)";

/// Whether `target` is a valid `encode` target name: `[a-z0-9][a-z0-9-]*`, at most 64 bytes. Shared by
/// argument parsing and by `encode --list`'s filter over `PATH` (a plugin file whose suffix is not a
/// valid target is not listed).
#[must_use]
pub fn is_valid_target(target: &str) -> bool {
    if target.is_empty() || target.len() > 64 {
        return false;
    }
    let mut bytes = target.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    target
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The top-level usage text. Lists only commands that exist (CL-06).
pub const USAGE: &str = "\
brygge — carry version-control history into an intermediate representation (IR).

USAGE:
  brygge decode <git|hg|svn|cvs> <source> --out <artifact> [--infer-renames | --reconstruct-refs] [--spill-dir <DIR>] [--format human|machine]
  brygge inspect <artifact> [--atoms] [--format human|machine]
  brygge verify <artifact> [--against-source <source> [--spill-dir <DIR>]] [--format human|machine]
  brygge encode <target> <artifact> [--format human|machine] [-- <plugin arguments>…]
  brygge encode --list [--format human|machine]
  brygge transform <name> <artifact> --out <file> [--format human|machine] [-- <plugin arguments>…]
  brygge transform --list [--format human|machine]
  brygge --version | --help | <command> --help

COMMANDS:
  decode   read a source into an IR artifact (git, hg, svn, or cvs in this build). --out is required.
           For svn, <source> is a repository directory (dumped read-only via `svnadmin dump`) or a
           dumpfile; for cvs, a local repository directory of RCS ,v files. --infer-renames (git) and
           --reconstruct-refs (svn/cvs) each turn on an opt-in, always-marked derived layer; giving either
           to a source kind it does not apply to is a usage error. File content is kept on disk while
           decoding, in a private file removed when brygge exits — --spill-dir <DIR> chooses where (it
           must already exist); the default is --out's own directory. The option never changes the
           artifact; a RAM-backed directory (tmpfs) moves the memory cost rather than removing it.
           Artifacts are compressed (contract 0.3.0)
  inspect  print the fidelity report an artifact alone reproduces; --atoms adds the per-atom
           listing: epistemic status, source ids, derived parameters, and the loss boundary
  verify   the honesty checks any reader can run with no source (always); with --against-source, also
           re-derive from the source and confirm correspondence — the two results are reported
           separately, never merged. The re-derive's own file content is kept on disk the same way
           decode's is; --spill-dir <DIR> chooses where (it must already exist, and only applies with
           --against-source); the default is the artifact's own directory
  encode   run an external encoder plugin, `brygge-encode-<target>`, found on PATH only — never the
           bare name, never the current directory. brygge verifies the artifact first
           (the same checks as `verify`, without --against-source) and refuses to run the plugin on
           anything but a clean pass, exiting with verify's own code. Arguments after -- are passed to
           the plugin verbatim, with no shell. The plugin is not part of brygge, and its stdout is the
           user's output, untouched. `encode --list` shows every brygge-encode-* plugin found on PATH,
           in PATH order, marking one shadowed by an earlier directory's plugin of the same target.
           `encode prikk` is reserved for the built-in encoder (Track B1): not available yet, and never
           looked up on PATH
  transform run an external transform plugin, `brygge-transform-<name>`, found on PATH only — never the
           bare name, never the current directory. brygge verifies the input artifact first (the same
           checks as `verify`, without --against-source) and refuses to run the plugin on anything but a
           clean pass; it then refuses an input with two atoms sharing one source identity, before the
           plugin ever runs. The plugin writes its own output to a private temp file in --out's own
           directory; brygge then runs the same internal checks on that output, and brygge_ir::transform's
           own honesty check, before accepting it — any failure leaves --out untouched and exits 50, never
           writing a half-trusted result. --out is required, and is treated exactly as `decode --out` is
           (silently, atomically overwritten if it already exists); it must not be the same file as
           <artifact>. Arguments after -- are passed to the plugin verbatim, with no shell. The plugin is
           not part of brygge. `transform --list` shows every brygge-transform-* plugin found on PATH, in
           PATH order, marking one shadowed by an earlier directory's plugin of the same name

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
        "encode" => parse_encode(args.get(1..).unwrap_or(&[])),
        "transform" => parse_transform(args.get(1..).unwrap_or(&[])),
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
    let mut spill_dir: Option<PathBuf> = None;
    let mut spill_dir_seen = false;
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
            "--spill-dir" => {
                if spill_dir_seen {
                    return Err(usage_error("decode", "--spill-dir given more than once"));
                }
                spill_dir_seen = true;
                spill_dir = Some(need_value(&mut it, "--spill-dir", "decode")?);
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

    if infer_renames && !matches!(kind, SourceKind::Git) {
        return Err(usage_error(
            "decode",
            inapplicable_option("infer-renames", "git", kind),
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
        spill_dir,
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
    let mut spill_dir: Option<PathBuf> = None;
    let mut spill_dir_seen = false;
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
            "--spill-dir" => {
                if spill_dir_seen {
                    return Err(usage_error("verify", "--spill-dir given more than once"));
                }
                spill_dir_seen = true;
                spill_dir = Some(need_value(&mut it, "--spill-dir", "verify")?);
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
    if spill_dir.is_some() && against_source.is_none() {
        return Err(usage_error(
            "verify",
            "--spill-dir only applies with --against-source",
        ));
    }
    Ok(Command::Verify {
        artifact,
        against_source,
        spill_dir,
        format,
    })
}

fn parse_encode(args: &[String]) -> Result<Command, String> {
    if wants_help(args) {
        return Ok(Command::Help);
    }
    let mut it = args.iter();
    let Some(first) = it.next() else {
        return Err(usage_error(
            "encode",
            "encode needs a target: encode <target> <artifact> [-- <plugin arguments>...] (see \
             encode --list)",
        ));
    };
    if first == "--list" {
        return parse_encode_list(args.get(1..).unwrap_or(&[]));
    }
    if first.starts_with('-') {
        return Err(usage_error("encode", format!("unknown flag '{first}'")));
    }
    let target = first.clone();
    if target == PRIKK_TARGET {
        return Err(PRIKK_NOT_AVAILABLE.to_string());
    }
    if !is_valid_target(&target) {
        return Err(usage_error(
            "encode",
            format!(
                "target '{target}' is not valid: it must match [a-z0-9][a-z0-9-]*, at most 64 bytes"
            ),
        ));
    }
    let mut artifact: Option<PathBuf> = None;
    let mut plugin_args: Vec<String> = Vec::new();
    let mut format = Format::Human;
    let mut format_seen = false;
    let mut saw_dashdash = false;
    while let Some(arg) = it.next() {
        if saw_dashdash {
            plugin_args.push(arg.clone());
            continue;
        }
        match arg.as_str() {
            "--" => saw_dashdash = true,
            "--format" => {
                if format_seen {
                    return Err(usage_error("encode", "--format given more than once"));
                }
                format_seen = true;
                format = parse_format(&mut it, "encode")?;
            }
            other if other.starts_with('-') => {
                return Err(usage_error("encode", format!("unknown flag '{other}'")));
            }
            other => {
                if artifact.is_some() {
                    return Err(usage_error(
                        "encode",
                        format!(
                            "unexpected extra argument '{other}' (plugin arguments must follow --)"
                        ),
                    ));
                }
                artifact = Some(PathBuf::from(other));
            }
        }
    }
    let artifact = artifact.ok_or_else(|| {
        usage_error(
            "encode",
            "encode needs an <artifact>: encode <target> <artifact>",
        )
    })?;
    Ok(Command::Encode {
        target,
        artifact,
        plugin_args,
        format,
    })
}

fn parse_encode_list(args: &[String]) -> Result<Command, String> {
    if wants_help(args) {
        return Ok(Command::Help);
    }
    let mut it = args.iter();
    let mut format = Format::Human;
    let mut format_seen = false;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--format" => {
                if format_seen {
                    return Err(usage_error("encode", "--format given more than once"));
                }
                format_seen = true;
                format = parse_format(&mut it, "encode")?;
            }
            other => {
                return Err(usage_error(
                    "encode",
                    format!("unexpected argument '{other}'"),
                ));
            }
        }
    }
    Ok(Command::EncodeList { format })
}

fn parse_transform(args: &[String]) -> Result<Command, String> {
    if wants_help(args) {
        return Ok(Command::Help);
    }
    let mut it = args.iter();
    let Some(first) = it.next() else {
        return Err(usage_error(
            "transform",
            "transform needs a name: transform <name> <artifact> --out <file> [-- <plugin \
             arguments>...] (see transform --list)",
        ));
    };
    if first == "--list" {
        return parse_transform_list(args.get(1..).unwrap_or(&[]));
    }
    if first.starts_with('-') {
        return Err(usage_error("transform", format!("unknown flag '{first}'")));
    }
    let name = first.clone();
    if !is_valid_target(&name) {
        return Err(usage_error(
            "transform",
            format!(
                "name '{name}' is not valid: it must match [a-z0-9][a-z0-9-]*, at most 64 bytes"
            ),
        ));
    }
    let mut artifact: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut out_seen = false;
    let mut plugin_args: Vec<String> = Vec::new();
    let mut format = Format::Human;
    let mut format_seen = false;
    let mut saw_dashdash = false;
    while let Some(arg) = it.next() {
        if saw_dashdash {
            plugin_args.push(arg.clone());
            continue;
        }
        match arg.as_str() {
            "--" => saw_dashdash = true,
            "--out" => {
                if out_seen {
                    return Err(usage_error("transform", "--out given more than once"));
                }
                out_seen = true;
                out = Some(need_value(&mut it, "--out", "transform")?);
            }
            "--format" => {
                if format_seen {
                    return Err(usage_error("transform", "--format given more than once"));
                }
                format_seen = true;
                format = parse_format(&mut it, "transform")?;
            }
            other if other.starts_with('-') => {
                return Err(usage_error("transform", format!("unknown flag '{other}'")));
            }
            other => {
                if artifact.is_some() {
                    return Err(usage_error(
                        "transform",
                        format!(
                            "unexpected extra argument '{other}' (plugin arguments must follow --)"
                        ),
                    ));
                }
                artifact = Some(PathBuf::from(other));
            }
        }
    }
    let artifact = artifact.ok_or_else(|| {
        usage_error(
            "transform",
            "transform needs an <artifact>: transform <name> <artifact> --out <file>",
        )
    })?;
    let out = out.ok_or_else(|| usage_error("transform", "transform needs --out <file>"))?;
    Ok(Command::Transform {
        name,
        artifact,
        out,
        plugin_args,
        format,
    })
}

fn parse_transform_list(args: &[String]) -> Result<Command, String> {
    if wants_help(args) {
        return Ok(Command::Help);
    }
    let mut it = args.iter();
    let mut format = Format::Human;
    let mut format_seen = false;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--format" => {
                if format_seen {
                    return Err(usage_error("transform", "--format given more than once"));
                }
                format_seen = true;
                format = parse_format(&mut it, "transform")?;
            }
            other => {
                return Err(usage_error(
                    "transform",
                    format!("unexpected argument '{other}'"),
                ));
            }
        }
    }
    Ok(Command::TransformList { format })
}

#[cfg(test)]
mod tests;
