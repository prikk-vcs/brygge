# Handoff — RFC 013 D-1: Mercurial's store path encoding, complete (`_hybridencode`, with `dh/`)

**Governing:** RFC 013 D-1 (accepted 2026-09-24). **Crate:** `brygge-decode-hg`. **Batch H** of 0.3.0,
independent of batch S. One review request.

**Today:**
- `fncache::store_path` refuses a path whose encoded name exceeds 120 characters (`UnsupportedFormat`,
  "hashed store path").
- The reversible encoding it implements is also **incomplete**. The architect ran Mercurial 7.2.4's own
  `store._hybridencode` against brygge's rules:

  | Logical path | Mercurial's store name | brygge computes |
  |---|---|---|
  | `aux/x` | `data/au~78/x.i` | `data/aux/x.i` |
  | `con.txt` | `data/co~6e.txt.i` | `data/con.txt.i` |
  | `com1` | `data/co~6d1.i` | `data/com1.i` |
  | `dir.i/f` | `data/dir.i.hg/f.i` | `data/dir.i/f.i` |
  | `.hg/x` | `data/~2ehg.hg/x.i` | `data/~2ehg/x.i` |
  | `a./b` | `data/a~2e/b.i` | `data/a./b.i` |
  | `a /b` | `data/a~20/b.i` | `data/a /b.i` |

  Today these fail safely (`Read`: the file is absent; C-2f would catch any wrong file). Still, such
  repositories cannot be decoded.
- **`Revlog::open` derives the `.d` path from the `.i` path** (`revlog.rs:156`, `with_extension("d")`).
  That is right for reversible names only: a hashed name embeds the SHA-1 of its **own** path, so the `.i`
  and `.d` names differ in more than the extension. For one long path, Mercurial gives:
  - `dh/director/…/bbbbbbbbbbbbf41c68e34dc8fe94918b7918fc6654a1a2ac51fb.i`;
  - `dh/director/…/bbbbbbbbbbbb146b1bac56118baacce45ebcb896c102a0a9a489.d`.

**The change:** implement Mercurial's `_hybridencode` exactly, and open both revlog files by their own
encoded names.

---

## 1. The encoding (`mercurial/store.py`, 7.2.4 — cite the lines in the review request)

For a store file path `data/<logical><ext>`, where the extension is `.i` or `.d`:

1. **`encodedir`:** a directory component ending in `.hg`, `.i` or `.d` gets `.hg` appended (`dir.i/` →
   `dir.i.hg/`). It is a C builtin in Mercurial; use its documented rule and the table above.
2. **`encodefilename`:** the reversible per-byte encoding brygge already has (`A`–`Z` → `_` + lowercase,
   `_` → `__`, reserved/control/high bytes → `~xx`).
3. **`_auxencode`** on each component of the result, with `dotencode` (§2):
   - with `dotencode`, a leading `.` or space → `~2e` / `~20`;
   - **otherwise**, when the part before the first `.` is exactly `aux`, `con`, `prn` or `nul`, or is `com`
     or `lpt` followed by one digit `1`–`9`, the **third** character becomes `~xx`;
   - then, in every case, a trailing `.` or space → `~2e` / `~20`.
4. If the joined result is **at most 120** characters, it is the store name. Otherwise, use **`_hashencode`
   on the path from step 1** (after `encodedir`, before steps 2–3):
   - `digest` = SHA-1 hex of that whole path (`data/…<ext>`);
   - `lowerencode` the path after `data/`: `A`–`Z` → plain lowercase; reserved, control and high bytes →
     `~xx`. **`_` is not doubled, and there is no `_` prefix**; it is not step 2;
   - `_auxencode` the components (as in step 3);
   - **the directories:** the first 8 characters of each directory component, a trailing `.` or space
     replaced by `_`. Joined with `/`, stop before the total would exceed 68;
   - `dh/` + dirs (+ `/` if any) + digest + ext. If that is shorter than 120, insert
     `basename[:space left]` before the digest, where basename is the encoded last component, extension
     included.
   - Compute SHA-1 with `sha1-checked` (already in this crate). A detected collision is `Read`.
- **The API:** `store_path` returns the store names of **both** the `.i` and the `.d` files, each encoded
  from its own path. `Revlog::open` takes both, and no longer derives one from the other.
- **A computed name whose file is absent** is `Error::Read("filelog for <path> not found at <store
  name>")`, never a skipped file. That rule is unchanged for `.i`; the same holds for a non-inline
  revlog's `.d`.

## 2. Which encoding a repository uses (`.hg/requires`)

- **`fncache` and `dotencode`** (every repository since Mercurial 1.7): the above, with `dotencode`.
- **`fncache` without `dotencode`:** the above, **without** the leading dot/space rule of step 3.
  Mercurial passes the flag; so does brygge, from the requirements already read by the gate.
- **`store` without `fncache`** (before Mercurial 1.1, 2008) uses a different encoding. **Refuse it by
  name** (`UnsupportedFormat`, requirement `store-without-fncache`). Today brygge assumes fncache there.

## 3. What else changes

- The "hashed store path" refusal and its error text are removed, from the code, the README and
  `docs/src/guide/hg.md`.
- **Nothing else changes:** the ceilings, node verification (C-2f, unchanged for these filelogs), the
  published view, and the output of every repository that decodes today.

## 4. Tests

1. **Against Mercurial's own encoder:** for a table of paths, `store_path` equals
   `mercurial.store._hybridencode(path, dotencode)`, for both `.i` and `.d`. Generate the expected values
   once with the local `hg`'s Python, and commit them as a fixed table in the test; the test itself does
   not need Python. The table:
   - every row of the table above;
   - `Aux.txt` (not reserved after step 2: `data/_aux.txt.i`);
   - `auxx`; `nul`; `lpt9.c`; `x.`; `x ` (a final component is not trailing: `data/x..i`);
   - uppercase, `_`, `~` and non-ASCII in long paths;
   - a long path with many deep directories (the 68-character cut);
   - a long basename (the filler), including one just at 120 and one just over;
   - a directory prefix ending in `.` or space after the 8-character cut;
   - each of these with `dotencode` false.
2. **End to end, real `hg`:** a repository with a long path (non-inline, so a `.d` exists: the content must
   exceed Mercurial's inline threshold, 128 KiB) and a short inline one, plus a file under `dir.i/` and one
   named `aux`. It decodes, and each file's content and history match `hg cat` and `hg log`.
3. **Regression:** every existing hg fixture decodes byte-identically (two builds, compared with `cmp`).
4. **The absent file:** removing a hashed `.i` gives the `Read` error naming the path. Removing a
   non-inline `.d` does the same.
5. **`store` without `fncache`:** a `requires` with `store` only is refused by name.

## 5. CHANGELOG and docs

- **CHANGELOG `[Unreleased]`, Added:** "Mercurial: paths stored under hashed `dh/` names (very long paths)
  are read, not refused."
- **Fixed:** "Mercurial: paths with a Windows-reserved component (`aux`, `con`, `com1`, …), a directory
  named `*.i`, `*.d` or `*.hg`, or a directory ending in `.` or space are read."
- **Changed:** "a `store` repository without `fncache` (before Mercurial 1.1) is refused by name."
- `docs/src/guide/hg.md` and the hg README: remove the hashed-path refusal, and add the pre-1.1 refusal.

## 6. Review request

`.git-exclude/review-request/028-hg-store-path-encoding.md`, with the `store.py` line citations and the
generated table. After approval, commit and push, and append CI.
