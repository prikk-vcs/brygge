# Handoff — Mercurial decoder corrections: the published view, copies with their source, claims and extras

**Governing RFC:** RFC 005 (Mercurial decoder), Accepted; this handoff inherits that state. It builds on
the IR contract 0.2.0 types (RFC 011, handoff 5).

**Owner ruling applied: D-4 (revised, 2026-09-23).** brygge computes the repository's **published view**
itself: exactly what `hg clone` would share. Secret and hidden changesets are excluded, and the report
counts them. The user does nothing and remembers nothing.

**Corrections carried** (numbered in the architect's intake review; this handoff defines them durably):

| ID | What |
|---|---|
| CR-05 (hg) | Secret-phase and obsolete (hidden) changesets are imported as ordinary history, and an obsolete head even surfaces as a named-branch head |
| CR-04 (hg) | `user` is copied into both author and committer, and `date` into both times |
| CR-06 (hg) | Timezone offsets and changeset extras (e.g. `close=1`) are dropped silently; bookmarks naming unknown changesets are skipped silently |
| RFC 011 D-6 | A stated copy's `from_atom` is p1, not where the copy actually came from (`copyrev`) |

**Order:** after handoff 5 is approved. It is independent of the CVS, SVN and Git batch-2 handoffs.

---

## 1. The published view (CR-05, D-4)

Compute, from the store alone, the set of changesets `hg clone` would transfer. That is Mercurial's
**`served`** view. Only those become atoms. Build it in four steps.

### 1.1 Phases (`.hg/store/phaseroots`)

- **The file:** one line per root, `<phase> <40-hex node>`. The phase values are 1 (draft), 2 (secret),
  32 (archived) and 96 (internal). Nodes not below any root are public (0).
- **The rule:** a changeset's phase is the **maximum** phase of the roots it descends from (or is).
- **Exclusion:** every changeset with **phase ≥ 2** (secret, archived, internal) is excluded. By the
  descends-from rule, their descendants are too.
- **Robustness:** an absent file means everything is public. A malformed line is `Error::Read`, because
  a store that cannot say what is secret must not be decoded as if nothing were. Any node not present
  in the changelog is ignored.

### 1.2 Obsolescence (`.hg/store/obsstore`)

- **Reading.** Implement a minimal, bounds-checked reader for format **version 1**, following Mercurial's
  `obsolete.py` `_fm1readmarkers`:
  - a leading version byte;
  - then records, each `u32 size ‖ f64 date ‖ i16 tz ‖ u16 flags ‖ u8 numsuc ‖ u8 numpar ‖ u8 nummeta`;
  - then the precursor node (20 bytes, or 32 when flag bit `usingsha256` is set);
  - then the successor nodes, the parent nodes (none when `numpar == 3`), the metadata sizes, and the
    metadata.
- **Only the precursor node is needed.** The reader still checks every declared size against the bytes
  present, and against a `Limits` ceiling on the file size.
- **Other formats:** version 0, or any other version, is refused: `UnsupportedFormat { requirement:
  "obsstore format N" }`, exit 20.
- **What counts as obsolete:** a precursor that is **present** in the changelog **and not public**. Both
  successor markers and prune markers make their precursor obsolete.

### 1.3 Hidden

A changeset is **hidden** when it is obsolete **and** it is not an ancestor-or-self of any changeset that
is either:
- non-obsolete, or
- **pinned**. The pinned set is:
  - bookmark targets;
  - the working-directory parents (the first two 20-byte nodes of `.hg/dirstate`, ignoring the null
    node);
  - nodes named in `.hg/localtags`;
  - nodes named in `.hgtags`, read from each non-obsolete head's manifest, where later lines override.

This is Mercurial's `repoview.computehidden`.

### 1.4 Served

**Served** = all changesets − hidden − phase ≥ 2.
- Only served changesets become atoms.
- Named-branch heads are computed over the served set.
- A served changeset cannot have an unserved parent, by construction. Assert it: a violation is
  `Error::Read("published view is not closed under ancestry")`.

**Recorded** (omit each when zero):

| Record | Class | Reason |
|---|---|---|
| `changesets not published: secret or archived (N)` | `Other` | *"not published by the source repository (phase), so not imported: brygge imports what the repository would publish"* |
| `hidden (obsolete) changesets (N)` | `Other` | *"rewritten or pruned by the source's own history editing (obsolescence markers); not part of the published history"* |

The existing `obsolescence markers` drop (`AdvisoryUnreliable`) stays.

**Faithfulness statement:** append to the hg text in `crates/brygge/src/commands.rs`:
> ` Secret and hidden (obsolete) changesets are not imported: brygge imports what the repository would
> publish. Tags are carried as the .hgtags file, not as refs.`

## 2. Copies with their true source (RFC 011 D-6)

For a filelog revision carrying `copy: <from>` and `copyrev: <filenode>`, find the atom in which `from` has
exactly that file revision:
1. **p1**, if p1's manifest maps `from` to that filenode. This is the ordinary case (`hg cp`/`hg mv`
   copy from the working-directory parent). **It must win**, so `ChangeAtom::is_move` is true for an
   `hg mv` (from_atom = the first parent, and `from` is deleted).
