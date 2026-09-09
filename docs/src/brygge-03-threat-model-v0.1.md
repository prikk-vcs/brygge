# brygge — Threat Model

| | |
|---|---|
| Document | brygge Threat Model (security) |
| Version | v0.2 |
| Date | 2026-09-09 (v0.2 revision; v0.1 2026-09-03) |
| Revised | **v0.2 (2026-09-09)** folds the RFC 004 (gix) and RFC 006 (SVN Tier D) security-review deltas: **C-2e/C-4b/INV-4** — a C surface may be isolated to a **subprocess**, not only a dedicated FFI crate; **TB-2** acknowledges subprocess decoder producers; **C-2b** clarifies that invoking a trusted external tool is not executing source-provided code; and three residuals added — **RR-gix-sha1**, **RR-svn-svnadmin**, **RR-svn-svnadmin-version**. |
| Basis | brygge Requirements v0.2 (PU/NG/PR/HO/VF/ID/FA/BN/IR/UD/OQ) and External Design v0.2 (BD/CL/IX/FS/PX/CF/FL/CT/OP/GATED); RFC 113 (import contract); project rules (`.git-exclude/rules/`, §Release Deliverables — a threat model is a first-class release deliverable) |
| ID scheme | `A-` asset · `TB-` trust boundary · `T-` threat · `C-` control · `INV-` security invariant · `RR-` residual risk · `ASSUME-` assumption |
| Not | code, an API, or a dependency audit report. It states what brygge must defend, against whom, and how — so the design and tests can be checked against it. |

**The essence, in one paragraph.** brygge is unusual among the ecosystem's tools: it **reads untrusted input** (a source repository is attacker-controllable in every byte), it **links or drives heavy third-party code** to do so (`gix` for Git; an `svnadmin` subprocess for SVN — the very reason it is a separate project from prikk, PU-5), and it **produces artifacts that a target system will trust and import**. Its two signature dangers follow directly: (1) **manufactured verification** — producing history that reads as native or verified when it is neither (the failure RFC 110 §4 and NG-3 name), and (2) **hostile source input** — a crafted repository that exploits a decoder, escapes the output directory, exhausts the host, or smuggles executable content. Everything below organizes the defense of those two, plus the supply-chain and boundary properties that keep brygge's weight from becoming prikk's.

---

## 1. Assets (A-…)

- **A-IMPORT — the integrity and *honesty* of the produced IR (intermediate representation) and target proposal.** This is the primary asset: downstream readers and the target trust it. Its value is not just correct bytes but the truthful marking of what was stated vs derived, what was preserved vs dropped, and that authorship is unverified (HO-1…HO-4).
- **A-HOST — the operator's machine and the brygge process.** brygge parses attacker-controlled repositories with large parsers; a compromise or crash here is a real cost.
- **A-TARGET-TRUST — the target's trust boundary.** The target (prikk first) decides admission/seal (BN-2); brygge must never produce output that induces the target — or a human reviewing it — to over-trust imported history.
- **A-SUPPLY — brygge's dependency supply chain.** The heavy decoder libraries are both the enabling asset and the largest attack/audit surface.
- **A-SOURCE-CONTENT — secrets and PII that live *inside* source history.** brygge carries source content faithfully by design; that content may include committed secrets, private emails, and internal paths. brygge must not *worsen* their exposure.

## 2. Trust boundaries (TB-…)

| ID | Boundary | Direction & trust |
|---|---|---|
| **TB-1** | **source repository → brygge** | **Untrusted in.** Every object, path, name, message, size, and delta is attacker-controllable. This is brygge's defining boundary and has no analogue in stikk. |
| **TB-2** | **heavy decoder libraries (or tools) → brygge** | Semi-trusted, large. `gix`/`libgit2`/SVN/CVS libraries — **or a decoder *subprocess* such as `svnadmin dump`** (RFC 006 Tier D) — process the untrusted bytes of TB-1; a vulnerability in them is a vulnerability in brygge, though a **subprocess confines that vulnerability to a separate address space** (C-2e/C-4b). |
| **TB-3** | **brygge → target system** | brygge's output is a **claim**, not authority. The target owns admission/trust/seal (BN-2). brygge's obligation is honest, sufficient provenance (BN-3). |
| **TB-4** | **operator ↔ brygge** | Trusted. The operator runs brygge, sets inference parameters (CF-01), and the owner sets the floor policy (CF-03). brygge trusts the operator's host and configuration. |
| **TB-5** | **brygge output → third party / bundle receiver** | The import's honesty and provenance must survive the hop to someone holding only the output (VF-3) or the output plus the original source (VF-2). |

