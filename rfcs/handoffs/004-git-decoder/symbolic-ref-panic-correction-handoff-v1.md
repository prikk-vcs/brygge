# Handoff — correction: `decode git` must not panic on symbolic refs (CR-16)

**Governing RFC:** RFC 004 (Git decoder), Accepted. This handoff inherits that state. It is a
**correction**: the code diverges from accepted design (RFC 004 D-6/D-2, `HANDOFF.md` §7 "never panic on
input"). It is not a design change.

**Origin:** the dev team's onboarding review (it reproduced the panic by decoding brygge's own repository)
and the architect's intake review, where this correction is numbered **CR-16**. This handoff is the
durable definition of CR-16.

**Priority:** stage 0 of the 0.1.0 work order. It is independent of every pending owner decision.
**Start now.**

---

## 1. Purpose

Make `brygge decode git` and `brygge verify --against-source` work on ordinary Git repositories.
- A repository carrying a **symbolic ref** under `refs/` must decode.
  - `git clone` always creates one: `refs/remotes/origin/HEAD` → `refs/remotes/origin/main`.
- More generally, no Git input, well-formed or malformed, may crash the process or silently fabricate a
  value.

## 2. Background (verified)

- `crates/brygge-decode-git/src/decode.rs:216`: `scan_refs` calls `r.id()` for **every** ref.
  - In gix 0.87.1, `Reference::id()` is `self.try_id().expect("BUG: tries to obtain object id from
    symbolic target")` (`gix-0.87.1/src/reference/mod.rs:35–37`). A symbolic ref therefore panics.
  - The dev team reproduced this by decoding brygge's own repository.
- `decode.rs:497–498`: `author.seconds()` / `committer.seconds()`.
  - gix-actor 0.42.0 documents `seconds()` as "silently default to 0 if parsing fails"
    (`gix-actor-0.42.0/src/signature/mod.rs:53–64`).
  - A malformed commit time becomes the claim "1970-01-01" with no record. That violates NG-5 (asserting
    what the source did not state) and HO-2.
- The workspace lints (`expect_used`, `unwrap_used`) cannot see a panic *inside a dependency*. Nothing in
  the CLI converts an unexpected panic into a CL-08 outcome, so a crash exits 101, which is not a
  documented class.

## 3. Change scope

1. **`crates/brygge-decode-git/src/decode.rs`**
   - **a.** `scan_refs`: replace `r.id()` with the fallible `try_id()`.
   - **b. Symbolic-ref policy (architect ruling, within RFC 004 OQ-B).** The IR `RefRecord` cannot express
     aliasing.
     - A symbolic ref, in **any** namespace, is not carried as a ref and contributes **no walk tip**. Its
       target ref, if it exists and is in a carried namespace, is carried on its own.
     - Record one drop, class `Representation`: `what = "symbolic refs (N)"`, reason "an alias to
       another ref; the IR has no alias concept; the target ref is carried on its own (RFC 004 OQ-B)".
     - A **dangling** symbolic ref (target missing) is dropped under the same record.
     - A symbolic ref is detected by the absence of a direct id (`try_id()` is `None`), or by gix's
       explicit target-kind API. Choose one and say which in the review request.
   - **c.** Metadata times: **parse the seconds strictly from the signature's raw `time` field**
     (`SignatureRef::time`, a `&str`). Do not use `seconds()`, and do not use `time()` either.
     *(Amended 2026-09-23 after review. The first version of this handoff said "use the fallible
     `time()`", but gix-date's `parse_header` is lenient in two ways: it salvages the leading digits of a
     malformed seconds token (`"16954-56000"` → `16954`), and it silently defaults a malformed
     timezone offset to `+0000`. Both are the silent-approximation class this correction exists to
     remove.)*
     - The rule: the first whitespace-separated token must match `-?[0-9]+` exactly and fit `i64`.
       Otherwise the claim is `None`: never `0`, never a guess, never a salvaged prefix.
     - Record one drop, class `Other`: `what = "unparseable author/committer times (N)"`, counting
       fields, not atoms. Reason: "the source's time field could not be parsed; the claim is absent
       rather than fabricated (NG-5)".
     - Only the seconds value goes into the existing IR field. The timezone offset is **not** carried in
       this handoff (that is CR-06/CR-09). When it is carried, it must also be parsed strictly, never via
       gix's defaulting parser.
   - **d. Sweep.** Audit every gix call in `brygge-decode-git` (`decode.rs`, `open.rs`) for convenience
     APIs that **panic** (`expect`/`unwrap` inside the dependency) or **silently default**
     (`unwrap_or_default`, lossy fallbacks). Replace each with its fallible form, handled as a typed error,
     a named refusal, or a recorded drop, per RFC 004's rules.
     - **Exception:** `to_str_lossy` is out of scope here. It is CR-03, which waits on owner decisions
       D-1/D-3. Leave it as is, but list its sites in the sweep report.
2. **`crates/brygge/src/commands.rs` — panic boundary (defense in depth)**
   - Wrap the decoder invocation in `decode_source` (the single path used by both `decode` and
     `verify --against-source`, for all four sources) in `std::panic::catch_unwind`, with
     `AssertUnwindSafe` justified in a comment.
   - Factor this into a small helper (e.g. `guard_decoder`) so it is unit-testable.
   - A caught panic maps to exit `FAILURE` (1) with the message:
     `internal decoder fault: <payload if it is a &str/String> — this is a brygge bug; please report it`.
     No artifact is written. The default panic hook's stderr output may remain.
   - The boundary relies on unwinding. Add a comment next to it, and in the workspace `Cargo.toml`
     `[profile]` area, stating that `panic = "abort"` must not be set without revisiting it.
