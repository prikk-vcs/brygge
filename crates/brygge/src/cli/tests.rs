//! Parser tests (external design v0.3 CL-01…CL-08, handoff `cli-and-verify-handoff-v2.md` §5.1).
//!
//! Every usage problem here is an `Err(String)` from `parse()`; `main` maps every `Err` to exit `USAGE`
//! (2) uniformly (one line, `crates/brygge/src/main.rs`), so these tests assert the parser's own
//! contract rather than re-testing that one-line mapping.

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
    assert_eq!(parse(&v(&["decode", "--help"])).unwrap(), Command::Help);
    assert_eq!(parse(&v(&["inspect", "--help"])).unwrap(), Command::Help);
    assert_eq!(parse(&v(&["verify", "--help"])).unwrap(), Command::Help);
}

#[test]
fn decode_git_full() {
    let c = parse(&v(&[
        "decode",
        "git",
        "/repo",
        "--out",
        "out.ir",
        "--infer-renames",
        "--format",
        "machine",
    ]))
    .unwrap();
    match c {
        Command::Decode {
            kind,
            source,
            out,
            infer_renames,
            reconstruct_refs,
            spill_dir,
            format,
        } => {
            assert_eq!(kind, SourceKind::Git);
            assert_eq!(source, PathBuf::from("/repo"));
            assert_eq!(out, PathBuf::from("out.ir"));
            assert!(infer_renames);
            assert!(!reconstruct_refs);
            assert_eq!(spill_dir, None);
            assert_eq!(format, Format::Machine);
        }
        other => panic!("expected Decode, got {other:?}"),
    }
}