## 3. Threats and controls (T-…, C-…)

### T-1 (Spoofing) — manufactured verification / authorship laundering
A source (or a careless run) yields history that reads as natively authored or prikk-verified. The archetype: a GPG-signed Git commit presented as a verified prikk author; or imported history that looks indistinguishable from sealed native history. This is brygge's worst failure — a trust-destroying one for the whole ecosystem (A-IMPORT, A-TARGET-TRUST).

- **C-1a — `Unverifiable` by construction** (HO-3, NG-3). Imported authorship lands in exactly the target's vocabulary for "present, readable, not verified as authored"; for prikk that is `Unverifiable`. It is never shown as sound/green/native anywhere in any brygge surface (FS-04).
- **C-1b — honesty is a property of every object, not a report** (HO-4, FS-01/FS-02). The derived-marking, the loss boundary, and the `Unverifiable` status live *in* the IR/proposal and are recoverable from them (`brygge summary`), so they cannot be lost, skipped, or separated from the history they describe.
- **C-1c — honesty is non-configurable** (HO-5, CF-02). No flag suppresses derived-marking, the loss boundary, `Unverifiable`, or the fidelity summary. Configuration tunes *inference*, never *honesty*.
- **C-1d — the two claims are never conflated** (VF-4, FS-04). "verified by the target" and "faithfully imported, authorship unverified" are distinguishable by any reader at any time; a preserved source signature is shown as *preserved and verifying nothing in the target*, never as a target signature.
- **C-1e — derived ≠ stated, in the object** (HO-1, IR-2). An inferred rename / reconstructed CVS changeset / inferred SVN branch is marked derived where it appears; a reader tells judgment from fact without re-running the heuristic.

### T-2 (Tampering / Elevation / DoS) — hostile source repository
A crafted source exploits brygge or its host through TB-1/TB-2: memory-safety bugs in a decoder; **path traversal / symlink escape / absolute paths** in source-declared file paths; **decompression or delta bombs**; pathological object counts or sizes; and **source-embedded executable content** — Git hooks, `.gitattributes` clean/smudge filters, submodule URLs, SVN hook scripts, CVS `.cvsignore`/wrappers (A-HOST, A-IMPORT).

- **C-2a — all source bytes are untrusted** (TB-1). No decoder path assumes well-formedness; malformed input is a refusal (FA-2/FA-3), never undefined behaviour.
- **C-2b — brygge never executes source-provided code.** Decode reads *objects*; it does not run hooks, apply clean/smudge filters, fetch submodules, or execute any script the source carries. Filter/hook content is preserved opaquely as data (PR-4) if it is history, never invoked. **This is an invariant (INV-2).** Running a **trusted external tool** to read a source (RFC 006 Tier D invokes `svnadmin dump`) is distinct from and does not breach this: the tool is operator-environment code (TB-4), not source-provided, and `svnadmin dump` executes **no** repository hook scripts (hooks fire on commit/revprop paths, never on dump). A tool that *did* run source-embedded code, or a source construct that reaches out (`svn:externals`), is refused — externals are refused, not resolved (→ the SVN floor).
- **C-2c — source-declared paths are data, not filesystem targets.** Paths from the source name content *inside the IR*; brygge never uses a source-declared path as a write destination. Any place brygge does touch the filesystem for source content refuses `..`, absolute paths, and symlink escapes, and confines to the operator's output location (→ C-7).
- **C-2d — resource bounds, refuse rather than exhaust** (→ T-8). Streaming where possible; declared ceilings on object size, total count, path depth, and decompression ratio; hitting a ceiling is a recorded refusal (FA-3), not an OOM or a hang.
- **C-2e — memory-safety posture.** brygge's own crates `forbid(unsafe_code)`. The unsafe/C surface is confined to dependencies (RR-1) and, if a C library is used at all, to a single dedicated FFI crate — **or, preferably where available, to a subprocess** (RFC 006 Tier D reads SVN via `svnadmin dump` rather than linking `libsvn`), which contains any memory-safety fault in that C code in a **separate address space** where it cannot corrupt brygge's process or defeat `forbid(unsafe_code)` (→ C-4b).

