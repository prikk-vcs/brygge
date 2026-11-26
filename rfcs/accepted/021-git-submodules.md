# RFC 021 — Git submodules, carried as pinned references (0.10.0, contract 0.4.0)

**Status.** **Accepted (2026-11-26)** by the owner: "Yes on both" (this RFC, and letter 008's sending). Handoffs:
`rfcs/handoffs/021-git-submodules/`. Proposed (2026-11-26) by the architect, and revised the same day after the review
against the owner's philosophy (review 184).
**Refinement at batch K (the owner, 2026-11-26, review 186):** D-1's wire is **flat and additive**, not nested. A
nested record would have changed every existing atom's id, because an id is the hash of its record's encoding.
The model keeps `Content`. The identity rule below is added.
**Refinement at batch G (the architect, 2026-11-27):** `inspect --atoms` prints counts, not ops, so D-4's marker
there is the atom's op count gaining `, K submodule`, instead of `submodule <hex>`. It follows the owner's rulings of
2026-11-26 (`.git-exclude/reviewed/182-…`):
- the Git floor's `submodule` item is removed, **under conditions A and B**;
- mergeinfo stays dropped (PR-8 and SRC-S2 unchanged).

## Summary

- **The gap today:** a Git repository with even one submodule is refused entirely. One of the corpus's three Git
  repositories, jq, is refused this way, for every target.
- **The change:** brygge carries each submodule as exactly what Git records, **a path and the commit id it pins in
  another repository**. It never reads, fetches, imports or verifies the submodule's own repository.
- **Where it shows:** `encode git` writes it back as a submodule. The reports say plainly what was and was not done
  (condition A), and every target holds the reference or refuses it by name (condition B).
- **The cost:** one contract change, 0.4.0. brygge informs prikk first (letter 008).

## The one name

The concept is called **submodule** everywhere: the IR type, the decode report, `inspect`, the machine keys, the
guides and the CHANGELOG. "Submodule" is Git's own user-facing word, the one users know. "Gitlink" (Git's internal
name for the tree entry) is used nowhere in brygge's surface. The concept is made no more general than its one
source needs:
- Subversion's `svn:externals` stays refused (usually unpinned, and so not reproducible);
- Mercurial subrepositories are recorded in `.hgsubstate`, an ordinary file brygge already carries.

## Decisions

### D-1 — In the IR, an entry's content is a blob or a submodule

**The choice:** today Add, Modify and Replace carry `blob` and `mode`. A submodule has no blob. Three ways were
weighed:

| Option | What it is | Against the owner's philosophy |
|---|---|---|
| **(a) a fake blob** | the commit id stored as a blob's content, under mode `160000` | **rejected:** a blob that is not content. An encoder that ignores the mode silently writes a file holding hex, which is "implied" and "silently" both |
| **(b) three new op variants** (`AddSubmodule`, `ModifySubmodule`, `ReplaceSubmodule`) | one variant per operation and kind | **rejected:** it doubles the operations for one concept, which is not "less is more" |
| **(c) the content becomes a choice** | each of Add, Modify and Replace carries `content: Content`, where `Content` is `Blob { blob, mode }` or `Submodule { commit }` | **chosen:** one concept, one place. An encoder written against `brygge-ir` must handle `Submodule` explicitly, because the type makes it choose |

**The model:**
- `Content::Blob { blob: BlobId, mode: u32 }` is today's two fields, moved;
- `Content::Submodule { commit: Vec<u8> }` holds the pinned commit id, 20 bytes (SHA-1) or 32 (SHA-256), opaque, as
  the source stated it;
- **`Delete` is unchanged:** it removes whatever is at the path.

**The wire (contract 0.4.0, `ir-artifact-format.md`): flat and additive (review 186).**
- **A blob entry** in Add, Modify and Replace is `{1 path, 2 blob: id32, 3 mode: uvarint, 4 status}`, **byte for byte
  as in 0.2.0 and 0.3.0**;
- **a submodule entry** is `{1 path, 5 submodule: bytes, 4 status}`, with no field 2 or 3;
- **exactly one** of {2 and 3 together} or {5} is present. The strict reader refuses anything else as
  `NonCanonical`;
- field 5's length must be 20 or 32;
- the critical bit is set, as for every field.

**Where illegal states are prevented:**
- **in code,** by the type: `Content` cannot hold both or neither, and no encoder or transform can construct one;
- **at the boundary,** by the canonical reader: bytes that state both or neither are refused, as the reader refuses
  every other non-canonical form.
