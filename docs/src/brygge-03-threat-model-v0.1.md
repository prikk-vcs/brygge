# brygge — Threat Model

| | |
|---|---|
| Document | brygge Threat Model (security) |
| Version | v0.6 |
| Date | 2026-09-25 (v0.6); 2026-09-24 (v0.5, v0.4 and v0.3 revisions; v0.2 2026-09-09; v0.1 2026-09-03) |
| Revised | **v0.6 (2026-09-25, 0.3.0, RFC 013 and batch I)** reviews the three new input paths and closes **RR-svn-special-toggle**. **New parsers, each bounds-checked, panic-free and ceilinged:** the svndiff version 0 reader (SVN delta dumps; every length and offset checked before use, the target ceiling applied before a window runs, svndiff 1/2 refused by name), SVN property deltas, Mercurial's complete store path encoding (hashed `dh/` names computed from the logical path, never reversed; a missing file is a read error), and CVS branch history (per-branch clustering, the parent rule, the branch-point atom). **C-2d** gains the SVN per-node text ceiling (1 GiB) and the total a delta dump may reconstruct (the dump ceiling), so a small delta dump cannot amplify. **C-2f** gains the SVN dump checksums (MD5 and SHA-1 on every node where stated; a consistency check, not authenticity). **C-3b:** the reader's canonical order for refs, drops and flags now follows the specified variant numbers; before, an artifact with drops of two classes or with both flag kinds (any CVS repository with branch revisions) failed its own integrity check, a false failure, fail-closed, never a false pass. Separately, the Mercurial decoder wrote two refs of one name for a branch with two heads, which the contract forbids: that artifact was invalid, and `verify` rightly rejected it. The decoder now writes one ref per branch, and the builder refuses to write any artifact with a repeated ref, drop or flag key, so brygge can no longer emit an artifact its own reader rejects. **C-4d:** one new dependency, `md-5` (RustCrypto, MIT OR Apache-2.0, in the SVN decoder only); `brygge-ir`'s closure is unchanged. **RR-cvs-reconstruction** extends to branch points. **Re-verified for 0.2.0 (2026-09-24):** increments 5, 3 and 3b change how the Git and CVS decoders hold and compute state, not what they read or trust: no new parser, dependency, input path or output. The ceilings (C-2d) and every control hold, and the byte-identical output was checked by two-build comparison. **v0.5 (2026-09-24, 0.2.0 batch C)** closes **RR-cvs-read-toctou**: the CVS reader checks that the file it opened is the file its walk saw (C-2c), and a narrower note replaces the residual. **v0.4 (2026-09-24)** folds RFC 012 (release automation) and the new supported platforms. **New control:** **C-4f** the release pipeline (a verified, gated, tagged commit on `main`; published only on the owner's approval; crates.io trusted publishing with no stored secret; SHA-pinned actions in the jobs that hold credentials; published crates checked byte for byte against the tag; binaries with checksums and build-provenance attestations; a published version's tag never moved, by governance rule). **C-4d** gains the weekly advisory check. **C-2c** is proven on Windows (symlinks and directory junctions refused, in CI) and noted for macOS. **New residuals:** **RR-release-agent-credentials** and **RR-release-dispatch-tools**. **v0.3 (2026-09-24)** folds the 0.1.0 correction cycle (intake review CR-01…CR-21, reviews 002–011) and closes the model against the code as built. **New controls:** **C-1f** output neutralization; **C-2f** source-object integrity (Git objects re-hashed, the commit-graph cache not trusted, tag chains verified; every hg revision checked against its node with a collision-detecting SHA-1); **C-6d** only what the source would publish is imported (Git carried namespaces; the hg published view). **Restated to match the code:** **C-2a** (strict, typed refusal; the panic boundary; release overflow checks), **C-2c** (read confinement, and the floor refusals that enforce it), **C-2d** (the declared ceilings, checked before allocation), **C-3a** (the seven `verify` checks and the three-valued verdict), **C-3b** (IR contract 0.2.0: version gate first, critical bit, strict canonical form, digest over the stored bytes), **C-7** (atomic write), **C-8** (cancellation and progress are **not** built; stated as such), **C-9** (no stated non-determinism remains in the IR). **Residuals:** **RR-cvs-reconstruction**, **RR-cvs-read-toctou** and **RR-svn-special-toggle** added; **RR-git-loose-object-symlink**, **RR-git-object-id-unverified** and **RR-hg-node-unverified** (found and closed in the same cycle) closed; **RR-gix-sha1** narrowed; **RR-4** and **RR-svn-svnadmin-version** updated to the rulings and the code. **v0.2 (2026-09-09)** folds the RFC 004 (gix) and RFC 006 (SVN Tier D) security-review deltas: **C-2e/C-4b/INV-4** — a C surface may be isolated to a **subprocess**, not only a dedicated FFI crate; **TB-2** acknowledges subprocess decoder producers; **C-2b** clarifies that invoking a trusted external tool is not executing source-provided code; and three residuals added — **RR-gix-sha1**, **RR-svn-svnadmin**, **RR-svn-svnadmin-version**. |
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
- **C-1b — honesty is a property of every object, not a report** (HO-4, FS-01/FS-02). The derived-marking, the loss boundary, and the `Unverifiable` status live *in* the IR/proposal and are recoverable from them (`brygge inspect`), so they cannot be lost, skipped, or separated from the history they describe.
- **C-1c — honesty is non-configurable** (HO-5, CF-02). No flag suppresses derived-marking, the loss boundary, `Unverifiable`, or the fidelity summary. Configuration tunes *inference*, never *honesty*.
- **C-1d — the two claims are never conflated** (VF-4, FS-04). "verified by the target" and "faithfully imported, authorship unverified" are distinguishable by any reader at any time; a preserved source signature is shown as *preserved and verifying nothing in the target*, never as a target signature.
- **C-1e — derived ≠ stated, in the object** (HO-1, IR-2). An inferred rename / reconstructed CVS changeset / inferred SVN branch is marked derived where it appears; a reader tells judgment from fact without re-running the heuristic.
- **C-1f — untrusted text cannot forge brygge's own output** (CL-07; CR-19). Source text (names, messages, paths, ref names, reasons derived from them) could otherwise print a fake `PASS` line, a fake key, or terminal control sequences that hide or recolor a verdict.
  - **Human output:** every control character and every bidirectional or invisible Unicode format character is shown escaped (`\u{XXXX}`), and a literal backslash is doubled, so an escape in the output is never ambiguous with one in the input.
  - **Machine output:** untrusted text never appears in a key (items are indexed). Values are percent-encoded outside a small safe set, so a value can never contain `=` or a newline and can never forge a line or a key.
  - **One renderer:** only the CLI prints. Decoder libraries write nothing to stdout or stderr, so no text can bypass the neutralization.

