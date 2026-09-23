# Handoff — 0.1.0 release preparation, part 2: a green CI on the declared MSRV, two honesty gaps, and a clean machine output

**Governing:** the 0.1.0 release plan (`ROADMAP.md`); RFC 009 (dependencies, gates); `brygge-02` CL-07/CL-08/CT-04
(machine output); RFC 005 and RFC 006 (the two decoder items). It continues
`release-prep-0.1.0-handoff-v1.md`.

**Why.** The architect's stage-5 pass at `6eac97e` found these:
- **CI has been red since 2026-09-03.** The last green run was `7baf133`, and 49 of the last 51 runs
  failed, including every commit of this release cycle. The architect's intake review did not check CI
  either; it does now. Every local gate report said "green", because the local toolchain is newer than the declared
  MSRV, which is what CI uses. A release whose declared MSRV does not build is not releasable.
- **Two honesty gaps**, found while writing the user guides.
- **Machine-output defects**, found while writing `docs/src/reference/machine-output.md`. The output
  contract is still unreleased, so v0 lets us fix them now rather than carry them.

**Order:** one batch, filed as review request 013.

---

## 1. Build and gate on the declared MSRV (1.85)

**Finding.** Architect reproduction:
- `human_format 1.2.1` uses let-chains, which do not build on 1.85, and does not declare a `rust-version`.
  It comes in through `gix-features → prodash`.
- Pinned to `1.1.0` (`prodash` requires `>=1.0.3`), the workspace builds on 1.85 and **466 tests pass**.
- clippy 1.85 then reports six `format_collect` errors, all in test code:
  - `brygge-decode-hg`: `decode/tests.rs:410, 708, 848`; `published/tests.rs:181`; `revlog/tests.rs:84`;
  - `brygge-decode-git`: `decode/tests.rs:674`.

**Change.**
- `cargo update -p human_format --precise 1.1.0`: a lock-only downgrade of one crate. It is accepted under
  RFC 009 as the architect's decision, because it adds no new crate. Put a comment in the root
  `Cargo.toml`'s `[workspace.dependencies]` area saying why the lock holds `human_format` at 1.1.0: it is
  the newest release that builds on the MSRV, and it undeclares its `rust-version`.
- Fix the six clippy sites. Use the crate's existing hex helper; do not `#[allow]`.
- **The review request reports the local 1.85 run** below, which is exactly what CI runs. **After** the
  approved commit is pushed, report the CI run id and its result as well. A red CI blocks the cut, and it
  is fixed before anything else.
- **The local gate list gains the MSRV run**, and every future review request reports it:
  `cargo +1.85 clippy --workspace --all-targets --all-features --locked -- -D warnings` and
  `cargo +1.85 test --workspace --locked`.
  - Add both to `docs/src/development/handoffs/HANDOFF.md`'s gate list.
  - Add a note that CI's result for the pushed commit is part of every report.

## 2. hg: `--infer-renames` must not be accepted when nothing is inferred (RFC 005)

**Finding.**
- The CLI accepts `--infer-renames` for hg (`cli.rs:265-269`), and the hg `Options::as_params` records
  `infer_renames=true`, `rename_algorithm=exact-content-move` and `rename_threshold` in provenance
  (`hg options.rs:33-47`).
- But **no hg code infers anything.** RFC 005 never offered inference for hg, because Mercurial records
  its renames.
- So the artifact claims an algorithm ran that did not. That is a false claim in provenance, and a flag
  that silently does nothing.

**Change.**
- `--infer-renames` with `hg` is a **usage error** (exit 2), with the same shape of message as
  `--reconstruct-refs` given to Git.
- `--help` says `--infer-renames (git)`.
- Remove the inference fields from the hg `Options` and from its provenance params.
- `verify --against-source` for hg no longer reads them.
- **Tests:**
  - the usage error;
  - an hg artifact's params carry no `rename_*`/`infer_renames` keys.
- **Breaking:** hg artifacts' params change. Record it in the CHANGELOG.

## 3. SVN: non-UTF-8 `svn:author`/`svn:log` are carried as bytes, never dropped (RFC 006; RFC 011 D-4)

**Finding.** `get_str` (`svn decode.rs:245, 251`) turns a non-UTF-8 `svn:author` or `svn:log` into an
**absent** claim, with no record: a silent drop. Subversion validates these as UTF-8, but repositories
loaded with `--bypass-prop-validation`, or converted by old tools, can hold other bytes.

