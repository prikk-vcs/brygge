# Handoff — RFC 019 refinement: the CVS main line gets a ref (batch G1-C, request 071)

**Governing:** RFC 019, refinement F-3 (review 136), recorded in its status by this handoff's commit.
**After:** batch G1 (on `main` at `bf82ac1`). **On `main`, no topic branch:** a decoder change with no new process
or platform code.

## 0. Evidence plan first (N-3)

- **Level 1:** the decoder's ref set over hand-written `,v` fixtures.
- **Level 2:** decode, then `encode git`, on the existing CVS fixtures.
- **Level 3:** the CVS corpus entries, `openbsd-cpio` and `openbsd-games`, each decoded with `--reconstruct-refs`
  and encoded to Git, with nothing refused.
- **Remains for G4:** the formal corpus row.

## 1. The problem (review 136 F-2/F-3, measured)

- **With `--reconstruct-refs`,** the CVS decoder emits a ref per branch and tag symbol, but **none for the main
  line itself.** Main-line changesets after the last branch point or tag are reachable from no ref.
- **So `encode git` refuses the repository** (`openbsd-cpio`: one atom), and when it does not, the default branch
  is a side branch.

## 2. The change

- **With `--reconstruct-refs`, the CVS decoder adds one branch ref, `trunk`,** targeting the main line's last
  changeset: the last of `main_atoms`, the line the decoder already reconstructs (the trunk, plus the vendor branch
  while it is the default branch).
- **Status:** `Derived`, `DerivationKind::ReconstructedBranch`, as for the other CVS refs, with
  `params = {source: "cvs-main-line"}`.
- **The name `trunk`:**
  - it matches the Subversion decoder's reconstructed `trunk`, so one name serves both sources' main line, and
    D-3's default-branch order picks it;
  - **a CVS symbol named `trunk`** refuses the decode, naming the symbol and the first file that carries it;
  - with no main-line changeset at all, there is no ref.
- **Without `--reconstruct-refs`:** nothing changes. There are no refs, and `encode git`'s hint names the flag.
- **Documentation:**
  - `docs/src/guide/cvs.md`, the "Branches (with `--reconstruct-refs`)" section, and its list of what is imported:
    the main line is the branch `trunk`, derived;
  - `docs/src/guide/git-target.md`: remove the main-line case and every mention of "request 071".

## 3. Remove the interim text from the product

User-facing text must never name a project request. Remove "request 071" from `crates/brygge/src/encode_git/mod.rs`
(the hint and its comment), from its test, and from the guide.
- **With refs present, the svn/cvs hint becomes** "no reconstructed ref reaches these atoms".
- **The listed ids** are printed as written in the trailer (F-7's forms), truncated, not Rust-`Debug`-quoted. Today
  a CVS id shows as `"COPYING.LIB@1.3\\n COPYING@1.2…"`.

## 4. Checks

- **Level 1:**
  - a main line with a branch cut from it, and a commit after the branch point: `trunk` targets the last main-line
    changeset, and every atom is reachable;
  - a vendor-branch fixture;
  - the `trunk` symbol refusal;
  - no ref without `--reconstruct-refs`.
- **Level 2:** a CVS fixture with a branch: `encode git` writes `refs/heads/trunk`, `HEAD` points to it, and
  `git fsck --strict` is clean.
- **Level 3:** `openbsd-cpio` and `openbsd-games` with `--reconstruct-refs`, then `encode git`:
  - nothing refused;
  - `trunk` is the default branch;
  - `git rev-list --all --count` equals the IR's atom count;
  - `fsck` is clean.
- **This changes the CVS decoder,** so:
  - run the deep mutation depth for `brygge-decode-cvs`;
  - run the usual gates on stable and 1.87;
  - `CHANGELOG.md` `[Unreleased]`, `### Changed`: "CVS: with `--reconstruct-refs`, the main line is a derived
    branch ref, `trunk`, so every reconstructed changeset is reachable from a ref."

## 5. Request

- **File** `.git-exclude/review-requests/071-cvs-main-line-ref.md`.
- **Push after approval only** (114).
- **G2 (D-6, the check against the artifact)** is handed off when 071 is approved.
