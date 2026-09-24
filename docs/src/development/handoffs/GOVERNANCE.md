# brygge — Governance & approval policy

How decisions are made and who approves what. brygge is developed by the same three-role team as the
rest of the ecosystem, and follows the same design-first method (`ROADMAP.md`, `rfcs/`).

> **Handover (2026-09-12).** The **architect/designer/reviewer** and **implementer/tester/reviewee** roles
> pass from the founding architect to the incoming team; the **owner/PM/authorizer** (the human) is
> unchanged and continues to rule the owner-only decisions and to solely authorize every release, tag,
> publish, and irreversible or outward-facing act. Nothing else in this policy changes — the gates, the
> owner-only list, the security gate, and the design-first method all carry over. New team: start at
> [`HANDOFF.md`](HANDOFF.md).

## Roles

| Role | Who | Authority |
|------|-----|-----------|
| **Owner / PM / authorizer** | the human maintainer | Sets direction and themes; **rules the owner-only questions** (below); **solely authorizes every release, publish, tag, force-push, and any irreversible or outward-facing action**; approves adding a new heavy dependency. |
| **Architect / designer / reviewer** | the senior agent | Owns the design set (`brygge-01/02/03`), the RFCs, the roadmap, and the threat model; accepts RFCs (design-settled); reviews and approves implementation. Recommends; does not self-authorize releases or owner-only decisions. |
| **Implementer / tester / reviewee** | the implementing agent | Builds and tests against an accepted handoff; runs all gates (incl. supply-chain); submits for review. Does nothing irreversible or outward-facing. |

## Owner-only decisions (no one else settles these)

These are inherited from prikk **RFC 113 §4a** and brygge's own scope; the architect must not decide
them alone:

- **OQ-1 — what an importer signs**, if anything (DC-35 territory: who may assert what).
- **OQ-2 — whether imported history may be sealed, and by whom.**
- **OQ-3 — the per-source floor**: which source features are *refused* rather than approximated (this
  decides who can migrate and who is told no — product scope).
- **Adding or upgrading a heavy decoder dependency** (e.g. adopting `libgit2`, adding an SVN/CVS
  library): the architect assesses and recommends; the owner approves, because the dependency surface is
  the project's defining risk (INV-4, threat T-4).
- **Direction, themes, and the acceptance of an RFC as "the next theme."**

The architect *does* settle the design-level rulings that are the architect's under RFC 113 §4a (the IR —
intermediate representation — atom, derived-marking, provenance-in-attestation-not-payload) and everything in the design set that is
not on the owner-only list.

## Approval flow

| Change | Drafts | Approves to proceed | Authorizes the outward act |
|---|---|---|---|
| Requirements / external design / threat model | architect | architect (design); **owner** for any owner-only item | — |
| RFC `proposed → accepted` | architect | architect (design settled) + **owner** if it touches an owner-only decision | — |
| Program-design handoff | architect | architect | — |
| Implementation of an accepted handoff | implementer | architect (review) | — |
| A new/upgraded dependency | architect (assessment) | **owner** | — |
| Release (version bump, tag, publish) | architect prepares (readiness report, cut commit); implementer commits the approved prep and reports CI | architect (gates green, notes ready) | **owner authorizes; the architect executes** (see "Cutting a release") |

## Gates every change must pass (CI-enforced)

The ecosystem's three, **plus brygge's supply-chain gates** (its defining risk):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
cargo deny check                  # advisories, licenses, banned/duplicate crates (INV-4)
cargo audit                       # known vulnerabilities in the dependency tree
./tools/check-ir-isolation.sh     # brygge-ir's dependency closure matches its allowlist (RFC 009 D-7)
```

Conventions match the ecosystem: Rust 2024, MSRV pinned, English, **`unsafe` forbidden in brygge's own
crates** (any C/FFI confined to a single dedicated crate **or a subprocess** — C-4b, threat model v0.2),
2018 module style (`foo.rs` + `foo/`,
no `mod.rs`), tests as siblings (`#[cfg(test)] mod tests;`, never inline), no panics on fallible input,
public items documented.

