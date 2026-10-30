//! A controllable fake `brygge transform` plugin, built only to drive `crates/brygge`'s own integration
//! tests of the `transform` command (RFC 018 D-6). It is not a real transform: never published, never
//! shipped. Built alongside `brygge` by the same `cargo build`/`cargo test --workspace`, so the tests
//! can copy it onto a temporary `PATH` under a name-shaped name (`brygge-transform-<name>`) and run it
//! exactly as `brygge transform <name> …` would.
//!
//! Controls, read from its own argv:
//! - `--artifact <path> --out <path>`: brygge's own mandatory arguments — read by every mode below.
//! - `--mode <copy-unchanged|dishonest-output>` (default `copy-unchanged`): what to write to `--out`.
//!   `copy-unchanged` decodes `--artifact`, makes an honest no-op [`brygge_ir::transform::Edit`] (no
//!   atom, op, ref, or metadata actually touched) and writes that. `dishonest-output` copies
//!   `--artifact`'s own bytes to `--out` verbatim — no new transform record at all, so
//!   `brygge_ir::transform::check`'s very first rule (the chain must grow by exactly one record)
//!   refuses it; `brygge transform` must then delete the temp file and leave `--out` untouched.
//! - `--marker <path>`: write an empty file there as the very first thing it does, proving it started
//!   (used to prove the opposite: that `transform`'s verify-first gate kept it from starting at all).
//! - `--then-exit <n>`: exit with code `n` (default `0`) after the steps above.
//! - `--then-abort`: call [`std::process::abort`] instead of exiting normally — raises `SIGABRT` (6) on
//!   Unix, with no handler installed, so the process dies by that signal.
//! - every argument, recognized or not, is echoed verbatim to stdout as `argv: <arg>`, one per line,
//!   before any of the above — proving `brygge transform` passes plugin arguments through exactly, with
//!   no shell.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut stdout = std::io::stdout();
    for arg in &args {
        let _ = writeln!(stdout, "argv: {arg}");
    }
    let _ = stdout.flush();

    let mut artifact: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut mode = "copy-unchanged".to_string();
    let mut then_exit: i32 = 0;
    let mut then_abort = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--artifact" => {
                artifact = Some(PathBuf::from(it.next().expect("--artifact needs a value")))
            }
            "--out" => out = Some(PathBuf::from(it.next().expect("--out needs a value"))),
            "--mode" => mode = it.next().expect("--mode needs a value").clone(),
            "--marker" => {
                let path = it.next().expect("--marker needs a value");
                std::fs::write(path, b"").expect("write marker file");
            }
            "--then-exit" => {
                let n = it.next().expect("--then-exit needs a value");
                then_exit = n.parse().expect("--then-exit needs an integer");
            }
            "--then-abort" => then_abort = true,
            _ => {}
        }
    }

    if then_abort {
        std::process::abort();
    }

    if then_exit == 0 {
        let artifact = artifact.expect("--artifact is required");
        let out = out.expect("--out is required");
        match mode.as_str() {
            "copy-unchanged" => {
                // A real plugin's own name is fixed (it is `brygge-transform-<name>`); this fake one is
                // installed under whatever name a test chooses, so it reads its own argv[0] back —
                // `brygge transform`'s own `check` call passes the CLI's `<name>` through unchanged, and
                // the new transform record must name that same thing.
                let own_exe = std::env::args().next().expect("argv[0]");
                let own_name = std::path::Path::new(&own_exe)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.strip_prefix("brygge-transform-"))
                    .expect("this binary's own name must be brygge-transform-<name>")
                    .to_string();
                let input = brygge_ir::from_file(&artifact).expect("decode --artifact");
                let edit =
                    brygge_ir::transform::Edit::begin(&input, own_name, "0", BTreeMap::new());
                let ir = edit
                    .finish()
                    .expect("an honest no-op edit always passes check");
                let bytes = brygge_ir::to_bytes(&ir);
                std::fs::write(&out, bytes).expect("write --out");
            }
            "dishonest-output" => {
                std::fs::copy(&artifact, &out).expect("copy --artifact to --out verbatim");
            }
            other => panic!("unknown --mode {other}"),
        }
    }

    std::process::exit(then_exit);
}
