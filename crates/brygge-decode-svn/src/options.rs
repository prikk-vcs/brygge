//! Decoder options (RFC 006 D-4, CF-01). Every option that can change the output is recorded into the IR
//! provenance (`PR-5`). The default is the maximally-honest one: **branch/tag reconstruction off** — the
//! stated linear spine always imports; ref synthesis is an opt-in `Derived` layer (RFC 006 D-4).

use std::collections::BTreeMap;

/// The trunk/branches/tags layout convention used when reconstructing branch/tag refs (RFC 006 D-4/OQ-C;
/// RFC 020 D-3: `--layout-trunk`/`--layout-branches`/`--layout-tags`). The assumed convention is recorded
/// as a derivation parameter (`PR-5`) on every reconstructed ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutPolicy {
    /// The trunk directory (repo-relative, no trailing slash), e.g. `"trunk"` — or `"."`, meaning the
    /// whole repository is the trunk (RFC 020 D-3: `--layout-trunk .`), in which case `branches` and
    /// `tags` are both `None` and [`crate::layout::classify`] puts every path in the trunk root.
    pub trunk: String,
    /// The branches container directory, e.g. `"branches"` (each child directory is one branch). `None`
    /// only when `trunk` is `"."`: a whole-repository trunk has no separate branches container.
    pub branches: Option<String>,
    /// The tags container directory, e.g. `"tags"` (each child directory is one tag). `None` only when
    /// `trunk` is `"."`.
    pub tags: Option<String>,
}

impl Default for LayoutPolicy {
    fn default() -> Self {
        Self {
            trunk: "trunk".to_string(),
            branches: Some("branches".to_string()),
            tags: Some("tags".to_string()),
        }
    }
}

/// Percent-encode exactly `%`, `,` and `=` — the three bytes [`LayoutPolicy::label`]'s own `,`/`=` syntax
/// uses as separators, so a Subversion path that happens to contain one of them is neither misread nor
/// refused (RFC 020 D-3 §2). Nothing else is encoded, so the label stays readable for every ordinary
/// path; the default label (`trunk=trunk,branches=branches,tags=tags`) contains none of the three, so it
/// is byte-for-byte unchanged by this. Operates byte-wise, never re-interpreting a byte as a `char`: a
/// UTF-8 continuation byte is always `>= 0x80`, so it can never collide with one of the three (all
/// `< 0x80`), and is copied through untouched either way.
fn percent_encode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'%' => out.extend_from_slice(b"%25"),
            b',' => out.extend_from_slice(b"%2C"),
            b'=' => out.extend_from_slice(b"%3D"),
            _ => out.push(b),
        }
    }
    // Every byte pushed is either an original, untouched source byte or part of a pure-ASCII `%XX`
    // escape, so the result is valid UTF-8 whenever `s` was.
    String::from_utf8(out).unwrap_or_default()
}

/// The inverse of [`percent_encode`]: `%` followed by exactly two hex digits becomes the one byte they
/// encode; any other use of `%` (too few digits, or non-hex ones) is a malformed label — `None`, so
/// [`LayoutPolicy::from_label`] can refuse it rather than silently keep a literal `%`.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

impl LayoutPolicy {
    /// A stable label for the convention, recorded in the derivation params (`PR-5`). The root form
    /// (`trunk == "."`) writes only `trunk=.`, with no `branches=`/`tags=` at all.
    #[must_use]
    pub fn label(&self) -> String {
        let mut s = format!("trunk={}", percent_encode(&self.trunk));
        if let Some(branches) = &self.branches {
            s.push_str(&format!(",branches={}", percent_encode(branches)));
        }
        if let Some(tags) = &self.tags {
            s.push_str(&format!(",tags={}", percent_encode(tags)));
        }
        s
    }

    /// Parse a [`label`](Self::label) back into a policy — for reproducing an import from its recorded
    /// provenance (`verify --against-source`). `None` if the label is not in `label` form (including a
    /// value `percent_decode` cannot read) — never a silent fallback to a default for a label that is
    /// present but malformed; that distinction is the caller's to make (RFC 020 D-3 §2).
    #[must_use]
    pub fn from_label(s: &str) -> Option<Self> {
        let (mut trunk, mut branches, mut tags) = (None, None, None);
        for part in s.split(',') {
            let (k, v) = part.split_once('=')?;
            let v = percent_decode(v)?;
            match k {
                "trunk" => trunk = Some(v),
                "branches" => branches = Some(v),
                "tags" => tags = Some(v),
                _ => {}
            }
        }
        Some(Self {
            trunk: trunk?,
            branches,
            tags,
        })
    }