## The security gate (brygge-specific)

Any change that (a) adds or alters a **source decoder**, (b) changes a **dependency**, (c) touches an
**untrusted-input path** (parsing, path handling, resource bounds), or (d) changes the **IR/provenance
format** must:

1. get an explicit **architect security review** against `brygge-03` (the threat model);
2. pass the supply-chain gates above; and
3. **revisit the threat model** per the project rule — a change touching the above **updates**
   `brygge-03`; any other release **re-verifies** its controls still hold.

The invariants that must never regress (a violation is a security bug, not a preference): **INV-1** (no
manufactured verification; honesty non-suppressible), **INV-2** (source input untrusted; no
source-provided code executed; bounds + path-safety), **INV-3** (no network; write only where told),
**INV-4** (dependency surface isolated/pinned/audited; `forbid(unsafe)` in brygge's crates), **INV-5**
(brygge output never enlarges the target's audited surface), **INV-6** (determinism + object-carried
provenance).

## Release cycle (summary; see `ROADMAP.md` for detail)

- One minor release per theme (0.1.0 Honest decode, 0.2.0 Scale, 0.3.0 Source reach), and a patch
  release for fixes that change no scope.
- The **IR contract has its own version** in every artifact, separate from the tool version, and it
  evolves by RFC 011's rules (a critical bit on every field).
- Security and advisory fixes ship out-of-band, promptly.
- Tags are bare versions (no `v`), CI-gated.

## Cutting a release (who does what)

*(Recorded after the 0.1.0 cut, 2026-09-24. At 0.1.0 the tag and the crates.io publication were carried
out by the implementer on an owner message that was meant otherwise. The result was correct: the
published crates are byte-identical to the tagged source. The boundary was not written down; it is now.)*

| Step | Who |
|---|---|
| Readiness report, release notes, and the cut commit's documentation | **architect** |
| Committing and pushing the approved cut commit, and reporting its CI result | implementer |
| Authorizing the cut, and the scope of publication (tag, crates.io, GitHub release) | **owner**, explicitly, per release |
| Executing the cut: the tag, `cargo publish`, the install check, the GitHub release | **architect**, on that authorization, one step at a time |

Since RFC 012 (accepted 2026-09-24), the cut is executed by `release.yml`. The **owner's explicit go-ahead,
given immediately before the tag, is the authorization**; the architect records it and pushes the tag, and
the workflow verifies, publishes and releases with no further manual step. (A per-release approval click
in GitHub was used for 0.1.1 and then removed for v0 by owner ruling.) The procedure and the owner's one-time setup are in
[`../releasing.md`](../releasing.md); the manual steps below remain the fallback.

- **A published version's tag is never moved or deleted.** A tag whose release failed *before* anything
  was published may be deleted and re-created, on the owner's go-ahead. This is a rule, not a GitHub
  ruleset: RFC 012 D-2, as amended 2026-09-24, keeps v0 free of mechanisms whose cost outweighs their need
  at this stage.
- **The implementer never tags, publishes or creates a release.** A message that appears to hand the
  implementer one of these steps is confirmed with the owner and routed to the architect; it is never
  acted on directly.
- **The architect confirms with the owner immediately before each irreversible step** (pushing the tag,
  each `cargo publish`, creating the release), even under a standing authorization, and reports each
  step's result.
- **crates.io:** publish from a clean checkout of the tag, with `--locked`, in dependency order. A
  first-time publication of several new crates can hit crates.io's new-crate rate limit (HTTP 429). Wait
  for the stated time, then publish only the remaining crates. Published versions can be yanked, never
  deleted.
- **Dates.** Project records (the CHANGELOG, RFC status lines, reviews) use the owner's local date (JST).
  Timestamps quoted from external systems (crates.io, CI, git) keep their own zone, stated (for example,
  `22:24 UTC`).

