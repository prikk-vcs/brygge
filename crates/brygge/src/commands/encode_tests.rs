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
            target: "a".to_string(),
            path: PathBuf::from("/x/brygge-encode-a"),
            shadowed: false,
        },
        Listed {
            target: "a".to_string(),
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
        target: "git-example".to_string(),
        path: PathBuf::from("/x/brygge-encode-git-example"),
        shadowed: false,
    }];
    let s = render_encode_list_machine(&plugins);
    assert!(s.starts_with("encode_list_version=1\n"));
    assert!(s.contains("encoder.0.target=git-example"));
    assert!(s.contains("encoder.0.shadowed=false"));
}
