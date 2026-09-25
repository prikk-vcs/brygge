# RFC 013 — Source reach (0.3.0): hashed Mercurial store paths, SVN delta dumps, CVS branch history

**Status.** **Done (2026-09-25): implemented in 0.3.0.** **Accepted (2026-09-24)** by the owner ("013 is accepted"). The owner stated no other answer
to OQ-1…OQ-4, so they are recorded as the recommendations, as for RFC 011's OQ-A; the owner can object.
- **OQ-1:** CVS branches are imported with `--reconstruct-refs`.
- **OQ-2:** unnamed branches are still dropped and counted.
- **OQ-3:** adaptive windows are deferred.
- **OQ-4:** svndiff 1 and 2 are refused by name.

Handoffs: `rfcs/handoffs/013-source-reach/`.

**Amended (2026-09-24, the architect, while writing the handoffs):**
- **D-1 made precise** against Mercurial 7.2.4's own `store._hybridencode`:
  - the digest is taken over the path **after `encodedir`**;
  - the `.d` file is hashed from its own path, so `Revlog::open` must not derive it from the `.i`;
  - brygge's **reversible** encoding also lacks `encodedir` and `_auxencode`'s reserved names and trailing
    `.`/space, and D-1 completes it. Today such paths fail safely (`Read`); they will be read;
  - a `store` repository without `fncache` (before Mercurial 1.1) is refused by name.
- **OQ-5 ruled (2026-09-25, owner): the `md-5` crate** (RustCrypto), as recommended.
- **D-3 made precise** (2026-09-24, while writing the C handoff):
  - **the parent is the *earliest* covering changeset, not the latest.** Every covered file's branch point is
    present there. A later changeset that only adds a file the branch does not carry is evidence the cut
    preceded it. "Latest" could also place a parent after the branch's first commit. The param is
    `branch_point_rule = earliest-covering-changeset`;
  - **a branch's tree is what `cvs checkout -r B` gives:** only the files tagged on it, at their branch
    points. Where the parent's tree differs (an approximate point, a subdirectory branch, files `B` does not
    carry), a **branch-point atom** of `Derived(ReconstructedBranch)` ops reconciles it. This is cvs2git's
    semantics too;
  - **nested branches:** the parent line is the line of the branch-point revisions.
- **OQ-6 (vendor branches after clearing):** built as recommended (not reconstructed; counted with an exact record); design and implementation deferred until a real repository needs it.

## Summary

0.3.0 lifts three refusals and one limit that 0.1.x and 0.2.0 kept on purpose:

1. **Mercurial hashed store paths** (`dh/`). A tracked path long enough that Mercurial stores its filelog
   under a hashed name is refused today (`UnsupportedFormat`, "hashed store path"). **→ Read it.**
2. **SVN delta dumps** (`svnadmin dump --deltas`). These are refused today as an unsupported dump format.
   **→ Read them.** This also closes `RR-svn-special-toggle`.
3. **CVS branch history.** Today only the main line is imported, and branch revisions are dropped with a
   record. **→ Import branches as reconstructed changesets and refs.**

Each lifts a documented user-facing limit, and each is proven with fixtures made by the real tool (`hg`,
`svnadmin`, `cvs`), per the ROADMAP's exit criterion. **No IR contract change is needed:** the IR already
has `ReconstructedChangeset`, `ReconstructedBranch`, atom parents and derivation params.

## Constraints

- **Faithful, or refused with a name.** Every new path is either carried exactly as the source states it,
  or marked `Derived`, with its rule recorded.
- **A byte-identical regression.** Every repository that decodes today decodes to the same artifact,
  apart from the recorded versions. The one exception is CVS repositories with branches: their branch
  revisions stop being dropped. That is the point of item 3, and it is stated as **Breaking** in the
  CHANGELOG.
- **Untrusted input** (threat model T-2/T-8): every new parser is bounds-checked, panic-free and ceilinged
  through `Limits`, and it gets the architect's security review.
- **No new dependency** unless an open question below says otherwise.

## Decisions

### D-1 — Mercurial hashed store paths