3. **Tests** (§5).
4. **Docs:**
   - `crates/brygge-decode-git/README.md`: symbolic refs are dropped-with-record.
   - `rfcs/accepted/004-git-decoder.md` OQ-B resolution: add one sentence recording the symbolic-ref
     policy, citing this handoff.

## 4. Non-change scope (do not touch)

- The IR (`brygge-ir`), its types, codec and contract version. No new field, variant or loss class.
- Walk-tip policy beyond symbolic refs. Stash, notes and remote-tracking commits are still walked; that is
  CR-05, a separate handoff.
- Lossy UTF-8 (`to_str_lossy`): CR-03. Timezone carrying: CR-06/CR-09.
- The hg, SVN and CVS decoders. They only gain the CLI-level panic boundary.
- Exit-code taxonomy (no new class), CLI flags, output formats (`render_*` unchanged except for the new
  drop records appearing naturally).

## 5. Required tests

All go in the sibling test modules (`src/decode/tests.rs`, `src/commands/tests.rs`). Git-dependent tests
skip when `git` is absent, as the existing ones do.

1. **Symbolic ref in a dropped namespace:** a fixture with `git symbolic-ref refs/remotes/origin/HEAD
   refs/remotes/origin/main` (plus the `origin/main` ref). It decodes without panic; the symbolic ref is
   absent from `ir.refs`; the `symbolic refs (1)` drop is present.
2. **Real clone:** `git clone` a local fixture repository into a second temp dir and decode the clone.
   - It succeeds.
   - `verify --against-source` on the clone passes.
   - The atom set equals that of decoding the original. Compare the `source.atom_id` sets, not the bytes:
     `repo_id`/refs may differ legitimately.
3. **Symbolic ref in a carried namespace:** `git symbolic-ref refs/heads/alias refs/heads/main`. `alias`
   is not a `RefRecord`, `main` is, and the drop is recorded.
4. **Dangling symbolic ref:** a symbolic ref to a nonexistent branch. It decodes, and the ref is dropped
   under the same record.
5. **Malformed commit time:** a commit object written with an overflowing numeric time, *and* one whose time token is malformed
   but made only of bytes gix's signature scanner keeps (`[-+0-9 \t]`, e.g. `16954-56000`). A token such as
   `1695456000abc` cannot reach the parser through a real commit: the scanner stops at `a` and the commit
   object then fails to parse as a whole, so exercise it with a direct unit test of the parser instead.
   The claim must be `None` in every case. Use
   `git hash-object -t commit -w --literally` on a hand-built commit body, then point a branch at it. The
   resulting claim is `None` (not `0`), the `unparseable author/committer times (N)` drop is present, and there is
   no panic.
6. **Panic boundary:** a unit test of the helper with a closure that panics. It returns exit `FAILURE`
   and the fault message, and does not propagate the panic.
7. **Regression guard:** all existing tests still pass unchanged, and the determinism test still holds.
   The new drop records must be deterministic: counts, not iteration-order-dependent text.

## 6. Acceptance criteria

- `brygge decode git <a git clone>` succeeds and `verify --against-source` passes (tests 1–4). Also run
  it manually on brygge's own repository and paste the output into the review request.
- No commit time is ever fabricated (test 5).
- A decoder panic is never an unclassified crash (test 6).
- The sweep report (§8) lists every gix call site in the crate, its disposition, and its line.
- All gates pass, `--locked`: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets
  --all-features --locked -- -D warnings`; `cargo test --workspace --locked`; `cargo deny check`;
  `cargo audit`.
- No new crate dependency (`Cargo.lock` unchanged).

## 7. Prohibited shortcuts

- Do **not** "fix" by skipping all refs outside `refs/heads`/`refs/tags` before calling `id()`. A symbolic
  ref can live in any namespace (test 3).
- Do **not** default an unparseable time to `0`, to another commit's time, or to "now".
- Do **not** use the panic boundary *instead of* fixing a panic. It is the last line, not the fix.
- Do **not** add `#[allow(clippy::…)]` to production code to silence lints introduced by the change.
- Do **not** change the IR, the exit-code set, or CR-03/CR-05/CR-06 behaviour. If a fix seems to require
  it, stop and report (governance §8.1).

## 8. Security gate (GOVERNANCE)

This touches an untrusted-input path (ref scanning and metadata parsing), so the architect runs a security
review of the change against `brygge-03` at review time.
- **INV-2 (untrusted input, never panic on input):** this change strengthens it; the threat model is
  re-verified, not updated.
- Put the **sweep report** in the review request. It is the evidence the review relies on.

## 9. Review request

File it as `.git-exclude/review-request/002-cr16-symbolic-ref-panic.md`, with the standard sections:
1. implementation summary
2. addressed CR / requirements
3. changed files
4. important implementation decisions (including the symbolic-ref detection choice, §3.1b)
5. differences from this handoff
6. executed tests
7. test results
8. gate results (all five commands, with exit codes)
9. unresolved issues
10. known limitations
11. requested review focus

Add two appendices:
- **the sweep report:** a table of call site (file:line), API, panicking/defaulting/safe, and disposition;
- **the manual run on brygge's own repository** (§6).

Do not commit or push until the architect's review outcome is "Approved".