**Change.**
- Carry both as `Text { bytes, encoding: None }`, byte-exact, whatever the bytes are. The IR's text is
  bytes, so nothing is lost and nothing needs refusing.
- `svn:date` keeps its rule (unparseable → absent and counted).
- **Test:** a dumpfile with a Latin-1 `svn:log` and `svn:author` carries both byte-exact.

## 4. hg: the last check-then-read and a second, weaker bookmarks read (handoff 3 §6)

- **The obsstore.** `obsstore.rs:81-92` checks the size, then calls `fs::read`. Read it through
  `take(max + 1)`, as `read_bounded` does, or through `read_bounded` itself.
- **Bookmarks.** `add_refs` (`hg decode.rs:740-741`) re-reads `.hg/bookmarks` with an unbounded
  `read_to_string`, and skips malformed lines silently. The strict, bounded parse already happens in
  `published.rs`; reuse its result. There must be one parse, not two.
- **Tests:** existing behaviour holds, and a malformed bookmark line is still `Read` via the single path.

## 5. The machine output, before 0.1.0 freezes it (CL-07, CT-04)

These rules are the architect's decisions. `docs/src/reference/machine-output.md` (drafted, uncommitted)
is updated by the architect to match once you land them.

- **Naming rule:**
  - **keys** are dot-separated segments in `snake_case`;
  - **enumerated values** (labels) are `kebab-case`.
- **Report fixes:**
  1. **Keys that embed a label follow the key rule.**
     - `derived.inferred-rename` → `derived.inferred_rename`, and so on for every `derived.*`,
       `dropped.*` and `flagged.*` key.
     - `verify.check.source-invariants` → `verify.check.source_invariants`, and
       `verify.check.loss-boundary` → `verify.check.loss_boundary`.
     - Values keep kebab-case.
  2. **One key, one line.** `inspect --atoms` prints `atoms` twice (`commands.rs:628`, `honesty.rs:100`).
     The listing does not repeat it.
  3. **`atom.N.message` is the raw bytes, percent-encoded once**, like every other value. It is not
     prepared for the terminal first (`commands.rs:122-127, 635-640`).
     - A consumer percent-decodes to the exact bytes, including non-UTF-8.
     - The human form keeps its display escaping.
  4. **Details belong to their check.**
     - Replace `verify.detail.N` with `verify.check.<name>.detail`, printed right after that check's
       line, only when it is not `pass`.
     - The source comparison's explanation becomes `verify.against_source.detail`.
     - `verify.note.0` becomes `verify.against_source.note`.
  5. **A check that could not run** is `not-checked`, not `n/a`. It is the same word `against_source`
     already uses for "requested but could not run". `not-run` keeps its meaning: not requested.
  6. **Skipped fields are visible to machines.**
     - The fidelity report prints `skipped_non_critical_fields=N`: fields from a newer contract that this
       build skipped, always printed, 0 when none.
     - `verify` prints `verify.skipped_non_critical_fields=N`.
     - Today this reaches stderr only (`commands.rs:494-500`).
  7. **`Other(name)` is not flattened.** Where a status or kind is `Other(name)`, print the label `other`
     and a companion key `…_other=<escaped name>`: `atom.N.status_other`, `atom.N.copy.M.status_other`,
     `ref.N.status_other`, `ref.N.kind_other`.
- **Versions:** `report_version` 2 → **3**, `inspect_version` 3 → **4**, `verify_version` 3 → **4**.
- **Exit codes are unchanged.** `verify`'s `incomplete` stays exit 1, and `verify.result=incomplete` on
  stdout (which is empty on a runtime failure) distinguishes it. The reference documents that.
- **Tests:** the existing machine-output tests are updated to the new keys, plus one test per item above.
  Add one test asserting that every key printed across a full `decode`/`inspect --atoms`/`verify` run
  matches `^[a-z0-9_]+(\.[a-z0-9_]+)*$`.

## 6. Non-change scope

- The IR contract (no artifact format change).
- Decoder behaviour, apart from §2 (hg params) and §3 (SVN claims).
- Exit codes.

## 7. Security gate

- §1 restores CI enforcement of every gate, including `cargo deny`/`audit` on the real build.
- §3 closes a silent drop (INV-1).
- §5's item 3 removes a lossy transform from an output that CI consumes.

## 8. Review request

File `.git-exclude/review-request/013-release-prep-part2.md` with the standard sections, plus:
- the 1.85 gate output (clippy and test);
- the full old → new key map for the machine output.
