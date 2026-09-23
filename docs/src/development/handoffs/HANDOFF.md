# brygge — Project handoff

**Read this first.** This document hands brygge from its founding architect to the incoming team. It is the
map: what brygge is, the state at handover, the invariants you must never regress, where every artifact
lives, how to build and gate it, and the prioritized backlog. Everything it references is in this
repository; nothing load-bearing lives only in someone's head.

**Date of handover:** 2026-09-12. **State:** the decode → IR half is delivered for all four named sources;
the IR contract is frozen at 1.0.0; all gates are green. The encode → prikk half is owner/prikk-gated and
deliberately not started past design (see §8).

---

## 1. What brygge is, in one paragraph

**brygge** carries version-control history **out of** an existing system (Git, Mercurial, Subversion, CVS)
into an **intermediate representation (IR)** that belongs to no particular system, so a target — prikk
first — can encode from it. The pipeline is two deliberately separated halves: **decode** a source into
the IR; **encode** a target from the IR. The one idea everything turns on is **faithfulness with
provenance, not neutrality**: brygge records what the source *literally guaranteed*, and marks — *in the
object itself* — anything it had to infer. It reads untrusted input, links no network, and writes only
where told. The governing upstream contract is prikk **RFC 113** (History import foundations).

## 2. Status at handover

| Area | State |
|---|---|
| **Decode → IR (Track A)** | **Delivered for all four sources.** Git (M1), Mercurial (M2), Subversion (M3), CVS (M4). |
| **IR contract** | **Frozen at 1.0.0** (RFC 003 D-7, 2026-09-08). All four sources fit it **with no contract change** — the strongest possible evidence for the freeze. Post-freeze: additive-only within major 1. |
| **CLI** | `brygge decode <git\|hg\|svn\|cvs> <path>`, `inspect`, `verify --internal`, `verify --against-source`, `summary`; human + machine output; CL-08 exit classes. |
| **Encode → prikk (Track B)** | **Gated, not started past design.** Waits on prikk's UD-1…UD-3 / OQ-1…OQ-3 (RFC 008; see §8). |
| **Gates** | fmt · clippy `-D warnings` · test (**123 passing**) · `cargo deny` · `cargo audit` — all green, all `--locked`. |
| **Toolchain** | Rust 2024, MSRV **1.85**; built/tested on rustc 1.98.1. |
| **Third-party deps** | `sha2` (core); `gix` (Git); `flate2`+`ruzstd` (hg). SVN and CVS decoders add **zero** third-party deps. |

## 3. The non-negotiables (never regress these)

brygge's value *is* these invariants. A change that breaks one is a security/trust bug, not a preference.
They are stated in the threat model (`docs/src/brygge-03`, §4) and the requirements (`brygge-01`); the
short form:

- **INV-1 — No manufactured verification.** Imported authorship is `Unverifiable` by construction; the
  derived-marking, loss boundary, and fidelity summary are properties of *every produced object* and are
  **not configurable off**. This is the whole point of brygge. (See `honesty.rs`; `verify --internal`.)
- **INV-2 — Source input is untrusted; brygge never executes source-provided code.** No hooks, filters,
  submodule/externals fetches, or scripts. Every parser is bounds-checked and panic-free.
- **INV-3 — No network I/O; writes only to operator-specified outputs.** (svnrdump-over-network refused;
  `:pserver:` refused; source-declared paths are never write targets.)
- **INV-4 — The dependency surface is isolated, minimized, pinned, audited;** brygge's own crates
  `forbid(unsafe_code)`; C is isolated to an FFI crate **or a subprocess** (RFC 009; brygge-03 C-4b).
- **INV-5 — brygge output never enlarges the target's audited surface** — the IR and `verify --internal`
  link **no** decoder; a target checks an import with only its own dependencies.
- **INV-6 — Determinism + object-carried provenance are integrity controls.** Same input → byte-identical
  IR. Non-determinism is a trust hole (a tamper could hide in it), so it is forbidden, not merely avoided.

**And the owner's design philosophy, which decided every close call:** *clean, safe, secure, robust — over
rich-but-complicated.* Lean to **refuse or defer** rather than approximate; keep trust surfaces small;
surface trade-offs to the owner rather than silently choosing. When in doubt, that is the tie-breaker.

## 4. Architecture and the crate map

Two-layer design enforced by the dependency graph (RFC 009 D-1): the **light core** (`brygge-ir`) that a
target can depend on, and the **isolated decoders** that read one source each. **The core links no
decoder** — verified structurally (`cargo tree`) and by an isolation test.

