# Handoff — Subversion decoder corrections: symlinks, replacements, live refs, layout honesty, source form

**Governing RFC:** RFC 006 (Subversion decoder), Accepted; this handoff inherits that state. It builds on
the IR contract 0.2.0 types (RFC 011, handoff 5).

**Corrections carried** (numbered in the architect's intake review; this handoff defines them durably):

| ID | What |
|---|---|
| CR-07.1 | A symlink retarget without a property block stores `link <target>` verbatim (content corruption) |
| CR-07.2 | A `replace` becomes a plain `Modify`: the source's "new node" is lost. RFC 011 D-7 gives it `PathOp::Replace` |
| CR-07.3 | A deleted (or moved-away) branch or tag is reconstructed as a live ref pointing at the revision that deleted it |
| CR-07.4 | Every added directory is counted as a dropped "empty directory" |
| CR-07.5 | Paths outside trunk/branches/tags in an otherwise conventional repository are not flagged |
| CR-07.6 | The source form (dumpfile vs live `svnadmin dump`) and the `svnadmin` version are not recorded, although RR-svn-svnadmin-version relies on them |
| CR-04 (SVN) | `svn:author` is copied into both author and committer, and `svn:date` into both times |

**Order:** after handoff 5 is approved. It is independent of the CVS, hg and Git batch-2 handoffs.

---

## 1. Reproduce first

Write the failing fixtures for **CR-07.1** and **CR-07.3** before changing code, and report what they
show. If either does not reproduce, stop and report it.

## 2. Change scope (`crates/brygge-decode-svn/`)

### 2.1 Symlinks (CR-07.1)

- **Mode:** a file's mode comes from its **resolved** entry. That means its own property block when the
  node carries one, otherwise the copy source, otherwise the existing entry.
- **Content:** when the resulting mode is a symlink and the node carries text, strip the `link ` prefix.
  **This applies whether or not the node carried a property block.**
- **Rejected content:** if a symlink's text does not start with `link `, that is `Error::Read("svn:special
  file without a link target")`. A malformed dump is not guessed at.

### 2.2 Replacements (CR-07.2, RFC 011 D-7)

- Track the set of paths a revision **replaced**:
  - for a file `replace` node, its path;
  - for a directory `replace` node, every file under it, before and after.
- In the diff, a replaced path gives:
  - **`PathOp::Replace`** if present before *and* after, **even if the content is identical**, because
    the source stated a new node;
  - `Delete` if present only before;
  - `Add` if present only after.
- Git, hg and CVS are unaffected.

### 2.3 Live refs only (CR-07.3)

With `--reconstruct-refs`:
- **Emit a ref only for a branch or tag root that exists in the final tree,** i.e. at least one file
  lives under its prefix at the last revision. Its target stays the last revision that touched the root.
- **A root that existed and no longer does** (deleted, or moved away) is not emitted. Count it:
  `deleted or moved branches/tags not represented (N)`, class `Other`, with reason *"the IR's refs name
  live history; a deleted SVN branch or tag has no live head"*.
- **A moved root** appears under its new name, if the new name follows the layout.

### 2.4 Empty directories (CR-07.4)

Count a directory as an **empty directory** only if, **after the revision**, no file lives under it. That
applies to directories added in the revision, whether by `add` or copied. A directory that received files
in the same revision is not empty.

### 2.5 Layout honesty (CR-07.5)

With `--reconstruct-refs`, count the files, in the final tree, that live **outside** every root the
layout recognizes (trunk, `branches/<x>`, `tags/<y>`).
- If there are any, add `Flag { kind: ConventionViolation, what: "paths outside the trunk/branches/tags
  layout (N)", count: N, reason: "the repository partly follows the layout; these paths belong to no
  reconstructed branch or tag" }`. The CLI exits 30.
- The existing whole-layout-not-found flag is unchanged.

### 2.6 Source form (CR-07.6)

- **Provenance params:** add `source_form = "dumpfile" | "svnadmin-dump"`. For `svnadmin-dump`, also add
  `svnadmin_version` = the first line of `svnadmin --version --quiet`, run with a fixed argv, no shell,
  bounded output and neutralized text.
- **`verify --against-source`** (`crates/brygge/src/commands.rs`):
  - if the given source's form differs from the recorded `source_form` (a directory vs a file), report
    **`not-checked`** with reason *"the artifact was made from a <form>; verify against the same form"*,
    and exit `1`, per the handoff-1 rules;
  - if the forms match but `svnadmin_version` differs and the comparison then fails, the detail adds
    *"(svnadmin versions differ: <a> vs <b>)"*.

### 2.7 Claims (CR-04)

- `svn:author` → `author`, as `Text` bytes with the email absent;
- `svn:date` → `author_time`, with `offset_minutes: Some(0)` (`svn:date` is UTC);
- **`committer` and `commit_time` are absent.**
- An unparseable `svn:date` gives no time, counted as `unparseable svn:date (N)`, class `Other`.

## 3. Non-change scope

- The IR contract.
- The dumpstream parser, except as needed for §2.1.
- The ceilings.
- Delta dumps (0.3.0).

## 4. Required tests (svnadmin-dependent tests skip when it is absent; dumpfile fixtures are preferred)

1. **CR-07.1:** a symlink retargeted in a later revision without a property block gives blob content
   equal to the bare target.
2. **CR-07.2:**
   - `svn rm f; svn add f` in one revision (a file `replace`) gives `Replace`, even with identical
     content;
   - a directory replace gives `Replace`, `Delete` and `Add` as specified.
3. **CR-07.3:** a branch created and later deleted gives no ref and a count of 1; a moved branch gives
   the new name only.
4. **CR-07.4:** `svn mkdir d` plus a file added under it in the same revision gives no empty-dir count; a
   truly empty `svn mkdir e` gives a count of 1.
5. **CR-07.5:** a conventional repository plus a root-level `README` gives one `ConventionViolation`
   flag and exit 30.
6. **CR-07.6:**
   - a dumpfile decode records `source_form=dumpfile`;
   - verifying it against the live repository directory gives `not-checked`, exit 1;
   - a live decode records `svnadmin_version`.
7. **CR-04:** `committer` and `commit_time` are absent; `offset_minutes == Some(0)`.
8. **Determinism:** decoding the same dumpfile twice gives identical bytes.

## 5. Security gate

The `svnadmin --version` subprocess (fixed argv, bounded, neutralized) and the symlink-content handling
are reviewed against `brygge-03`. RR-svn-svnadmin-version's mitigation becomes true with §2.6.

## 6. Review request

File `.git-exclude/review-request/010-svn-corrections.md` with the standard sections and the §1
reproduction output.
