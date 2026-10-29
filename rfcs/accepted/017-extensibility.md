# RFC 017 — Extensibility: the IR as the extension point, external encoders, and honest transforms

**Status.** **Accepted (2026-10-21)**; two stale lines corrected by the architect on 2026-10-29 (Part C is in
0.7.0, per OQ-3; D-3's calling convention stated once) by the owner ("Accepted."). **Parts A and B implemented in
0.6.0 (2026-10-29); Part C follows with the contract revision in 0.7.0.** No separate answers to OQ-1…OQ-4 were given, so they
are recorded as the recommendations (as for RFCs 011 and 013), and the owner can object:
- **OQ-1:** plugins are found on `PATH` only;
- **OQ-2:** the reference encoder lives in this repository, `publish = false`, labelled unsupported, and tested;
- **OQ-3:** **0.6.0 "Extensibility"** ships Parts A and B, and **0.7.0** is the contract revision (0.3.0), with
  compact artifacts and transforms together;
- **OQ-4:** brygge always verifies before running a plugin.

**Handoffs** (`rfcs/handoffs/017-extensibility/`) are issued **after 0.5.0 is released,** so that 0.5.0's scope
stays as planned. Proposed (2026-10-21) by the architect, at the owner's request ("it will be very helpful if a module
to decode our IR data to a Git repository joins ours. However, we are not capable enough to support ones except
prikk now. Instead, we may be able to offer flexible extensibility as plugins or middlewares").
**Owner questions:** OQ-1…OQ-4, each with a recommendation.

## Summary

- **brygge separates decoding from encoding by design.** The artifact format is published and versioned (contract
  0.2.0, `ir-artifact-format.md`), `brygge-ir` is a light crate anyone can depend on, and since RFC 015 a reader
  streams any artifact with every check enforced (`artifact::read_from`). The ROADMAP's Phase B2 already promises
  "a non-prikk target's encoder written against the IR alone, with no brygge change" (PU-3).
- **This RFC turns that into a supported extension surface, without brygge supporting the extensions
  themselves.** It has three parts:
  - **Part A:** the IR is the official extension point. That means an encoder author's guide, and a small,
    explicitly unsupported reference encoder from IR to Git;
  - **Part B:** external encoder plugins (`brygge encode <target>` → `brygge-encode-<target>` on `PATH`), in the
    style of git and cargo subcommands;
  - **Part C:** honest **transforms** (middlewares, IR → IR), which **must record themselves** in the artifact. That
    needs a contract revision, so it is designed with compact artifacts (RFC 015 OQ-1), in 0.7.0 (OQ-3).
- **Ruled out: in-process plugins.**
  - dynamic libraries need `unsafe` and an ABI Rust does not have;
  - WebAssembly needs a heavy runtime;
  - and both run third-party code inside the tool that guards provenance.

## Constraints

- **Honesty survives every extension.** brygge's value is honest provenance, and its signature danger is
  "manufactured verification" (the threat model). An extension inside brygge's chain (a transform) must leave a
  record a reader can check. An extension after the chain (an encoder) is outside brygge's guarantee, and must be
  presented as such, never as something brygge verified.
- **Support stays honest too.** Only what brygge supports is called supported. The reference encoder, and every
  plugin, is labelled as not part of brygge.
- **No change to prikk's import design.** prikk's encoder (RFC 008) remains one encoder among others, on the same
  IR. Nothing here prescribes how prikk imports.
- **Trigger-driven, stable Rust, no `unsafe`, no new heavy dependency** (GOVERNANCE, RFC 009).

## Decisions

### D-1 — Part A: the encoder author's guide

- **A new book page,** `docs/src/reference/writing-an-encoder.md`, covering:
  - **reading:** `brygge_ir::artifact::read_from` / `from_file`, what they verify, and that a later blob read is
    re-checked (`ArtifactChanged`);
  - **the order:** the atoms in canonical order (parents before children), refs, the loss boundary and flags;
  - **blob access,** one at a time;
  - **the honesty obligations of an encoder:**
    - carry `Derived` marks and their parameters into the target (as trailers, notes, or the target's own
      mechanism), or state in the target that they were dropped;
    - **never invent a value the IR does not carry** (a committer, a time, an email) without marking it as
      supplied by the encoder;
    - report the IR's loss boundary;
  - **versioning:** pin the IR contract version, not the brygge version (IX-07).

### D-2 — Part A: the reference encoder, IR → Git (unsupported, illustrative)

- **A small, `publish = false` crate,** `examples/encode-git` (the binary `brygge-encode-git-example`). It reads an
  artifact with `brygge-ir`, and **writes a `git fast-import` stream** (plain text: no Git library, no new
  dependency). `git fast-import` then builds the repository.
- **It demonstrates the guide's honesty rules,** on purpose:
  - where Git requires a value the IR does not state (a committer), it uses an explicit encoder identity, and adds
    a trailer: `Encoded-by: brygge-encode-git-example (unsupported example)`;
  - `Derived` atoms get a `Brygge-Derived: <kind> <params>` trailer;
  - every commit records the source artifact's digest (`Brygge-Artifact: sha256:…`).
- **Proof:**
  - decode a Git fixture, encode it back with the example, and compare the trees and messages of the result with
    the original **where the IR carries them**. Differences are only where the IR records a loss;
  - it runs in the ordinary test suite (it needs `git`; it skips where `git` is absent, as the other real-tool
    tests do).
- **The labelling:** its README, its `--help` and its trailers all say **"an unsupported example; not part of
  brygge"**. It is never released as a binary, and never published.

### D-3 — Part B: external encoder plugins

- **`brygge encode <target> <artifact> [-- <plugin arguments>]`.** brygge runs the plugin as
  `brygge-encode-<target> --artifact <artifact> [<plugin arguments>…]` (the contract below).
  - A built-in target (`prikk`, once RFC 008 lands) is used first.
  - Otherwise brygge runs **`brygge-encode-<target>`**, found **on `PATH` only**: never in the current directory,
    and never in a source repository.
- **This amends the external design's CL-03** ("`encode` is not part of the surface until Track B1"). The `encode`
  verb opens now, for plugins, and `encode prikk` stays Track B1's.
- **Safety:**
  - `<target>` must match `[a-z0-9][a-z0-9-]*`, so that no path or shell character reaches the lookup;
  - brygge runs the plugin directly (no shell), with the artifact path and the plugin arguments as separate
    arguments;
  - **before running it, brygge runs the internal `verify` checks on the artifact, and refuses** (exit 50) if they
    fail (OQ-4). A plugin is never handed an artifact brygge would reject.
- **The output honesty:** brygge prints, before the plugin runs, "encoding with the external plugin
  `brygge-encode-<target>` (`<resolved path>`): not part of brygge; brygge verified the artifact, not the
  plugin's output". The exit code is the plugin's own, except for brygge's own failures (an unknown target, or a
  failed verify).
