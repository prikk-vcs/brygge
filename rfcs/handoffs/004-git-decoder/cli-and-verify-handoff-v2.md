# Handoff — the CLI surface and honest verification (v2)

**Supersedes** `cli-and-verify-handoff-v1.md` for the command surface. v1 remains the record of how the
first surface was built.

**Governing design:**
- `docs/src/brygge-02-external-design-v0.1.md` **v0.3, §2.1 (CL-01…CL-08)**, revised per owner ruling D-7
  (2026-09-23);
- RFC 002 (the honesty machinery: VF-3/VF-4, FS-01/FS-02/FS-04/FS-05/FS-06);
- RFC 004 increment 2 (the CLI), whose state this handoff inherits (Accepted).

**Corrections carried** (numbered in the architect's intake review; defined by this handoff):

| ID | What |
|---|---|
| CR-18 | the three-verb surface and one vocabulary |
| CR-02 | `verify` checks that can actually fail |
| CR-13 | atomic artifact writes |
| CR-19 | untrusted text neutralized in every output |
| CR-21 | the before-the-run faithfulness statement (FS-05/FS-06), specified but never implemented |

**Policy:** brygge is v0 and has never been released, so breaking the CLI and the machine formats is
intended (owner, 2026-09-23). Do not add compatibility aliases for the old surface.

**Order:** handoff **1 of 4** in the first 0.1.0 batch. Build it first; the other three do not touch
`crates/brygge` except where they say so.

---

## 1. Purpose

Make the tool's surface impossible to misunderstand and its verification impossible to fake:
- three verbs, one noun per concept, nothing silently ignored;
- every `verify` check able to fail;
- source text unable to forge output or drive the terminal;
- a user told what faithfulness means for their source **before** the run.

## 2. The target surface (brygge-02 v0.3 §2.1: build exactly this)

```
brygge decode  <git|hg|svn|cvs> <source> --out <artifact> [--infer-renames | --reconstruct-refs] [--format human|machine]
brygge inspect <artifact> [--atoms] [--format human|machine]
brygge verify  <artifact> [--against-source <source>] [--format human|machine]
brygge --version | --help | <command> --help
```

**Removed:** `summary` (folded into `inspect`), the `--ir`/`--import` flags, and the `encode` stub.

## 3. Change scope

### 3.1 Parser (`crates/brygge/src/cli.rs`)

1. **Positional nouns.** `<source>` is positional on `decode`; `<artifact>` is positional on `inspect` and
   `verify`. `--out <artifact>` is **required** on `decode`.
2. **`--infer-renames`** replaces `--detect-renames` everywhere: the CLI, the `Options` field of
   `brygge-decode-git` and `brygge-decode-hg` (`detect_renames` → `infer_renames`), their provenance param
   key (`detect_renames` → `infer_renames`), and the against-source reconstruction that reads it back.
   Nothing called "detect" remains.
3. **Refuse inapplicable options.**
   - `--infer-renames` applies to git and hg only; `--reconstruct-refs` to svn and cvs only.
   - Any other combination fails with exit **2** and the message
     `--<option> applies to <kinds>, not to <kind>`.
   - Repeating a flag is also a usage error.
4. **Usage errors exit 2**, not 1: an unknown command, an unknown or missing flag, a missing value, a
   missing positional, or an inapplicable option. The message names the problem, then points to
   `brygge <command> --help`. Help text lists only existing commands.
5. **`encode`** is an unknown command whose message reads:
   `'encode' is not available yet: the prikk encoder follows prikk's import foundations (see ROADMAP)`.
   It exits 2.

### 3.2 Exit codes (`crates/brygge/src/exit.rs`)

- **Add `USAGE = 2`.** The module doc lists all codes exactly as brygge-02 CL-08 does.
- `FAILURE = 1` keeps runtime failures only: unreadable input, I/O, and an internal decoder fault (the
  CR-16 panic boundary).

### 3.3 `decode` (`crates/brygge/src/commands.rs`)

1. **Before-the-run statement (CR-21, FS-05/FS-06).**
   - Before decoding starts, print one short statement for the source kind to **stderr**, in both formats.
     The text below is the architect's and is used verbatim:
     - **git:** `Git: content, history and messages are carried as the source recorded them. Renames are
       inferred only with --infer-renames, and are then marked derived. Authorship is Unverifiable.`
     - **hg:** `Mercurial: content, history and messages are carried as recorded, including renames the
       source recorded. Authorship is Unverifiable.`
     - **svn:** `Subversion: revisions are carried as recorded. Branches and tags are only a directory
       convention; with --reconstruct-refs they are reconstructed and marked derived. Authorship is
       Unverifiable.`
     - **cvs:** `CVS has no atomic commits: every changeset is brygge's reconstruction (derived), and a
       changeset cannot be checked against the source. File contents and per-file history are carried as
       recorded. Authorship is Unverifiable.`
   - The CVS decoder handoff (later in 0.1.0) appends its branch sentence to the CVS text; leave the text
     in one place (a `faithfulness_statement(kind) -> &'static str` function) so that edit is one line.
2. **Atomic write (CR-13).**
   - Serialize, write to a temporary file in the **same directory** as `--out` (a name that cannot
     collide, e.g. `.<name>.brygge-tmp-<pid>`), flush and `sync_all`, then `rename` over `--out`.
   - On any failure, remove the temporary file and leave any existing `--out` untouched.
   - Print `wrote <artifact> (<n> bytes)` only after the rename succeeds.
3. **The end-of-run report** is `inspect`'s default report (§3.4), so `decode` and `inspect <artifact>`
   print the same text (FS-02).
4. **Exit class.** Keep today's rules, including the `30` precedence.

### 3.4 `inspect <artifact> [--atoms]`

1. **Default: the fidelity report** (`brygge_ir::honesty::summary(&ir)`, rendered). The human rendering
   changes in `crates/brygge-ir/src/honesty.rs`:
   - **"Unverified" → "Unverifiable"** everywhere, including doc comments. The sentence is
     `Authorship is Unverifiable (carried as the source claimed it; no target can verify it).`
   - **Split the dropped section**, so a newcomer is not alarmed by storage-layout drops:
     - `not history (no content or claim lost): N`: the `Representation` class, one line with the count;
     - `dropped (recorded loss): …`: `AdvisoryUnreliable` and `Other`, by class, as today.
   - The machine rendering (`render_machine`) **is unchanged**, so `REPORT_VERSION` stays `1`.
2. **`--atoms`** appends the per-atom listing (today's `inspect` body): atoms with status, source ids,
   rename hints with status, refs, and loss records.
   - In human form the loss records show `what` and `reason`, and derived records show their parameters:
     `derived:<kind> {k=v, …}`, with confidence where present.
   - The artifact is read once.

### 3.5 `verify <artifact> [--against-source <source>]` (CR-02)

**Always run the internal checks.** Each reports `pass`, `fail` or `n/a`; `n/a` is only for a check that
cannot apply, and says why. Each must be able to fail: write a failing test for every one (§5).

| Check | Rule |
|---|---|
| `integrity` | `brygge_ir::from_bytes` succeeds (digest, blob addresses, atom ids, version) |
| `structure` | Every parent id names an atom that appears **earlier** in the artifact's atom order. Atom ids are unique. Every ref target is an atom in the artifact. `(name, kind)` is unique among refs. At most one op per path per atom |
| `replay` | Replay each atom's ops against its **first parent's** tree (a root replays against the empty tree). `Add` needs the path absent; `Modify` and `Delete` need it present. Every rename hint's `to` exists in the atom's resulting tree. (It need not have an op: an SVN copy onto identical content produces none.) |
| `derivations` | Every `Derived` record (atom, op, hint, ref) has non-empty `by` and `decoder_version`, `confidence ≤ 100` if present, and the parameters its kind requires: `InferredRename`: `rename_algorithm`, `rename_threshold`; `ReconstructedChangeset`: `window_secs`, `cluster_keys`; `ReconstructedBranch`: `layout` or `source` |
| `source-invariants` | By `provenance.source.kind`: every atom's `source.kind` equals it, and `provenance.decoder` is that kind's decoder name. **cvs:** every atom `Derived(ReconstructedChangeset)`; every ref `Derived`; no rename hint. **svn:** every atom `Stated` with ≤ 1 parent; every ref `Derived(ReconstructedBranch)`. **git:** no `Stated` rename hint. **hg:** ≤ 2 parents per atom. **Other/unknown kind:** `n/a` ("no invariants known for this source kind") |
| `provenance` | `decoder`, `decoder_version` and `brygge_version` are non-empty; `source.kind` and `repo_id` are present |
| `loss-boundary` | Every drop record has a non-empty `what` and `reason` |

- **Replay memory is bounded:** retain a replayed tree only until the last atom whose first parent it is
  has been checked (count the remaining first-parent references). Do not keep a tree per atom.
- **Authorship is not a check.** Print a fixed line, `authorship: Unverifiable by construction (no IR
  field can express target verification)`, never `[ok]`.

**With `--against-source <source>`, additionally**, the existing re-derivation (unchanged logic). The
report prints **two separate result lines**:
- `internal: pass|fail`;
- `against-source: corresponds|does not correspond`, or for CVS `reproduces|does not reproduce`, with
  the note that changesets are brygge's reconstruction.

If the internal checks fail, still run against-source; the claims are independent (VF-4). Exit `50` if
anything fails.

### 3.6 Untrusted text in every output (CR-19)

Add a module `crates/brygge/src/display.rs` (with sibling tests) holding two functions. Use them for
**every** string that can originate in a source repository (paths, names, emails, messages, ref names,
drop `what` strings, error texts carrying paths) on stdout **and** stderr.
- **`human(s) -> Cow<str>`.** Replace every control character (U+0000–U+001F, U+007F,
  U+0080–U+009F) and every Unicode bidirectional or invisible format control (U+200B–U+200F,
  U+202A–U+202E, U+2060–U+2069, U+FEFF) with `\u{XXXX}`. Replace a literal backslash with `\\` so the
  escaping cannot be imitated.
- **`machine_value(s) -> String`.** Pass through ASCII `A–Z a–z 0–9 - . _ ~ / @ : +`, and percent-encode
  every other byte of the UTF-8 encoding as `%XX` (uppercase hex), including `%`, `=`, space and all
  non-ASCII bytes. A value can then never contain `=` or a newline.

**Machine formats change, so bump them:** `inspect_version=2`, `verify_version=2`.
- Untrusted text never appears in a **key**; items are indexed:
  - `ref.<i>.name=<value>`, `ref.<i>.kind=<label>`, `ref.<i>.target=<hex>`;
  - `loss.<i>.class=<label>`, `loss.<i>.what=<value>`;
  - `atom.<i>.…` as today.
- Labels are fixed lowercase words (`branch`, `tag`, `bookmark`, `named-branch`, `other`;
  `representation`, `advisory-unreliable`, `other`). Never Rust `Debug` output.
- `verify` machine lines:

  ```
  verify_version=2
  verify.check.<name>=pass|fail|n/a
  verify.authorship=unverifiable
  verify.internal=pass|fail
  verify.against_source=corresponds|does-not-correspond|reproduces|does-not-reproduce|not-run
  verify.detail.<i>=<value>
  verify.result=pass|fail
  ```

### 3.7 Documentation

- **`README.md`:**
  - the 30-second start, the fidelity-report section (the split dropped section, "Unverifiable"), and the
    exit codes (add `2`), all using the new surface;
  - a short **Vocabulary** line: artifact, source, stated, derived, dropped, refused, Unverifiable.
- **`crates/brygge/README.md`, and the doc comments in `main.rs` and `cli.rs`:** the new surface. Remove
  the "RFC 004 increment 2" framing.
- **`crates/brygge-decode-git/README.md` and its example `decode_repo.rs`:** use `--infer-renames` /
  `infer_renames`.

## 4. Non-change scope

- **The IR model, codec, contract version and `from_bytes`** (RFC 011 owns them). Read-only use only.
- **Decoder logic**, except the `detect_renames` → `infer_renames` rename (§3.1.2).
- **The per-source decode rules, floors and loss records.** Other handoffs own them.
- **`REPORT_VERSION` and `render_machine`** (§3.4).

## 5. Required tests

The CLI tests in `crates/brygge/src/cli/tests.rs` and `src/commands/tests.rs` are updated to the new
surface; none may be deleted to make the suite pass.

1. **Parser:**
   - each valid form parses;
   - each removed form (`summary`, `--ir`, `--import`, `--detect-renames`) is a usage error, exit 2;
   - `decode` without `--out` → exit 2;
   - `--infer-renames` with svn/cvs and `--reconstruct-refs` with git/hg → exit 2, with the exact message;
   - `encode` → exit 2 with its message.
2. **Faithfulness statement:** for each kind, it is printed before decoding. Test the function's text,
   and test that `decode` emits it even when the decode then fails.
3. **Atomic write:**
   - a pre-existing good artifact at `--out` survives a decode that fails (e.g. a refused source);
   - a successful decode replaces it;
   - no temporary file remains in either case.
4. **`inspect`:**
   - the default output equals `decode`'s end-of-run report for the same artifact;
   - `--atoms` adds the listing;
   - the human report says `Unverifiable` and never `Unverified`;
   - representation drops appear only on the `not history` line.
5. **`verify`, one failing artifact per check.** Build each with `IrBuilder` or by editing a decoded IR,
   then `to_bytes`:
   - `integrity`: a flipped byte;
   - `structure`: a dangling parent; a duplicate ref; two ops on one path;
   - `replay`: `Modify` of an absent path; `Add` of a present path;
   - `derivations`: a missing required param; confidence 101;
   - `source-invariants`: a CVS artifact with one atom relabelled `Stated`, with ids and digest
     recomputed. **This is the stripped-honesty case C-3a names, and it must now fail**;
   - `provenance`: an empty decoder.

   Also: a good artifact from each of the four decoders passes all checks; with `--against-source`,
   the two result lines are separate, and a failing internal check with a corresponding source still
   reports `against-source: corresponds` and exits 50.
6. **Replay memory:** a 10,000-atom linear history verifies while retaining at most two trees. Expose a
   counter for the test; do not assert on RSS.
7. **Neutralization:**
   - a commit whose message holds `\x1b[2J` and a U+202E renders escaped in `inspect --atoms` (human);
   - a ref named `a=b` cannot produce a key containing its name. In machine form it appears only as
     `ref.<i>.name=a%3Db`.
   - Newlines cannot enter a Git ref name, so test `machine_value` directly for `\n` and `=`.
8. **Regression:** every existing decode/verify scenario (git, hg, svn dumpfile + live, cvs, the clone
   test, submodule refusal) is ported to the new surface and still passes.

## 6. Acceptance criteria

- The surface, messages and exit codes match brygge-02 v0.3 §2.1 exactly. Nothing is silently ignored,
  and help lists only existing commands.
- Every `verify` check can fail (tests prove it), and the stripped-honesty artifact fails.
- No source text reaches a terminal or a machine key unneutralized.
- The five gates pass, `--locked`; `Cargo.lock` unchanged; no new dependency.

## 7. Prohibited shortcuts

- No aliases or hidden compatibility for the removed surface.
- No check that returns a constant: every check must read the artifact.
- No `n/a` used to hide a failure. `n/a` means the rule cannot apply, and says why.
- No escaping of only some output paths. Route every source-derived string through `display`.

## 8. Security gate

This touches the honesty surface and output handling (T-1/T-3), so the architect reviews it against
`brygge-03`. The threat model gains an **output-neutralization control** at the 0.1.0 revision; cite
this handoff.

## 9. Review request

File `.git-exclude/review-request/003-cli-and-verify.md` with:
- the standard sections;
- appendix A: the table of every call site routed through `display`;
- appendix B: before/after output for `decode`, `inspect`, `inspect --atoms`, `verify` and
  `verify --against-source` on brygge's own repository, human and machine.

Do not commit until the review outcome is "Approved".
