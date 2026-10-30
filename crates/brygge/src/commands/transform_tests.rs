#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;

use brygge_ir::transform::StepSummary;

use crate::cli::Format;
use crate::plugin::Listed;

use super::{
    create_transform_temp, render_transform_list_human, render_transform_list_machine,
    same_file_as, transform_banner_text, transform_exit_text, transform_result_text,
    transform_signal_text,
};

#[test]
fn banner_human_names_the_name_and_resolved_path() {
    let s = transform_banner_text(Format::Human, "authors", &PathBuf::from("/usr/bin/x"));
    assert!(s.contains("brygge-transform-authors"));
    assert!(s.contains("/usr/bin/x"));
    assert!(s.contains("not part of brygge"));
}

#[test]
fn banner_machine_is_keyed_and_versioned() {
    let s = transform_banner_text(Format::Machine, "authors", &PathBuf::from("/usr/bin/x"));
    assert!(s.contains("transform_version=1"));
    assert!(s.contains("transform.name=authors"));
    assert!(s.contains("transform.plugin_path=/usr/bin/x"));
}

#[test]
fn exit_text_human_and_machine() {
    assert_eq!(
        transform_exit_text(Format::Human, "foo", 3),
        "`brygge-transform-foo` exited with 3"
    );
    assert_eq!(
        transform_exit_text(Format::Machine, "foo", 3),
        "transform.exit_code=3"
    );
}

#[test]
fn signal_text_names_a_known_signal() {
    let s = transform_signal_text(Format::Human, "foo", 6);
    assert!(s.contains("SIGABRT"));
    assert_eq!(
        transform_signal_text(Format::Machine, "foo", 6),
        "transform.signal=6"
    );
}

#[test]
fn signal_text_falls_back_to_the_bare_number_when_unknown() {
    let s = transform_signal_text(Format::Human, "foo", 200);
    assert!(s.contains("200"));
    assert!(!s.contains("SIG"));
}

#[test]
fn result_text_human_and_machine_carry_every_count() {
    let summary = StepSummary {
        step: 2,
        atoms_changed: 3,
        ops_changed: 4,
        refs_changed: 1,
        dropped: 5,
    };
    let human = transform_result_text(Format::Human, &summary);
    assert!(human.contains("step 2"), "{human}");
    assert!(human.contains("3 atom(s)"), "{human}");
    assert!(human.contains("4 op(s)"), "{human}");
    assert!(human.contains("1 ref(s)"), "{human}");
    assert!(human.contains("5 dropped"), "{human}");

    let machine = transform_result_text(Format::Machine, &summary);
    assert!(machine.contains("transform.step=2"));
    assert!(machine.contains("transform.atoms_changed=3"));
    assert!(machine.contains("transform.ops_changed=4"));
    assert!(machine.contains("transform.refs_changed=1"));
    assert!(machine.contains("transform.dropped=5"));
}

#[test]
fn list_human_marks_shadowed_entries() {
    let plugins = vec![
        Listed {
            name: "a".to_string(),
            path: PathBuf::from("/x/brygge-transform-a"),
            shadowed: false,
        },
        Listed {
            name: "a".to_string(),
            path: PathBuf::from("/y/brygge-transform-a"),
            shadowed: true,
        },
    ];
    let s = render_transform_list_human(&plugins);
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(!lines[0].contains("shadowed"));
    assert!(lines[1].contains("shadowed"));
}

#[test]
fn list_human_empty_says_so() {
    let s = render_transform_list_human(&[]);
    assert!(s.contains("no brygge-transform-* plugins found on PATH"));
}

#[test]
fn list_machine_has_the_version_and_shadowed_flag() {
    let plugins = vec![Listed {
        name: "authors".to_string(),
        path: PathBuf::from("/x/brygge-transform-authors"),
        shadowed: false,
    }];
    let s = render_transform_list_machine(&plugins);
    assert!(s.starts_with("transform_list_version=1\n"));
    assert!(s.contains("transformer.0.name=authors"));
    assert!(s.contains("transformer.0.shadowed=false"));
}

#[test]
fn same_file_as_true_for_the_identical_path_given_twice() {
    let p = std::env::temp_dir().join("brygge-transform-test-same-file-nonexistent");
    assert!(same_file_as(&p, &p));
}

#[test]
fn same_file_as_false_for_different_nonexistent_paths() {
    let a = std::env::temp_dir().join("brygge-transform-test-a");
    let b = std::env::temp_dir().join("brygge-transform-test-b");
    assert!(!same_file_as(&a, &b));
}

#[test]
fn same_file_as_true_through_a_symlink_when_both_exist() {
    let dir = std::env::temp_dir().join(format!(
        "brygge-transform-test-symlink-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let real = dir.join("real");
    std::fs::write(&real, b"x").unwrap();
    #[cfg(unix)]
    {
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(same_file_as(&real, &link));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn create_transform_temp_makes_a_unique_visible_file() {
    let dir =
        std::env::temp_dir().join(format!("brygge-transform-test-temp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = create_transform_temp(&dir).unwrap();
    let b = create_transform_temp(&dir).unwrap();
    assert_ne!(a, b);
    assert!(a.is_file());
    assert!(b.is_file());
    let name = a.file_name().unwrap().to_string_lossy();
    assert!(name.starts_with(".brygge-transform-"), "{name}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&a).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