- **The rule.** A path's store name is Mercurial's `_hybridencode`: the reversible encoding when it fits
  `_maxstorepathlen` (120), otherwise `_hashencode` (`mercurial/store.py` 360–395):
  - `dh/`;
  - then up to 8 characters of each directory level, while they fit 68 characters, with a trailing `.` or
    space replaced by `_`;
  - then a filler from the basename;
  - then the SHA-1 hex of the full `data/<path>.i`;
  - then the extension.
- **brygge knows every logical path** from the manifest, so it **computes** the hashed name and opens that
  filelog; it never reverses one. SHA-1 comes from `sha1-checked`, already a dependency of this crate.
- **A computed name whose file is absent** is `Error::Read` ("filelog for <path> not found at <store
  name>"), never a skipped file.
- **Proof:** a real `hg` repository with a path over the limit; its node-verified decode (C-2f) is
  identical to what `hg` reports.

### D-2 — SVN delta dumps (svndiff version 0)

- **What the tools emit** (checked by the architect with 1.14.5):
  - `svnadmin dump --deltas` and **`svnrdump dump`** both write dump format version 3, with
    `Text-delta: true` nodes in **svndiff version 0** (`SVN\0`, uncompressed);
  - `svnrdump` also emits `Prop-delta: true`.
  - `svnrdump` is how a user dumps a **remote** repository, and its dumps are always delta dumps.
    brygge refuses them today, although the SVN guide's own remedy for `remote-source` is to run
    `svnrdump` yourself. So D-2 makes that remedy actually work.
- **Text deltas.** Parse svndiff0 windows (source view, target length, instructions: copy from source,
  copy from target, new data) and apply each to the node's **base**:
  - the path's previous content, or its copy source for a node with `Node-copyfrom-*`;
  - empty for a new file without history.
- **Verification** (C-2f for SVN, where the dump provides it):
  - when present, `Text-delta-base-md5` is checked against the base, and `Text-content-md5` (and
    `Text-content-sha1`) against the result;
  - a mismatch is `Error::Read`: the dump or the base is not what it claims.
- **Property deltas.** `Prop-delta: true` means the node's property block changes the previous properties
  (a `D` key deletes one) rather than replacing them. Apply it to the resolved properties.
- **Bounds.** Every window length, instruction offset and length is checked against the source view, the
  target and the data before use. The target length is ceilinged per node by `Limits`, and a delta that
  would read outside its source is `Read`.
- **Verification needs MD5** (OQ-5): `svnrdump` writes `Text-content-md5` and `Text-delta-base-md5` only,
  never SHA-1 (checked with 1.14.5). SHA-1 is checked with `sha1-checked` when present. Checksums are
  verified on fulltext nodes too, as the end-to-end check of delta application. They are consistency,
  not authenticity: the dump is untrusted and can state any checksum.
- **Refused by name:** svndiff version 1 (zlib) and 2 (lz4) are `UnsupportedFormat` (see OQ-4).
- **`RR-svn-special-toggle` closes here.** A property-only change of `svn:special` recomputes the node's
  content from its current base (adding or stripping `link `), since the base is now always at hand.

### D-3 — CVS branch history

- **What a branch is.** A branch is a **branch symbol**: a magic number, or a literal odd-length vendor
  number, together with the branch revisions of every file under it. **Branches are identified by symbol
  name across files, never by number** (numbers are per-file).
- **Changesets on a branch.** A branch's revisions are clustered into `Derived(ReconstructedChangeset)`
  atoms with the same rule as the main line: `(author, log)`, the window, one revision per path, per-file
  order, and `span-overlap-v1` confidence, **within that branch only**.
- **The branch point** is brygge's judgment, and it is marked as such:
  - *(Amended 2026-09-24: the **earliest** covering changeset, on the line of the branch points, with
    `branch_point_rule = earliest-covering-changeset`; see Status. The original text follows.)*
  - the first changeset of a branch has as its parent the **latest main-line changeset at which every file
    on the branch is exactly at its branch-point revision**. That is, the file's branch-point revision is
    in the tree there, and the file's next trunk revision is not yet;
  - the rule is recorded as a derivation param, `branch_point_rule = latest-covering-trunk-changeset`;
  - **when no main-line changeset has every branched file at its branch point** (the branch was cut across
    reconstructed changesets):
    - the parent is the latest main-line changeset with the **most** branched files at their branch points,
      with ties going to the later one;
    - the branch is **flagged** (`ConventionViolation`, "branch point spans reconstructed changesets
    (N files not at their branch point)");
    - the branch changeset's own tree still gives each file its true branch-point content, so no content is
      wrong, only the parent is approximate.
- **Content.** A branch changeset's tree is its parent's tree plus the branch revisions' content. A file
  never changed on the branch keeps its branch-point content. Forward deltas are applied in one pass per
  branch (RFC 010 increment 3's method).
- **Merges.** CVS records none. **No merge parents are ever inferred.** The guide says so, and a
  consumer sees two independent lines.
- **Refs.** A branch symbol becomes a `Derived(ReconstructedBranch)` ref at the branch's last changeset,
  with `source = cvs-symbol`. A tag on branch revisions is reconstructed against branch changesets.
- **The vendor branch.** While it is the default branch, its revisions stay on the main line (RFC 007 as
  corrected). Once cleared, it is an ordinary branch named by its symbol.
- **Records.** Branch revisions and symbols stop being dropped when imported. Whatever still is not
  imported (see OQ-2) keeps an exact drop record.

## Open questions (the owner's)

- **OQ-1 — When are CVS branches imported?**
  - **Recommended:** with `--reconstruct-refs`, as SVN branches are. A branch without its name is not a
    meaningful thing to import. Without the flag, the main line only, as today.
  - **Alternative:** always. Every CVS decode would change, and branch heads would have no refs.
- **OQ-2 — Unnamed branches** (branch revisions with no symbol in any file).
  - **Recommended:** keep dropping them, counted, with the existing record. There is no name for a ref,
    and they are rare leftovers of deleted symbols.
  - **Alternative:** import them as nameless lines of history.
- **OQ-3 — CVS adaptive clustering windows** (RFC 007 OQ-E, listed in the ROADMAP's 0.3.0 row).
  - **Recommended:** defer. There is no measured need yet, and it would introduce a new `confidence_rule`
    (every CVS artifact's derived values would change).
  - **Alternative:** design it in 0.3.0.
- **OQ-4 — svndiff 1 and 2.**
  - **Recommended:** refuse them by name in 0.3.0. `svnadmin` emits version 0, so no known producer needs
    more; zlib would add a dependency edge (`flate2`, already in the workspace), and lz4 a new crate.
  - **Alternative:** support version 1 now.

- **OQ-5 — MD5 for SVN checksums** (added 2026-09-24).
  - **The need:** `svnrdump` dumps, the remote case D-2 exists for, carry MD5 only. Without MD5, no
    delta applied from them is checked end to end. The Constraints say no new dependency unless an open
    question does.
  - **Recommended:** `md-5` 0.10 (RustCrypto, MIT OR Apache-2.0). It is the family of `sha1`/`digest`,
    already in the lockfile, and it adds one crate.
  - **Alternative A:** an in-crate MD5 (~100 lines, checked against RFC 1321's vectors). No dependency,
    but hash code of our own to maintain.
  - **Alternative B:** SHA-1 only. `svnrdump` dumps go unverified.

- **OQ-6 — Vendor branches once cleared** (added 2026-09-24).
  - **The problem:** a literal vendor symbol's revisions are main-line in files where it is still the default,
    and branch revisions where it was cleared. The vendor-import skip also removes its branch point from the
    spine. "Once cleared, an ordinary branch" does not say which line owns them.
  - **Recommended:** in 0.3.0, vendor branches are not reconstructed as lines. Their non-main-line
    revisions stay dropped, with their own exact record, and are revisited when a real repository needs
    it.
  - **Alternative:** reconstruct them as lines, importing the default-branch files' vendor revisions a
    second time on that line. That needs its own coverage special cases.

## Order and proof

- **Order:**
  1. D-1 (hg, small, independent);
  2. D-2 (SVN, medium, independent);
  3. D-3 (CVS, large).

  D-1 and D-2 can run in parallel; D-3 follows its own handoff after this RFC is accepted.
- **Proof, per item:**
  - fixtures from the real tool;
  - a byte-identical regression on every existing fixture;
  - a security review against `brygge-03`, which is revised to v0.6 at the cut.

## Consequences

- Three documented limits disappear from the guides and the floors.
- `RR-svn-special-toggle` closes.
- CVS artifacts with branches change (Breaking, v0).