    /// The trunk's own path as [`crate::tree::Tree`]'s paths use it: `""` (never `"."`) when
    /// `--layout-trunk .` makes the whole repository the trunk — no file or directory is ever literally
    /// named `"."`, so every tree lookup against the trunk container must use this, not `trunk` itself.
    #[must_use]
    pub(crate) fn trunk_prefix(&self) -> &str {
        if self.trunk == "." { "" } else { &self.trunk }
    }
}

/// How the Subversion decoder should behave.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    /// Reconstruct branch/tag refs from directory-copy convention, marking each `Derived` (RFC 006 D-4).
    /// **Off by default** — with it off, only the `Stated` linear spine (revisions + literal directory
    /// operations) is produced. Turning it on never yields an *unmarked* branch (INV-1): every
    /// reconstructed ref is `Derived`, and a convention-violating layout is recorded loudly, not guessed.
    pub reconstruct_refs: bool,
    /// The layout convention to assume when `reconstruct_refs` is on (RFC 006 OQ-C).
    pub layout: LayoutPolicy,
    /// One line of history per branch/tag root, each with its own files at the root of its tree, instead
    /// of the single whole-repository spine (RFC 019 Amendment A, D-9). Implies `reconstruct_refs`.
    /// **Off by default** — with it off, nothing about the single-spine decode changes, byte for byte.
    pub branch_lines: bool,
}

impl Options {
    /// Render the options as canonical, sorted key→value strings for the IR provenance (`PR-5/CF-01`).
    #[must_use]
    pub fn as_params(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert(
            "reconstruct_refs".to_string(),
            self.reconstruct_refs.to_string(),
        );
        if self.reconstruct_refs {
            m.insert("layout".to_string(), self.layout.label());
        }
        if self.branch_lines {
            m.insert("branch_lines".to_string(), "true".to_string());
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::LayoutPolicy;

    /// RFC 020 D-3 §2, level 1: the default label's bytes must never change — every existing artifact
    /// and every default decode stays byte-identical.
    #[test]
    fn the_default_label_is_byte_identical_to_before_this_rfc() {
        assert_eq!(
            LayoutPolicy::default().label(),
            "trunk=trunk,branches=branches,tags=tags"
        );
    }

    /// RFC 020 D-3 §2, level 1: the root form writes only `trunk=.`.
    #[test]
    fn the_root_form_has_no_branches_or_tags_key() {
        let root = LayoutPolicy {
            trunk: ".".to_string(),
            branches: None,
            tags: None,
        };
        assert_eq!(root.label(), "trunk=.");
        assert_eq!(LayoutPolicy::from_label(&root.label()), Some(root));
    }

    /// RFC 020 D-3 §2, level 1: a path holding `,`, `=`, `%`, a space, and non-ASCII round-trips through
    /// `label`/`from_label` exactly.
    #[test]
    fn a_label_round_trips_paths_with_commas_equals_percents_spaces_and_non_ascii() {
        let l = LayoutPolicy {
            trunk: "a,b=c%d e".to_string(),
            branches: Some("brünch/dïr".to_string()),
            tags: Some("tag,s=1%2".to_string()),
        };
        let label = l.label();
        // The separators are escaped, so splitting the label on `,` must not be fooled by a `,` inside a
        // value.
        assert_eq!(LayoutPolicy::from_label(&label), Some(l));
    }

    /// `from_label` must refuse a label it cannot read, not silently fall back to a default — the
    /// caller is the one who decides what "absent" vs "present but unreadable" means (RFC 020 D-3 §2).
    #[test]
    fn from_label_refuses_a_malformed_percent_escape() {
        assert_eq!(LayoutPolicy::from_label("trunk=100%"), None);
        assert_eq!(LayoutPolicy::from_label("trunk=100%zz"), None);
        assert_eq!(LayoutPolicy::from_label("not a label at all"), None);
    }

    #[test]
    fn trunk_prefix_maps_the_root_form_to_an_empty_string() {
        let root = LayoutPolicy {
            trunk: ".".to_string(),
            branches: None,
            tags: None,
        };
        assert_eq!(root.trunk_prefix(), "");
        assert_eq!(LayoutPolicy::default().trunk_prefix(), "trunk");
    }
}
