# Handoff — RFC 014 D-7: the advisory watch (batch A, small, first)

**Governing:** RFC 014 D-7, accepted 2026-10-17. It answers the owner's concern that a "weekly" job would
read as precedent for date-driven work. **Do this first;** batches M and R follow.

## 0. First, commit the architect's files on their own

Commit as "RFC 014 accepted (Hardening, 0.4.0); handoffs":
- `rfcs/done/014-hardening.md`;
- `rfcs/handoffs/014-hardening/` (this file and its two siblings);
- `rfcs/README.md`;
- `ROADMAP.md`.

The links resolve on that commit alone.

## 1. Rename and reframe the workflow

- **`git mv .github/workflows/security-audit.yml .github/workflows/advisory-watch.yml`.**
- `name: Scheduled Security Audit` becomes **`name: Advisory watch`**. The job name stays descriptive, for
  example `cargo audit (newest advisory database)`.
- **Replace the header comment** with one that says, in substance:
  - **it watches one external event:** a new RustSec advisory against a dependency. Nothing in this
    repository can signal that event, and GitHub offers no event for it, **so this workflow polls**;
  - **it is brygge's only scheduled workflow.** Every other job runs because something changed (a push, the
    paths it guards), because a release is cut, or on demand. See `GOVERNANCE.md`, "Work is triggered, never
    scheduled";
  - adding another scheduled workflow requires an RFC;
  - **keep** the existing note on installing `cargo-audit` from source with `--locked`, and why.
- **The `cron` line:** keep the interval unchanged. Its comment becomes "polling interval for the advisory
  database (not a task cadence)". Do not use the word "weekly" anywhere in the file.
- Keep `workflow_dispatch`.

## 2. The rule in `GOVERNANCE.md`

Add a short section after "Gates every change must pass", titled "Work is triggered, never scheduled", with
RFC 014 D-7's text:

> **Work is triggered, never scheduled.** Jobs run on a push, a change to the paths they guard, a release
> cut, a finding, or on demand. The single exception is the **advisory watch**
> (`.github/workflows/advisory-watch.yml`), which polls the RustSec database because a new advisory is an
> external event with no trigger here. A new scheduled job needs an RFC.

## 3. Every other mention

- **`rfcs/accepted/012-release-automation.md`,** D-7: add one line under the heading: "**Amended by RFC 014
  D-7 (2026-10-17):** the job is `advisory-watch.yml`, framed as the one poll of an external source; see
  `GOVERNANCE.md`." Leave the original text below it.
- **Search the repository** for `security-audit`, `Scheduled Security Audit` and `weekly`, excluding
  released `CHANGELOG.md` sections, which are history. Report each hit, and update every live reference to
  the file or job name: docs, `releasing.md`, `HANDOFF.md`, the CI badges if any.
- **Leave the threat model's** C-4d and RR-1 wording alone. The architect rewrites them in v0.8 at the 0.4.0
  cut.
- **CHANGELOG `[Unreleased]`, Changed:** "The scheduled advisory check is renamed **Advisory watch**
  (`advisory-watch.yml`) and documented as brygge's only scheduled job, the one poll of an external source;
  all other work is triggered (RFC 014 D-7)."

## 4. Proof

- **After the push, the architect dispatches** the renamed workflow once, and checks that it passes with only the
  allowed `RUSTSEC-2026-0306`. Do not dispatch it yourself: GOVERNANCE keeps every dispatch with the
  architect, and an exception, even a harmless one, would blur that rule.
- The links resolve, and the book builds.

## 5. Review request

`.git-exclude/review-requests/039-advisory-watch.md`, with the search results of §3. After approval, commit
and push, and append CI. The architect then runs §4's dispatch.
