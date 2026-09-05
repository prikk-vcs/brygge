# Handoff — the read-side CLI + `verify` (RFC 004 increment 2)

**Realizes:** external design **CL-01/02/04/05/06/07/08** (the command surface, machine output, exit
classes), **RFC 004 D-7** (against-source verify, VF-2), and the **RFC 001/002** honesty surface that
`inspect`/`summary` read (`IX-02/03/04`, `FS-02`). No new owner-gated decision — the design is settled;
this consolidates it into a build.
**Status.** Inherits Accepted (RFC 004). Build against it now.
**Scope (increment 2):** the `brygge` binary's **read side** — `decode`, `inspect`, `verify`
(`--internal` and `--against-source`), `summary`, `--version`/`--help`, human/machine output, and the
CL-08 exit-code classes. `decode` and `verify --against-source` link `brygge-decode-git` (the CLI *may*
wire decoders in — RFC 009 D-1); `inspect`/`summary`/`verify --internal` link **only `brygge-ir`**.
**Out (gated):** `encode` — the prikk encoder is gated on prikk UD-1..3 / owner OQ-1..3 (RFC 008,
GATED-1..3). The command exists only as an honest "not available in this build" stub.

## Decisions to build to

- **D-A — Hand-rolled argument parsing (no `clap`).** The surface is small and brygge prizes a small,
  auditable dependency surface (RFC 009). A tiny parser in `cli.rs` maps `argv` to a `Command` enum;
  unknown flags/commands print usage and exit `FAILURE`. (`clap` would be the first CLI-only heavy dep for
  a surface this size — declined.)
- **D-B — Exit-code taxonomy (CL-08), documented and stable.** A CI gate distinguishes outcomes without
  parsing prose:
  - `0` **clean** — completed; at most **Representation**-class drops (the benign Git baseline: packfiles,
    index, reflogs — reconstructible/local, not lossy in the meaningful sense).
  - `10` **recorded loss** — completed, but a drop of class **AdvisoryUnreliable** or **Other** is
    recorded (the honest non-silent signal; not reached by a clean Git import, reserved for SVN/CVS).
  - `20` **floor refusal** (`FA-3`) — a source feature below the floor was refused.
  - `30` **convention violation** (`FA-2`) — reserved (SVN); not reachable for Git.
  - `40` **partial/interrupted** (`FA-1`) — reserved.
  - `50` **verify failed** — a `verify` check did not hold.
  - `1` **failure** — bad arguments, unreadable input, I/O.
- **D-C — `verify --internal` (VF-3) proves honesty with no source present.** It loads the artifact
  (`brygge_ir::from_bytes`, which already re-checks the integrity digest, blob content-addresses,
  referential integrity, and contract major), then affirms: provenance names a decoder and source; the
  loss boundary is present/stated; every `Derived` record is well-formed (has a kind + its parameters);
  and the fidelity summary is recoverable and reports authorship **Unverified** (`FS-04/VF-4`). Prints a
  per-check report; any failure → exit `50`.
- **D-D — `verify --against-source` (VF-2) re-derives and compares.** Because decode is
  byte-deterministic (RFC 004 D-6), verification *re-runs the derivation*: reconstruct the decode
  `Options` from the artifact's `provenance.params` (`PR-5`), `decode()` the named source repository, then
  compare the two IRs' **identity-bearing** content (import time normalised out — `ID-4`). Equal ⇒ "the
  import corresponds to its source" **without trusting the earlier run** (VF-2). Unequal ⇒ report the
  first divergence (atom count, or the first mismatching source SHA) and exit `50`. This mode links
  `brygge-decode-git` and that does **not** violate RFC 009 D-1: the protected property is that a
  *target* checks on its own surface — `verify --internal` — which links no decoder. The report never
  conflates "corresponds to source" (VF-2) with "internally honest" (VF-3) (`VF-4`).
- **D-E — Human default, `--format machine` for CI (CL-07).** `inspect`/`summary` reuse
  `brygge_ir::honesty::summary(&ir)` and its versioned `render_human`/`render_machine` (CT-04,
  `REPORT_VERSION`). `verify` prints its own small, versioned, line-oriented report. Authorship is shown
  `Unverified` everywhere, never dressed up (`FS-04`).

## Command surface (external design CL-*)

- `brygge decode git <path> [--ir <out>] [--detect-renames] [--format human|machine]` (CL-01, FL-01).
  Records the options into provenance (`PR-5`); writes the artifact if `--ir`; prints the decode fidelity
  record; exit per D-B.
- `brygge inspect --ir <file> [--format …]` (CL-02) — atoms with epistemic status (`IX-02`), the opaque
  source ids (`IX-03`), and the loss boundary (`IX-04`). Read-only.
- `brygge verify --internal --import <file> [--format …]` (CL-04, D-C).
- `brygge verify --against-source <repo> --import <file> [--format …]` (CL-04, D-D).
- `brygge summary --import <file> [--format …]` (CL-05, FS-02) — reproduce the fidelity summary from the
  artifact alone.
- `brygge --version`, `brygge --help`, `brygge <cmd> --help` (CL-06). `encode` → gated stub.

## Module layout (`crates/brygge`, 2018 style, tests as siblings)

- `main.rs` — parse `argv`, dispatch, `std::process::exit(code)`.
- `cli.rs` (+ `cli/tests.rs`) — the `Command` enum, `Format`, `parse()`, usage text.
- `exit.rs` — the CL-08 code constants.
- `commands.rs` (+ `commands/tests.rs`) — `run_decode` / `run_inspect` / `run_verify` / `run_summary`,
  each returning an exit code; an integration test builds a fixture repo (git CLI), runs
  decode→inspect→verify(internal+against-source)→summary, and asserts the outcomes and exit classes.

`brygge`'s `Cargo.toml` gains `brygge-decode-git` (workspace dep).

## Acceptance criteria

- The five read-side commands work end-to-end; `--format machine` output is stable and versioned.
- `verify --internal` passes on a good artifact and fails (exit `50`) on a tampered one (integrity), with
  no source present. `verify --against-source` confirms correspondence and detects a mismatch.
- Exit codes follow D-B; a floor refusal from `decode` surfaces as exit `20` with a named reason.
- `inspect` shows per-atom epistemic status, the opaque Git SHAs, and the loss boundary.
- `encode` prints the gated message and is never silently absent.
- Gates green: fmt; clippy `--all-targets -D warnings`; `test --workspace`; `cargo deny`; `cargo audit`.
  `brygge-ir` and `verify --internal`'s path still link no decoder (RFC 009 D-1).

## Queued next

Rename-detection tuning (OQ-A); ref-namespace confirmations (OQ-B); large-repo streaming (OQ-D); then
**RFC 005 (Mercurial) → M2** — the cross-source exercise that is the RFC 003 D-7 contract-freeze
precondition. `encode` unblocks when the owner rules GATED-1..3 (RFC 008).