| Crate | Role | Third-party deps |
|---|---|---|
| **`brygge-ir`** | The IR: types, canonical codec, content store, epistemic-status taxonomy, the versioned artifact, the recoverable fidelity report. The durable product boundary (PU-3). | `sha2` only |
| **`brygge-decode-git`** | Git → IR. Renames inferred (off by default, marked `Derived`). | `gix` (isolated; security-reviewed) |
| **`brygge-decode-hg`** | Mercurial → IR. Pure-Rust revlog reader; **stated** renames carried as `Stated`. | `flate2`, `ruzstd` (pure-Rust decompression) |
| **`brygge-decode-svn`** | Subversion → IR. Pure-Rust dumpstream parser; branches/tags `Derived` by convention; `svnadmin` is an optional producer *subprocess*, not linked. | **none** |
| **`brygge-decode-cvs`** | CVS → IR. Pure-Rust RCS `,v` reader; **the changeset itself is `Derived`** (no atomic commit). | **none** |
| **`brygge`** | The CLI. Wires the decoders in (permitted for the binary, RFC 009 D-1); the core + `verify --internal` still link none. | — |
| **`tools/bench`** | Dev-only memory/time harness (RFC 010). Not published. | — |

Each decoder exposes one narrow `decode(...) -> Result<brygge_ir::Ir, Error>` and follows the same shape:
an all-`Stated` faithful spine, a marked `Derived` layer beside the literal ops, a read-a-policy floor that
refuses with a named reason, and against-source verification by preserved source ids. Conventions: 2018
module style (`foo.rs` + `foo/`, no `mod.rs`), tests as siblings (`#[cfg(test)] mod tests;`).

## 5. The document map (where everything lives)

| You want… | Read |
|---|---|
| What brygge must do / never do / must decide | `docs/src/brygge-01-requirements-spec-v0.1.md` (requirements, v0.2) |
| The black-box surface (commands, IR contract, honesty surface, flows) | `docs/src/brygge-02-external-design-v0.1.md` (external design, v0.2) |
| What brygge defends, against whom, how | `docs/src/brygge-03-threat-model-v0.1.md` (threat model, v0.2) |
| Direction, milestones, release cycles, prikk dependencies | `ROADMAP.md` |
| How decisions are made, who approves what, the gates | `GOVERNANCE.md` |
| Every design decision + its build spec | `rfcs/` — see `rfcs/README.md` (the index and the source of truth for RFC state) |
| A decoder's build spec / security review / D-9 fit | `rfcs/handoffs/<rfc>/` |
| How to use brygge; how to read the fidelity report | `README.md` |
| What a crate is and how it's bounded | `crates/*/README.md` |
| Decoder memory/time measurement | `tools/bench/README.md` |

**The RFC set** (all accepted unless noted; `rfcs/README.md` is authoritative):
001 IR foundations · 002 honesty & provenance · 003 determinism, format & versioning (the freeze, D-7) ·
004 Git decoder · 005 Mercurial decoder · 006 Subversion decoder · 007 CVS decoder · 009 dependency &
supply-chain policy · 010 bounded memory & streaming (increment 1 done). **008 is the reserved number for
the gated Track-B prikk-encoder RFC — not yet written; its design begins when prikk unblocks it (§8.6).**
000 (RFC lifecycle policy) is in `done/`.

## 6. Governance and method (unchanged by the handover)

- **Roles** (`GOVERNANCE.md`): **owner/PM/authorizer** (the human — unchanged; rules the owner-only
  questions, solely authorizes every release/tag/publish/irreversible act), **architect/designer/reviewer**
  and **implementer/tester** (these two roles pass to the incoming team).
- **Design-first, always:** requirements → external design → threat model → **RFC + handoff** →
  implementation → tests. Never inverted. Every accepted RFC gets a program-design handoff before code.
- **Owner-only decisions** (never settle these alone): the per-source floor contents (OQ-3), adopting or
  upgrading a heavy/decoder dependency (RFC 009 — the read tier is owner-ruled), direction/themes, and
  every release/tag/publish. When you hit one, surface it with a recommendation; do not choose silently.
- **The RFC lifecycle** is the five-folder variant (`proposed → accepted → done`, plus `archive/`): the
  folder is the source of truth for state; update `rfcs/README.md` in the same commit that moves an RFC.
- **The security gate:** any change that adds/alters a source decoder, changes a dependency, or touches an
  untrusted-input path must get an architect security review against `brygge-03` and **revisit the threat
  model** (a touching change *updates* it; others *re-verify*).

## 7. Building, testing, and the gates

