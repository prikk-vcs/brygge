# Handoff — RUSTSEC-2026-0344: upgrade `ruzstd`, and make the zstd ceiling true (security fix)

**Governing:**
- RFC 009 D-6 (supply-chain gates; an advisory fails the build);
- the threat model `brygge-03` T-8, C-2d and C-4d;
- review 047 (the architect confirmed the gap by reading `ruzstd` 0.7.3).

**Crate:** `brygge-decode-hg`. One review request. **Priority: first;** `main`'s supply-chain gate is red
until this lands.

## The defect, as confirmed

- **`ruzstd` < 0.9.1** never bounded a compressed block's *decompressed* size.
  `decoding/sequence_execution.rs::execute_sequences` appends every sequence of a block into
  `scratch.buffer` (literals pushed, matches `repeat(offset, ml)`), with no running bound.
- **The whole block runs inside one `read()`,** before it returns. So brygge's `.take(max_revision_bytes +
  1)` in `revlog.rs::decompress` (marker `0x28`) cannot stop it.
- **The impact:** a crafted zstd chunk of under 128 KB, in a hostile Mercurial repository, can allocate
  gigabytes and kill brygge (out of memory). That is a denial of service, never memory corruption.
- **Since when:** every release from 0.1.0 to 0.3.0.
- **The zlib path (`b'x'`) is not affected:** `flate2` inflates into the caller's buffer, so `.take()`
  really bounds it.

## The fix

1. **Upgrade:** `ruzstd = "0.9"` in `[workspace.dependencies]` (the lock at ≥ 0.9.1), with
   `cargo update -p ruzstd`.
   - Adapt the call site if the 0.9 API differs, and keep the shape: decoder → `.take(max + 1)` → the
     ceiling check.
   - Read the 0.9.1 changelog. Report any other behaviour change that reaches brygge, in the request.
2. **The window ceiling.** A zstd frame declares its window size, and a decoder may allocate a window that
   large.
   - Confirm what 0.9.1 does with a frame that declares a huge window: is there a default maximum, and what
     is it?
   - Make brygge's limit explicit at the call site: a window over `max_revision_bytes`, or over a stated zstd
     window ceiling if 0.9 offers a setter, is a typed `ResourceLimit`, never an allocation.
   - State the effective bound in the request, and in the crate README's ceilings list (C-2d's "each decoder
     keeps its ceilings in one place").
3. **Errors:** every failure from the decoder (an oversized block, a bad frame) is `Read` or `ResourceLimit`,
   naming the revision. A panic from it is caught by the existing panic boundary; say whether any remains
   reachable in 0.9.1.

## Tests

1. **The bomb is refused.** Use a crafted frame whose single block decompresses to more than 128 KiB.
   - **Source:** if the upstream fix commit (`d3907c9`) carries its regression input, use it (MIT; attribute
     it in the test). Otherwise build one with predefined-mode sequences.
   - **Assert:** `decompress` returns a typed error, quickly. Put it behind a test that runs everywhere.
2. **A frame declaring an enormous window** gives a typed error, not an allocation.
3. **Before and after, as evidence (local only, not CI):**
   - run test 1's input through the **old** build under a memory cap (`ulimit -v`, about 2 GiB), and through
     the new one;
   - report both outcomes. Expected: the old one is killed or aborts on allocation, and the new one returns
     the error.
   - This is the evidence for the threat model's wording. Do not commit anything that runs the old build.
4. **Regression:** every existing hg fixture with zstd chunks (and the rest) decodes byte-identically
   between `c6f9348` and this tree (two builds, compared with `cmp`). Say how many of the fixtures use
   `revlog-compression-zstd`. If none does, add one: a real `hg` repository with `format.revlog-compression=zstd`.
5. **The gates:** `cargo audit` and `cargo deny` are clean, and CI's supply-chain job is green.

## Also, in the same request (an audit, no change expected)

List every other place where brygge bounds an untrusted decompression or expansion only from outside the
callee, and say for each whether the callee's internal allocation is bounded:
- hg zlib (`flate2`);
- the CVS RCS delta application;
- the SVN svndiff reader (brygge's own code);
- Git through `gix` (the object header size check, before the body is read).

The architect expects only zstd to be exposed. Confirm or correct that.

## CHANGELOG `[Unreleased]`

A **Security** heading, above the others:

> "Mercurial: a crafted zstd-compressed revlog chunk could exhaust memory (RUSTSEC-2026-0344 in `ruzstd`
> before 0.9.1; brygge 0.1.0–0.3.0). `ruzstd` is upgraded, and every zstd frame's block and window sizes
> are bounded before allocation."

## Review request

`.git-exclude/review-requests/035-ruzstd-security-fix.md`. Do not edit `brygge-03`: the architect corrects
the threat model (the C-2d claim was false for zstd until this fix) in the release that ships it. After
approval, commit and push to `main`, and append CI (the supply-chain job green).
