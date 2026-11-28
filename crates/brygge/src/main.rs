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

mod authors;
mod cli;
mod commands;
mod display;
mod encode_git;
mod exit;
mod output;
mod plugin;
mod watchdog;

use cli::Command;

fn main() {
    // RFC 022 D-2: brygge's own internal watchdog entry, checked before anything else — including
    // argument parsing, so it is never reachable through the ordinary command surface, never in
    // `--help`, and never confused with a usage error. D-1a(a): this is the only place that ever
    // constructs the watchdog's half of the protocol; a command function never does.
    if let Some(code) = watchdog::maybe_run_internal_entry() {
        std::process::exit(code);
    }

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
    // RFC 022 D-1a(a)/W1: exactly one function (`run_writing`, below) ever calls `Watchdog::start()`,
    // and only for the forms `watchdog::command_needs_watchdog` names — itself the one, independently
    // tested, pure source of truth for which commands those are. Every command function still takes the
    // guard by `&mut` (`Guard::disabled()` for every other command, and for every unit test) — a command
    // function can never start a watchdog merely by being called, however a test calls it.
    if watchdog::command_needs_watchdog(&cmd) {
        return run_writing(cmd);
    }
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
        Command::Inspect {
            artifact,
            atoms,
            format,
        } => commands::run_inspect(&artifact, atoms, format),
        Command::InspectAuthors { artifact, format } => {
            commands::run_inspect_authors(&artifact, format)
        }
        // Reached only when `against_source` is `None` (otherwise `command_needs_watchdog` already
        // routed this command to `run_writing`, above) — read-only, so `Guard::disabled()`.
        Command::Verify {
            artifact,
            against_source,
            against_target,
            spill_dir,
            format,
        } => commands::run_verify(
            &mut watchdog::Guard::Disabled,
            &artifact,
            against_source.as_deref(),
            against_target.as_deref(),
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
        Command::TransformList { format } => commands::run_transform_list(format),
        // `command_needs_watchdog` is unconditionally `true` for these three forms, so `run_writing`
        // above always returns before any of them would be matched here.
        Command::Decode { .. } | Command::EncodeGit { .. } | Command::Transform { .. } => {
            unreachable!("command_needs_watchdog is true for this form unconditionally")
        }
        Command::TransformAuthors { .. } => {
            unreachable!("command_needs_watchdog is true for transform authors unconditionally")
        }
    }
}

/// The one function (handoff §2) that starts the clean-stop watchdog, for exactly the forms
/// [`watchdog::command_needs_watchdog`] accepts — called only from [`run`], and only once that check has
/// already passed. Builds the [`watchdog::Guard`] (D-1a(a): only `main` ever calls
/// [`watchdog::Watchdog::start`]), dispatches to the one matching command function, and calls
/// [`watchdog::Guard::finish`] before returning — so the watchdog's own wait (and any sentence it has to
/// print on a stop) completes before this process's exit code is returned.
fn run_writing(cmd: Command) -> i32 {
    let mut guard = match watchdog::Watchdog::start() {
        Ok(w) => watchdog::Guard::Active(w),
        Err(e) => {
            output::stderr(&format!("note: no clean-stop watchdog for this run: {e}"));
            watchdog::Guard::Disabled
        }
    };
    let code = match cmd {
        Command::Decode {
            kind,
            source,
            out,
            infer_renames,
            reconstruct_refs,
            branch_lines,
            layout_trunk,
            layout_branches,
            layout_tags,
            spill_dir,
            format,
        } => commands::run_decode(
            &mut guard,
            kind,
            &source,
            &out,
            infer_renames,
            reconstruct_refs,
            branch_lines,
            layout_trunk,
            layout_branches,
            layout_tags,
            spill_dir.as_deref(),
            format,
        ),
        Command::EncodeGit {
            artifact,
            out,
            format,
        } => commands::run_encode_git(&mut guard, &artifact, &out, format),
        Command::Transform {
            name,
            artifact,
            out,
            plugin_args,
            format,
        } => commands::run_transform(&mut guard, &name, &artifact, &out, &plugin_args, format),
        Command::TransformAuthors {
            artifact,
            map,
            out,
            require_all,
            format,
        } => {
            commands::run_transform_authors(&mut guard, &artifact, &map, &out, require_all, format)
        }
        Command::Verify {
            artifact,
            against_source,
            against_target,
            spill_dir,
            format,
        } => commands::run_verify(
            &mut guard,
            &artifact,
            against_source.as_deref(),
            against_target.as_deref(),
            spill_dir.as_deref(),
            format,
        ),
        _ => unreachable!(
            "run_writing is only ever called for a command command_needs_watchdog accepts"
        ),
    };
    guard.finish();
    code
}
