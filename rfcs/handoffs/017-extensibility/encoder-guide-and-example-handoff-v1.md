# Handoff — RFC 017 D-1 and D-2: the encoder author's guide, and the unsupported reference encoder IR → Git (batch G)

**Governing:** RFC 017, accepted 2026-10-21 (OQ-2: in this repository, `publish = false`, labelled unsupported,
tested). **Target:** 0.6.0, "Extensibility". **Batch G comes first;** batch E (`brygge encode` and plugins,
`encode-plugins-handoff-v1.md`) follows, and uses this example as its test plugin. One review request; no topic
branch (no workflow or platform file API changes).

## 0. First, commit the architect's files on their own

Commit as "RFC 017 handoffs (0.6.0); RFC 017 corrected":
- `rfcs/handoffs/017-extensibility/` (this file and its sibling);
- `rfcs/accepted/017-extensibility.md` (two stale lines corrected: Part C is 0.7.0's; D-3's calling convention
  stated once);
- `rfcs/README.md` and `ROADMAP.md` (0.6.0 in progress).

## 1. The guide: `docs/src/reference/writing-an-encoder.md`

Add it to `SUMMARY.md` under the reference pages. Write it for an author who knows Rust and the target, and has
never read brygge's code. It covers:

- **Reading an artifact:**
  - `brygge_ir::artifact::from_file` / `read_from`, and what they verify before returning: the version gate, the
    digest, the canonical form, each blob's content address;
  - a blob read later is re-hashed, so an artifact that changes while open gives `ArtifactChanged`, never other
    bytes;
  - read blobs one at a time (`ContentStore::get`), never all at once.
- **The order:** atoms in canonical order (parents before children), so an encoder can emit them in one pass;
  then refs, the loss boundary and the flags.
- **The honesty obligations of an encoder** (the core of the page, with one short example each):
  - carry `Derived` marks and their parameters into the target (trailers, notes, or the target's own mechanism),
    or state in the target that they were dropped;
  - **never invent a value the IR does not carry** (a committer, a time, a time zone, an email). If the target
    requires one, supply it **and mark it as supplied by the encoder**;
  - report the IR's loss boundary and flags to the user;
  - present the result as the encoder's work, never as something brygge verified.
- **Versioning:** pin the IR **contract** version you support (`brygge_ir::version::CURRENT`), not the brygge
  version (IX-07), and refuse an artifact of a contract you do not know. `brygge-ir`'s reader already refuses an
  unknown contract; say so.
- **The plugin contract:** a section heading only, "Running as a `brygge encode` plugin", with "See batch E" as a
  placeholder comment. Batch E writes it.
- **What brygge does not do for you:** it supports no third-party encoder, and verifies the artifact, never an
  encoder's output.

## 2. The reference encoder: `examples/encode-git`

- **First, one addition to `brygge-ir`: the verified digest.** `artifact::Decoded` gives the IR and the skipped
  field count, but not the artifact's digest, which the `Brygge-Artifact:` trailer needs. Reading 32 bytes at a
  fixed offset in the file would bypass the reader. So:
  - add `pub digest: [u8; 32]` to `Decoded`, set by the reader to the digest it has just checked;
  - `from_bytes`, `read_from` and `from_file` all fill it, through the one parser;
  - adding a public field breaks code that builds or destructures `Decoded`, so list it under **Breaking** in the
    CHANGELOG (v0);
  - a test: the field equals the digest stored at the artifact's header, for a fixture artifact.

- **The crate:** `brygge-encode-git-example`, at `examples/encode-git/`, `publish = false`.
  - Add `examples/*` to the workspace `members`.
  - **It depends on `brygge-ir` only** (and std), and on no Git library: it writes a **`git fast-import` stream**
    (plain text) to stdout or to `--out <file>`. That is the isolation proof of PU-3: an encoder on the IR alone.
    Update `tools/check-ir-isolation.sh`'s allowlist if it needs to know the new crate, and say how.
  - The release tools must skip it: `cargo publish --workspace --dry-run` packages the same six crates as today,
    and `tools/check-release-dev-deps.sh` passes.
- **The interface** (batch E's contract, so the same binary works directly and as a plugin):
  `brygge-encode-git-example --artifact <path> [--out <file>]`. It refuses a second `--artifact`, and any unknown
  argument (exit 2).
- **The mapping:** report it in the request as a table: each IR field, and how it is written, supplied, or
  dropped. At least:
  - atoms → `commit` (marks, `from` and `merge` for the parents), in canonical order;
  - path ops → `M`/`D` with the IR's Unix modes (regular, executable, symlink; refuse any other mode by name);
    blobs written once each, by mark,
    read one at a time;
  - message, author and author time → as carried. **A missing time zone is supplied as `+0000`, and marked as
    supplied;**
  - committer and commit time → as carried; when the IR has none, the encoder supplies them and marks it;
  - refs → branches with `reset`, tags as lightweight tags, or `tag` with the IR's annotation where it exists.
- **The trailers,** appended to each commit message after one blank line:
  - `Encoded-by: brygge-encode-git-example (unsupported example; not part of brygge)`;
  - `Encoded-supplied: <fields>` (for example `committer, commit-time-zone`), only when something was supplied;
  - `Brygge-Derived: <kind> <params>` for a `Derived` atom, and its op-level derivations in a form you choose
    and document;
  - `Brygge-Artifact: sha256:<the artifact digest>`.
- **On stderr:** the IR's loss boundary and flags, and `Decoded`'s skipped non-critical field count when it is
  not zero, summarized, before the stream ends. A reader of the target
  must be able to find what the IR said it did not carry.
- **The labelling:** its `README.md`, its `--help` and its trailers all say **"an unsupported example; not part
  of brygge"**. It is never released as a binary, never published, and never added to the release workflow.

## 3. Proof

1. **The Git round trip:** decode a Git fixture with merges, an executable file, a symlink, a rename and an
   annotated tag. Encode it with the example, `git fast-import` it into an empty repository, and compare with
   the original:
   - for every commit, the tree (`git ls-tree -r`) and the message minus the trailers, the author, and the
     author time;
   - the refs and the tag annotation;
   - differences only where the IR records a loss. List any you find, with the loss record that explains it.
2. **The other sources:** encode one hg, one SVN and one CVS fixture artifact, and `git fast-import` each without
   error. Show the trailers on a CVS commit (`Derived`, and the supplied fields).
3. **Refusals:** a corrupted artifact (one flipped byte) is refused by the reader before any output; a second
   `--artifact` and an unknown flag give exit 2.
4. **The isolation:** `cargo tree -p brygge-encode-git-example` shows `brygge-ir`'s closure only.
5. These tests run in the ordinary suite, and skip with "skipping: git not on PATH" where `git` is absent, as the
   other real-tool tests do.
6. **The usual gates** (stable and 1.87, clippy on three targets, deny, audit, isolation, the dev-dependency guard,
   the links and the book, `actionlint` if a workflow changes).

## 4. CHANGELOG `[Unreleased]`

- **Breaking:** "`brygge-ir`: `artifact::Decoded` gains `digest`, the artifact digest the reader verified."

- **Added:** "An encoder author's guide (`docs/src/reference/writing-an-encoder.md`): how to build an encoder for
  any target on the IR alone, and the honesty rules it must keep (RFC 017)."
- **Added:** "`examples/encode-git`: an unsupported example encoder from the IR to Git, through `git
  fast-import`. It is not part of brygge, and is never released (RFC 017)."

## 5. Review request

`.git-exclude/review-requests/058-encoder-guide-and-example.md`. Commit and push to `main` after approval. Then
batch E.