- **Why not a nested record** (the draft that review 184 accepted, and review 186 reversed): an atom's id is the
  SHA-256 of its record's encoding, so re-encoding blob entries would change the id of every atom ever made, for a
  history that did not change.

**The identity rule (added to `ir-artifact-format.md`):** an id is computed over the record's canonical encoding. **A
contract may add fields and variants, but must never re-encode an existing one,** so the same content keeps the same
id across contract versions. `brygge-bench ir-equal` (batch K §5) proves it for this contract: the same history,
decoded by 0.9.0 and by 0.10.0, gives equal `Ir` values, ids included.

**Kind changes:**
- **`Modify` keeps the content's kind:** a blob stays a blob, and a submodule stays a submodule (a moved pointer);
- **a change of kind at one path is `Replace`,** which already means "replaced in place" (a symlink swapped for a
  file, RFC 011 D-7);
- **`verify`'s tree replay** (`check_source_invariants`) tracks each path's kind, not only its presence, and refuses
  a `Modify` across kinds. Today it replays a set of paths; it becomes a map from a path to its kind.

**The 32-byte form:** SHA-256 Git repositories stay refused (the floor's `sha256-object-format`), so the decoder
produces only 20-byte ids today. The wire admits 32 now, so that lifting that floor item later needs no new contract.
The reader's acceptance of 32 is tested at level 1, with a hand-built artifact.

**Readers:**
- a 0.4.0 reader reads 0.2.0, 0.3.0 and 0.4.0;
- **an older reader refuses a 0.4.0 artifact at its version gate (RFC 011)**, before any field, so no older tool can
  misread a submodule.

### D-2 — Every artifact is written as contract 0.4.0

One contract per build, as at 0.3.0 (RFC 018), even when an artifact holds no submodule.
- **The alternative,** writing the lowest contract an artifact needs, would let brygge 0.9 read most 0.10 artifacts.
  But it makes one build write two formats, and a user could not tell which one a file is without inspecting it.
- **Rejected** for "less is more". v0 has no production users, and the CHANGELOG says it under `### Breaking`.

### D-3 — The Git decoder carries a submodule entry

- **The entry:** a tree entry of kind commit (`gitlink`, mode `160000`) becomes `Content::Submodule { commit }` at its
  path. Changes between commits become `Add`, `Modify` (a moved pointer) and `Delete`, as for files. A path that
  changes between a file or directory and a submodule is `Replace`.
- **Never** opened, fetched or recursed into. `.gitmodules` is an ordinary file in the tree, carried as content today
  (its URLs included). Nothing about it changes.
- **The floor:** `submodule` is removed from the Git floor (`floor.rs`). That list is the owner's, ruled 2026-11-26.
  `params["floor"]` records it, and `--against-source`'s floor alignment (review 140) keeps earlier artifacts
  verifiable.
- **The path checks** (C-2c) apply to a submodule's path as to any entry's.
- **The commit id's length** is checked: 20 or 32 bytes, matching the repository's object format.

### D-4 — Condition A: nothing implied

**Wherever a submodule appears, the user is told what brygge did and did not do:**
- **`decode`:** the human report gains one line, when there is at least one:
  `submodules: P path(s) in the history, T at the branch and tag tips; their own repositories were not read,
  imported or verified`.
  - `P` counts distinct paths that ever held a submodule; `T` counts those present at any ref's target. Both counts
    are defined in `machine-output.md`, and no count names an internal concept such as "references" or "ops";
  - the machine keys are `decode.submodules.paths` and `decode.submodules.at_tips`, under a versioned key;
- **`inspect`:**
  - the same line;
  - `inspect --atoms` marks an op whose content is a submodule `submodule <hex>`, never as a file;
- **`encode git`'s report:** `encode_git.submodules=N`, and the human line;
- **the guides:** a "Submodules" section in `git.md` and `git-target.md`. It says:
  - that the result's submodules point at the same commits as the source's;
  - that `git submodule update` fetches them from the URLs in `.gitmodules`, the source's own and unchanged, so
    running it contacts those hosts. brygge contacts none;
  - that brygge checked none of the submodules' repositories;
  - **that a submodule whose path has no entry in `.gitmodules`** is carried as the source holds it, and Git then
    cannot fetch it. That is the source's state, not brygge's.

### D-5 — Condition B: every target holds it or refuses it

- **`encode git`** writes `M 160000 <commit-hex> <path>` (`fast-import`'s gitlink form, an object id, not a mark).
  - **The check (G2) compares it:** the Git decoder reads the result's entry back as a submodule;
  - the pre-flight's list of refused modes loses `160000`.
- **A future prikk encoder (B1)** refuses a submodule by name until prikk decides otherwise (letter 008). This RFC
  writes no prikk code; it records the rule.
- **Plugins:** `writing-an-encoder.md` says an encoder must hold a submodule or refuse it by name, and **never** write
  it as a file. The type (D-1) makes the choice explicit.
- **The API change is stated, not hidden:** `PathOp`'s `blob` and `mode` move into `content`. Every encoder and
  transform plugin built on `brygge-ir` stops compiling until it handles `Content`, which is the intent. The
  CHANGELOG's `### Breaking` and both guides give the two-line migration.
- **Transforms:** `transform::Edit` and `transform::check` handle `Content`. A submodule op can be removed with a
  recorded `Transform` drop, like any op, and never silently. `transform authors` touches identities only.

### D-6 — Verification

- **`verify`:** the canonical form covers D-1's choice rule.
- **`--against-source`:** a submodule is compared like any entry (path, commit id).
- **G2:** as in D-5.
- **No check claims anything about the submodule's own repository.** That is D-4's text.

## Evidence (the release's proof)

- **jq** (`real.toml`, already pinned):
  - decoded with its submodule carried;
  - `verify`, and `--against-source` (`corresponds`);
  - `encode git` with the check;
  - in the result, `git ls-tree` shows the `160000` entry at the same path and commit as the source.
- **Every other corpus entry:** the same history as 0.9.0, proved by the cross-version check (review 174). The
  artifact bytes differ by the contract version.
- **Level 2:** a built repository with a submodule added, moved to another commit, replaced by a file, and removed.
  Every op, and the round trip through `encode git` and back.
- **Level 1:** D-1's wire rules (each malformed choice refused), the length rule, and D-4's report lines.

## Threat model

**`.gitmodules` URLs (T-6):** they are carried as file content, as before. A URL that holds credentials
(`https://user:token@…`) is the source's own published content, and is treated as every other file's content is
(RR-1's sandbox advice, and C-6's scope). Repositories with submodules are newly decodable, so the guide names this
case once.

`brygge-03` is updated:
- **C-2a / C-2c:** a submodule's path and its id are checked, and never followed;
- **C-1g:** the reports' statement (D-4);
- **C-3b:** contract 0.4.0's choice rule.
- **No new parser and no network:** the submodule is never opened.

## The prikk letter

**Letter 008 (draft, for the owner's authorization):** it tells prikk what 0.4.0 will carry, says brygge's prikk
encoder will refuse a submodule by name until prikk decides, and asks nothing that needs an answer before prikk's
import work is scheduled. **It is sent before contract 0.4.0 is released.**

## The release plan

| Batch | What |
|---|---|
| **K** | the contract, D-1 and D-6: `brygge-ir`'s `Content`, the wire, the reader and writer, `Edit` and `check`, the docs |
| **G** | the Git decoder (D-3), the reports (D-4), `encode git` and the check (D-5), the guides |
| **cut** | — |

## Reviewed against the owner's philosophy (2026-11-26)

Each judgment call, stated so it can be checked:

| Clause | Where this RFC meets it |
|---|---|
| clean | one concept with one name (submodule); one content choice (D-1); **one encoding** for the wire and for identity; one contract per build (D-2) |
| safe and secure | the submodule is never opened, fetched or followed; its path is checked; its id's length is checked; `.gitmodules` is handled as before, and its URL case is named |
| robust | **the same content keeps the same id across contract versions** (the identity rule); illegal states cannot be constructed in code, and are refused at the boundary; a `Modify` across kinds is refused by `verify`; `--against-source` and G2 compare submodules; older readers refuse 0.4.0 at the gate |
| sophisticated | the type makes every encoder and transform decide how to handle a submodule; nothing is generalized beyond its one real source |
| no confusion or misunderstanding | D-4's sentences say what brygge did **not** do, wherever a submodule appears; the counts are defined in user terms; the guide names what `git submodule update` will contact |

## Open questions for the owner

None remain. Decisions 1 to 3 of 181 are ruled (182). The judgment calls above are the architect's. If any reads
wrongly to you, say so before acceptance.
