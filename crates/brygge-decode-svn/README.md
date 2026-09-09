# brygge-decode-svn

brygge's **Subversion source decoder** (RFC 006, milestone M3). Reads an SVN history from a **dumpstream**
— either a user-supplied dumpfile or a read-only local `svnadmin dump` brygge invokes — and produces a
[`brygge_ir::Ir`].

- **Tier D (RFC 006 D-1):** a pure-Rust dumpstream parser. **No linked SVN library, no FFI, no network.**
  The only external touch is running `svnadmin dump` as a producer subprocess (skipped entirely when a
  dumpfile is supplied). `svnadmin dump` emits a uniform stream from FSFS or BDB, so the on-disk backend is
  never touched.
- **What it carries:** a **`Stated`** linear revision spine; **`Stated`** copies (`copyfrom` →
  `RenameHint`); symlinks and exec bits; an **opt-in, `Derived`** branch/tag layer reconstructed by layout
  convention (off by default).
- **What it refuses (the floor, RFC 006 OQ-B):** `svn:externals`, URL/remote sources, unknown dump-format
  versions, and **delta-format dumps** (svndiff — deferred; re-dump without `--deltas`). A
  convention-violating layout is **imported with a loud `Derived` record**, not refused.
- **What it drops-with-record:** `svn:mergeinfo` (advisory, never a merge parent), `svn:eol-style` /
  `svn:keywords` (the stored normal-form bytes are carried), custom properties, empty directories.

The only crate that reads SVN (RFC 009 D-1); [`brygge_ir`] and `verify --internal` link none of it. It
reads untrusted input, so every parser is bounds-checked and panic-free. See
`rfcs/handoffs/006-subversion-decoder/` for the implementation handoff and the security review.