### T-2 (Tampering / Elevation / DoS) — hostile source repository
A crafted source exploits brygge or its host through TB-1/TB-2: memory-safety bugs in a decoder; **path traversal / symlink escape / absolute paths** in source-declared file paths; **decompression or delta bombs**; pathological object counts or sizes; and **source-embedded executable content** — Git hooks, `.gitattributes` clean/smudge filters, submodule URLs, SVN hook scripts, CVS `.cvsignore`/wrappers (A-HOST, A-IMPORT).

- **C-2a — all source bytes are untrusted** (TB-1). No decoder path assumes well-formedness; malformed input is a refusal (FA-2/FA-3), never undefined behaviour.
  - **Strict, never guessed.** A malformed field is a typed error, not a default. Examples: an unparseable RCS date or an out-of-range date component; an unknown escape or duplicate key in an hg extra; a malformed phaseroots, bookmark or localtag line; an SVN symlink without its `link ` target.
  - **Refused where carriage cannot be exact.** A shape brygge cannot carry byte-exact is a named floor refusal (exit 20), never a lossy conversion. This covers non-UTF-8 paths, and names that the IR holds as text: Git ref names and commit header names, CVS symbol names, and hg extra keys.
  - **Contained failure.** An unexpected panic inside a decoder or a decoder dependency is caught at one boundary in the CLI and becomes a typed failure (exit 1), never an unclassified abort. Release builds keep integer overflow checks, so an arithmetic wrap that review missed becomes that typed failure rather than a false value.
