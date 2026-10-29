//! The brygge command-line tool: carry version-control history into an intermediate representation.
//!
//! The command surface (external design v0.3 CL-01…CL-08, handoff `cli-and-verify-handoff-v2.md`):
//! `decode` (git, hg, svn, cvs), `inspect` (the fidelity report, `--atoms` for the reviewer's detail),
//! `verify` (always the internal honesty checks; `--against-source` additionally re-derives and
//! compares, VF-2), and `encode <target>` (RFC 017 D-3): runs an external encoder plugin,
//! `brygge-encode-<target>`, found on `PATH` only — never brygge's own; `encode prikk` is reserved for
//! the built-in encoder, which still waits on prikk's import foundations (Track B1). The IR core and
//! `verify`'s internal checks link no decoder (RFC 009 D-1); `decode` and `verify --against-source` wire
//! in the four decoder crates. Every string that can originate in a source repository is neutralized
//! before it reaches stdout or stderr (`display`, CR-19).
//!
//! `print!`/`println!`/`eprint!`/`eprintln!` are denied outside tests: every write goes through
//! [`output::stdout`]/[`output::stderr`] instead, so a reader that closes the pipe early (`brygge … |
//! head`) cannot turn into a panic (`cli-broken-pipe-handoff-v1.md`).
#![deny(clippy::print_stdout, clippy::print_stderr)]

mod cli;
mod commands;
mod display;
mod exit;
mod output;
mod plugin;

use cli::Command;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match cli::parse(&args) {
        Ok(cmd) => run(cmd),
        Err(usage) => {
            output::stderr(&usage);
            exit::USAGE
        }
    };
    std::process::exit(code);
}

fn run(cmd: Command) -> i32 {
    match cmd {
        Command::Version => {
            let report = format!(
                "brygge {}\nIR contract version: {}\n",
                env!("CARGO_PKG_VERSION"),
                brygge_ir::version::CURRENT
            );
            if let Some(code) = output::stdout(&report) {
                return code;
            }
            exit::CLEAN
        }
        Command::Help => {
            if let Some(code) = output::stdout(&format!("{}\n", cli::USAGE)) {
                return code;
            }
            exit::CLEAN
        }
        Command::Decode {
            kind,
            source,
            out,
            infer_renames,
            reconstruct_refs,
            spill_dir,
            format,
        } => commands::run_decode(
            kind,
            &source,
            &out,
            infer_renames,
            reconstruct_refs,
            spill_dir.as_deref(),
            format,
        ),
        Command::Inspect {
            artifact,
            atoms,
            format,
        } => commands::run_inspect(&artifact, atoms, format),
        Command::Verify {
            artifact,
            against_source,
            spill_dir,
            format,
        } => commands::run_verify(
            &artifact,
            against_source.as_deref(),
            spill_dir.as_deref(),
            format,
        ),
        Command::Encode {
            target,
            artifact,
            plugin_args,
            format,
        } => commands::run_encode(&target, &artifact, &plugin_args, format),
        Command::EncodeList { format } => commands::run_encode_list(format),
    }
}
