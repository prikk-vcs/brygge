# Handoff — project hygiene: enforced isolation, declared floors, narrow APIs, accurate docs

**Governing:**
- RFC 009 (dependency surface), Accepted. This handoff inherits its state. **D-7** says the downstream
  boundary is "a tested property"; today nothing tests it.
- RFC 000 (the RFC index must reflect every RFC's state).
- The project rules (change tracking via RFCs or `CHANGELOG.md`).

**Corrections carried** (numbered in the architect's intake review; this handoff defines them durably):

| ID | What |
|---|---|
| CR-12 | The isolation test is claimed but absent; the floor is described as "a declared policy" but is only scattered code |
| CR-20 | Decoders publish internal modules as public API for test convenience |
| CR-14 | Broken links, stale status text, no CHANGELOG |

**Order:** handoff **4 of 4**. Independent of handoffs 2 and 3. For the README and CLI documentation, do
not touch what handoff 1 rewrites (its §3.7).

---

## 1. Change scope

### 1.1 Isolation as a CI gate (CR-12.1, RFC 009 D-7, INV-4/INV-5)

1. Add `crates/brygge-ir/allowed-dependencies.txt`: one crate **name** per line, sorted, covering
   `brygge-ir`'s full normal-dependency closure as it is today. Names only, not versions, so routine patch
   bumps do not fail the gate. The first line is a comment citing RFC 009 D-7 and saying that adding a
   name requires an architect review.
2. Add a CI step to `.github/workflows/ci.yml` (in the `supply-chain` job) and a local script,
   `tools/check-ir-isolation.sh`, that the step runs:
   - list `brygge-ir`'s normal dependency closure by name (`cargo tree -p brygge-ir -e normal
     --prefix none --format '{p}'`, reduced to names, de-duplicated, sorted);
   - `diff` it against the file; any difference fails the build.
3. Add the script to `HANDOFF.md` §7's gate list as the **sixth gate**.

### 1.2 The floor as a declared policy (CR-12.2, RFC 004 D-4, CF-03)

- Each decoder declares its floor in **one** private constant: the list of feature names it refuses,
  matching the `feature` strings its refusals use. Every refusal site uses these names; no duplicated
  literal.
- The decoder records it in provenance as `params["floor"] = "<names joined by ','>"`, so a reviewer
  reads from the artifact which floor applied (PR-5).
- The current floors by decoder:
  - **Git:** submodule, replace ref, grafts, shallow clone, object alternates, redirected git directory,
    non-UTF-8 path, non-UTF-8 ref name. Add whichever of these handoff 2 has not yet landed when it lands.
  - **hg:** subrepo, largefiles, lfs, censored revision.
  - **SVN:** svn:externals, remote-source.
  - **CVS:** remote-source, whole-import-under-confidence-floor.
- **Text:** RFC 004 D-4, RFC 005 D-4, RFC 006 and RFC 007 say the decoder "reads a floor policy". Amend
  each with one sentence: the floor is **declared in code as one owner-ratified list and recorded in
  provenance**. Changing it is a reviewed code change, not a runtime knob (CF-03: the migrator gets no
  knob).

### 1.3 Narrow public APIs (CR-20)

- Each decoder crate publishes only what a caller needs:
  - `decode`, `Options`, `Error` and `decoder_version` (all four);
  - `Source` (SVN, CVS);
  - `LayoutPolicy` (SVN).
- **Change these to `pub(crate)`:**
  - `brygge-decode-hg`: `pub mod revlog` and `pub mod requires`;
  - `brygge-decode-cvs`: the `pub mod rcsfile` re-export (remove it).
- Tests stay in-crate, as siblings.
- **Keep, marked transitional:** `brygge-decode-svn::LAYOUT_UNMATCHED` and `brygge-decode-cvs::UNDER_FLOOR`.
  The CLI still string-matches them to pick exit 30, until RFC 011 gives violations a typed record. Give
  each a doc comment "transitional: replaced by the typed refused/violation record of RFC 011".
- Each decoder's `lib.rs` crate doc lists its public items in one short paragraph.

### 1.4 Documentation accuracy (CR-14)

1. **Links broken by commit `74dc0eb`**, where `HANDOFF.md` and `GOVERNANCE.md` moved to
   `docs/src/development/handoffs/`:
   - `rfcs/README.md` (line 6);
   - `docs/src/brygge-01-requirements-spec-v0.1.md` (line 13).

   `README.md`'s links are fixed by handoff 1; `ROADMAP.md`'s and `brygge-02`'s are already fixed. Afterwards, check that
   every relative link in every `*.md` under the repository (excluding `.git-exclude/` and `target/`)
   resolves, and include that check's output in the review request. A small script is fine;
   `tools/check-links.sh` may stay as a tool.
2. **Stale status text:**
   - `crates/brygge-decode-hg/README.md` and its `lib.rs` crate doc still say "foundation increment";
     describe the built decoder, in the tone of the SVN and CVS READMEs.
   - RFC 005 D-6 still describes the rejected Tier 3 (`hg` CLI). Add a one-line note that Tier 2 was
     ruled, so only the first sentence applies.
   - RFC 006 and RFC 007 status blocks: remove the "Implementation toward M3/M4 may begin" sentences
     beside "built and green".
   - `rfcs/README.md`:
     - replace the long "State" prose with a state-grouped index table as RFC 000 recommends
       (Accepted · Done · Proposed · Archive), listing every RFC (000–007, 009, 010) with a one-line
       scope and a pointer to its handoffs folder;
     - note 008 as reserved (Track B1) and 011 as upcoming (the IR contract re-cut).
3. **`GOVERNANCE.md`:** the conventions line says "any C/FFI confined to a single dedicated crate —
   C-4b". Since threat model v0.2, C-4b reads "a dedicated FFI crate **or a subprocess**"; align it.
4. **`HANDOFF.md`:**
   - §8.4 lists `RR-svn-svnadmin` and `RR-svn-svnadmin-version` as unfolded, but `brygge-03` v0.2
     already contains them; only `RR-cvs-reconstruction` is outstanding.
   - §2 and §8 describe M1–M4 as delivered; say "built, not released", pointing to `ROADMAP.md`'s
     release plan.
5. **Create `CHANGELOG.md`** at the root, in the Keep a Changelog shape, with an `## [Unreleased]`
   section (Added / Changed / Fixed / Removed). Seed it with:
   - the CR-16 fix (`20f43f9`);
   - the ROADMAP release plan (`420b88d`);
   - this handoff's own changes.

   From now on every handoff adds its entry in its own commit.

## 2. Non-change scope

- Code behaviour, other than the declared-floor refactor (§1.2) and visibility changes (§1.3). No decode
  output may change except the added `floor` provenance param.
- The IR (RFC 011), the CLI surface (handoff 1), the threat model and design documents (the architect
  revises those at the 0.1.0 release preparation).

## 3. Required tests

1. **Isolation:** the CI step passes on `main`. Locally, temporarily adding any dependency to `brygge-ir`
   makes `tools/check-ir-isolation.sh` fail. Describe the manual run in the review request; do not commit
   the temporary dependency.
2. **Floor:** for each decoder, a decoded artifact's `params["floor"]` equals the declared list, and every
   existing floor-refusal test still passes, now using the declared names.
3. **API:** the crates still build and all tests pass with the narrowed visibility. The workspace has no
   external user of the removed items (`rg` evidence in the review request).
4. **Links:** the link check reports zero broken relative links.

## 4. Acceptance criteria

- Isolation is enforced by CI, not asserted.
- Each floor is one declared list, recorded in every artifact.
- The decoders' public APIs are minimal and documented.
- No broken link and no stale status claim remains in the files listed; `CHANGELOG.md` exists.
- The five gates plus the new isolation gate pass, `--locked`; `Cargo.lock` unchanged.

## 5. Prohibited shortcuts

- No version pins in the allowlist.
- No `#[doc(hidden)] pub` in place of `pub(crate)`.
- Do not rewrite design documents beyond the link fixes listed.

## 6. Review request

File `.git-exclude/review-request/006-project-hygiene.md` with the standard sections, the link-check
output, and the isolation-gate negative run.
