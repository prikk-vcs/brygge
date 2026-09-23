# Handoff — input ceilings that actually bound, and the hg format gate

**Governing RFC:** RFC 010 (bounded memory), Accepted; this handoff inherits that state. Its **D-4** says:
"Bounds refuse rather than exhaust (T-8), and that already ships." This handoff makes D-4 true.

**Threat model:** `brygge-03` C-2d requires declared ceilings on object size, count, path depth and
decompression ratio, and says a hit is a recorded refusal, never an OOM or a hang.

**Corrections carried** (numbered in the architect's intake review; this handoff defines them durably):

| ID | What |
|---|---|
| CR-10 | The SVN and CVS size caps are checked *after* the whole input is in memory. Git has no caps. `walk_tree` recurses to an attacker-chosen depth, so a deep tree can overflow the stack, and a stack overflow aborts the process where the CR-16 panic boundary cannot catch it |
| CR-17 | hg revlog decompression has no output bound (a decompression bomb) |
| CR-15 | hg format-gate robustness |

**Order:** handoff **3 of 4**. It touches the four decoders, plus one mapping in
`crates/brygge/src/commands.rs`: build it **after** handoff 1 has been approved, to avoid conflicting edits.

---

## 1. Principle

A ceiling is enforced **before** the memory it protects is allocated. Hitting one is a **refusal**:
- a typed `Error::ResourceLimit { what, ceiling }` naming what exceeded and the ceiling with its unit
  (§2.6);
- exit **20**, the same class as any floor refusal, because brygge refused rather than exhausted;
- never a panic, an OOM, a stack overflow or a hang.

Each crate keeps its ceilings in one private `Limits` struct with documented defaults. Tests may construct
it with small values (`pub(crate)`, not public API), so no test needs gigabytes.

## 2. Change scope

### 2.1 Subversion (`crates/brygge-decode-svn/src/source.rs`)

- **Dumpfile:** check the file's metadata length against `MAX_DUMP_BYTES` *before* reading. Then read
  through `take(MAX_DUMP_BYTES + 1)`, in case the file grows while being read.
- **`svnadmin dump`:**
  - spawn with piped stdout and stderr (the fixed argv is unchanged);
  - read stdout through `take(MAX_DUMP_BYTES + 1)`. On overflow, kill the child, wait for it, and refuse;
  - read stderr on a separate thread, **keeping** only the first 64 KiB but **draining** it to EOF
    (discarding the rest), so the child is never starved, killed or deadlocked by brygge's reader
    *(amended after review 005, R-1)*;
  - a non-zero exit is `Error::Open` with the (neutralized, capped) stderr, as today.

### 2.2 CVS (`crates/brygge-decode-cvs/src/scan.rs`, `rcs.rs`)

- Check each `,v` file's metadata length against `MAX_RCS_BYTES` before reading, and read through
  `take(MAX_RCS_BYTES + 1)`.
- `MAX_FILES` is unchanged.

### 2.3 Mercurial (`crates/brygge-decode-hg/src/revlog.rs`) — CR-17

1. **Parse the index's uncompressed-length field** (the full revision text length, bytes 12–15 of each
   64-byte entry) into `Entry`.
2. **Decompress through a bound:** at most `MAX_REVISION_BYTES` (default 1 GiB) per chunk, for zlib and
   zstd alike (`take(limit + 1)` on the decoder reader). An overflowing chunk is `ResourceLimit`.
3. **Every reconstructed text in a delta chain must equal its revision's recorded uncompressed length,**
   the snapshot and each intermediate included. A mismatch is `Error::Read("revision length does not
   match the index")`: a malformed store, not a ceiling.
4. **`mpatch` output** is bounded by the same `MAX_REVISION_BYTES`.

### 2.4 Mercurial format gate and identity (`crates/brygge-decode-hg/src/decode.rs`, `fncache.rs`, `changelog.rs`) — CR-15

1. **`.hg/requires` and `.hg/store/requires`:** only `NotFound` means "no requirements". Any other read
   error is `Error::Open`. Today a permission error silently disables the format gate.
2. **`repo_id`** is the **smallest root changeset node** (Git parity), not revision 0. Revision numbering
   depends on pull order, so two clones of one repository must get the same `repo_id`.
3. **Refusals are classified as refusals (exit 20), not `Read` failures (exit 1):**
   - a path needing the hashed `dh/` store encoding → `UnsupportedFormat { requirement: "hashed store
     path", … }`;
   - non-UTF-8 changeset metadata → `UnsupportedFormat { requirement: "non-UTF-8 changeset metadata", … }`,
     with a message saying it will be carried byte-exact once the IR contract re-cut (RFC 011) lands.

