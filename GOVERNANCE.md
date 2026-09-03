# brygge — Governance & approval policy

How decisions are made and who approves what. brygge is developed by the same three-role team as the
rest of the ecosystem, and follows the same design-first method (`ROADMAP.md`, `rfcs/`).

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

The architect *does* settle the design-level rulings that are the architect's under RFC 113 §4a (the IR
atom, derived-marking, provenance-in-attestation-not-payload) and everything in the design set that is
not on the owner-only list.

## Approval flow

| Change | Drafts | Approves to proceed | Authorizes the outward act |
|---|---|---|---|
| Requirements / external design / threat model | architect | architect (design); **owner** for any owner-only item | — |
| RFC `proposed → accepted` | architect | architect (design settled) + **owner** if it touches an owner-only decision | — |
| Program-design handoff | architect | architect | — |
| Implementation of an accepted handoff | implementer | architect (review) | — |
| A new/upgraded dependency | architect (assessment) | **owner** | — |
| Release (version bump, tag, publish) | implementer/architect prepare | architect (gates green, notes ready) | **owner only** |

## Gates every change must pass (CI-enforced)

The ecosystem's three, **plus brygge's supply-chain gates** (its defining risk):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
cargo deny check          # advisories, licenses, banned/duplicate crates (INV-4)
cargo audit               # known vulnerabilities in the dependency tree
```

Conventions match the ecosystem: Rust 2024, MSRV pinned, English, **`unsafe` forbidden in brygge's own
crates** (any C/FFI confined to a single dedicated crate — C-4b), 2018 module style (`foo.rs` + `foo/`,
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

- Milestone-driven minors (one per source: 0.1 Git, 0.2 hg, 0.3 SVN, 0.4 CVS).
- The **IR contract has its own semver** in the artifact, separate from the tool version; additive-only
  after IR-1.0.
- Security/advisory fixes ship out-of-band promptly.
- Bare-version tags (no `v`), CI-gated; **tagging and publishing are owner-only.**
