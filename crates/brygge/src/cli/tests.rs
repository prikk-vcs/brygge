//! Parser tests (external design CL-01..06, handoff D-A).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

fn v(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn no_args_is_help() {
    assert_eq!(parse(&v(&[])).unwrap(), Command::Help);
}

#[test]
fn version_and_help_flags() {
    assert_eq!(parse(&v(&["--version"])).unwrap(), Command::Version);
    assert_eq!(parse(&v(&["-V"])).unwrap(), Command::Version);
    assert_eq!(parse(&v(&["--help"])).unwrap(), Command::Help);
}

#[test]
fn decode_git_full() {
    let c = parse(&v(&[
        "decode",
        "git",
        "/repo",
        "--ir",
        "out.ir",
        "--detect-renames",
        "--format",
        "machine",
    ]))
    .unwrap();
    match c {
        Command::Decode {
            kind,
            path,
            out,
            detect_renames,
            reconstruct_refs,
            format,
        } => {
            assert_eq!(kind, SourceKind::Git);
            assert_eq!(path, PathBuf::from("/repo"));
            assert_eq!(out, Some(PathBuf::from("out.ir")));
            assert!(detect_renames);
            assert!(!reconstruct_refs);
            assert_eq!(format, Format::Machine);
        }
        other => panic!("expected Decode, got {other:?}"),
    }
}

#[test]
fn decode_defaults_and_kind_guard() {
    match parse(&v(&["decode", "git", "/r"])).unwrap() {
        Command::Decode {
            kind,
            out,
            detect_renames,
            format,
            ..
        } => {
            assert_eq!(kind, SourceKind::Git);
            assert_eq!(out, None);
            assert!(!detect_renames);
            assert_eq!(format, Format::Human);
        }
        other => panic!("got {other:?}"),
    }
    // hg and svn are both supported now.
    match parse(&v(&["decode", "hg", "/r"])).unwrap() {
        Command::Decode { kind, .. } => assert_eq!(kind, SourceKind::Hg),
        other => panic!("got {other:?}"),
    }
    match parse(&v(&["decode", "svn", "/r"])).unwrap() {
        Command::Decode {
            kind,
            reconstruct_refs,
            ..
        } => {
            assert_eq!(kind, SourceKind::Svn);
            assert!(!reconstruct_refs); // off by default
        }
        other => panic!("got {other:?}"),
    }
    // the svn ref-reconstruction flag parses.
    match parse(&v(&["decode", "svn", "/r", "--reconstruct-refs"])).unwrap() {
        Command::Decode {
            reconstruct_refs, ..
        } => assert!(reconstruct_refs),
        other => panic!("got {other:?}"),
    }
    assert!(parse(&v(&["decode", "git"])).is_err(), "path required");
}

#[test]
fn verify_modes() {
    match parse(&v(&["verify", "--internal", "--import", "x.ir"])).unwrap() {
        Command::VerifyInternal { import, .. } => assert_eq!(import, PathBuf::from("x.ir")),
        other => panic!("got {other:?}"),
    }
    match parse(&v(&[
        "verify",
        "--against-source",
        "/repo",
        "--import",
        "x.ir",
    ]))
    .unwrap()
    {
        Command::VerifyAgainstSource { repo, import, .. } => {
            assert_eq!(repo, PathBuf::from("/repo"));
            assert_eq!(import, PathBuf::from("x.ir"));
        }
        other => panic!("got {other:?}"),
    }
    assert!(
        parse(&v(&["verify", "--import", "x"])).is_err(),
        "needs a mode"
    );
    assert!(
        parse(&v(&[
            "verify",
            "--internal",
            "--against-source",
            "/r",
            "--import",
            "x"
        ]))
        .is_err(),
        "not both modes"
    );
    assert!(
        parse(&v(&["verify", "--internal"])).is_err(),
        "needs --import"
    );
}

#[test]
fn summary_and_inspect() {
    match parse(&v(&["summary", "--import", "a.ir"])).unwrap() {
        Command::Summary { import, .. } => assert_eq!(import, PathBuf::from("a.ir")),
        other => panic!("got {other:?}"),
    }
    match parse(&v(&["inspect", "--ir", "a.ir"])).unwrap() {
        Command::Inspect { ir, .. } => assert_eq!(ir, PathBuf::from("a.ir")),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn encode_is_gated_not_missing() {
    assert_eq!(
        parse(&v(&["encode", "prikk"])).unwrap(),
        Command::EncodeGated
    );
}

#[test]
fn unknown_command_and_bad_format_error() {
    assert!(parse(&v(&["frobnicate"])).is_err());
    assert!(parse(&v(&["summary", "--import", "x", "--format", "yaml"])).is_err());
}
