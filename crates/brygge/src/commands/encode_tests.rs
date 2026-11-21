#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;

use crate::cli::Format;
use crate::plugin::Listed;

use super::{
    encode_banner_text, encode_exit_text, encode_signal_text, render_encode_list_human,
    render_encode_list_machine,
};

#[test]
fn banner_human_names_the_target_and_resolved_path() {
    let s = encode_banner_text(Format::Human, "git-example", &PathBuf::from("/usr/bin/x"));
    assert!(s.contains("brygge-encode-git-example"));
    assert!(s.contains("/usr/bin/x"));
    assert!(s.contains("not part of brygge"));
}

#[test]
fn banner_machine_is_keyed_and_versioned() {
    let s = encode_banner_text(Format::Machine, "git-example", &PathBuf::from("/usr/bin/x"));
    assert!(s.contains("encode_version=1"));
    assert!(s.contains("encode.target=git-example"));
    assert!(s.contains("encode.plugin_path=/usr/bin/x"));
}

#[test]
fn exit_text_human_and_machine() {
    assert_eq!(
        encode_exit_text(Format::Human, "foo", 3),
        "`brygge-encode-foo` exited with 3"
    );
    assert_eq!(
        encode_exit_text(Format::Machine, "foo", 3),
        "encode.exit_code=3"
    );
}

#[test]
fn signal_text_names_a_known_signal() {
    let s = encode_signal_text(Format::Human, "foo", 6);
    assert!(s.contains("SIGABRT"));
    assert_eq!(
        encode_signal_text(Format::Machine, "foo", 6),
        "encode.signal=6"
    );
}

#[test]
fn signal_text_falls_back_to_the_bare_number_when_unknown() {
    let s = encode_signal_text(Format::Human, "foo", 200);
    assert!(s.contains("200"));
    assert!(!s.contains("SIG"));
}

#[test]
fn list_human_marks_shadowed_entries() {
    let plugins = vec![
        Listed {
            name: "a".to_string(),
            path: PathBuf::from("/x/brygge-encode-a"),
            shadowed: false,
        },
        Listed {
            name: "a".to_string(),
            path: PathBuf::from("/y/brygge-encode-a"),
            shadowed: true,
        },
    ];
    let s = render_encode_list_human(&plugins);
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(!lines[0].contains("shadowed"));
    assert!(lines[1].contains("shadowed"));
}

#[test]
fn list_human_empty_says_so() {
    let s = render_encode_list_human(&[]);
    assert!(s.contains("no brygge-encode-* plugins found on PATH"));
}

#[test]
fn list_machine_has_the_version_and_shadowed_flag() {
    let plugins = vec![Listed {
        name: "svn-example".to_string(),
        path: PathBuf::from("/x/brygge-encode-svn-example"),
        shadowed: false,
    }];
    let s = render_encode_list_machine(&plugins);
    // RFC 019: the list's version bumped to 2 (a `git` plugin is now always shown shadowed by the
    // built-in, a new meaning for the `shadowed` key and a new `shadowed_by` key).
    assert!(s.starts_with("encode_list_version=2\n"));
    assert!(s.contains("encoder.0.target=svn-example"));
    assert!(s.contains("encoder.0.shadowed=false"));
}

#[test]
fn a_git_plugin_on_path_is_always_shown_shadowed_by_the_built_in() {
    // Even though plugin::list's own `shadowed` is false here (no earlier PATH entry of this name), a
    // `brygge-encode-git` plugin can never run: `git` is the built-in, reserved target (RFC 019 D-1).
    let plugins = vec![Listed {
        name: "git".to_string(),
        path: PathBuf::from("/x/brygge-encode-git"),
        shadowed: false,
    }];
    let human = render_encode_list_human(&plugins);
    assert!(human.contains("shadowed by the built-in"), "{human}");

    let machine = render_encode_list_machine(&plugins);
    assert!(machine.contains("encoder.0.shadowed=true"), "{machine}");
    assert!(
        machine.contains("encoder.0.shadowed_by=built_in"),
        "{machine}"
    );
}
