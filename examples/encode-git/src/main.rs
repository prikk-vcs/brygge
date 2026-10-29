//! `brygge-encode-git-example --artifact <path> [--out <file>]`
//!
//! An unsupported example encoder from a brygge IR artifact to a `git fast-import` stream, on stdout or
//! `--out <file>`. **Not part of brygge; never released.** See `README.md`.
//!
//! This is also the interface `brygge encode` runs a plugin through (RFC 017 D-3, batch E): the same
//! binary, unmodified, works invoked directly or as `brygge-encode-git-example --artifact <path>`.

use std::io::Write;
use std::path::PathBuf;

const USAGE: &str = "usage: brygge-encode-git-example --artifact <path> [--out <file>]";

struct Args {
    artifact: PathBuf,
    out: Option<PathBuf>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut artifact = None;
    let mut out = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--artifact" => {
                if artifact.is_some() {
                    return Err("--artifact given more than once".to_string());
                }
                artifact = Some(PathBuf::from(
                    args.next().ok_or("--artifact needs a value")?,
                ));
            }
            "--out" => {
                if out.is_some() {
                    return Err("--out given more than once".to_string());
                }
                out = Some(PathBuf::from(args.next().ok_or("--out needs a value")?));
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        artifact: artifact.ok_or("--artifact is required")?,
        out,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help") {
        println!(
            "brygge-encode-git-example: {}\n\n{USAGE}\n\n    --artifact <path>  the brygge IR artifact \
             to read (required)\n    --out <file>       write the fast-import stream here (default: \
             stdout)\n    --help             show this text and exit",
            brygge_encode_git_example::LABEL
        );
        return;
    }
    let args = match parse_args(args.into_iter()) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("brygge-encode-git-example: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };

    let decoded = match brygge_ir::from_file(&args.artifact) {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "brygge-encode-git-example: cannot read {:?}: {e}",
                args.artifact
            );
            std::process::exit(1);
        }
    };

    let result = match &args.out {
        Some(path) => match std::fs::File::create(path) {
            Ok(mut f) => brygge_encode_git_example::encode(&decoded.ir, decoded.digest, &mut f),
            Err(e) => {
                eprintln!("brygge-encode-git-example: cannot create {path:?}: {e}");
                std::process::exit(1);
            }
        },
        None => {
            let mut out = std::io::stdout().lock();
            let r = brygge_encode_git_example::encode(&decoded.ir, decoded.digest, &mut out);
            let _ = out.flush();
            r
        }
    };

    if let Err(e) = result {
        eprintln!("brygge-encode-git-example: {e}");
        std::process::exit(1);
    }

    // RFC 018 D-5 (batch T): the whole transform chain, named once, regardless of how many atoms'
    // trailers each name one of its steps.
    brygge_encode_git_example::report_transform_chain(&decoded.ir.transforms);

    // RFC 017 D-1 §2: a reader of the target must be able to find what the IR said it did not carry.
    let report = brygge_ir::summary(&decoded.ir);
    eprint!("{}", report.render_human());
    if decoded.skipped_non_critical_fields > 0 {
        eprintln!(
            "  skipped (newer contract minor, non-critical): {}",
            decoded.skipped_non_critical_fields
        );
    }
}