- **The plugin contract,** documented in the guide:
  - the arguments brygge passes (`--artifact <path>`, then the user's arguments after `--`);
  - the expected exit codes;
  - a plugin should itself read the artifact with `brygge-ir`, and so get the same checks.
- **Discovery:** `brygge encode --list` prints the plugins found on `PATH`, with their resolved paths.

### D-4 — Part C: honest transforms (middlewares) — designed with the contract revision

- **What a transform is:** a program that reads an artifact and writes a new one (for example an author mapping, a
  path filter, or pseudonymisation, which prikk named as a later target-side decision).
- **The rule:** every transform **records itself** in the artifact. That needs, in the next contract version:
  - **a transform chain in the provenance:** each entry gives the transform's name, version and parameters, and the
    **digest of the artifact it consumed**, so the chain links back to the decoder's original;
  - **a `Transformed` derivation kind,** marking every op and claim a transform changed, with the transform's
    entry;
  - and **`verify`:** the chain is well-formed. `--against-source` on a transformed artifact compares against the
    **first** artifact of the chain where it can (re-decoding, and replaying the transforms if they are installed
    and deterministic), and otherwise answers `not-checked: transformed`. It never "corresponds" silently.
- **Scheduling:** this is a **contract change** (0.2.0 → 0.3.0), so it is designed **together with compact
  artifacts** (RFC 015 OQ-1), and the contract changes **once**. Its RFC is the 0.7.0 contract RFC (OQ-3), widened, or a
  sibling written at the same time.
- **Until then, no transform stage exists.** Users must not hand-edit artifacts; the digest stops that anyway.

## Open questions (the owner's)

- **OQ-1 — Plugin discovery.**
  - **Recommended:** `PATH` only, as git and cargo do. It is the user's own trust choice, and needs no new
    configuration file.
  - **Alternative:** a registered list in a config file. That is more control, but one more file to explain.
- **OQ-2 — The reference encoder.**
  - **Recommended:** in this repository, `publish = false`, **labelled unsupported**, and tested. It proves PU-3,
    and gives users the Git direction without promising to support it.
  - **Alternatives:** a separate repository; or no example at all, with the guide only.
- **OQ-3 — The release order.** Parts A and B need no contract change, while Part C does.
  - **Recommended:**
    - **0.6.0 "Extensibility"** ships Parts A and B;
    - **0.7.0** is the **contract revision** (0.3.0), with **compact artifacts and transforms** together.
    - The compact-artifacts RFC still **opens when 0.5.0 is released,** as ruled (RFC 015 OQ-1). Only the version
      it ships in moves, so the contract changes once.
  - **Alternative:** keep 0.6.0 as compact artifacts, and put Extensibility after it.
- **OQ-4 — Verify before running a plugin.**
  - **Recommended: yes, always.** A plugin is never handed an artifact brygge rejects. The cost is one internal
    verify, streamed since RFC 015.
  - **Alternative:** a `--no-verify` flag. It is faster, but invites handing broken artifacts to third-party code.

## Consequences

- People can build encoders (Git first, by example) on the IR alone, and run them as `brygge encode <target>`, with
  brygge's verification in front and honest labelling around them.
- Transforms become possible later, without a way to launder history.
- brygge supports exactly what it ships as supported, which is no third-party encoder.