### 2.5 Git (`crates/brygge-decode-git/src/`)

1. **Add** `Error::ResourceLimit { what: String, ceiling: String }` (§2.6). The enum is
   `#[non_exhaustive]`, so this is additive.
2. **Ceilings, with their defaults:**

   | Ceiling | Default | Checked |
   |---|---|---|
   | `MAX_BLOB_BYTES` | 1 GiB | from the object **header** (e.g. gix's header lookup, which gives the size without inflating) *before* reading the blob |
   | `MAX_COMMITS` | 10,000,000 | during the walk, before building atoms |
   | `MAX_PATH_BYTES` | 4,096 | per path, while walking trees |
   | `MAX_TREE_DEPTH` | 256 | per nesting level, while walking trees |

3. **`walk_tree` becomes iterative** (an explicit stack), and `MAX_TREE_DEPTH` is enforced on it.
   - Recursion on attacker-controlled depth risks a stack overflow, which aborts the process: no
     `catch_unwind` can contain it, so CR-16's panic boundary would not help.
   - The iterative walk must produce the identical snapshot, so the determinism tests hold unchanged.

### 2.6 CLI mapping (`crates/brygge/src/commands.rs`)

`ResourceLimit` from **every** decoder maps to exit **20**, with the message
`refused: <what> exceeds brygge's ceiling (<ceiling>)`. Today it falls through to exit 1.
*(Amended after review 005, R-2.)* Every decoder's variant is
`ResourceLimit { what: String, ceiling: String }`: `what` is a noun phrase ("a blob"), and `ceiling`
is the value with its unit ("1073741824 bytes").

### 2.7 Documentation

Each decoder's `README.md` gains a short **Ceilings** section listing its limits and their values. The
threat model's C-2d text is updated at the 0.1.0 revision; cite this handoff.

## 3. Non-change scope

- Output bytes for any input under the ceilings: artifacts stay byte-identical. The existing determinism
  tests must pass unchanged.
- The streaming work of RFC 010 increments 2–4 (0.2.0). This handoff bounds, it does not stream.

## 4. Required tests

Each ceiling is exercised with a **small injected limit** through `Limits`, not with gigabytes:

1. **SVN:**
   - a dumpfile larger than the limit is refused **without being read**. Prove it with a sparse file
     (`set_len` past the limit) and a limit below it;
   - an over-limit `svnadmin` stdout is refused (skip if `svnadmin` is absent);
   - a child writing a lot of stderr does not deadlock.
2. **CVS:** an over-limit `,v` (sparse) is refused before reading.
3. **hg:**
   - a crafted zlib chunk that inflates past the limit is refused;
   - the same for zstd;
   - a snapshot whose inflated length differs from the index field → `Read` error;
   - a normal repository still decodes byte-identically.
4. **hg format gate:**
   - an unreadable `requires` (permission 000, Unix only) → `Open`, not success;
   - two clones pulled in different orders give the same `repo_id` (hg-dependent; skip if absent);
   - a path needing `dh/` → exit 20;
   - non-UTF-8 metadata → exit 20.
5. **Git:**
   - a blob over the injected `MAX_BLOB_BYTES` → refused, and the blob bytes are never read (assert on a
     counter or trace in a test build);
   - a tree 300 levels deep (built with `git mktree`) → refused by depth, **and** a tree 200 levels deep
     decodes (the iterative walk works);
   - a path over the injected `MAX_PATH_BYTES` → refused;
   - the commit cap with an injected small value → refused.
6. **CLI:** a `ResourceLimit` from each decoder exits 20 with the stated message.

## 5. Acceptance criteria

- Every ceiling is checked before allocation, is a typed refusal (exit 20), and is documented.
- No attacker-controlled recursion remains in the Git decoder.
- hg decompression cannot exceed its bound, and every reconstructed revision is length-checked against the
  index.
- The five gates pass, `--locked`; `Cargo.lock` unchanged.

## 6. Prohibited shortcuts

- No "read, then check the length".
- No recursion kept "because depth is usually small".
- No ceiling hardcoded twice. `Limits` is the one place.

## 7. Security gate

This touches untrusted-input paths in all four decoders, so the architect reviews it against `brygge-03`
(T-2/T-8, C-2a/C-2d). The threat model is updated at the 0.1.0 revision.

## 8. Review request

File `.git-exclude/review-request/005-input-ceilings.md` with the standard sections, plus a table of every
ceiling: crate, name, default, where it is checked (file:line), and the test that exercises it.