2. Else **p2**, under the same test. This covers copying from the other side of a merge.
3. Else the changeset named by the source revision's **linkrev** in `from`'s filelog, if it is served
   and an ancestor of the current atom.
4. Else walk the current atom's first-parent ancestry, nearest first, for a served changeset whose
   manifest maps `from` to that filenode.
5. If none is found, the copy cannot be placed. **Omit** that `CopyRecord` and count it:
   `copy sources not resolvable (N)`, class `Other`. A stated copy is never placed on a guessed atom.

## 3. Claims and extras (CR-04, CR-06)

- **Claims:**
  - `user` → `author`: the name and email parsed as today, both as `Text` bytes, with the email absent
    when there is none;
  - `date` → `author_time`;
  - **`committer` and `commit_time` are absent.**
- **Timezone:** Mercurial stores seconds **west** of UTC, so `offset_minutes = -(tz / 60)`. If
  `tz % 60 != 0`, or the result does not fit `i16`, the offset is `None`, counted as `unrepresentable
  timezone offsets (N)`, class `Other`.
- **Extras:** every changeset extra except `branch` becomes an `Extra { label: <key>, bytes: <value> }`
  on the atom's `SourceIdentity`, in the order stored. This carries `close=1`, so a closed branch's head
  says so in the object. Document that meaning in the crate README.
- **Bookmarks** naming a changeset that is unknown or not served: count them, `bookmarks naming
  unimported changesets (N)`, class `Other`. Never skip one silently.

## 4. Non-change scope

- The IR contract.
- The revlog reader.
- The ceilings (handoff 3), except the new obsstore-size limit.
- Rename *inference* (not offered for hg).

## 5. Required tests (hg-dependent tests skip when `hg` is absent)

1. **Secret:** `hg phase --secret --force` on a changeset and its child. Neither becomes an atom, and the
   count is 2.
2. **Hidden:**
   - with evolve or the core `experimental.evolution=createmarkers`, amend a commit;
   - the pre-amend changeset is not imported and is counted;
   - the published head is the amended one;
   - **no extra named-branch head appears.**
3. **Orphan stays visible:** an obsolete changeset with a non-obsolete descendant is imported. It is
   visible, as in `hg log`.
4. **Pinned stays visible:** an obsolete changeset that a bookmark points at is imported.
5. **Parity with Mercurial:** for fixtures 1–4, the imported node set equals
   `hg log -r 'not secret()' -T '{node}\n'`. This is the acceptance oracle. `hg log`'s default view
   already filters hidden, archived and internal changesets, so this is the served set.
6. **Obsstore:**
   - version 0 → refused, exit 20;
   - a truncated or oversize-declared record → a typed error, not a panic;
   - over the size `Limits` → `ResourceLimit`.
7. **Copies:**
   - `hg mv a b` gives `from_atom` = p1, and `is_move` is true;
   - in a merge, copying a file that exists only on p2's side gives `from_atom` = p2;
   - a fixture forcing the fallback (step 3 or 4) resolves correctly;
   - an unresolvable fixture is omitted and counted.
8. **Claims:** `committer` and `commit_time` are absent; a `+0900` commit (tz `-32400`) gives
   `offset_minutes == Some(540)`.
9. **Extras:** `hg commit --close-branch` carries `Extra { label: "close", bytes: "1" }`.
10. **Determinism:** decoding twice gives identical bytes, with secret and obsolete changesets present.

## 6. Security gate

The obsstore reader and the phaseroots and dirstate reads are new untrusted-input parsers (T-2/T-8), so
the architect reviews them against `brygge-03`. They are bounds-checked, panic-free, and size-limited
through `Limits`.

## 7. Review request

File `.git-exclude/review-request/009-hg-corrections.md` with:
- the standard sections;
- the §5.5 parity output for each fixture: our node set next to `hg log`'s.
