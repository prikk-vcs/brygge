# brygge RFCs

Design decisions for brygge are recorded as RFCs, following the ecosystem's **five-folder lifecycle**
(the same one prikk and stikk use; the canonical policy is `done/000-rfc-lifecycle-policy.md`).

> New to brygge? Start at [`../HANDOFF.md`](../HANDOFF.md) for the whole map; this file is the authoritative
> record of RFC **state** (the folder an RFC lives in is the source of truth).

```
proposed/   a decision drafted for review, not yet settled
accepted/   settled — an implementer may build against it (each gets a handoff under handoffs/NNN-slug/)
done/       shipped — kept forever, never deleted
archive/    withdrawn or superseded, kept for the record
```

An RFC moves `proposed → accepted` when its design is settled and the owner has approved any decision
that is the owner's (see `GOVERNANCE.md`); `accepted → done` when the work ships. Every accepted RFC
gets a **program-design handoff** under `rfcs/handoffs/NNN-slug/` before implementation.

**The upstream contract is prikk RFC 113** (*History import foundations*), which lives in the prikk
repository, not here. brygge's RFCs realize the *decoder/IR/encoder* side of that contract; RFC 113
§4a's owner-open questions (OQ-1…OQ-3) gate brygge's encode-to-prikk work.

## RFC index (planned)

Numbering is brygge's own. **Track A** (decode → IR) depends on nothing in prikk and is buildable now;
**Track B** (encode → prikk) past a reviewable proposal is gated on prikk (UD-1…UD-3) and the owner
(OQ-1…OQ-3). Build order follows the difficulty gradient (requirements §7).

| RFC | Scope | Track / Phase | Gate |
|---|---|---|---|
| **001** | **IR foundations & obligations** — the intermediate representation satisfying `IR-1…IR-6` / `IX-01…07`: faithfulness-with-provenance, per-atom epistemic status, opaque source ids, the loss boundary, encoder-agnostic, versioned | A0 | none — **first** |
| **002** | **Honesty & provenance machinery** — derived-vs-stated marking, loss-boundary recording, the fidelity summary recoverable from the objects (HO-1/2/4, FS-02) | A0 | none |
| **003** | **Determinism, IR artifact format & versioning, integrity digest** — VF-1, IX-07, tamper-detectability (C-3b) | A0 | none |
| **009** | **Dependency-surface & supply-chain policy** — `gix` vs `libgit2`, FFI isolation, `cargo-deny`/`cargo-audit` gates (INV-4/INV-5) — *security-foundational, brought early* | A0 | none — **early** |
| **004** | **Git decoder** — identity inference (renames marked derived), the floor mechanism | A1 | decode: none · floor contents: OQ-3 |
| **005** | **Mercurial decoder** — stated renames (SRC-H2); named-branch/bookmark/phase/obsmarker handling | A2 | decode: none · floor contents: OQ-3 |
| **006** | **Subversion decoder** — branch reconstruction by convention (derived), mergeinfo discipline | A3 | decode: none · floor contents: OQ-3 |
| **007** | **CVS decoder** — changeset reconstruction, lossy-but-labelled verdict (SRC-C3) | A4 | decode: none · floor contents: OQ-3 |
| **008** | **prikk encoder** — the reviewable-proposal form now; sealed imports later | B0 now; B1 gated | UD-1/UD-2/UD-3, OQ-1/OQ-2 |

The list will grow (a second-target encoder RFC to prove PU-3; further sources under "etc."). Each RFC
is written by the architect and reviewed/approved per `GOVERNANCE.md`.

## State

**All four sources on the gradient are built, and the IR contract is FROZEN at 1.0.0 (RFC 003 D-7, executed
2026-09-08).** RFC 004 (Git), 005 (Mercurial), 006 (Subversion), and 007 (CVS) are accepted and implemented
through the CLI/verify surface; `brygge decode git|hg|svn|cvs`, `inspect`, `verify`, and `summary` all work
(Git/hg/SVN validated against real `git`/`hg`/`svnadmin`; CVS against hand-built RCS `,v` fixtures). The
freeze held across every source **with no contract change** — Git and hg were the pre-freeze basis; SVN
(convention-derived refs) and CVS (a `Derived` changeset *atom*, the deepest stress) were post-freeze and
fit **additive-only, needing nothing added**. The contract is `1.0.0`, additive-only within major 1.

