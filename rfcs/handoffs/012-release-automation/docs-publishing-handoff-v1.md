# Handoff — RFC 012 D-11: publish the book on GitHub Pages

**Governing:** RFC 012 D-11 (added 2026-09-24 at the owner's request); read it first. The owner has
already enabled GitHub Pages with *GitHub Actions* as its source (`https://prikk-vcs.github.io/brygge/`;
the `github-pages` environment exists).

**Order:** this lands **before** the 0.1.1 tag, so it is part of 0.1.1. The architect tags the commit
that contains it, after the owner's release setup.

**Already in the working tree (the architect's):**
- `docs/src/introduction.md`, the book's first page, and its `SUMMARY.md` entry;
- the RFC 012 amendment (D-2: no tag ruleset; D-11);
- the matching updates to `brygge-03`, `GOVERNANCE.md` and `releasing.md`.

Commit them with this work.

---

## 1. `docs/book.toml`

- `[book]`: `title = "brygge"`, `authors = ["nabbisen"]`, `language = "en"`, `src = "src"`.
- `[output.html]`:
  - `site-url = "/brygge/"`;
  - `git-repository-url = "https://github.com/prikk-vcs/brygge"`;
  - no `edit-url-template`, since edits go through the project's process, not the web editor.
- The search is on (the default).
- **No preprocessors or plugins.**
- Add `docs/book/` to `.gitignore`.

## 2. The build must be clean

- **Pin mdBook exactly:** `cargo install mdbook --version <X.Y.Z> --locked`, with the latest 0.5.x on
  the day. It is one pinned version, used by both workflows below. State in the review request which
  version was pinned, and that its default features were kept (search needs them).
- **`mdbook build docs`** must finish with **no warnings** about missing files, unused files or broken
  internal links. Check every `docs/src/**/*.md` is reachable from `SUMMARY.md`, or state which are
  deliberately not.
- **Render check:** open the built book locally and check that these render correctly: the tables, the
  code blocks, the long reference pages (`ir-artifact-format.md`, `machine-output.md`) and the threat
  model. Report any page that renders wrongly, instead of rewriting the architect's documents. The
  architect fixes the content.

## 3. `.github/workflows/docs.yml` (RFC 012 D-11)

- **Triggers:** `push` to `main` with `paths: ["docs/**", ".github/workflows/docs.yml"]`, and
  `workflow_dispatch`.
- **Permissions:** top-level `contents: read`.
- **Job `build`** (`contents: read`): checkout; Rust stable; install the pinned mdBook;
  `actions/configure-pages`; `mdbook build docs`; `actions/upload-pages-artifact` with `path: docs/book`.
- **Job `deploy`:**
  - `needs: build`;
  - `environment: { name: github-pages, url: ${{ steps.deployment.outputs.page_url }} }`;
  - permissions `pages: write` and `id-token: write`, on this job only;
  - runs `actions/deploy-pages`.
- **`concurrency: { group: pages, cancel-in-progress: true }`.**
- **Actions** use their current major tags, as in `ci.yml`. No release credential is involved.
- **No `${{ }}` expression inside a `run:` script.**

prikk's `docs.yml` (`.git-exclude/tmp/prikk-0.46.0/.github-workflows/`) is the model. Differences: the
permissions are per job, and there is no landing page and no mermaid.

## 4. The book in CI (`ci.yml`)

- `ci.yml`'s `repository` job installs the pinned mdBook and runs `mdbook build docs`, on every branch
  push and pull request, so a broken `SUMMARY.md` or page is caught before `docs.yml` ever deploys.
- Skip this step in a release run (`inputs.ref` set), as the tool tests are skipped: an old tag need not
  contain `book.toml`.

## 5. Links that would break on the site (`tools/check-links.sh`)

- **Extend the script:** a relative link in any `docs/src/**/*.md` whose resolved target lies **outside**
  `docs/src/` is an error. It resolves in the repository, but 404s on the site. The message says to use
  an absolute `https://github.com/prikk-vcs/brygge/blob/main/...` URL instead.
- There are none today (the architect checked). Add a test case to the script's own checks, or a small
  fixture run in CI, showing that such a link is refused.

## 6. Documentation and CHANGELOG

- `README.md` "Going deeper" and `crates/brygge/README.md` "Documentation": add the site,
  `https://prikk-vcs.github.io/brygge/`, as the first entry.
- `CHANGELOG.md`, under `[0.1.1]` → `#### Added`: **"The documentation is published as a book at
  <https://prikk-vcs.github.io/brygge/>, built with mdBook from `docs/src/` on every change to `main`."**
- The same `[0.1.1]` section gains one line under `#### Changed`: "Released tags are protected by a
  governance rule, not a GitHub ruleset."

## 7. Proof

- **Before the commit:**
  - `mdbook build docs` is clean;
  - `tools/check-links.sh` passes, and its new check is shown to refuse an outside link;
  - `actionlint` is clean on `docs.yml` and `ci.yml`;
  - the usual gates pass.
- **After the push:**
  - report the `ci.yml` run (seven jobs green; the `repository` job built the book) and the `docs.yml`
    run;
  - the site is live at `https://prikk-vcs.github.io/brygge/` and its introduction page loads. Report
    the deploy run id.

## 8. Not to do

- No tag, no dispatch of `release.yml`, no approval, no publication.
- No change to the architect's document content, except the lines named in §6. Rendering problems are
  reported, not fixed.

## 9. Review request

File `.git-exclude/review-request/019-docs-publishing.md` with the standard sections, the pinned mdBook
version, the render check (§2), and §7's evidence. **Append the post-push results to the same file.**
