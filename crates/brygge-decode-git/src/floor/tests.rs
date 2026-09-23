//! The floor vocabulary (release-prep handoff §3): every identifier is a stable, lowercase, kebab-case
//! name, so the same concept can carry the same name in every decoder.

use super::ALL;

/// `^[a-z0-9]+(-[a-z0-9]+)*$`, hand-rolled (no regex dependency).
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
            "floor identifier {id:?} is not kebab-case"
        );
    }
}

#[test]
fn the_kebab_matcher_rejects_the_old_phrase_forms() {
    for bad in [
        "non-UTF-8 path",
        "shallow clone",
        "SHA-256 object format",
        "",
        "a--b",
        "-a",
        "a-",
    ] {
        assert!(!is_kebab_identifier(bad), "{bad:?} must not match");
    }
}

#[test]
fn floor_identifiers_are_unique() {
    let mut seen = std::collections::BTreeSet::new();
    for id in ALL {
        assert!(seen.insert(*id), "duplicate floor identifier {id:?}");
    }
}

#[test]
fn the_recorded_floor_string_is_pinned() {
    // `params["floor"]` is identity-bearing in every artifact; changing it is a reviewed, breaking change.
    assert_eq!(
        super::joined(),
        "submodule,replace-ref,grafts,shallow-clone,object-alternates,redirected-git-directory,\
         non-utf8-path,non-utf8-ref-name,non-utf8-commit-header-name,sha256-object-format"
    );
}
