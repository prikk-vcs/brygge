# brygge — Project handoff

**Read this first.** This document hands brygge from its founding architect to the incoming team. It is the
map: what brygge is, the state at handover, the invariants you must never regress, where every artifact
lives, how to build and gate it, and the prioritized backlog. Everything it references is in this
repository; nothing load-bearing lives only in someone's head.

**Date of handover:** 2026-09-12. **Updated for the 0.1.1 release, 2026-09-24.** **State:** brygge
**0.1.0 is released**: the decode → IR half for all four named sources, on IR contract **0.2.0**.
**0.1.1** follows with Windows support and the release workflow (RFC 012). All gates
are green, and CI enforces them on the declared MSRV. The encode → prikk half waits on prikk's import
foundations (see §8).

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
| **Decode → IR (Track A)** | **Released in 0.1.0** for all four sources: Git, Mercurial, Subversion, CVS (main line). See `CHANGELOG.md` and the user guides in `docs/src/guide/`. |
| **IR contract** | **0.2.0** (RFC 011, which replaced the pre-release 1.0.0 freeze). Tagged fields with a critical bit, a strict canonical form, a digest over the stored bytes; published in `docs/src/reference/ir-artifact-format.md`. |
| **CLI** | `brygge decode <git\|hg\|svn\|cvs> <source> --out <artifact>`, `inspect <artifact> [--atoms]`, `verify <artifact> [--against-source <source>]`; human and versioned machine output (`docs/src/reference/machine-output.md`); CL-08 exit classes. |
| **Encode → prikk (Track B)** | **Not started past design.** prikk ruled OQ-1…OQ-3 (2026-09-13) and UD-4 (2026-09-23); the encoder waits on prikk building UD-1 and UD-2 (RFC 008; see §8). |
| **Gates** | fmt · clippy `-D warnings` · test · `cargo deny` · `cargo audit` · `tools/check-ir-isolation.sh` · `tools/check-links.sh` · `tools/test-release-tools.sh`, all `--locked`, on the default toolchain **and** on MSRV 1.85; CI-enforced (§7). CI runs fmt, clippy and test on Linux x86_64, Linux arm64, macOS (Apple Silicon) and Windows (RFC 012 D-9). |
| **Toolchain** | Rust 2024, MSRV **1.85** (enforced by CI). |
| **Third-party deps** | `sha2` (core); `gix` (Git); `flate2`, `ruzstd` and `sha1-checked` (hg); `sha2` (CVS). SVN adds **none**. |

## 3. The non-negotiables (never regress these)

brygge's value *is* these invariants. A change that breaks one is a security/trust bug, not a preference.
They are stated in the threat model (`docs/src/brygge-03`, §4) and the requirements (`brygge-01`); the
short form:

