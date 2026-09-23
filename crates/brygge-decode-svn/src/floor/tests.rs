//! The floor's identifiers are lowercase ASCII kebab-case, the one vocabulary shared by every decoder.

use super::ALL;

/// A floor feature is `^[a-z0-9]+(-[a-z0-9]+)*$`.
fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

#[test]
fn every_floor_feature_is_a_lowercase_kebab_case_identifier() {
    for feature in ALL {
        assert!(is_identifier(feature), "not an identifier: {feature:?}");
    }
}

#[test]
fn the_identifier_check_rejects_the_old_phrase_and_property_forms() {
    assert!(!is_identifier("svn:externals"));
    assert!(!is_identifier("non-UTF-8 path"));
    assert!(!is_identifier("shallow clone"));
    assert!(!is_identifier("a--b"));
    assert!(!is_identifier("-a"));
    assert!(!is_identifier(""));
    assert!(is_identifier("non-utf8-path"));
}

#[test]
fn the_floor_has_no_duplicates() {
    let mut seen = std::collections::BTreeSet::new();
    for feature in ALL {
        assert!(seen.insert(*feature), "duplicate floor feature {feature:?}");
    }
}