- **C-2b — brygge never executes source-provided code.** Decode reads *objects*; it does not run hooks, apply clean/smudge filters, fetch submodules, or execute any script the source carries. Filter/hook content is preserved opaquely as data (PR-4) if it is history, never invoked. **This is an invariant (INV-2).** Running a **trusted external tool** to read a source (RFC 006 Tier D invokes `svnadmin dump`) is distinct from and does not breach this: the tool is operator-environment code (TB-4), not source-provided, and `svnadmin dump` executes **no** repository hook scripts (hooks fire on commit/revprop paths, never on dump). A tool that *did* run source-embedded code, or a source construct that reaches out (`svn:externals`), is refused — externals are refused, not resolved (→ the SVN floor).
- **C-2c — source-declared paths are data, not filesystem targets; brygge reads only the repository it is given.** Paths from the source name content *inside the IR*; brygge never uses a source-declared path as a write destination (→ C-7). On the **read** side, a source must not be able to make brygge read another location on the host into the IR, which would be a confused deputy (T-6/T-7):
  - **Git:** opened with gix's isolated options (no global, system or environment configuration). Alternates, a redirected git directory (`gitdir:` files, `commondir`), and a symlinked `.git`, `objects`, `objects/info`, `objects/pack` or pack entry are refused. So are grafts, replace refs and shallow clones, which would substitute history.
  - **CVS:** any symlink under the repository root, file or directory, is refused; entries are examined with `lstat` and never followed.
    - **The file opened is the file walked.** On Unix it must have the same device and inode as the walk's `lstat`. On Windows it is opened without following a link, and refused if it is a link or any reparse point.
    - A swap between the walk and the open is `Read` ("changed while being read"), never a read of another file.
  - **Mercurial:** the store is read in place; a repository in the middle of a merge is refused.
  - **SVN:** a dumpfile is read as one stream; a live repository is read only through `svnadmin dump`.
  - **Per platform** (RFC 012; tested in CI on Linux x86_64 and arm64, macOS and Windows):
    - **Windows:** a file symlink, a directory symlink and a **directory junction** are each refused, under
      a CVS root and in Git's redirected-directory checks. `std` reports a junction as a symlink, because
      its reparse tag is a name surrogate, and a CI test asserts it. A Windows name that is not valid
      Unicode is refused as `non-utf8-path`.
    - **macOS:** APFS and HFS+ cannot hold a file name that is not valid UTF-8, so that refusal cannot
      arise there. Its escaping stays tested on every platform.
- **C-2d — resource bounds, refuse rather than exhaust** (→ T-8). Declared ceilings on object size, total count, path length and depth, and decompressed size. A ceiling is checked **before** the memory it protects is allocated, and hitting one is a typed refusal (`ResourceLimit`, exit 20) naming what exceeded and the ceiling with its unit, never an OOM, a stack overflow or a hang. Each decoder keeps its ceilings in one place, and its README lists them:
  - **Git:** blob size, read from the object header before the body is read; commit count; path length; tree depth, enforced by an iterative tree walk (recursion on attacker-chosen depth could abort the process beyond any panic boundary); tag-chain length.
  - **Mercurial:** every decompression (zlib, zstd) and every delta application is bounded, and each reconstructed revision must equal its index's recorded length. The obsstore and the small metadata files (phaseroots, bookmarks, localtags) have ceilings; the dirstate read is 40 bytes.
  - **SVN:** the dumpstream is size-checked before reading and read through a bound. One node's text (fulltext or reconstructed from a delta) is at most 1 GiB, and everything a delta dump reconstructs is at most the dumpstream ceiling; both are checked before a delta window runs. The `svnadmin` subprocess's stdout is bounded, and its stderr is drained with only a bounded prefix kept, so the child can never deadlock.
  - **CVS:** each `,v` file is size-checked before reading and read through a bound; the file count is bounded.
  - Streaming (bounded *memory*, not just bounded input): RFC 010's increments 2 and 4 were deferred by measurement in 0.2.0, since every peak tracks the IR itself.
- **C-2e — memory-safety posture.** brygge's own crates `forbid(unsafe_code)`. The unsafe/C surface is confined to dependencies (RR-1) and, if a C library is used at all, to a single dedicated FFI crate — **or, preferably where available, to a subprocess** (RFC 006 Tier D reads SVN via `svnadmin dump` rather than linking `libsvn`), which contains any memory-safety fault in that C code in a **separate address space** where it cannot corrupt brygge's process or defeat `forbid(unsafe_code)` (→ C-4b).
- **C-2f — source identifiers are verified, not trusted** (PR-4, VF-2). brygge preserves a source's own object ids (a Git SHA, an hg node) as the link back to the source. A crafted repository could present content under an id it does not hash to, and brygge would then publish a false link.
  - **Git:** every object whose content or links reach the IR is re-hashed with the repository's hash kind and compared with the id it was requested by; a mismatch stops the decode. That covers commits, trees, blobs, and every tag on a tag chain. History is walked from verified commits only: the commit-graph cache, an unverified index of parent ids, is not consulted, and a walk that disagrees with verified commit content is a read error. The SHA-256 object format is refused until the reader supports it.
  - **Mercurial:** every revision brygge reads (changelog, manifest, filelog) is checked against its node with Mercurial's own revision hash, `sha1(min(p1, p2) ‖ max(p1, p2) ‖ raw text)`, using a collision-detecting SHA-1 as Mercurial does. A mismatch, or a detected collision, stops the decode. A filelog's store name, including a hashed `dh/` name, is computed from the logical path with Mercurial's own encoding, so a wrong or missing file is a read error, never a silent skip. Revisions whose stored text is not the hashed text (ellipsis, external storage) and unknown revision flags are refused by name. The repository identity is taken from the smallest **published** root, which is read and therefore verified.
  - **SVN:** every checksum a dump states (`Text-content-md5`/`-sha1`, `Text-delta-base-*`, `Text-copy-source-*`) is checked on every node, and a mismatch stops the decode. This is **consistency, not authenticity**: the dump is untrusted and can state any checksum. It catches a corrupt dump, a delta applied to the wrong base, and a fault in brygge's own delta application. `svnrdump` writes MD5 only, hence `md-5`.

