# Handoff — 0.1.0 release preparation: hg node verification, one floor vocabulary, user-facing text

**Governing:** the 0.1.0 release plan (`ROADMAP.md`, authorized by the owner), and the RFCs each item
names. This handoff is cross-cutting, so it is filed next to the project-hygiene handoff it continues.

**Why now.** The architect's closure audit of the intake corrections at `5df4241` found every correction
closed except the items below, which are small but user-visible or security-relevant. The threat model
v0.3 (`docs/src/brygge-03-threat-model-v0.1.md`) already states two of them as 0.1.0 exit items:
- C-2f's Mercurial half;
- RR-hg-node-unverified.

**Order:** one batch. Items are independent; build them in any order, and file one review request.

---

## 1. Verify Mercurial nodes against their content (threat model C-2f; RFC 005)

**The gap.**
- brygge preserves every hg node (changeset, manifest, filelog) as the source's identifier (PR-4), but
  never checks that the node hashes to the revision's content.
- A crafted store can therefore present content under a node it does not hash to, and the IR would carry
  a false link. `verify --against-source` re-reads the same store, so it cannot catch this.
- Mercurial itself checks every revision's hash on read, so brygge is weaker than `hg` on a hostile
  store. The Git decoder closed the same gap in batch 2.

**Change.**
- For **every revision brygge reads** (changelog, manifest, filelog), compute Mercurial's revision hash
  over the **raw text**: SHA-1 of `min(p1, p2) ‖ max(p1, p2) ‖ rawtext`, with the parents as 20-byte
  nodes and null as 20 zero bytes. Compare it with the entry's node.
  - This is `mercurial/utils/storageutil.py` `hashrevisionsha1` (lines 39-62 in Mercurial 7.2.4). When
    p2 is null it hashes `nullid ‖ p1 ‖ text`, which the min/max rule already gives, because null is all
    zeros. Cite the file and line in the review request.
  - The raw text includes a filelog revision's `\1\n…\1\n` copy-metadata header, exactly as stored.
- **A mismatch** is `Error::Read("revision <node hex> does not match its content (corrupt or crafted
  store)")`. The decode stops.
- **Hash with `sha1-checked`**, which detects SHA-1 collision attacks. Mercurial itself uses a
  collision-detecting SHA-1 (sha1dc, `mercurial/utils/hashutil.py:6-8`).
  - `sha1-checked 0.10.0` is already in `Cargo.lock` (through `gix-hash`), so this is a new **edge**,
    not a new crate.
  - The architect accepts that edge under RFC 009, as with CVS's `sha2`. Add it to `[workspace.dependencies]`
    at the locked version.
  - A detected collision is a `Read` error that says so.
- **Revision flags.** Verification applies to revisions whose stored text **is** the hashed text.
  - Censored revisions are already refused (`censored revision`).
  - For every other revision flag Mercurial defines (`revlogutils/constants.py:240-249`: ellipsis,
    external storage, has-copies-info, has-meta, and any sidedata flag), state from Mercurial's source whether the hash still holds over the raw text:
    - if it holds, verify as normal;
    - if it does not, refuse, with a floor feature named for it;
    - if the flag is unknown, refuse.
  - Report the table, flag by flag, with citations.
- **Where to hook it.** Hook into the one place a revision's full text is reconstructed (after the delta
  chain and the length check from handoff 3), so that no read path bypasses verification.
  - State in the review request that this is the only such place.
- **Cost.** One SHA-1 per revision read, which Mercurial also pays. Report the time on the largest
  fixture before and after.

**Tests.**
1. A store whose filelog revision text is altered without updating its node: write a tiny revlog by hand,
   or patch a fixture's `.i`/`.d` bytes. It is refused, naming the node.
2. The same for a manifest revision, and for a changeset revision.
3. Every existing hg fixture still decodes byte-identically (the parity and determinism tests are
   unchanged).
4. A filelog revision with copy metadata verifies, which proves the header is hashed.

## 2. hg: a non-UTF-8 path is a named refusal (CR-03, owner ruling D-3)

- **Today** a non-UTF-8 manifest path is `Error::Read` (exit 1, `manifest.rs:46-47`). Ruling D-3 makes
  it a **refusal** (exit 20), as Git and CVS already do.
- **Change.** Add floor feature `non-utf8-path` to the hg floor, refusing with the path shown as
  `\xNN`. Add a test.
- **SVN keeps `Read`.** A dumpstream's paths are UTF-8 by the format's own definition, so a non-UTF-8
  path there is a malformed dump, not a repository shape. Make sure its message shows `\xNN`, and state
  this in the SVN README.

## 3. One floor vocabulary across the four decoders

