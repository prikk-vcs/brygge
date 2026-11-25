# Handoff — RFC 020 batch L: `--layout-trunk`, `--layout-branches`, `--layout-tags` (D-3) (request 084)

**Governing:** RFC 020 D-3 and OQ-1 as accepted: each option changes only its own container, and
`--layout-trunk .` is the whole repository. **After:** batch S (on `main` at `3394c42`). **On `main`, no topic
branch:** no process or platform code.

## 0. Evidence plan first (N-3), with the machine's load stated

- **Level 1:** every rule of §1 and §2.
- **Level 2:** decodes with the options over built fixtures.
- **Level 3, the three runs of RFC 020's Evidence:**
  - **wordpress-core** with the standard layout given explicitly (`--layout-trunk trunk --layout-branches branches
    --layout-tags tags`): **byte-identical** to no options, in both `--reconstruct-refs` and `--branch-lines`;
  - **a flat repository:** the existing level-2 fixture with its file at the root, or a real one if you know one.
    `--layout-trunk . --branch-lines`, then `encode git` with the check, then `git fsck --strict`;
  - **a project in a subdirectory:** a level-2 multi-project dump (`proj-a/{trunk,branches,tags}` and
    `proj-b/{…}`). Decode `proj-a` alone: every `proj-b` path is flagged and dropped, and `proj-a`'s lines and refs
    are right. Say that it is level 2.
- **Report `uptime`** with every timing (review 170).

## 1. The options

- **On `decode svn`:** `--layout-trunk <path>`, `--layout-branches <path>`, `--layout-tags <path>`.
- **Each changes only its own container.** An option not given keeps its standard name (`trunk`, `branches`,
  `tags`).
- **`--layout-trunk .`:** the whole repository is the trunk. **With it, giving `--layout-branches` or
  `--layout-tags` is a usage error.** The policy then has no branch or tag container, and `classify` puts every path
  in the trunk root, prefix `""`.
- **Usage errors (exit 2),** each with a message naming the option and the rule:
  - any `--layout-*` without `--reconstruct-refs` or `--branch-lines`;
  - any option given twice;
  - an empty value, or a value that is absolute, contains a `..` or empty segment, or ends in `/`. `.` is allowed
    only as the whole value of `--layout-trunk`;
  - two containers that are the same path, or where one is inside another, checked after the defaults are filled
    in. For example, `--layout-trunk branches/main` with the default `branches` is refused: it is inside the
    branches container.
- **`git svn`'s semantics are not copied.** In `git svn`, giving `-T` alone means no branches. Here the standard
  branches container stays.

## 2. The policy, and what the artifact records

- **`LayoutPolicy`:** the branch and tag containers become `Option<String>`, which is `None` only under
  `--layout-trunk .`. `classify` handles the root. Every caller compiles and is checked.
- **`params["layout"]`** (provenance, read back by `--against-source` through `from_label`):
  - **the default label must be byte-identical to today's** (`trunk=trunk,branches=branches,tags=tags`), so every
    existing artifact and every default decode is unchanged. Prove it with the corpus `cmp`;
  - **a value is percent-encoded for `%`, `,` and `=` only,** so a Subversion path that holds one of them is
    neither misread nor refused. The default label contains none of them, so it is unchanged;
  - the root is `trunk=.`, with no `branches=` or `tags=`;
  - `from_label` decodes exactly what `label` writes. **Level 1:** a round trip for paths with `,`, `=`, `%`, a
    space and non-ASCII; the root form; the default form's bytes;
  - a label `from_label` cannot read is an error, never a silent fallback to the default. Today
    `layout_from_provenance` falls back with "default if absent/unparsable". Make "absent" mean the default (an
    old artifact), and "present but unreadable" give `not-checked`, naming the label.
- **The decode report** (human) always names the layout used when reconstructing: one line, `layout: trunk=…,
  branches=…, tags=…` (or `layout: the whole repository as trunk`), whether the values were given or standard.
  - **Machine output:** `decode.svn.layout.trunk`, `.branches` and `.tags` (the last two absent for the root).
    Bump the decode version, and document it in `machine-output.md`.
- **The refusal of 0.8.0** ("the trunk/branches/tags layout was not found, so `--branch-lines` has no branch to
  follow; decode without it") gains: "or give `--layout-trunk .` to decode the whole repository as one line".

## 3. Documentation and CHANGELOG

- **`docs/src/guide/svn.md`:** a "Layout" section with the three options, the each-changes-only-its-own rule, `.`,
  the usage errors, a subdirectory example, and the `git svn` mapping:
  - `-T`/`--trunk` → `--layout-trunk`;
  - `-b`/`--branches` → `--layout-branches`;
  - `-t`/`--tags` → `--layout-tags`;
  - `-s` → no option;
  - and the one difference (an option you do not give keeps its standard name).
- **`docs/src/guide/git-target.md`:** "Limits in 0.8.0" loses the no-layout limit. Add the command instead
  (`decode svn --layout-trunk . --branch-lines`).
- **`CHANGELOG.md` `[Unreleased]`, `### Added`:** "`decode svn --layout-trunk`, `--layout-branches` and
  `--layout-tags` set the Subversion layout for `--reconstruct-refs` and `--branch-lines`, each changing only its own
  container. `--layout-trunk .` decodes a repository with no layout as one line, so it can be encoded to Git
  (RFC 020)."

## 4. Checks

- fmt, and clippy `-D warnings` on stable, `x86_64-pc-windows-gnu`, `x86_64-apple-darwin` and 1.87;
- `cargo test --workspace` summed, on stable and 1.87, plus the no-identity run;
- the deep mutation sweep (`brygge-decode-svn` changes);
- the isolation check, links, the book, the release tools, `cargo deny`;
- **byte-identity, default options:** every Subversion corpus entry in all three modes against
  `fixtures/previous-release/0.8.1/brygge`.

## 5. Request

- **File** `.git-exclude/review-requests/084-layout-options.md`.
- **Push after approval only.**
- **Then:** the 0.9.0 cut prep.