### T-3 (Tampering) — altering the import, or stripping its honesty
An attacker (or accident) modifies the IR between `decode` and `encode`, or strips derived-marks / loss-boundary / provenance so the output looks more trustworthy than it is (A-IMPORT).

- **C-3a — inconsistency is detectable** (VF-3; CR-02). `brygge verify`, which needs no source, runs seven checks, and each can fail:
  - **integrity:** the contract version and digest;
  - **structure:** parents exist and precede their children, refs target atoms, ref names are unique per kind, and there is at most one op per path per atom;
  - **replay:** replaying each atom along its first parent is consistent (adds target absent paths; modifies, deletes and replaces target present ones);
  - **derivations:** every derived record carries the parameters its kind requires (for example, a reconstructed CVS changeset needs its window, keys, date rule and confidence rule);
  - **source-invariants:** an atom's status fits its source kind, so a CVS artifact relabelled `Stated` fails even with its ids and digest recomputed;
  - **provenance:** it names its decoder, versions and source;
  - **loss-boundary:** the loss boundary is present and well-formed.

  The verdict is three-valued: `pass`, `fail`, or `incomplete` (a requested `--against-source` could not run). It is never reported as `pass` with a non-zero exit.
- **C-3b — the artifact is integrity-checkable and versioned** (IX-07; RFC 011). The IR carries its contract version and a content digest, so tampering or truncation is detectable rather than silent (this is detectability, not authentication — see RR-4).
  - **The version gate runs first:** an artifact of an unknown contract is refused before anything else is parsed.
  - **The digest covers the stored bytes**, not a re-encoding.
  - **The canonical form is strict:** a non-canonical encoding of the same content is refused, so no two byte strings mean one artifact.
  - **Evolution is safe by construction:** each field carries a critical bit. An unknown critical field is refused, and an unknown non-critical field is skipped and reported, never silently ignored.
- **C-3c — determinism catches divergence** (VF-1, C-9). A fresh re-decode of the same source under the same version reproduces the IR; a tampered artifact diverges from the reproduction. `verify --against-source` reports `not-checked` (verdict `incomplete`), never `fail`, when the source given is a different **form** from the one recorded (an SVN dumpfile versus a live repository). It compares SVN decodes with the recorded `svnadmin` version aligned, noting a version difference rather than failing on it.

### T-4 (Supply chain) — the heavy dependency surface
The decoder libraries (~100 crates for `gix`, or C for `libgit2`, plus SVN/CVS) are the largest attack and audit surface, and the reason brygge is separate from prikk (A-SUPPLY, PU-5).