#[test]
fn decode_defaults_and_every_source_kind() {
    match parse(&v(&["decode", "git", "/r", "--out", "o.ir"])).unwrap() {
        Command::Decode {
            kind,
            infer_renames,
            format,
            ..
        } => {
            assert_eq!(kind, SourceKind::Git);
            assert!(!infer_renames);
            assert_eq!(format, Format::Human);
        }
        other => panic!("got {other:?}"),
    }
    match parse(&v(&["decode", "hg", "/r", "--out", "o.ir"])).unwrap() {
        Command::Decode { kind, .. } => assert_eq!(kind, SourceKind::Hg),
        other => panic!("got {other:?}"),
    }
    match parse(&v(&["decode", "svn", "/r", "--out", "o.ir"])).unwrap() {
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
    match parse(&v(&["decode", "cvs", "/r", "--out", "o.ir"])).unwrap() {
        Command::Decode { kind, .. } => assert_eq!(kind, SourceKind::Cvs),
        other => panic!("got {other:?}"),
    }
    match parse(&v(&[
        "decode",
        "svn",
        "/r",
        "--out",
        "o.ir",
        "--reconstruct-refs",
    ]))
    .unwrap()
    {
        Command::Decode {
            reconstruct_refs, ..
        } => assert!(reconstruct_refs),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn decode_without_out_is_a_usage_error() {
    assert!(parse(&v(&["decode", "git", "/r"])).is_err());
    assert!(
        parse(&v(&["decode", "git"])).is_err(),
        "source required too"
    );
}

#[test]
fn spill_dir_parses_and_rejects_a_repeat_or_a_missing_value() {
    match parse(&v(&[
        "decode",
        "git",
        "/repo",
        "--out",
        "out.ir",
        "--spill-dir",
        "/tmp/spill",
    ]))
    .unwrap()
    {
        Command::Decode { spill_dir, .. } => {
            assert_eq!(spill_dir, Some(PathBuf::from("/tmp/spill")));
        }
        other => panic!("got {other:?}"),
    }
    match parse(&v(&["decode", "git", "/repo", "--out", "out.ir"])).unwrap() {
        Command::Decode { spill_dir, .. } => assert_eq!(spill_dir, None, "off by default"),
        other => panic!("got {other:?}"),
    }
    let err = parse(&v(&[
        "decode",
        "git",
        "/repo",
        "--out",
        "out.ir",
        "--spill-dir",
        "/a",
        "--spill-dir",
        "/b",
    ]))
    .unwrap_err();
    assert!(err.contains("--spill-dir given more than once"), "{err}");
    let err = parse(&v(&[
        "decode",
        "git",
        "/repo",
        "--out",
        "out.ir",
        "--spill-dir",
    ]))
    .unwrap_err();
    assert!(err.contains("--spill-dir needs a value"), "{err}");
}

#[test]
fn infer_renames_for_hg_is_a_usage_error_because_mercurial_records_its_renames() {
    // Part-2 handoff §2: no hg code infers anything, so accepting the flag would be a silent no-op and
    // would put a false claim in provenance. Same message shape as `--reconstruct-refs` given to git.
    let err = parse(&v(&[
        "decode",
        "hg",
        "/r",
        "--out",
        "o.ir",
        "--infer-renames",
    ]))
    .unwrap_err();
    assert!(
        err.contains("--infer-renames applies to git, not to hg"),
        "{err}"
    );
    assert!(
        err.contains("run `brygge decode --help` for usage"),
        "{err}"
    );
    let err = parse(&v(&[
        "decode",
        "git",
        "/r",
        "--out",
        "o.ir",
        "--reconstruct-refs",
    ]))
    .unwrap_err();
    assert!(
        err.contains("--reconstruct-refs applies to svn, cvs, not to git"),
        "{err}"
    );
}

#[test]
fn help_says_infer_renames_is_a_git_option() {
    assert!(crate::cli::USAGE.contains("--infer-renames (git) and"));
    assert!(!crate::cli::USAGE.contains("(git/hg)"));
}

#[test]
fn inapplicable_option_is_a_usage_error_with_the_exact_message() {
    let err = parse(&v(&[
        "decode",
        "svn",
        "/r",
        "--out",
        "o.ir",
        "--infer-renames",
    ]))
    .unwrap_err();
    assert!(
        err.contains("--infer-renames applies to git, not to svn"),
        "{err}"
    );

    let err = parse(&v(&[
        "decode",
        "cvs",
        "/r",
        "--out",
        "o.ir",
        "--infer-renames",
    ]))
    .unwrap_err();
    assert!(
        err.contains("--infer-renames applies to git, not to cvs"),
        "{err}"
    );

    let err = parse(&v(&[
        "decode",
        "git",
        "/r",
        "--out",
        "o.ir",
        "--reconstruct-refs",
    ]))
    .unwrap_err();
    assert!(
        err.contains("--reconstruct-refs applies to svn, cvs, not to git"),
        "{err}"
    );

    let err = parse(&v(&[
        "decode",
        "hg",
        "/r",
        "--out",
        "o.ir",
        "--reconstruct-refs",
    ]))
    .unwrap_err();
    assert!(
        err.contains("--reconstruct-refs applies to svn, cvs, not to hg"),
        "{err}"
    );
}

#[test]
fn a_repeated_flag_is_a_usage_error() {
    assert!(parse(&v(&["decode", "git", "/r", "--out", "a", "--out", "b"])).is_err());
    assert!(
        parse(&v(&[
            "decode",
            "git",
            "/r",
            "--out",
            "a",
            "--infer-renames",
            "--infer-renames"
        ]))
        .is_err()
    );
    assert!(parse(&v(&["inspect", "a.ir", "--atoms", "--atoms"])).is_err());
    assert!(
        parse(&v(&[
            "verify", "a.ir", "--format", "human", "--format", "machine"
        ]))
        .is_err()
    );
}

#[test]
fn verify_forms() {
    match parse(&v(&["verify", "x.ir"])).unwrap() {
        Command::Verify {
            artifact,
            against_source,
            ..
        } => {
            assert_eq!(artifact, PathBuf::from("x.ir"));
            assert!(against_source.is_none());
        }
        other => panic!("got {other:?}"),
    }
    match parse(&v(&["verify", "x.ir", "--against-source", "/repo"])).unwrap() {
        Command::Verify {
            artifact,
            against_source,
            ..
        } => {
            assert_eq!(artifact, PathBuf::from("x.ir"));
            assert_eq!(against_source, Some(PathBuf::from("/repo")));
        }
        other => panic!("got {other:?}"),
    }
    assert!(parse(&v(&["verify"])).is_err(), "needs an artifact");
}

#[test]
fn verify_spill_dir_parses_and_requires_against_source() {
    match parse(&v(&[
        "verify",
        "x.ir",
        "--against-source",
        "/repo",
        "--spill-dir",
        "/tmp/spill",
    ]))
    .unwrap()
    {
        Command::Verify { spill_dir, .. } => {
            assert_eq!(spill_dir, Some(PathBuf::from("/tmp/spill")));
        }
        other => panic!("got {other:?}"),
    }
    match parse(&v(&["verify", "x.ir", "--against-source", "/repo"])).unwrap() {
        Command::Verify { spill_dir, .. } => assert_eq!(spill_dir, None, "off by default"),
        other => panic!("got {other:?}"),
    }
    assert!(
        parse(&v(&["verify", "x.ir", "--spill-dir", "/tmp/spill"])).is_err(),
        "--spill-dir without --against-source is meaningless"
    );
    assert!(
        parse(&v(&[
            "verify",
            "x.ir",
            "--against-source",
            "/repo",
            "--spill-dir",
            "/a",
            "--spill-dir",
            "/b",
        ]))
        .is_err(),
        "--spill-dir given more than once"
    );
    assert!(
        parse(&v(&[
            "verify",
            "x.ir",
            "--against-source",
            "/repo",
            "--spill-dir"
        ]))
        .is_err(),
        "--spill-dir needs a value"
    );
}

#[test]
fn inspect_forms() {
    match parse(&v(&["inspect", "a.ir"])).unwrap() {
        Command::Inspect {
            artifact, atoms, ..
        } => {
            assert_eq!(artifact, PathBuf::from("a.ir"));
            assert!(!atoms);
        }
        other => panic!("got {other:?}"),
    }
    match parse(&v(&["inspect", "a.ir", "--atoms"])).unwrap() {
        Command::Inspect { atoms, .. } => assert!(atoms),
        other => panic!("got {other:?}"),
    }
    assert!(parse(&v(&["inspect"])).is_err(), "needs an artifact");
}

#[test]
fn encode_prikk_is_reserved_with_its_own_message() {
    let err = parse(&v(&["encode", "prikk"])).unwrap_err();
    assert!(err.contains("'encode prikk' is not available yet"), "{err}");
    assert!(err.contains("ROADMAP"), "{err}");
    // Reserved even with trailing arguments: it never reaches the artifact/format parsing below.
    let err2 = parse(&v(&["encode", "prikk", "out.ir"])).unwrap_err();
    assert_eq!(err, err2);
}

#[test]
fn encode_needs_a_target_and_an_artifact() {
    assert!(parse(&v(&["encode"])).is_err());
    assert!(parse(&v(&["encode", "git-example"])).is_err());
}

#[test]
fn encode_basic() {
    let c = parse(&v(&["encode", "git-example", "out.ir"])).unwrap();
    assert_eq!(
        c,
        Command::Encode {
            target: "git-example".to_string(),
            artifact: PathBuf::from("out.ir"),
            plugin_args: vec![],
            format: Format::Human,
        }
    );
}

#[test]
fn encode_with_format_and_plugin_args() {
    let c = parse(&v(&[
        "encode",
        "git-example",
        "out.ir",
        "--format",
        "machine",
        "--",
        "--out",
        "result.txt",
        "-x",
    ]))
    .unwrap();
    assert_eq!(
        c,
        Command::Encode {
            target: "git-example".to_string(),
            artifact: PathBuf::from("out.ir"),
            plugin_args: vec![
                "--out".to_string(),
                "result.txt".to_string(),
                "-x".to_string()
            ],
            format: Format::Machine,
        }
    );
}

#[test]
fn encode_without_dashdash_has_no_plugin_args() {
    let c = parse(&v(&["encode", "git-example", "out.ir"])).unwrap();
    match c {
        Command::Encode { plugin_args, .. } => assert!(plugin_args.is_empty()),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn encode_extra_positional_without_dashdash_is_a_usage_error() {
    let err = parse(&v(&["encode", "git-example", "out.ir", "extra"])).unwrap_err();
    assert!(err.contains("plugin arguments must follow --"), "{err}");
}

#[test]
fn encode_rejects_invalid_targets() {
    let too_long = "x".repeat(65);
    for bad in ["../x", "A", "x/y", "", too_long.as_str()] {
        assert!(
            parse(&v(&["encode", bad, "out.ir"])).is_err(),
            "expected target {bad:?} to be rejected"
        );
    }
}

#[test]
fn encode_accepts_valid_targets() {
    for good in ["git-example", "a", "a1", "git2", "a-b-c"] {
        assert!(parse(&v(&["encode", good, "out.ir"])).is_ok(), "{good}");
    }
}

#[test]
fn encode_list_default() {
    assert_eq!(
        parse(&v(&["encode", "--list"])).unwrap(),
        Command::EncodeList {
            format: Format::Human
        }
    );
}

#[test]
fn encode_list_with_format() {
    assert_eq!(
        parse(&v(&["encode", "--list", "--format", "machine"])).unwrap(),
        Command::EncodeList {
            format: Format::Machine
        }
    );
}

#[test]
fn encode_list_rejects_extra_arguments() {
    assert!(parse(&v(&["encode", "--list", "extra"])).is_err());
}

#[test]
fn encode_help() {
    assert_eq!(parse(&v(&["encode", "--help"])).unwrap(), Command::Help);
    assert_eq!(
        parse(&v(&["encode", "--list", "--help"])).unwrap(),
        Command::Help
    );
}

#[test]
fn is_valid_target_matches_the_regex() {
    assert!(is_valid_target("a"));
    assert!(is_valid_target("a0-b9"));
    assert!(!is_valid_target(""));
    assert!(!is_valid_target("A"));
    assert!(!is_valid_target("-a"));
    assert!(!is_valid_target("a/b"));
    assert!(!is_valid_target("a."));
    assert!(!is_valid_target(&"a".repeat(65)));
    assert!(is_valid_target(&"a".repeat(64)));
}

#[test]
fn removed_forms_are_usage_errors() {
    // `summary` was folded into `inspect`.
    assert!(parse(&v(&["summary", "a.ir"])).is_err());
    // `--ir` and `--import` were replaced by positionals.
    assert!(parse(&v(&["decode", "git", "--ir", "o.ir", "/r"])).is_err());
    assert!(parse(&v(&["verify", "--import", "a.ir"])).is_err());
    // `--detect-renames` was replaced by `--infer-renames`.
    assert!(
        parse(&v(&[
            "decode",
            "git",
            "/r",
            "--out",
            "o.ir",
            "--detect-renames"
        ]))
        .is_err()
    );
    // `--internal` no longer exists; the internal checks always run.
    assert!(parse(&v(&["verify", "--internal", "a.ir"])).is_err());
}

#[test]
fn unknown_command_and_bad_format_error() {
    assert!(parse(&v(&["frobnicate"])).is_err());
    assert!(parse(&v(&["inspect", "x", "--format", "yaml"])).is_err());
}