- **INV-1 — No manufactured verification.** Imported authorship is `Unverifiable` by construction; the
  derived-marking, loss boundary, and fidelity summary are properties of *every produced object* and are
  **not configurable off**. This is the whole point of brygge. (See `honesty.rs`; `verify`'s seven checks.)
- **INV-2 — Source input is untrusted; brygge never executes source-provided code.** No hooks, filters,
  submodule/externals fetches, or scripts. Every parser is bounds-checked and panic-free.
- **INV-3 — No network I/O; writes only to operator-specified outputs.** (svnrdump-over-network refused;
  `:pserver:` refused; source-declared paths are never write targets.)
- **INV-4 — The dependency surface is isolated, minimized, pinned, audited;** brygge's own crates
  `forbid(unsafe_code)`; C is isolated to an FFI crate **or a subprocess** (RFC 009; brygge-03 C-4b).
- **INV-5 — brygge output never enlarges the target's audited surface** — the IR and `verify`'s
  artifact-only checks link **no** decoder; a target checks an import with only its own dependencies.
- **INV-6 — Determinism + object-carried provenance are integrity controls.** Same input → byte-identical
  IR. Non-determinism is a trust hole (a tamper could hide in it), so it is forbidden, not merely avoided.

**And the owner's design philosophy, which decided every close call:** *clean, safe, secure, robust — over
rich-but-complicated.* Lean to **refuse or defer** rather than approximate; keep trust surfaces small;
surface trade-offs to the owner rather than silently choosing. When in doubt, that is the tie-breaker.

## 4. Architecture and the crate map

Two-layer design enforced by the dependency graph (RFC 009 D-1): the **light core** (`brygge-ir`) that a
target can depend on, and the **isolated decoders** that read one source each. **The core links no
decoder**, enforced in CI by `tools/check-ir-isolation.sh` against a declared allowlist.

| Crate | Role | Third-party deps |
|---|---|---|
| **`brygge-ir`** | The IR: types, canonical codec, content store, epistemic-status taxonomy, the versioned artifact, the recoverable fidelity report. The durable product boundary (PU-3). | `sha2` only |
| **`brygge-decode-git`** | Git → IR. Every object verified against its id; renames inferred only on request (marked `Derived`). | `gix` (isolated; security-reviewed) |
| **`brygge-decode-hg`** | Mercurial → IR. Pure-Rust revlog reader; the published view; every revision verified against its node; **stated** renames and copies carried as `Stated`. | `flate2`, `ruzstd` (decompression), `sha1-checked` (node verification) |
| **`brygge-decode-svn`** | Subversion → IR. Pure-Rust dumpstream parser; branches/tags `Derived` by convention; `svnadmin` is an optional producer *subprocess*, not linked. | **none** |
| **`brygge-decode-cvs`** | CVS → IR. Pure-Rust RCS `,v` reader; main line only; **the changeset itself is `Derived`** (no atomic commit). | `sha2` (the repository fingerprint) |
| **`brygge`** | The CLI. Wires the decoders in (permitted for the binary, RFC 009 D-1); the core and `verify`'s artifact-only checks still link none. | — |
| **`tools/bench`** | Dev-only memory/time harness (RFC 010). Not published. | — |

Each decoder exposes one narrow `decode(...) -> Result<brygge_ir::Ir, Error>` and follows the same shape:
an all-`Stated` faithful spine, a marked `Derived` layer beside the literal ops, a read-a-policy floor that
refuses with a named reason, and against-source verification by preserved source ids. Conventions: 2018
module style (`foo.rs` + `foo/`, no `mod.rs`), tests as siblings (`#[cfg(test)] mod tests;`).

## 5. The document map (where everything lives)

| You want… | Read |
|---|---|
| What brygge must do / never do / must decide | `docs/src/brygge-01-requirements-spec-v0.1.md` (requirements, v0.3) |
| The black-box surface (commands, IR contract, honesty surface, flows) | `docs/src/brygge-02-external-design-v0.1.md` (external design, v0.3) |
| What brygge defends, against whom, how | `docs/src/brygge-03-threat-model-v0.1.md` (threat model, v0.3) |
| How to import from each source, what is carried, refused and why | `docs/src/guide/` (one guide per source) |
| The machine-readable output (keys, versions, exit codes) | `docs/src/reference/machine-output.md` |
| The whole docs set as a book | `docs/src/SUMMARY.md` (mdbook) |
| The exact IR artifact wire format (for a foreign encoder or reader) | `docs/src/reference/ir-artifact-format.md` (contract `0.2.0`, RFC 011, PU-3) |
| Direction, milestones, release cycles, prikk dependencies | `ROADMAP.md` |
| How decisions are made, who approves what, the gates | `GOVERNANCE.md` |
| Every design decision + its build spec | `rfcs/` — see `rfcs/README.md` (the index and the source of truth for RFC state) |
| A decoder's build spec / security review / D-9 fit | `rfcs/handoffs/<rfc>/` |
| How to use brygge; how to read the fidelity report | `README.md` |
| What a crate is and how it's bounded | `crates/*/README.md` |
| Decoder memory/time measurement | `tools/bench/README.md` |

**The RFC set** (`rfcs/README.md` is authoritative): **in `done/`, implemented in 0.1.0:** 001 IR
foundations · 002 honesty & provenance · 003 determinism, format & versioning · 004 Git decoder · 005
Mercurial decoder · 006 Subversion decoder · 007 CVS decoder · 009 dependency & supply-chain policy · 011
the IR contract re-cut (contract 0.2.0) · and 000, the RFC lifecycle policy. **Accepted:** 010 bounded
memory & streaming (increment 1 shipped in 0.1.0; increments 2–4 are 0.2.0). **008 is reserved** for the
Track-B prikk-encoder RFC; its design begins when prikk's import foundations exist (§8).

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
cargo deny check                  # advisories, licenses, banned/duplicate crates (INV-4)
cargo audit                       # known vulnerabilities in the dependency tree
./tools/check-ir-isolation.sh     # brygge-ir's dependency closure matches its allowlist (RFC 009 D-7)
./tools/check-links.sh            # relative links in every *.md file resolve
./tools/test-release-tools.sh     # the release tools (RFC 012); needs the network and the `0.1.0` tag

# Portability (RFC 012 D-9): CI proves Linux, macOS and Windows by running there. Before a commit, at least
# compile the other targets (the test code included):
cargo check --workspace --all-targets --locked --target x86_64-pc-windows-gnu
cargo check --workspace --all-targets --locked --target x86_64-apple-darwin

# The MSRV run. CI builds on the declared MSRV (1.85), not on your newer local toolchain, so a local
# "green" says nothing about CI until these two pass too (use a scratch --target-dir):
cargo +1.85 clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo +1.85 test --workspace --locked

# The measurement harness (dev-only), before/after any RFC 010 increment:
cargo run -p brygge-bench --release
```

**CI's result for the pushed commit is part of every report** (`gh run list`): a review request states the
local 1.85 run before the commit, and the CI run id and its result after the push. A red CI blocks the cut and
is fixed before anything else. `Cargo.lock` holds `human_format` at 1.1.0 for the MSRV (see the root
`Cargo.toml`); do not update it past that without re-running the 1.85 gates.

Conventions the gates and reviews enforce: **`forbid(unsafe_code)`** in every shipped crate; public items
documented (`missing_docs = warn`); the panic-prone clippy lints (`unwrap_used`, `expect_used`,
`indexing_slicing`) warn, so under `-D warnings` they are effectively denied in non-test code — **brygge
parses untrusted input and must never panic on malformed bytes.** Tests may `#![allow(...)]` these.

**To add a new source decoder** (the "etc." in PU-3): write the RFC (following 004–007's shape) → get the
read-tier ruled by the owner (RFC 009) → the handoff, the `brygge-03` security review, and the D-9
additive-fit confirmation → a new `brygge-decode-<src>` crate exposing `decode()` → wire it into the CLI's
`decode_source`/`source_kind_of`. The IR should hold it with no contract change, or with a new field under
RFC 011's rules (a non-critical field is additive; a critical one is a contract version bump). Any
contract change is a deliberate, owner-approved event, never a silent one.

## 8. The backlog — what's next, prioritized and honest

Nothing here is required for 0.1.0 to be correct; all of it is planned scope or gated work, recorded so
the team inherits the reasoning, not just the TODO. `ROADMAP.md` is authoritative for scheduling.

**0.2.0 — Scale** (RFC 010, measurement-gated; the baseline is in `tools/bench/README.md`):
1. **RFC 010 increment 5, the Git snapshot retention bound.** The baseline measured a 1.16 GiB peak for
   341 KiB of content at 20,000 commits: O(commits × tree).
2. **RFC 010 increment 3, CVS trunk reconstruction in one pass.** The baseline measured quadratic *time*
   (920 s at 5,000 revisions per file) at a constant ~4.5× content peak.
3. **Increment 4, a streaming artifact writer,** only if measured necessary (RFC 010 D-3).
4. **Deferred by measurement: increment 2** (the SVN dumpstream iterator). The baseline puts SVN at a
   constant ~4.6× content against the ~3× floor: the smallest gain, for the largest change. Revisit if a
   real import needs it.

**0.3.0 — Source reach:**
6. **CVS branch-aware import,** which lifts 0.1.0's main-line-only limit.
7. **SVN delta dumps** (svndiff), which also closes `RR-svn-special-toggle`.
8. **Mercurial hashed long paths** (`dh/`).
9. **CVS adaptive clustering windows.**

**Deferred by explicit owner decision:**
10. **A TUI:** deferred, *not* rejected (2026-09-12). If pursued, the standing architect recommendation is a
    **separate `brygge-tui` crate over `brygge-ir`** (never in the decode binary), designed to make
    derived/dropped/refused *more* visible, not less. Draft an RFC first.

**Track B — encode → prikk:**
11. **RFC 008 (prikk encoder).** prikk decides its import design, and brygge informs it proactively
    (ROADMAP Track B).
    - prikk ruled OQ-1…OQ-3 on 2026-09-13: the importer signs, and an adopted maintainer seals.
    - prikk ruled UD-4 on 2026-09-23. The importer is prikk's own import command.
    - The encoder waits on prikk building UD-1 (an import-shaped attestation) and UD-2 (an authorized
      import block kind). `brygge-01` §11 records the state as of prikk 0.46.0.

## 9. Known limits (set expectations honestly)

- **No encode yet.** 0.1.0 is decode + inspect + verify (§8, Track B).
- **CVS is lossy by nature** (SRC-C3): the changeset is brygge's reconstruction, and every atom is
  `Derived`. Changeset-level `verify --against-source` is *not offered*, because there is no source
  changeset to check against; only per-file content and deterministic reproduction are. 0.1.0 imports the
  main line only. All of this is stated before the run (VF-5) and in `docs/src/guide/cvs.md`.
- **SVN reads fulltext dumps**; delta-format dumps are refused. **CVS reads a local repository**;
  `:pserver:` is refused.
- **Git SHA-256 repositories are refused** until the Git dependency reads them.
- **Peak memory is O(content)**: the IR *is* the content. Ceilings refuse rather than exhaust, but nothing
  streams yet (0.2.0).
- **No progress reporting or cancellation.** Interrupting a decode is safe (the artifact write is atomic),
  but it produces nothing.
- The threat model is the living document, revisited every release (v0.3 for 0.1.0, v0.4 for 0.1.1, v0.5 in 0.2.0).

---

*Handover complete, and 0.1.0 released. The gates are green on the MSRV, the invariants hold, and the reasoning behind every decision is in
the RFCs and their handoffs. Build the next thing the way this was built: design first, refuse rather than
misread, keep honesty in the object, and measure before you optimize.*