- **C-4a — isolate the weight behind the decoder boundary.** The heavy deps live only in the per-source decoder crates; the IR, the honesty/verify path, and the encoders do not link them. VF-3 (internal verification) must run without any decoder dependency present — the internal analogue of BN-5.
- **C-4b — pure-Rust preferred; C isolated by FFI crate *or subprocess*.** Prefer pure Rust (e.g. `gix`, keeps `forbid(unsafe)` maximal) over a C library (e.g. `libgit2`). If C code is unavoidable for a source, isolate it — in **preference order**: (1) a pure-Rust reader (no C at all — the ideal); (2) a **subprocess** that produces a parseable stream brygge reads in pure Rust (RFC 006 SVN Tier D: `svnadmin dump` → brygge's own dumpstream parser) — the C fault surface is a *separate process*, not brygge's address space, and vanishes entirely when the operator supplies the dumped stream directly; (3) a single dedicated **FFI crate** — the one place `unsafe`/C is linked into brygge, mirroring prikk's `prikk-ffi` discipline — used only when neither (1) nor (2) is available. The subprocess (2) is stronger isolation than the FFI crate (3) and is preferred wherever the source ecosystem offers a suitable tool.
- **C-4c — pin and lock.** Exact dependency versions; the lockfile is committed; upgrades are deliberate and reviewed.
- **C-4d — supply-chain gates in CI.** `cargo-deny` (advisories, licenses, banned/duplicate crates) and `cargo-audit` run in CI; a new advisory fails the build. New or upgraded decoder dependencies get explicit architect review (governance). A **weekly** `cargo audit` against a fresh advisory database runs with no code change (`security-audit.yml`), because advisories arrive on their own schedule. The isolation of `brygge-ir` (C-4a/C-5) is enforced in CI against a declared allowlist of its dependency closure (`tools/check-ir-isolation.sh`), not merely observed.
- **C-4e — minimize.** The dependency set is kept as small as the mission allows; a dependency is justified, not defaulted-in.
- **C-4f — the release pipeline publishes only what was verified, on the owner's go-ahead** (RFC 012; `release.yml`). A compromised pipeline could publish a malicious `brygge` to every `cargo install` user, so it is the place brygge holds its only publish credential. The pipeline enforces:
  - **What is released:**
    - an annotated `X.Y.Z` tag, on `main`, equal to the workspace version, with its CHANGELOG section;
    - gated by the same reusable workflow as CI, on the tagged commit;
    - built for the four CI-proven platforms before anything is published, unless a dispatch turns the
      binaries off (as for 0.1.0, whose release has none, and whose gates run on Linux only). Publication is
      all-or-nothing behind these checks.
  - **Who authorizes it:** the owner's explicit go-ahead, given before the architect pushes the tag
    (`GOVERNANCE.md`); the tag push starts the publication. *(A per-release approval gate in the
    `release` environment was used for 0.1.1 and removed for v0 by owner ruling, RFC 012 D-2.)* The
    `release` environment stays, without a reviewer: it scopes crates.io trusted publishing, and it
    admits only `main` and release tags. A published version's tag is never moved or deleted: a governance rule (`GOVERNANCE.md`), not a GitHub ruleset. It was deliberately not
    mechanized in v0 (RFC 012 D-2, amended), because crates.io versions cannot change anyway.
  - **What the credential is:** crates.io trusted publishing issues a token that lasts minutes, to this
    workflow in this environment; no crates.io token is stored anywhere.
    - Permissions are least-privilege per job (`contents: read` by default; `id-token: write` only where a
      token is minted; `contents: write` only for the GitHub release).
    - Every third-party action in `release.yml` is pinned to a commit SHA.
    - No workflow expression is interpolated into a shell script.
  - **What was released is checked:**
    - after publication, `brygge` is installed from crates.io and smoke-tested;
    - every published crate is compared byte for byte with the tag;
    - binaries carry SHA-256 checksums and GitHub build-provenance attestations
      (`gh attestation verify`).

### T-5 (Elevation) — brygge output enlarging the target's audited surface
A design in which consuming a brygge import forces the target to link a brygge dependency would defeat the whole separation: prikk's deliberately small audited dependency surface would grow through the back door (A-TARGET-TRUST, BN-5).

- **C-5 — the boundary is a tested property** (CT-05). The IR, the proposal, and internal verification (VF-3) are consumable and checkable with **only the target's own dependency surface** — no brygge decoder dependency required downstream. This is verified by a test that consumes brygge output with none of brygge's decoder deps present.

### T-6 (Information disclosure) — leaking source secrets, or importing them unknowingly
Source history may contain committed secrets, private emails, GPG signatures, and internal paths. Threat: brygge exfiltrates them (network/telemetry/logs), or the operator imports secrets without realizing they travel (A-SOURCE-CONTENT).

- **C-6a — no network, no telemetry** (INV-3). brygge reads sources and writes operator-specified files only (CT-01). There is no phone-home, no analytics, no remote fetch during decode (submodules are refused, not fetched — C-2b).
- **C-6b — no content in logs beyond the operator's chosen surface.** Diagnostics name atoms, classes, and counts; they do not dump source content into logs that could outlive the operator's intent.
- **C-6c — carried-verbatim is stated, not silently scrubbed.** brygge preserves source content faithfully (VF-2 depends on it), so it must **not** silently redact — but the fidelity surface states plainly that content is carried verbatim from source, so the operator knows secrets-in-history come along and can act **in the source** before import. brygge names what it does not do; scrubbing is the operator's pre/post step (RR-3).
- **C-6d — only what the source would publish is imported** (CR-05). Some history in a repository was never meant to leave it.
  - **Git:** history comes only from the carried namespaces (`refs/heads/*`, `refs/tags/*`). Commits reachable only from stash, notes, remote-tracking or other refs are not imported, and they are counted.
  - **Mercurial:** brygge computes the repository's published view (what `hg clone` would share). Secret, archived and internal-phase changesets, and hidden (obsolete) ones, are not imported, and they are counted.
  - A migration therefore cannot carry a changeset the source marked private.

### T-7 (Elevation) — confused deputy / errant writes
A source with crafted paths tries to make brygge write outside `--out` (A-HOST).

- **C-7 — write only where told.** brygge writes one file, the artifact at `decode --out`; source-derived paths are never write targets (C-2c). This mirrors stikk's "never write inside a repository" discipline, one boundary over. The write is **atomic**: a temporary file in the same directory, synced, then renamed over the target. An interrupted or failed decode leaves the previous artifact intact, never a truncated one.

### T-8 (Denial of service) — resource exhaustion
Enormous repositories, pathological delta chains, deep trees, huge file counts, decompression bombs (A-HOST). (Called out separately from T-2 because it is a normal operating condition for real migrations, not only an attack.)

- **C-8 — bounded and honest at the limit.** A ceiling hit is a typed refusal with a named reason (FA-3, C-2d), never an OOM that leaves an ambiguous artifact. An interrupted run leaves no artifact, or the previous one intact (C-7); brygge does not write partial artifacts.
  - **Not built in 0.1.0:** progress reporting and graceful cancellation (OP-02), and streaming with bounded memory (RFC 010 increments 2–4, 0.2.0). Interrupting brygge today simply stops it. That is safe because of C-7, but it gives no progress and produces no labelled partial.

### T-9 (Tampering) — non-determinism as a trust hole
If brygge is non-deterministic, "faithful" is uncheckable (VF-1 fails) and a tamper can hide in the noise (A-IMPORT).

- **C-9 — determinism is a security property.** The same inputs yield byte-identical IR and proposal; any non-determinism brygge cannot avoid is **named** as a stated non-determinism, never left to silently perturb output. Under IR contract 0.2.0 the IR carries **no** import time, so no stated non-determinism remains in the IR itself. The one input-side dependency is the `svnadmin` version for a live SVN decode (RR-svn-svnadmin-version). prikk ruled that an import attestation's `created_at` is a fixed sentinel (UD-4), so the proposal adds none either.

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

- **RR-1 — The heavy decoder dependencies may contain vulnerabilities.** This is the accepted cost of the mission (it is *why* brygge is separate from prikk). Mitigated by isolation (C-4a), pure-Rust preference (C-4b), pinning + supply-chain gates (C-4c/d), and the recommendation that operators run brygge over **untrusted** source repositories in a sandbox (container / restricted user / no ambient credentials), since TB-1 input reaches those libraries. *(v0.4 note: `cargo audit` reports RUSTSEC-2026-0306, an **informational** "unsound" advisory against `faster-hex 0.10.0`, reached only through `gix-hash`. It is tracked by the weekly audit and reviewed with every dependency update.)*
- **RR-2 — brygge cannot make a lying source honest.** A faithfully-imported falsehood is still a falsehood; VF-2 checks *correspondence to the source*, not the source's own truthfulness. Detecting source-level fraud is out of scope.
- **RR-3 — Secrets/PII in source history are carried faithfully.** Redaction would break fidelity (VF-2) and is the operator's decision in the source, before or after import; brygge's duty is to *state* that content is carried verbatim (C-6c), not to scrub it.
- **RR-4 — brygge's artifact is integrity-*detectable* (C-3b) but not cryptographically *authenticated*, and brygge never signs.** prikk ruled on 2026-09-13 (RFC 113 OQ-1/OQ-2) that the **importer** signs the import declaration, and that the importer is prikk's own import command, run by an adopted maintainer. Authentication therefore happens at prikk's boundary, over what that maintainer imports, once prikk's import attestation (UD-1) is built. Until then, anyone who can alter an artifact can also recompute its digest; the defense is `verify --against-source` (C-3c) against the source.
- **RR-gix-sha1 — `gix` reads SHA-1 Git objects without collision detection** (RFC 004 security review; relates to RUSTSEC-2025-0021, cleared as an advisory by pinning forward). This is inherent to SHA-1 repositories and does **not** touch brygge's integrity model: the IR re-hashes everything under SHA-256 (`AtomId`/`BlobId`/digest, RFC 003 D-3), and the Git SHA-1 is preserved **opaquely** as a source identifier (PR-4), trusted for nothing in any target. The residual is *source misread under a deliberate SHA-1 collision*, mitigated by SHA-256 re-hashing and `verify --against-source` (RFC 004 D-7). **Narrowed in v0.3:** since every object is now re-hashed against its id (C-2f), a *mismatched* object is impossible; only a true SHA-1 collision remains. Track `gix`'s SHA-256-object support as it matures (the SHA-256 object format is refused until then).
- **RR-svn-svnadmin — brygge trusts the operator's `svnadmin` binary and its dump output** (RFC 006 Tier D; TB-4). A hostile or compromised `svnadmin` on the host could feed a false dumpstream; mitigated by ASSUME-1 (the host is trusted) and by the **dumpfile path**, which bypasses the subprocess entirely. Separately, `svnadmin` parsing a hostile *repository* is a semi-trusted-tool surface (a TB-2 variant in subprocess form), **isolated by process** (C-2e/C-4b) and covered by RR-1's sandbox recommendation for untrusted sources.
- **RR-svn-svnadmin-version — decode of a live SVN *repository* depends on the `svnadmin` version** (RFC 006 Tier D): two versions could frame the dumpstream differently, so the byte-for-byte result across hosts is guaranteed only for the same tool version. Named as a stated non-determinism (C-9), mitigated by treating the **dumpstream as the canonical deterministic input** (decoding the same dumpfile is always identical) and recording the source form (live dump vs supplied dumpfile) in provenance so `verify --against-source` (VF-2) is well-defined. **Implemented in 0.1.0:** provenance records `source_form`, plus `svnadmin_version` (neutralized, bounded) for a live decode; `verify --against-source` refuses to compare across forms (`not-checked`) and aligns the version before comparing (C-3c).
- **RR-cvs-reconstruction — a CVS changeset is brygge's derived judgment, not a source record** (RFC 007). A consumer that *ignores the `Derived` status* could over-trust the grouping as if CVS had recorded it. The marking is in every atom; the confidence rule (`span-overlap-v1`), its inputs and every order split are recorded; the fidelity report leads with it; and the faithfulness statement states the limit before the run. The residual is a consumer discarding honesty brygge attached, which brygge cannot prevent, only make impossible to lose (the CVS-specific sharpening of RR-2). **Since 0.3.0 it covers branch history too:** a branch's parent edge is brygge's judgment (`branch_point_rule`, and `branch_point` exact, approximate or unconstrained, with approximate flagged), and a branch-point atom's ops are `Derived`; no merge parent is ever inferred.
- **RR-cvs-open-blocking** *(replaces RR-cvs-read-toctou, closed in v0.5)*: on Unix the CVS reader's `open` is an ordinary one, and the identity check follows it. So someone who can swap a `,v` path for a FIFO *while brygge reads* could make that `open` block: a hang, never a read of the wrong file. This needs concurrent write access to the source, a compromise of A-HOST (ASSUME-1). It is not mechanized in v0, because a non-blocking open needs a per-OS constant or a dependency.
- **RR-release-agent-credentials — the AI agents (architect, implementer) act through the owner's own git
  and GitHub credentials, so GitHub cannot tell them apart from the owner** (RFC 012 D-2).
  - Nothing mechanical stops an agent from creating, moving or deleting a tag.
  - With no approval gate, **pushing a release tag is the act that publishes**, so an agent that pushed a
    version tag could start a publication.
  - **Mitigations:**
    - `GOVERNANCE.md` ("Cutting a release") and the agents' standing instructions: only the architect
      tags, only after the owner's go-ahead, and the implementer never tags;
    - `verify`'s checks narrow what such a tag could publish to a commit on `main`, at the workspace
      version, with a CHANGELOG section and green gates;
    - two-factor authentication on the owner's accounts.
- **RR-release-dispatch-tools — a dispatched release runs the release tools of the commit it was dispatched
  from, not of the tag** (needed to release a tag that predates the tools, like 0.1.0).
  - **Mitigation:** the `release` environment admits only `main` and release tags, so those tools are
    always code committed to `main` or to a release tag. It is reviewed by the project's process, not
    enforced by a branch rule: RFC 012 D-2 deliberately adds no branch protection, and CI gates such a
    commit after it is pushed.
  - The tools' test-only environment hooks (`BRYGGE_TEST_*`) are set by no workflow, and
    `verify-published` checks the real crates.io independently.
- **Closed in v0.3:**
  - **RR-git-loose-object-symlink** (a symlinked loose object could read outside the repository): a loose object now either hashes to its id, so it is the right object wherever it was read from, or the decode stops (C-2f).
  - **RR-git-object-id-unverified** (a preserved Git id could be a false link): every object is verified (C-2f).
  - **RR-hg-node-unverified** (the same gap for Mercurial nodes, found in the 0.1.0 release review): every revision read is verified (C-2f).
- **Closed in v0.6:** **RR-svn-special-toggle** (a property-only change of `svn:special` kept the previous content): the node's content is recomputed from its base's SVN text under the new flag, and malformed symlink text is still refused (C-2a). It is checked against `svn cat` in all three dump forms.
- **Closed in v0.5:** **RR-cvs-read-toctou** (a symlink swapped in between the walk and the open would be followed): the opened file must be the walked file (C-2c).
- **ASSUME-1 — The operator and their host are trusted; the source repository is not.** brygge defends A-HOST against the source (TB-1), not against a hostile operator.
- **ASSUME-2 — The target enforces its own admission/trust/seal** (BN-2). brygge's honesty is necessary but not sufficient; the target's policy is the last line, and OQ-1…OQ-3 (owner's) decide what that policy is for prikk.

## 6. Controls × threats

| Control ↓ / Threat → | T-1 | T-2 | T-3 | T-4 | T-5 | T-6 | T-7 | T-8 | T-9 |
|---|---|---|---|---|---|---|---|---|---|
| C-1a…e honesty in the object | ● | | ● | | | | | | |
| C-1f output neutralization | ● | | ● | | | | | | |
| C-2a…e untrusted-input handling | | ● | | | | ○ | ● | ● | |
| C-2f source identifiers verified | ○ | ● | ● | | | | | | |
| C-3a…c integrity/verify/determinism | | | ● | | | | | | ● |
| C-4a…e dependency isolation/audit | | ○ | | ● | ○ | | | | |
| C-4f the release pipeline | ○ | | ● | ● | | | | | |
| C-5 boundary-not-enlarged (tested) | | | | ○ | ● | | | | |
| C-6a…c no-network / carried-verbatim | | | | | | ● | ○ | | |
| C-6d only what the source would publish | | | | | | ● | | | |
| C-7 write-only-where-told | | ○ | | | | ○ | ● | | |
| C-8 bounded/cancellable | | ○ | | | | | | ● | |
| C-9 determinism | | | ○ | | | | | | ● |

(● primary control, ○ contributing.)

## 7. Traceability

| Threat-model element | Requirements / design basis |
|---|---|
| T-1 / C-1* / INV-1 | NG-3, HO-1…HO-5, VF-4, FS-01/02/04, CF-02; RFC 113 §2/§3; C-1f: CL-07, CR-19 |
| T-2 / C-2* / INV-2 | TB-1, FA-2/FA-3, PR-4; PU-5 (why the surface exists); C-2c: CR-11; C-2d: RFC 010 D-4, CR-10/CR-17; C-2f: PR-4, VF-2 |
| T-3 / C-3* / INV-6 | VF-1/VF-3, IX-07, HO-4, FS-02, ID-4; C-3a: CR-02, RFC 011 D-10; C-3b: RFC 011 |
| T-4 / C-4f | RFC 012 (release automation); `GOVERNANCE.md` "Cutting a release"; `docs/src/development/releasing.md` |
| T-4 / C-4* / INV-4 | PU-5, BN-5; project dependency-discipline (mirrors prikk's small, audited dependency posture and `prikk-ffi`) |
| T-5 / C-5 / INV-5 | BN-5, CT-05 |
| T-6 / C-6* / INV-3 | CT-01, NG-3, PR-3/PR-4; C-6d: CR-05, owner ruling D-4 (2026-09-23) |
| T-7 / C-7 / INV-3 | BD-04, CT-02; CR-13 |
| T-8 / C-8 | FA-1/FA-3/FA-4/FA-5, OP-02 |
| T-9 / C-9 / INV-6 | VF-1, ID-4, UD-4 |

*End of Threat Model v0.5. Per project rules, this document is revisited every release: a release whose changes touch new source parsers, new dependencies, the IR/provenance format, or any untrusted-input path **updates** this model; other releases **re-verify** its controls still hold. The controls most likely to need a test from day one: INV-2 (no source code executed; path-safety), INV-4/INV-5 (dependency isolation; output consumable without brygge deps), and INV-1 (honesty is present and non-suppressible in every produced object).*