```sh
# The full gate suite — all must pass, all --locked (CI-enforced; mirror it locally before every commit):
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
cargo deny check          # advisories, licenses, banned/duplicate crates (INV-4)
cargo audit               # known vulnerabilities in the dependency tree

# The measurement harness (dev-only), before/after any RFC 010 increment:
cargo run -p brygge-bench --release
```

Conventions the gates and reviews enforce: **`forbid(unsafe_code)`** in every shipped crate; public items
documented (`missing_docs = warn`); the panic-prone clippy lints (`unwrap_used`, `expect_used`,
`indexing_slicing`) warn, so under `-D warnings` they are effectively denied in non-test code — **brygge
parses untrusted input and must never panic on malformed bytes.** Tests may `#![allow(...)]` these.

**To add a new source decoder** (the "etc." in PU-3): write the RFC (following 004–007's shape) → get the
read-tier ruled by the owner (RFC 009) → the handoff, the `brygge-03` security review, and the D-9
additive-fit confirmation → a new `brygge-decode-<src>` crate exposing `decode()` → wire it into the CLI's
`decode_source`/`source_kind_of`. The IR should hold it additively (major 1) or the change is a deliberate,
owner-approved contract event — never a silent one.

## 8. The backlog — what's next, prioritized and honest

Nothing here is required for the delivered product to be correct; all of it is deferred scope or gated
work, recorded so the team inherits the reasoning, not just the TODO.

**Track A follow-ups (buildable now; each is scoped in its RFC):**
1. **RFC 010 increments 2–4 — bounded memory / streaming, measurement-gated.** Increment 1 (SVN snapshot
   bound) is done and measured (~46× peak reduction at 20k revisions). Increment 2 (SVN dumpstream
   *iterator*, to stop holding the whole parsed dump), 3 (CVS reconstruction bound), and 4 (a streaming
   artifact *writer* — the only part touching `brygge-ir`, **gated on a measured ~2×IR peak**, RFC 010 D-3).
   Use `tools/bench` before/after each. **This was the active track at handover.**
2. **SVN delta dumps** (RFC 006, queued): svndiff support so `svnadmin dump --deltas`/`svnrdump` files are
   readable; currently refused with a named reason.
3. **CVS refinements** (RFC 007, queued): branch-aware changeset parenting (the baseline threads linearly),
   adaptive clustering windows (OQ-E), and optional rename inference (OQ-C, off by default).
4. **Fold the threat-model residuals into `brygge-03`** at its next revision: `RR-svn-svnadmin`,
   `RR-svn-svnadmin-version`, `RR-cvs-reconstruction` (identified by the security reviews). `RR-gix-sha1`
   and the C-4b subprocess refinement are already folded (brygge-03 v0.2).

**Deferred by explicit owner decision:**
5. **A TUI** — deferred, *not* rejected (2026-09-12). If pursued, the standing architect recommendation is a
   **separate `brygge-tui` crate over `brygge-ir`** (never in the decode binary), designed to make
   derived/dropped/refused *more* visible, not less. Draft an RFC first.

**Track B — encode → prikk (owner/prikk-gated):**
6. **RFC 008 (prikk encoder).** B0 (a labelled, unsealed, `Unverifiable` reviewable proposal) is buildable
   once designed; B1 (real/sealed imports) waits on prikk landing UD-1 (import-shaped attestation), UD-2
   (an authorized import block kind), UD-3/OQ-2 (sealing ruling), OQ-1 (what the importer signs), UD-5
   (format stability). The `brygge-01` §11 UD table must be **re-verified against the current prikk** when
   Track B design begins (it was written at prikk 0.27.1; prikk has moved on). These are owner territory.

## 9. Known limits (set expectations honestly)

- **No encode yet.** This build is decode + inspect + verify + summary. `encode` is gated (§8.6).
- **CVS is lossy by nature** (SRC-C3): the changeset is brygge's reconstruction, every atom `Derived`;
  changeset-level `verify --against-source` is *not offered* (there is no source changeset to check
  against) — only per-file content + deterministic reproduction. This is stated before the run (VF-5).
- **SVN reads fulltext dumps**; delta-format dumps are refused (§8.2). **CVS reads a local repository**;
  `:pserver:` is refused.
- **Peak memory is O(content)** — the IR *is* the content; RFC 010 bounds *scratch*, not the IR floor.
- The design docs are marked "v0.2 (draft for review)"; they are the working contract and are accurate to
  the delivered decode half. Treat the requirements/external-design as stable and the threat model as the
  living document (revisited every release).

---

*Handover complete. The gates are green, the invariants hold, and the reasoning behind every decision is in
the RFCs and their handoffs. Build the next thing the way this was built: design first, refuse rather than
misread, keep honesty in the object, and measure before you optimize.*
