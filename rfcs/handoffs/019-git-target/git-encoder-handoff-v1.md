# Handoff — RFC 019 D-1, D-2, D-3, D-5, D-7: `brygge encode git` (batch G1)

**Governing:** RFC 019 (accepted 2026-11-21), with the refinement at acceptance: the encoder is a module of the
`brygge` crate, not a new crate.
**Branch:** a topic branch, `batch-g1-encode-git`. It spawns `git` through the per-OS lookup, so the Windows and
macOS jobs must show its tests running, as for batch E.
**What comes after:** batch G2 (D-6, the check against the artifact) follows it. Until G2 lands, `encode git` writes
its result without checking it. Nothing is released in between.

## 0. Evidence plan first (N-3)

Open the request with the questions, the level each is checked at, and what remains for CI and level 3. The
minimum:
- **level 1:** the stream writer over hand-built `Ir`s. Every row of §3 and §4, and every refusal of §5;
- **level 2:** `fast-import` on fixtures from all four decoders (the 0.6.0 example's fixtures), checked with
  `git cat-file`, `git for-each-ref` and `git interpret-trailers --parse`;
- **level 3:** the corpus runs of RFC 019 D-8. They belong to G4, so name them as remaining.

## 1. The command

```
brygge encode git <artifact> --out <dir>
```

- **`<dir>` must not exist.** Refuse otherwise (exit 2), naming it.
- **The order:**
  1. verify the artifact (the existing verify-first gate);
  2. the pre-flight (§5): every refusal in one report, exit 2, nothing written;
  3. create `<dir>.partial-<pid>` beside `<dir>` (C-7) and run `git init --bare` in it. Name the default branch
     with `--initial-branch` from §4.
  4. stream into `git fast-import --quiet --done`. brygge writes `done` last, so a truncated stream fails;
  5. rename the partial directory to `<dir>`.

  On any failure, remove the partial directory and write nothing at `<dir>`.
- **The report** (RFC 019 D-2, OQ-1) goes to `<dir>.brygge-report`, written atomically after the rename. It holds,
  in `machine-output.md`'s style (`encode.*` keys):
  - the artifact's digest;
  - the brygge version;
  - every drop and flag the artifact carries;
  - the counts of commits, tags and refs written;
  - every `Brygge-Supplied` and `Brygge-Altered` field, counted per field.

  Specify the keys in `machine-output.md`.
- **`git` lookup and environment (D-1):**
  - resolve `git` with the plugin lookup's rules (`plugin.rs`: absolute `PATH` entries only, never the current
    directory, `.exe` only on Windows), and spawn the resolved path;
  - `env_clear()`, then set only `PATH` (the same value), `HOME` (an empty temporary directory, removed after),
    `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL` (an empty file), `LC_ALL=C` and, on Windows, `SystemRoot`;
  - so no user configuration, template, hook or `core.*` setting can reach the result. Test this: a hostile
    `$HOME/.gitconfig` and `GIT_TEMPLATE_DIR` in the parent environment must change nothing.
- **The reserved name `git`:**
  - `encode git` never runs a plugin;
  - `encode --list` shows a `brygge-encode-git` on `PATH` as `shadowed by the built-in`;
  - `prikk` stays reserved as it is.

## 2. Where the code lives, and the isolation check

- **The module:** `crates/brygge/src/encode_git/`. A stream writer, `pub(crate) fn write_stream<W: Write>(ir:
  &Ir, digest: &Digest, out: W) -> Result<Summary, Refusals>`, plus the pre-flight. The CLI part (process,
  directories, report) lives in `commands.rs` beside `encode`.
- **The isolation (RFC 019 refinement):**
  - the module uses `brygge_ir` and `std` only, and nothing from the decoders, `gix` or the CLI;
  - **the check:** a level-2 check in `tools/check-ir-isolation.sh`. It refuses any `use` or path in
    `crates/brygge/src/encode_git/` that names a crate other than `brygge_ir`, `std`, `core`, `alloc`, `crate::encode_git` or
    `super`;
  - test it with a planted bad `use`.
- **The example's retirement:**
  - move `examples/encode-git`'s logic into the module, adapted to §3;
  - delete the example crate and remove it from the workspace;
  - `writing-an-encoder.md` points to the module as the worked example, and keeps its rules;
  - the example's tests move too.

