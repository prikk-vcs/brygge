//! The brygge command-line tool: carry version-control history into an intermediate representation.
//!
//! This increment implements the **read side** (RFC 004 increment 2, external design CL-01..08): `decode`
//! (Git), `inspect`, `verify` (`--internal` VF-3 / `--against-source` VF-2), and `summary`, with human and
//! machine output and the CL-08 exit-code classes. `encode` is gated pending the prikk import surface
//! (RFC 008). The IR core and `verify --internal` link no decoder (RFC 009 D-1); `decode` and
//! `verify --against-source` wire in `brygge-decode-git`.

mod cli;
mod commands;
mod exit;

use cli::Command;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match cli::parse(&args) {
        Ok(cmd) => run(cmd),
        Err(usage) => {
            eprintln!("{usage}");
            exit::FAILURE
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
        Command::EncodeGated => {
            eprintln!(
                "the prikk encoder is gated pending owner decisions (RFC 008, GATED-1..3); \
                 not available in this build"
            );
            exit::FAILURE
        }
        Command::Decode {
            kind,
            path,
            out,
            detect_renames,
            format,
        } => commands::run_decode(kind, &path, out.as_deref(), detect_renames, format),
        Command::Inspect { ir, format } => commands::run_inspect(&ir, format),
        Command::VerifyInternal { import, format } => {
            commands::run_verify_internal(&import, format)
        }
        Command::VerifyAgainstSource {
            repo,
            import,
            format,
        } => commands::run_verify_against_source(&repo, &import, format),
        Command::Summary { import, format } => commands::run_summary(&import, format),
    }
}