- **Accepted:**
  - [RFC 006 — Subversion decoder](accepted/006-subversion-decoder.md) — accepted 2026-09-08 (M3);
    **increments 1–2 built and green** (`brygge-decode-svn` + CLI wiring, zero new crate dependencies). The gradient's third
    source and the IR's **derived-side** stress test: SVN revisions are
    atomic and linear (a `Stated` spine), but branches and tags are directory copies by *convention* —
    reconstructed only as `Derived` (SRC-S1/FA-2, the derived-marking archetype). Owner rulings: **read
    tier Tier D** — a pure-Rust *dumpstream* parser fed by a user-supplied dumpfile or a read-only local
    `svnadmin dump` (no FFI, no network; over hand-rolling FSFS/BDB or linking libsvn, OQ-A); **floor** —
    `svn:externals` refused, and a convention-violating layout **imported with a loud `Derived` record, not
    refused** (widest honest migration reach, OQ-B). First **post-freeze** source, so it must fit IR 1.0.0
    additive-only (RFC 003 D-7). **All three acceptance artifacts are done** (under
    `handoffs/006-subversion-decoder/`): D-9 confirmed SVN fits IR 1.0.0 with **zero contract changes**;
    the **program-design handoff** (Tier D needs no new crate dependency; the dumpstream is
    backend-uniform); and the **architect security review against `brygge-03`** (verdict: proceed — the
    supply-chain surface shrinks, the C-format risk is isolated by *subprocess* not linked, INV-1/2/3/5/6
    hold as bound tests). **Implementation toward M3 may begin.**
  - [RFC 005 — Mercurial decoder](accepted/005-mercurial-decoder.md) — accepted 2026-09-06; **built and
    delivered (M2)**. Read tier Tier 2 (pure-Rust revlog reader: index + delta chains + zlib/zstd via
    flate2/ruzstd, no C, no hg binary), ground-truth-validated against `hg debugdata`/`debugindex`. The
    **stated-rename** discipline is live (hg renames carried `Stated` → zero derived marks). Floor: subrepos
    / largefiles / censored / unknown-requires refused. **D-8 confirmed: no IR contract change for a second
    source.** Queued follow-ups: rename inference (OQ-A), `.hgtags`→tag refs (OQ-C), large-repo streaming +
    hashed-fncache long paths (OQ-E).
  - [RFC 004 — Git decoder](accepted/004-git-decoder.md) — accepted 2026-09-04, built through both
    increments; OQ-A (rename detection) and OQ-B (ref/tag fidelity) resolved 2026-09-06. `gix` approved
    with the security review at
    [`handoffs/004-git-decoder/gix-security-review-v1.md`](handoffs/004-git-decoder/gix-security-review-v1.md);
    the Git feature floor ratified (OQ-3).
  - [RFC 001 — IR foundations](accepted/001-ir-foundations.md) — handoffs under
    [`handoffs/001-ir-foundations/`](handoffs/001-ir-foundations/): the design handoff, and the
    **consolidated `brygge-ir` build spec** (folds in 002/003) that the implementation follows.
  - [RFC 002 — Honesty & provenance machinery](accepted/002-honesty-and-provenance-machinery.md)
  - [RFC 003 — Determinism, format & versioning](accepted/003-determinism-format-and-versioning.md)
    (resolves RFC 001's OQ-A/B/C)
  - [RFC 007 — CVS decoder](accepted/007-cvs-decoder.md) — accepted 2026-09-10 (M4), **built and green**
    (`brygge-decode-cvs` + CLI, zero new crate dependencies), the **last source on the gradient**. The IR's deepest epistemic stress: CVS has **no atomic commit**, so the
    **changeset itself is reconstructed** — a `Derived(ReconstructedChangeset)` *atom*, not just derived refs
    (SRC-C1, IR-2). The honest deliverable is **lossy-but-labelled** (SRC-C3): per-file content and history
    faithful, changeset grouping carried as brygge's derived judgment with its clustering parameters and a
    confidence, and **changeset-level VF-2 honestly absent** (no source atom to check against). Owner
    rulings: **read tier Tier R** — a pure-Rust RCS `,v` reader (uncompressed, so a **third
    zero-new-dependency** decoder, no subprocess; OQ-A); **confidence floor per-changeset** — import the
    confident majority, loudly flag/refuse the under-floor ones (OQ-B). Second **post-freeze** source, so it
    must fit IR 1.0.0 additive-only (preliminary D-9: fits — `ReconstructedChangeset` and `confidence`
    already exist). **All three acceptance artifacts are done** (under `handoffs/007-cvs-decoder/`): D-9
    confirmed CVS fits IR 1.0.0 with **zero contract changes**; the **program-design handoff** (zero new
    dependency, no subprocess, pure Rust); and the **architect security review** (verdict proceed — the
    cleanest surface of any decoder, INV-1 at its purest). **Implementation toward M4 may begin.**
  - [RFC 009 — Dependency-surface & supply-chain policy](accepted/009-dependency-surface-and-supply-chain-policy.md)
  - [RFC 010 — Bounded memory & streaming](accepted/010-bounded-memory-and-streaming.md) — accepted
    2026-09-12 (OQ-F), owner-directed. Streaming cannot shrink the IR (the IR *is* the content); it bounds a
    decoder's **scratch** to ~O(IR) instead of O(IR × depth). **Increment 1 built:** `brygge-decode-svn`
    retains only the tree snapshots a `copyfrom` names — O(revisions × tree) → O(copy-targets × tree), no
    format/determinism change. Increments 2–4 (SVN dumpstream iterator, CVS reconstruction bound, and a
    measurement-gated streaming writer) queued.
- **Done:** [RFC 000 — RFC lifecycle policy](done/000-rfc-lifecycle-policy.md) (brygge uses the
  **5-folder variant**: `proposed → accepted → done`, plus `archive/` and optional `draft/`).

RFC 004 is realized in two increments (handoffs under `handoffs/004-git-decoder/`): **increment 1** the
`brygge-decode-git` decoder (Git → IR), **increment 2** the read-side CLI (`decode`/`inspect`/`verify`/
`summary`, CL-08 exit classes) + against-source verify (VF-2). Both are built and green, and RFC 004's
open questions **OQ-A** (rename detection: exact-content 1:1, similarity deferred) and **OQ-B** (ref
namespace policy + annotated-tag identity preservation) are now **resolved**.
RFC 005 (Mercurial) is realized in three parts (handoff under `handoffs/005-mercurial-decoder/`): the
format-safety gate, the ground-truth-validated revlog reader, and the object layer + `decode()` — all
built and green, plus CLI `decode hg` and against-source dispatch.
The **RFC 003 D-7 contract freeze is done** (IR `1.0.0`, 2026-09-08). **RFC 006 (Subversion → M3) is
accepted** (2026-09-08): read tier Tier D (dumpstream parser), floor ruled (externals refused,
convention-violations imported-with-loud-derived-record). **All three acceptance artifacts are complete**
(D-9 additive-fit, program-design handoff, security review — verdict proceed), and **`brygge-decode-svn`
increments 1 and 2 are built and green** (the `decode` library — dumpstream reader + tree model → IR — and
the CLI: `brygge decode svn <repo|dumpfile> [--reconstruct-refs]` + `verify --against-source`, validated
against real `svnadmin` 1.14.5; delta dumps and streaming queued). **RFC 007 (CVS → M4) is accepted**
(2026-09-10): read tier Tier R (pure-Rust RCS reader, zero new deps), confidence floor ruled per-changeset
(import the confident majority, loudly flag/refuse under-floor). **`brygge-decode-cvs` is built and green**
(CLI wired; `verify --against-source` checks per-file content + deterministic reproduction, not changeset
correspondence, D-7). **M4 is delivered, and the difficulty gradient is complete: Git, hg, SVN, and CVS have
all been decoded into the IR with no contract change** — the strongest evidence for PU-1/PU-3 and the
RFC 003 D-7 freeze. The RFC 005 follow-ups (rename inference / `.hgtags` / large repos) remain available as
a parallel track. `encode` unblocks when the owner rules GATED-1..3 (RFC 008).

Per the lifecycle policy, the folder is the source of truth for state; this section is the index the
policy asks each project to keep. Update it in the same commit that moves an RFC between folders.
