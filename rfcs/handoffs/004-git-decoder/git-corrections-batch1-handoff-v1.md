# Handoff — Git decoder corrections, batch 1 (history scope, repository shape, path integrity)

**Governing RFC:** RFC 004 (Git decoder), Accepted. This handoff inherits that state.

**Owner rulings applied:** D-3 (2026-09-23): non-UTF-8 paths are refused; repositories whose objects
would be read from outside themselves are refused, unconditionally.

**Corrections carried** (numbered in the architect's intake review; this handoff defines them durably):

| ID | What |
|---|---|
| CR-05 (Git part) | Non-history is imported as history: commits reachable only from dropped ref namespaces become atoms |
| CR-11 + D-3(ii) | Alternates and redirected git directories |
| CR-03 (paths only) + D-3(i) | Non-UTF-8 file and ref names are converted lossily; two names can collide |
| CR-06 (refs part) | A tag or branch whose target is not a commit is skipped silently |

**Out of scope, handled elsewhere:**
- lossy text in messages and names (carried as bytes after RFC 011);
- timezones and `mergetag` (RFC 011 and the Git batch-2 handoff);
- resource ceilings (the input-safety handoff).

**Order:** handoff **2 of 4**. Independent of handoff 1, except for the CLI message wording, which comes
from the decoder's `Error` text.

---

## 1. Purpose

Import exactly the history the repository's carried refs name, read nothing from outside the repository,
and never alter or merge a path.

## 2. Change scope (`crates/brygge-decode-git/`)

### 2.1 History scope (CR-05)

1. **Walk tips come only from carried refs:** non-symbolic `refs/heads/*` and `refs/tags/*` (a tag peeled
   to its commit).
   - Refs in dropped namespaces contribute no tip: `refs/remotes/*`, `refs/notes/*`, `refs/stash`, and
     anything else.
   - Commits reachable **only** from them are not imported.
2. **Loss records carry counts** (deterministic):
   - each dropped namespace names its ref count, e.g. `remote-tracking refs (3)`, `stash refs (1)`;
   - one record states the commits not imported: `what = "commits reachable only from dropped refs (N)"`,
     class `Representation`, reason "workflow state (stash, notes, remote-tracking), not authored history
     of a carried ref (RFC 004 D-5, OQ-B)". Omit the record when N = 0.
3. **`repo_id`** (the smallest root commit) is computed over the imported commits only.
4. **A repository with no carried ref** decodes to an empty IR with its loss records. It is not an error.

### 2.2 Repository shape (CR-11, D-3(ii))

Refuse before reading any object, as a floor refusal (`Error::FloorRefusal`, exit 20). Detect from the git
directory `open.rs` resolves:

| Condition | `feature` | Message must say |
|---|---|---|
| `objects/info/alternates` exists and is non-empty | `object alternates` | the repository borrows objects from another store; brygge reads only the repository it is given; run `git repack -a -d` in it (which copies borrowed objects in) and then remove `objects/info/alternates` |
| `objects/info/http-alternates` exists | `object alternates` | the same |
| the given path's `.git` is a **file** (`gitdir:` redirect), or the git directory contains `commondir` (a linked worktree) | `redirected git directory` | point brygge at the repository's main git directory (`git rev-parse --git-common-dir`) |

**Investigate and report (the CR-11 finding).** Does gix 0.87 with `open::Options::isolated()` follow
alternates, `gitdir:` and `commondir`, and does it read `GIT_DIR`, `GIT_OBJECT_DIRECTORY` or
`GIT_ALTERNATE_OBJECT_DIRECTORIES` from the environment? Answer from the gix source, cite file:line, and
put the answer in the review request. The refusal stands either way. The investigation tells us whether
the environment must also be cleared, and if it must, do so.

### 2.3 Path and ref-name integrity (CR-03 paths, D-3(i))

1. **File path components** (tree entry names) must be valid UTF-8. Otherwise:
   - `Error::FloorRefusal`, `feature = "non-UTF-8 path"`;
   - the message names the commit (full hex) and the path with its invalid bytes shown as `\xNN`. Build
     it without `to_str_lossy`: escape each invalid byte explicitly.
2. **Ref names** (after the `refs/heads/` or `refs/tags/` prefix) must be valid UTF-8, with the same
   treatment and `feature = "non-UTF-8 ref name"`.
3. **Replace every `to_str_lossy`** on a path or ref name with the checked conversion.
   - The `to_str_lossy` calls on **messages, names and emails remain** until RFC 011 carries them as bytes.
   - Add one `// CR-03: lossy until RFC 011 (text as bytes)` comment at each remaining site, so the next
     handoff finds them.
4. **Paths must be unique.** After conversion, a tree snapshot must never receive the same path twice.
   `walk_tree` returns `Error::Read` if an insert would overwrite. Valid UTF-8 cannot collide, so this is a
   defense-in-depth assertion in code form.

### 2.4 Refs whose target is not a commit (CR-06, refs part)

A `refs/heads/*` or `refs/tags/*` ref that peels to a **tree or blob** (legal in Git, e.g. a tag on a
blob) cannot be a `RefRecord`, which targets an atom. Record it; do not skip it silently:
- `what = "refs to non-commit objects (N)"`, class `Other`;
- reason "a tag or branch naming a tree or blob; the IR's refs point at history atoms (PR-9)".

A ref whose target is missing entirely is `Error::Read` (a broken repository), not a drop.

## 3. Non-change scope

- The IR, the CLI (beyond messages flowing through unchanged), the rename inference, the floor items
  already shipped, and the CR-16 behaviour.
- Messages, names and emails: still lossy (see §2.3.3).

## 4. Required tests (`src/decode/tests.rs`, git-dependent tests skip without `git`)

1. **Stash, notes and remote-tracking:** create a stash, a note and a commit reachable only from
   `refs/remotes/origin/x`. None becomes an atom; the counts appear in the loss records; `repo_id` is
   unchanged versus the same repository without them.
2. **Alternates:** `git clone --shared` (creates `objects/info/alternates`) → refused, with feature
   `object alternates`. Also a hand-written empty alternates file → not refused.
3. **Redirected git directory:** `git worktree add` gives a linked worktree whose `.git` is a file.
   Decoding the worktree path → refused, feature `redirected git directory`. Decoding the main repository
   → succeeds.
4. **Non-UTF-8 path:** commit a file named with byte `0xFF` (use `git update-index --add --cacheinfo` with
   a name built from bytes, or a hand-built tree via `git mktree`). Expect refusal, and a message holding
   `\xFF` and the commit hex.
5. **Non-UTF-8 ref name:** `git update-ref` with a non-UTF-8 name, if git permits it on the test platform;
   otherwise document why the case cannot be built and cover the conversion with a unit test.
6. **Tag on a blob:** `git tag blobtag $(git hash-object -w file)` → no `RefRecord`, the drop counted.
7. **Regression:** all existing Git tests pass. The determinism and pack-independence tests still hold,
   now also with a stash present.

## 5. Acceptance criteria

- Only carried history is imported, and everything left out is counted.
- No object is read from outside the given repository.
- No path or ref name is ever altered.
- The five gates pass, `--locked`; `Cargo.lock` unchanged.

## 6. Prohibited shortcuts

- Do not filter dropped-namespace commits *after* building atoms. Compute the tips first, so no
  out-of-scope commit is ever read.
- Do not make the alternates refusal conditional on the investigation's result (owner ruling D-3).
- Do not substitute `U+FFFD` or any replacement in a path, anywhere.

## 7. Security gate

This touches an untrusted-input path and adds refusals, so the architect reviews it against `brygge-03`.
CR-11's answer may add a threat-model control (read confinement to the given repository) at the 0.1.0
revision.

## 8. Review request

File `.git-exclude/review-request/004-git-corrections-batch1.md` with the standard sections, plus the
CR-11 investigation answer with gix file:line citations.
