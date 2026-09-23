//! The floor vocabulary (release-prep handoff §3): every identifier is lowercase ASCII kebab-case, and
//! the list has no duplicates.

use super::ALL;

/// `^[a-z0-9]+(-[a-z0-9]+)*$`, without a regex dependency.
fn is_kebab_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

#[test]
fn every_floor_identifier_is_lowercase_kebab_case() {
    for id in ALL {
        assert!(
            is_kebab_identifier(id),
            "not a kebab-case identifier: {id:?}"
        );
    }
}

#[test]
fn the_identifier_check_rejects_the_old_phrase_forms() {
    for bad in [
        "censored revision",
        "non-UTF-8 extra key",
        "Unfinished-merge",
        "-x",
        "x-",
        "a--b",
        "",
    ] {
        assert!(!is_kebab_identifier(bad), "should be rejected: {bad:?}");
    }
    assert!(is_kebab_identifier("non-utf8-path"));
}

#[test]
fn the_floor_has_no_duplicates() {
    let mut seen = std::collections::BTreeSet::new();
    for id in ALL {
        assert!(seen.insert(*id), "duplicate floor identifier {id:?}");
    }
}
