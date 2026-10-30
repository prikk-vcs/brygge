//! `brygge-transform-authors-example --artifact <path> --out <file> --map <file>`
//!
//! An unsupported example transform, rewriting author/committer identities through a mailmap-style map
//! file. **Not part of brygge; never released.** See `README.md`.
//!
//! This is also the interface `brygge transform` runs a plugin through (RFC 018 D-6 §3): the same
//! binary, unmodified, works invoked directly or as a `brygge transform` plugin.

use std::path::PathBuf;

const USAGE: &str =
    "usage: brygge-transform-authors-example --artifact <path> --out <file> --map <file>";

struct Args {
    artifact: PathBuf,
    out: PathBuf,
    map: PathBuf,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut artifact = None;
    let mut out = None;
    let mut map = None;
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
            "--map" => {
                if map.is_some() {
                    return Err("--map given more than once".to_string());
                }
                map = Some(PathBuf::from(args.next().ok_or("--map needs a value")?));
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        artifact: artifact.ok_or("--artifact is required")?,
        out: out.ok_or("--out is required")?,
        map: map.ok_or("--map is required")?,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help") {
        println!(
            "brygge-transform-authors-example: {}\n\n{USAGE}\n\n    --artifact <path>  the brygge IR \
             artifact to read (required)\n    --out <file>       write the transformed artifact here \
             (required)\n    --map <file>       the mailmap-style map file (required; lines of \
             'Old Name <old@email> = New Name <new@email>')\n    --help             show this text \
             and exit",
            brygge_transform_authors_example::LABEL
        );
        return;
    }
    let args = match parse_args(args.into_iter()) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("brygge-transform-authors-example: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };

    let map_bytes = match std::fs::read(&args.map) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "brygge-transform-authors-example: cannot read {:?}: {e}",
                args.map
            );
            std::process::exit(1);
        }
    };
    let map = match brygge_transform_authors_example::parse_map(&map_bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("brygge-transform-authors-example: {:?}: {e}", args.map);
            std::process::exit(2);
        }
    };

    let input = match brygge_ir::from_file(&args.artifact) {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "brygge-transform-authors-example: cannot read {:?}: {e}",
                args.artifact
            );
            std::process::exit(1);
        }
    };

    let output = match brygge_transform_authors_example::transform(&input, &map, &map_bytes) {
        Ok(ir) => ir,
        Err(e) => {
            eprintln!("brygge-transform-authors-example: {e}");
            std::process::exit(1);
        }
    };

    let bytes = brygge_ir::to_bytes(&output);
    if let Err(e) = std::fs::write(&args.out, bytes) {
        eprintln!(
            "brygge-transform-authors-example: cannot write {:?}: {e}",
            args.out
        );
        std::process::exit(1);
    }

    eprintln!(
        "brygge-transform-authors-example ({}): {} mapping(s) applied from {:?}",
        brygge_transform_authors_example::LABEL,
        map.len(),
        args.map
    );
}