### T-3 (Tampering) — altering the import, or stripping its honesty
An attacker (or accident) modifies the IR between `decode` and `encode`, or strips derived-marks / loss-boundary / provenance so the output looks more trustworthy than it is (A-IMPORT).

- **C-3a — inconsistency is detectable** (VF-3). `brygge verify --internal` checks that every derived record is marked, the loss boundary is stated, authorship is `Unverifiable`, and provenance names its source — an IR whose honesty was stripped fails this check.
- **C-3b — the artifact is integrity-checkable and versioned** (IX-07). The IR carries its contract version and a content digest, so tampering or truncation is detectable rather than silent (this is detectability, not authentication — see RR-4).
- **C-3c — determinism catches divergence** (VF-1, C-9). A fresh re-decode of the same source under the same version reproduces the IR; a tampered artifact diverges from the reproduction.

### T-4 (Supply chain) — the heavy dependency surface
The decoder libraries (~100 crates for `gix`, or C for `libgit2`, plus SVN/CVS) are the largest attack and audit surface, and the reason brygge is separate from prikk (A-SUPPLY, PU-5).

- **C-4a — isolate the weight behind the decoder boundary.** The heavy deps live only in the per-source decoder crates; the IR, the honesty/verify path, and the encoders do not link them. VF-3 (internal verification) must run without any decoder dependency present — the internal analogue of BN-5.
- **C-4b — pure-Rust preferred; C isolated by FFI crate *or subprocess*.** Prefer pure Rust (e.g. `gix`, keeps `forbid(unsafe)` maximal) over a C library (e.g. `libgit2`). If C code is unavoidable for a source, isolate it — in **preference order**: (1) a pure-Rust reader (no C at all — the ideal); (2) a **subprocess** that produces a parseable stream brygge reads in pure Rust (RFC 006 SVN Tier D: `svnadmin dump` → brygge's own dumpstream parser) — the C fault surface is a *separate process*, not brygge's address space, and vanishes entirely when the operator supplies the dumped stream directly; (3) a single dedicated **FFI crate** — the one place `unsafe`/C is linked into brygge, mirroring prikk's `prikk-ffi` discipline — used only when neither (1) nor (2) is available. The subprocess (2) is stronger isolation than the FFI crate (3) and is preferred wherever the source ecosystem offers a suitable tool.
- **C-4c — pin and lock.** Exact dependency versions; the lockfile is committed; upgrades are deliberate and reviewed.
- **C-4d — supply-chain gates in CI.** `cargo-deny` (advisories, licenses, banned/duplicate crates) and `cargo-audit` run in CI; a new advisory fails the build. New or upgraded decoder dependencies get explicit architect review (governance).
- **C-4e — minimize.** The dependency set is kept as small as the mission allows; a dependency is justified, not defaulted-in.

### T-5 (Elevation) — brygge output enlarging the target's audited surface
A design in which consuming a brygge import forces the target to link a brygge dependency would defeat the whole separation: prikk's five-crate audited surface would grow through the back door (A-TARGET-TRUST, BN-5).

- **C-5 — the boundary is a tested property** (CT-05). The IR, the proposal, and internal verification (VF-3) are consumable and checkable with **only the target's own dependency surface** — no brygge decoder dependency required downstream. This is verified by a test that consumes brygge output with none of brygge's decoder deps present.

### T-6 (Information disclosure) — leaking source secrets, or importing them unknowingly
Source history may contain committed secrets, private emails, GPG signatures, and internal paths. Threat: brygge exfiltrates them (network/telemetry/logs), or the operator imports secrets without realizing they travel (A-SOURCE-CONTENT).

- **C-6a — no network, no telemetry** (INV-3). brygge reads sources and writes operator-specified files only (CT-01). There is no phone-home, no analytics, no remote fetch during decode (submodules are refused, not fetched — C-2b).
- **C-6b — no content in logs beyond the operator's chosen surface.** Diagnostics name atoms, classes, and counts; they do not dump source content into logs that could outlive the operator's intent.
- **C-6c — carried-verbatim is stated, not silently scrubbed.** brygge preserves source content faithfully (VF-2 depends on it), so it must **not** silently redact — but the fidelity surface states plainly that content is carried verbatim from source, so the operator knows secrets-in-history come along and can act **in the source** before import. brygge names what it does not do; scrubbing is the operator's pre/post step (RR-3).

### T-7 (Elevation) — confused deputy / errant writes
A source with crafted paths tries to make brygge write outside `--ir` / `--out` (A-HOST).

- **C-7 — write only where told.** All brygge writes are confined to the operator-specified output locations; source-derived paths are never write targets (C-2c). This mirrors stikk's "never write inside a repository" discipline, one boundary over.

### T-8 (Denial of service) — resource exhaustion
Enormous repositories, pathological delta chains, deep trees, huge file counts, decompression bombs (A-HOST). (Called out separately from T-2 because it is a normal operating condition for real migrations, not only an attack.)

- **C-8 — bounded, cancellable, honest at the limit.** Streaming and bounded memory; progress reporting and cancellation (OP-02); a stop is a clean, labelled partial (FA-1/FA-4/FA-5); a ceiling hit is a recorded refusal with a named reason (FA-3), never an OOM that leaves an ambiguous artifact.

### T-9 (Tampering) — non-determinism as a trust hole
If brygge is non-deterministic, "faithful" is uncheckable (VF-1 fails) and a tamper can hide in the noise (A-IMPORT).

- **C-9 — determinism is a security property.** The same inputs yield byte-identical IR and proposal; any non-determinism brygge cannot avoid (e.g., an import timestamp, if the target's provenance object carries authoritative time — ID-4/UD-4) is **named** as a stated non-determinism, never left to silently perturb output.

---

## 4. Security invariants (INV-…) — the non-negotiables

A change that breaks one of these is a security bug, not a preference. Several must be enforced by test.

- **INV-1 — No manufactured verification.** Imported authorship is `Unverifiable` by construction; honesty (derived-marking, loss boundary, status, fidelity summary) is a property of every produced object and is **not configurable off**. (T-1)
- **INV-2 — Source input is untrusted, and brygge never executes source-provided code.** No hooks, filters, submodule fetches, or scripts are run; all source-driven resource use is bounded. (T-2/T-8)
- **INV-3 — No network I/O; writes only to operator-specified outputs.** (T-6/T-7)
- **INV-4 — The dependency surface is isolated, minimized, pinned, and audited;** brygge's own crates `forbid(unsafe_code)`; any C code lives only in a dedicated FFI crate **or a subprocess** (C-4b). (T-4)
- **INV-5 — brygge output never enlarges the target's audited surface** — consumable and internally verifiable with the target's own dependencies alone. (T-5)
- **INV-6 — Determinism and object-carried, recoverable provenance are integrity controls.** (T-3/T-9)

## 5. Residual risks & assumptions (RR-…, ASSUME-…)

- **RR-1 — The heavy decoder dependencies may contain vulnerabilities.** This is the accepted cost of the mission (it is *why* brygge is separate from prikk). Mitigated by isolation (C-4a), pure-Rust preference (C-4b), pinning + supply-chain gates (C-4c/d), and the recommendation that operators run brygge over **untrusted** source repositories in a sandbox (container / restricted user / no ambient credentials), since TB-1 input reaches those libraries.
- **RR-2 — brygge cannot make a lying source honest.** A faithfully-imported falsehood is still a falsehood; VF-2 checks *correspondence to the source*, not the source's own truthfulness. Detecting source-level fraud is out of scope.
- **RR-3 — Secrets/PII in source history are carried faithfully.** Redaction would break fidelity (VF-2) and is the operator's decision in the source, before or after import; brygge's duty is to *state* that content is carried verbatim (C-6c), not to scrub it.
- **RR-4 — Until prikk's import provenance is built (UD-1) and the signing question is ruled (OQ-1/DC-35), a proposal's provenance is integrity-*detectable* (C-3b) but not cryptographically *authenticated*.** This is stated in the proposal, not hidden; authenticated provenance arrives with prikk's attestation surface.
- **RR-gix-sha1 — `gix` reads SHA-1 Git objects without collision detection** (RFC 004 security review; relates to RUSTSEC-2025-0021, cleared as an advisory by pinning forward). This is inherent to SHA-1 repositories and does **not** touch brygge's integrity model: the IR re-hashes everything under SHA-256 (`AtomId`/`BlobId`/digest, RFC 003 D-3), and the Git SHA-1 is preserved **opaquely** as a source identifier (PR-4), trusted for nothing in any target. The residual is *source misread under a deliberate SHA-1 collision*, mitigated by SHA-256 re-hashing and `verify --against-source` (RFC 004 D-7). Track `gix`'s SHA-256-object support as it matures.
- **RR-svn-svnadmin — brygge trusts the operator's `svnadmin` binary and its dump output** (RFC 006 Tier D; TB-4). A hostile or compromised `svnadmin` on the host could feed a false dumpstream; mitigated by ASSUME-1 (the host is trusted) and by the **dumpfile path**, which bypasses the subprocess entirely. Separately, `svnadmin` parsing a hostile *repository* is a semi-trusted-tool surface (a TB-2 variant in subprocess form), **isolated by process** (C-2e/C-4b) and covered by RR-1's sandbox recommendation for untrusted sources.
- **RR-svn-svnadmin-version — decode of a live SVN *repository* depends on the `svnadmin` version** (RFC 006 Tier D): two versions could frame the dumpstream differently, so the byte-for-byte result across hosts is guaranteed only for the same tool version. Named as a stated non-determinism (C-9), mitigated by treating the **dumpstream as the canonical deterministic input** (decoding the same dumpfile is always identical) and recording the source form (live dump vs supplied dumpfile) in provenance so `verify --against-source` (VF-2) is well-defined.
- **ASSUME-1 — The operator and their host are trusted; the source repository is not.** brygge defends A-HOST against the source (TB-1), not against a hostile operator.
- **ASSUME-2 — The target enforces its own admission/trust/seal** (BN-2). brygge's honesty is necessary but not sufficient; the target's policy is the last line, and OQ-1…OQ-3 (owner's) decide what that policy is for prikk.

## 6. Controls × threats

| Control ↓ / Threat → | T-1 | T-2 | T-3 | T-4 | T-5 | T-6 | T-7 | T-8 | T-9 |
|---|---|---|---|---|---|---|---|---|---|
| C-1a…e honesty in the object | ● | | ● | | | | | | |
| C-2a…e untrusted-input handling | | ● | | | | | ● | ● | |
| C-3a…c integrity/verify/determinism | | | ● | | | | | | ● |
| C-4a…e dependency isolation/audit | | ○ | | ● | ○ | | | | |
| C-5 boundary-not-enlarged (tested) | | | | ○ | ● | | | | |
| C-6a…c no-network / carried-verbatim | | | | | | ● | ○ | | |
| C-7 write-only-where-told | | ○ | | | | ○ | ● | | |
| C-8 bounded/cancellable | | ○ | | | | | | ● | |
| C-9 determinism | | | ○ | | | | | | ● |

(● primary control, ○ contributing.)

## 7. Traceability

| Threat-model element | Requirements / design basis |
|---|---|
| T-1 / C-1* / INV-1 | NG-3, HO-1…HO-5, VF-4, FS-01/02/04, CF-02; RFC 113 §2/§3 |
| T-2 / C-2* / INV-2 | TB-1, FA-2/FA-3, PR-4; PU-5 (why the surface exists) |
| T-3 / C-3* / INV-6 | VF-1/VF-3, IX-07, HO-4, FS-02, ID-4 |
| T-4 / C-4* / INV-4 | PU-5, BN-5; project dependency-discipline (mirrors prikk's five-crate posture, `prikk-ffi`) |
| T-5 / C-5 / INV-5 | BN-5, CT-05 |
| T-6 / C-6* / INV-3 | CT-01, NG-3, PR-3/PR-4 |
| T-7 / C-7 / INV-3 | BD-04, CT-02 |
| T-8 / C-8 | FA-1/FA-3/FA-4/FA-5, OP-02 |
| T-9 / C-9 / INV-6 | VF-1, ID-4, UD-4 |

*End of Threat Model v0.1. Per project rules, this document is revisited every release: a release whose changes touch new source parsers, new dependencies, the IR/provenance format, or any untrusted-input path **updates** this model; other releases **re-verify** its controls still hold. The controls most likely to need a test from day one: INV-2 (no source code executed; path-safety), INV-4/INV-5 (dependency isolation; output consumable without brygge deps), and INV-1 (honesty is present and non-suppressible in every produced object).*