## 3. Trailers (D-2)

- **One block** after the message, separated by one blank line. The order is fixed, and so is the order of lines
  within each kind:

  | Trailer | Rule |
  |---|---|
  | `Brygge-Encoded: brygge <CARGO_PKG_VERSION> from <git\|hg\|svn\|cvs>` | always |
  | `Brygge-Source-Id: <id>` | always. The atom's `source.atom_id` **as is**, if every byte is printable ASCII (0x21–0x7E). Otherwise `hex:` followed by lowercase hex. One rule, specified in the guide. |
  | `Brygge-Derived: …` | the example's format, one per assertion, scoped `atom` or `op:<path>` |
  | `Brygge-Transformed: …` | the example's format |
  | `Brygge-Supplied: <field>, …` | the example's `Encoded-supplied`, renamed |
  | `Brygge-Altered: <field> (<why>)` | the example's `Encoded-altered`, renamed, one per field |
  | `Brygge-Artifact: sha256:<digest>` | always |

- **A message with no trailing newline** gets one, then the blank line. An empty message is just the block.
- **The same block goes on annotated tags.** `Brygge-Source-Id` is omitted there, because a ref has no atom.
- **A ref's `Transformed` mark** with nowhere to go (a lightweight tag or a branch) goes into the report, not to
  stderr. That is the change from the example.
- **Level 2:** `git interpret-trailers --parse` reads back exactly the block brygge wrote, for one commit per
  fixture.

## 4. Refs and the default branch (D-3)

- **The mapping:** the table in RFC 019 D-3. `Other` goes to `refs/brygge/other/<name>`.
- **Before writing:**
  - collisions are refused, naming both refs;
  - names are checked with `git check-ref-format`, once per name, under the §1 environment, before the stream
    starts. Collect every failure into the pre-flight report.
- **The default branch:**
  - the first of `trunk`, `main`, `master`, `default` that the IR has as a branch-like ref; otherwise the first
    branch-like ref in canonical order;
  - with no branch-like ref at all (tags only), `HEAD` names `refs/heads/main`, which does not exist. Say so in
    the report;
  - **the report names the choice.**
- **The scratch ref:** keep the example's scratch-ref technique, and keep the scratch ref out of the result.

## 5. The pre-flight (D-7)

- **What it collects, before anything is written:**
  - modes other than `100644`, `100755` and `120000`;
  - ref collisions;
  - invalid ref names;
  - non-UTF-8 annotated-tag messages;
  - any other condition under which the stream would be refused.
- **The report:** every refusal, one line each, naming the atom (by source id) or the ref. Then exit 2.
- **Level 1:** one test per refusal kind, plus one with three kinds at once, all listed.

## 6. Determinism (D-5)

- **Level 2:** encode the same fixture twice into two directories. `git for-each-ref --format='%(objectname)
  %(refname)'` must be byte-identical across the two.
- **Also level 2:** encode with a different `HOME`, `TZ` and locale in the parent environment, and get the same
  ids.

## 7. Documentation and CHANGELOG

- **`docs/src/guide/`:** a new page, "Migrating to Git" (`git-target.md`). This batch writes its command, mapping,
  trailers and report sections. G2 adds the check, G3 the authors, G4 the evidence.
- **`docs/src/reference/machine-output.md`:** the `encode.*` keys.
- **`CHANGELOG.md` `[Unreleased]`, `### Added`:** "`brygge encode git`: a supported encoder that writes a new
  bare Git repository from an artifact, every commit labelled with `Brygge-` trailers (RFC 019)." Under
  `### Breaking` (v0): `examples/encode-git` is removed, and a `brygge-encode-git` plugin is no longer run.

## 8. Checks

- fmt, and clippy `-D warnings` on stable, `x86_64-pc-windows-gnu`, `x86_64-apple-darwin` and 1.87;
- `cargo test --workspace --locked` on stable and 1.87;
- `tools/check-ir-isolation.sh`, including the new rule, with its planted-violation test;
- links, the book with no warnings, the release tools and `cargo deny`.

## 9. Request

- **File** `.git-exclude/review-requests/070-encode-git.md`.
- **Push the topic branch only after approval** (114), and name it in the request.
- **After CI is green on the branch,** with the Windows and macOS logs showing the `git` tests running, `main` is
  fast-forwarded on my approval.
