//! A small hand-rolled argument parser for brygge's read-side command surface (external design CL-01..07,
//! handoff D-A). No `clap`: the surface is small and brygge keeps its dependency surface small (RFC 009).

use std::path::PathBuf;

/// Report rendering (CL-07): human by default, machine for CI gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// The human-facing form.
    Human,
    /// The stable, versioned, line-oriented form.
    Machine,
}

/// A parsed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `decode git <path> [--ir <out>] [--detect-renames] [--format …]`
    Decode {
        /// The source repository path.
        path: PathBuf,
        /// Where to write the IR artifact, if given.
        out: Option<PathBuf>,
        /// Turn on opt-in, always-marked rename inference.
        detect_renames: bool,
        /// Output format.
        format: Format,
    },
    /// `inspect --ir <file> [--format …]`
    Inspect {
        /// The IR artifact to read.
        ir: PathBuf,
        /// Output format.
        format: Format,
    },
    /// `verify --internal --import <file> [--format …]`
    VerifyInternal {
        /// The IR artifact to check.
        import: PathBuf,
        /// Output format.
        format: Format,
    },
    /// `verify --against-source <repo> --import <file> [--format …]`
    VerifyAgainstSource {
        /// The original source repository.
        repo: PathBuf,
        /// The IR artifact to check against it.
        import: PathBuf,
        /// Output format.
        format: Format,
    },
    /// `summary --import <file> [--format …]`
    Summary {
        /// The IR artifact whose fidelity summary to reproduce.
        import: PathBuf,
        /// Output format.
        format: Format,
    },
    /// `encode …` — gated pending the prikk import surface (RFC 008).
    EncodeGated,
    /// `--version`.
    Version,
    /// `--help` or no arguments.
    Help,
}

/// The top-level usage text.
pub const USAGE: &str = "\
brygge — carry version-control history into an intermediate representation (IR).

USAGE:
  brygge decode git <path> [--ir <out>] [--detect-renames] [--format human|machine]
  brygge inspect --ir <file> [--format human|machine]
  brygge verify --internal --import <file> [--format human|machine]
  brygge verify --against-source <repo> --import <file> [--format human|machine]
  brygge summary --import <file> [--format human|machine]
  brygge --version | --help

COMMANDS:
  decode   read a source repository into an IR artifact (Git only in this build)
  inspect  list atoms with their epistemic status, source ids, and the loss boundary
  verify   --internal: honesty checks provable with no source (VF-3);
           --against-source: re-derive from the source and confirm correspondence (VF-2)
  summary  reproduce the fidelity summary from an artifact alone (FS-02)
  encode   GATED — the prikk encoder awaits owner decisions (RFC 008); not in this build";

/// Parse arguments (those after the program name) into a [`Command`].
///
/// # Errors
/// Returns a usage message on a malformed or unknown invocation.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut it = args.iter();
    let Some(first) = it.next() else {
        return Ok(Command::Help);
    };
    match first.as_str() {
        "--version" | "-V" => Ok(Command::Version),
        "--help" | "-h" => Ok(Command::Help),
        "encode" => Ok(Command::EncodeGated),
        "decode" => parse_decode(args.get(1..).unwrap_or(&[])),
        "inspect" => parse_inspect(args.get(1..).unwrap_or(&[])),
        "verify" => parse_verify(args.get(1..).unwrap_or(&[])),
        "summary" => parse_summary(args.get(1..).unwrap_or(&[])),
        other => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    }
}

fn wants_help(args: &[String]) -> bool {
    args.iter().any(|a| a == "--help" || a == "-h")
}

fn parse_format(rest: &mut std::slice::Iter<'_, String>) -> Result<Format, String> {
    match rest.next().map(String::as_str) {
        Some("human") => Ok(Format::Human),
        Some("machine") => Ok(Format::Machine),
        Some(other) => Err(format!(
            "--format expects 'human' or 'machine', got '{other}'"
        )),
        None => Err("--format needs a value ('human' or 'machine')".to_string()),
    }
}

fn need_value(rest: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<PathBuf, String> {
    rest.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn parse_decode(args: &[String]) -> Result<Command, String> {
    if wants_help(args) {
        return Ok(Command::Help);
    }
    let mut it = args.iter();
    match it.next().map(String::as_str) {
        Some("git") => {}
        Some(other) => {
            return Err(format!(
                "source kind '{other}' is not supported (only 'git' in this build)"
            ));
        }
        None => return Err("decode needs a source kind and path: decode git <path>".to_string()),
    }
    let mut path: Option<PathBuf> = None;
    let mut out = None;
    let mut detect_renames = false;
    let mut format = Format::Human;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--ir" => out = Some(need_value(&mut it, "--ir")?),
            "--detect-renames" => detect_renames = true,
            "--format" => format = parse_format(&mut it)?,
            other if other.starts_with('-') => return Err(format!("unknown flag '{other}'")),
            other => path = Some(PathBuf::from(other)),
        }
    }
    let path = path.ok_or_else(|| "decode git needs a repository path".to_string())?;
    Ok(Command::Decode {
        path,
        out,
        detect_renames,
        format,
    })
}

fn parse_inspect(args: &[String]) -> Result<Command, String> {
    if wants_help(args) {
        return Ok(Command::Help);
    }
    let mut it = args.iter();
    let mut ir: Option<PathBuf> = None;
    let mut format = Format::Human;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--ir" => ir = Some(need_value(&mut it, "--ir")?),
            "--format" => format = parse_format(&mut it)?,
            other => return Err(format!("unknown argument '{other}' for inspect")),
        }
    }
    let ir = ir.ok_or_else(|| "inspect needs --ir <file>".to_string())?;
    Ok(Command::Inspect { ir, format })
}

fn parse_verify(args: &[String]) -> Result<Command, String> {
    if wants_help(args) {
        return Ok(Command::Help);
    }
    let mut it = args.iter();
    let mut internal = false;
    let mut against: Option<PathBuf> = None;
    let mut import: Option<PathBuf> = None;
    let mut format = Format::Human;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--internal" => internal = true,
            "--against-source" => against = Some(need_value(&mut it, "--against-source")?),
            "--import" => import = Some(need_value(&mut it, "--import")?),
            "--format" => format = parse_format(&mut it)?,
            other => return Err(format!("unknown argument '{other}' for verify")),
        }
    }
    let import = import.ok_or_else(|| "verify needs --import <file>".to_string())?;
    match (internal, against) {
        (true, Some(_)) => {
            Err("verify takes either --internal or --against-source, not both".to_string())
        }
        (true, None) => Ok(Command::VerifyInternal { import, format }),
        (false, Some(repo)) => Ok(Command::VerifyAgainstSource {
            repo,
            import,
            format,
        }),
        (false, None) => Err("verify needs --internal or --against-source <repo>".to_string()),
    }
}

fn parse_summary(args: &[String]) -> Result<Command, String> {
    if wants_help(args) {
        return Ok(Command::Help);
    }
    let mut it = args.iter();
    let mut import: Option<PathBuf> = None;
    let mut format = Format::Human;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--import" => import = Some(need_value(&mut it, "--import")?),
            "--format" => format = parse_format(&mut it)?,
            other => return Err(format!("unknown argument '{other}' for summary")),
        }
    }
    let import = import.ok_or_else(|| "summary needs --import <file>".to_string())?;
    Ok(Command::Summary { import, format })
}

#[cfg(test)]
mod tests;