**The problem.** The same concept is spelled two ways in `params["floor"]` and in refusal messages: Git
and hg use phrases (`"non-UTF-8 path"`, `"shallow clone"`), while SVN and CVS use identifiers
(`"non-utf8-path"`, `"remote-source"`). A user or a CI gate matching on a refusal would have to know both.
The owner's rule is one name per concept.

**Rule (architect decision, v0):**
- A floor feature is an **identifier**: lowercase ASCII, kebab-case, stable. The **same concept uses the
  same identifier in every decoder**: `non-utf8-path` in Git, hg and CVS, and `remote-source` in SVN and
  CVS.
- The human refusal message carries the explanation and the remedy. The identifier is what machine
  output and provenance carry.
- This supersedes the phrase-form names written in earlier handoffs:
  - "SHA-256 object format" → `sha256-object-format`;
  - "unfinished merge" → `unfinished-merge`;
  - "non-UTF-8 extra key" → `non-utf8-extra-key`;
  - "non-UTF-8 commit header name" → `non-utf8-commit-header-name`;
  - and so on for every constant in each `floor.rs`.

**Change.**
- Rename the Git and hg constants' values to the rule. SVN and CVS already conform.
- Each decoder's README gets one **Floor** table: identifier, what is refused, and what the user can do.
  - The Git README table replaces the stale list at `:55-56`, adding alternates, the redirected git
    directory, the non-UTF-8 names and SHA-256.
- A workspace test asserts every floor identifier in every `floor::ALL` matches `^[a-z0-9]+(-[a-z0-9]+)*$`.
- **Breaking:** `params["floor"]` changes in every Git and hg artifact. Record it in the CHANGELOG.

## 4. User-facing text carries no internal references

Artifact text (drop `reason`s, flag `reason`s, refusal messages) and CLI help are read by users and by
prikk, who have no access to our RFC, CR, PR, INV, NG, SRC, FS or VF numbering.

**Change.**
- In every decoder and in `brygge`, remove internal identifiers from any string that reaches an
  artifact, a refusal message or `--help`. Replace them with plain words where the reference carried
  meaning. Known sites include:
  - SVN `decode.rs:176, 300-301, 311, 320, 330`, `props.rs:89`, `source.rs:137`;
  - `--help`'s "(FS-02)" and "(VF-2)".
  - Search all four decoders and `brygge` for the rest.
- **Stale phrases:**
  - "the frozen IR" (SVN `decode.rs:320`) → "the IR contract";
  - the Git `decode.rs:433` comment "lossy until RFC 011";
  - search for others ("frozen", "1.0.0", "Tier").
- **`--help`'s VOCABULARY line** lacks **flagged**, which `brygge-02` defines. Add it.
- Code comments and rustdoc may keep internal references; only user-facing strings change.
- **A test** that scans every `DropRecord`/`Flag` `reason` and `what` produced by the test fixtures of
  all four decoders, and fails on `\b(RFC|CR|PR|INV|NG|SRC|FS|VF|HO|CL|CT)-?[0-9]`.
- **Breaking:** artifact reasons change. Record it in the CHANGELOG.

## 5. The atomic write refuses an existing temporary path

- **Today** `atomic_write` (`commands.rs:393-422`) opens `.<name>.brygge-tmp-<pid>` with `File::create`,
  which follows a symlink and truncates an existing file.
- The output directory is the operator's, so this is not a TB-1 threat. But a write that could land
  through a planted symlink is the pattern C-7 exists to exclude.
- **Change:**
  - open with `OpenOptions::new().write(true).create_new(true)`, so an existing file or symlink is an
    error, never followed;
  - build the name from the `OsStr` file name, not `to_string_lossy`.
- **Test:** a pre-planted symlink at the temporary path makes the decode fail, and the symlink's target
  is untouched.

## 6. CI runs the link check

`tools/check-links.sh` is a project gate (CR-14) but is not in `.github/workflows/ci.yml`. Add it to the
`gates` job.

## 7. Non-change scope

- The IR contract.
- Decoder behaviour, apart from §1's verification, §2's refusal and §3's identifiers.
- Ceilings.
- Machine-output keys: the architect documents them in `docs/src/reference/machine-output.md` from the
  code as it stands. Do not change them in this batch.

## 8. Security gate

- §1 is an integrity control (T-3; C-2f at the source end), so the architect reviews it against
  `brygge-03` v0.3. When it lands, RR-hg-node-unverified closes.
- §5 hardens C-7.

## 9. Review request

File `.git-exclude/review-request/012-release-prep.md` with the standard sections, plus:
- the §1 revision-flag table and the before/after timing;
- the full old → new floor identifier map (§3);
- the list of text sites changed (§4).
