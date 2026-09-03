# Architect security review — adopting `gix` for the Git decoder (RFC 004 D-1)

**Required by** RFC 009 D-6 (a new heavy dependency needs owner approval **and** an architect security
review + a threat-model revisit). **Owner approval:** granted 2026-09-04 ("Approve gix"). **Verdict:**
**adopt**, with the containment and pins recorded below. **Scope:** the dependency surface `gix` adds and
how RFC 009's guarantees survive it — not the decoder's logic (that is the program-design handoff).
**Snapshot:** `gix = "0.87"` (locked 0.87.1), workspace `Cargo.lock` committed, gates green
(`cargo deny check` ok; `cargo audit` exit 0) as of this review.

## What enters the tree

| Metric | Value |
|---|---|
| gix version | **0.87.1** (declares `rust-version = "1.85"`) |
| gix feature set | **default** + `blob-diff`, `revision`; **no** `*-network-client` / `*-http-transport-*` |
| Crates in the `brygge-decode-git` subtree | **141** (of which **60** are `gix-*` family) |
| Crates in the `brygge-ir` core subtree | **10** — *unchanged*; links none of gix (RFC 009 D-1) |
| Workspace lock total | 168 (165 external + brygge's 3 crates) |
| C toolchain / `*-sys` C libraries | **none** — `cc`/`bindgen`/`cmake`/`pkg-config` and `*-sys` C wrappers all absent |
| Network backend (reqwest/curl/hyper/TLS/SSH) | **none linked** |
| MSRV impact on the core | **none** — gix 0.87.1 is MSRV 1.85; `brygge-ir` stays 1.85 |

## Findings and dispositions

- **F-1 — The surface is real but contained (RFC 009 D-1).** 141 crates is a large audit surface — the
  exact reason brygge exists (PU-5): prikk would refuse this, brygge owns it. It is contained one layer
  in: `gix` is a dependency of **`brygge-decode-git` only**. `brygge-ir`, the encoders, and
  `verify --internal` resolve to the **same 10-crate core** with gix absent — a property RFC 009 D-7 makes
  a *test* (added with the decoder: the IR + internal-verify build and run with no decoder crate present).
  **Disposition:** accepted; the isolation is structural and will be test-enforced.

- **F-2 — Pure Rust, no C, no build-time code-gen.** No `cc`, `bindgen`, `cmake`, `pkg-config`, or `*-sys`
  C-library wrapper is in the tree; the lowest-level crate is `linux-raw-sys` (pure-Rust kernel-ABI
  constants used by `rustix`, **not** a linked C library). This is the decisive advantage of the RFC 009
  D-2 tier-1 choice over `libgit2` (C, GPLv2-with-linking-exception): no C compiler in the supply chain,
  no FFI crate, and `forbid(unsafe_code)` stays maximal across **brygge's own** crates.
  **Disposition:** accepted; this is why tier-1 was preferred.

- **F-3 — No network I/O is linked (INV-3, RFC 009 D-4).** gix's network transports are opt-in features
  (`blocking-network-client`, `async-network-client`, the `*-http-transport-*` family); brygge enables
  **none** of them, so no `reqwest`/`curl`/`hyper`/`native-tls`/`rustls`/SSH backend is compiled.
  `gix-transport` itself is present as a transitive type/trait crate but carries **no transport backend**,
  so it cannot perform I/O in this configuration. brygge reads a local object database and writes files;
  the "no network" property is enforced at the link level, not only by policy. The decoder additionally
  runs with no hooks, no filter/smudge/`.gitattributes` processing, and no submodule fetch (RFC 004 D-6),
  so **no source-provided code is executed** (INV-2/T-2). **Disposition:** accepted; re-audit the enabled
  feature list on every gix bump (a network feature must never be switched on).

- **F-4 — Two advisories on the MSRV-1.85 *floor* version, both cleared by resolving forward.** A first
  resolution pinned to the oldest MSRV-1.85-compatible versions selected gix 0.66, which carries
  **RUSTSEC-2025-0021** (gix-features SHA-1 without collision detection, medium 6.8; fixed ≥ 0.41.0) and
  **RUSTSEC-2025-0140** (gix-date `TimeBuf::as_str` non-UTF-8; fixed ≥ 0.12.0). Requesting `gix = "0.87"`
  resolves to **gix-features 0.49.1** and **gix-date 0.16.0** — both past the fixes — **without** leaving
  MSRV 1.85 (gix 0.87.1 declares 1.85). `cargo audit` is exit 0 and `cargo deny` advisories are ok.
  **Disposition:** cleared. The committed lockfile and the `--locked` CI gate hold the fixed versions; a
  downgrade would fail `cargo audit` in CI and is thereby prevented.

- **F-5 — SHA-1 residual is out of brygge's trust path (relates to RUSTSEC-2025-0021).** Even patched, gix
  reads SHA-1 Git object hashes (Git's format) without collision detection — inherent to SHA-1 repos. This
  does **not** touch brygge's integrity model: the IR re-hashes everything under **SHA-256** (`AtomId`,
  `BlobId`, the artifact digest — RFC 003 D-3); the Git SHA-1 is preserved **opaquely** as a source
  identifier (PR-4), trusted for nothing in any target. A crafted SHA-1 collision in an untrusted source
  could make gix read a wrong object, but it cannot forge brygge's IR identities or honesty guarantees,
  and `verify --against-source` (RFC 004 D-7) re-reads the source to expose a mismatch.
  **Disposition:** recorded as a **residual risk** (extends threat model `RR-*`); no code dependence on
  SHA-1 integrity. Track gix's SHA-256-object support as it matures.

- **F-6 — One MPL-2.0 crate, allowlisted with rationale (RFC 009 D-5).** `uluru` (gix-pack's LRU object
  cache) is **MPL-2.0**, a *file-level* weak copyleft on a *pure-Rust* crate: it obliges sharing changes
  to its own files only and imposes nothing on brygge's code. It is categorically distinct from the
  copyleft **C library** RFC 009 D-5 flags for owner sign-off. Added to the `deny.toml` allowlist with
  that note. **Disposition:** accepted as a license-policy note under the owner's gix approval; surfaced to
  the owner for the record.

- **F-7 — `unsafe` exists inside gix, not in brygge (RFC 009 D-3 scope clarified).** `forbid(unsafe_code)`
  binds **brygge's own crates**; gix (like most performance-sensitive pure-Rust crates) uses `unsafe`
  internally. This is expected and far less risky than the tier-4 alternative (a C library + an FFI crate
  brygge would own). **Disposition:** accepted; the forbid-unsafe guarantee is precisely scoped to
  brygge-authored code, and the decoder crate keeps `#![forbid(unsafe_code)]`.

- **F-8 — Minor: duplicate `thiserror` v1 and v2, and a pinned `tinyvec`.** `cargo deny` warns on a
  duplicate `thiserror` (v1 and v2 coexist in the tree) — benign, `multiple-versions = "warn"`. `tinyvec`
  is pinned to **1.9.0** because **1.13.0 fails to compile** (`vec!` macro shadowed by a `use alloc::vec;`
  module import — an upstream bug); the committed lockfile holds 1.9.0 and CI runs `--locked`.
  **Disposition:** accepted; revisit the `tinyvec` pin when a fixed release lands.

## Guardrails this review binds to the adoption

1. **Isolation is tested, not asserted** (RFC 009 D-7): a test builds/runs the IR + `verify --internal`
   path with no decoder crate present. Ships with the decoder.
2. **The enabled gix feature list is reviewed on every bump** — no network-client / http-transport feature
   is ever enabled (F-3).
3. **`cargo deny` + `cargo audit` stay CI gates on `--locked`** (RFC 009 D-6); a new advisory or a
   downgrade past a fix fails the build.
4. **A gix major/minor bump is a supply-chain event** — re-run this review's checks (tree size, C-crate
   scan, network-backend scan, advisories, licenses) and update the numbers above.

## Threat-model delta (RFC 009 D-6 requires the revisit)

No new trust boundary: gix reads the **untrusted source** (already `TB-*`/`T-2`) and is contained per
`INV-2/INV-4/INV-5`. `T-4` (heavy-dependency compromise) becomes concrete and is answered by F-1/F-3/F-4
and the guardrails. One residual is **added**: **RR-gix-sha1** — gix reads SHA-1 objects without collision
detection; brygge's identity/honesty model does not depend on SHA-1 (F-5), so the residual is *source
misread under a deliberate collision*, mitigated by SHA-256 re-hashing and against-source verify. Fold
RR-gix-sha1 into `docs/src/brygge-03-threat-model` at its next revision.
