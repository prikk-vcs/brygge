//! The brygge command-line tool: carry version-control history into an intermediate representation.
//!
//! The three-verb surface (external design v0.3 CL-01…CL-08, handoff `cli-and-verify-handoff-v2.md`):
//! `decode` (git, hg, svn, cvs), `inspect` (the fidelity report, `--atoms` for the reviewer's detail), and
//! `verify` (always the internal honesty checks; `--against-source` additionally re-derives and compares,
//! VF-2). `encode` is not part of the surface until prikk's import foundations exist (Track B1). The IR
//! core and `verify`'s internal checks link no decoder (RFC 009 D-1); `decode` and
//! `verify --against-source` wire in the four decoder crates. Every string that can originate in a source
//! repository is neutralized before it reaches stdout or stderr (`display`, CR-19).

mod cli;
mod commands;
mod display;
mod exit;

use cli::Command;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match cli::parse(&args) {
        Ok(cmd) => run(cmd),
        Err(usage) => {
            eprintln!("{usage}");
            exit::USAGE
        }
    };
    std::process::exit(code);
}

fn run(cmd: Command) -> i32 {
    match cmd {
        Command::Version => {
            println!("brygge {}", env!("CARGO_PKG_VERSION"));
            println!("IR contract version: {}", brygge_ir::version::CURRENT);
            exit::CLEAN
        }
        Command::Help => {
            println!("{}", cli::USAGE);
            exit::CLEAN
        }
        Command::Decode {
            kind,
            source,
            out,
            infer_renames,
            reconstruct_refs,
            format,
        } => commands::run_decode(kind, &source, &out, infer_renames, reconstruct_refs, format),
        Command::Inspect {
            artifact,
            atoms,
            format,
        } => commands::run_inspect(&artifact, atoms, format),
        Command::Verify {
            artifact,
            against_source,
            format,
        } => commands::run_verify(&artifact, against_source.as_deref(), format),
    }
}
