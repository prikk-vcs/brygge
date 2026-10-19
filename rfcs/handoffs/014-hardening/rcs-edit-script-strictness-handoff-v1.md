# Handoff — the RCS edit-script reader refuses what CVS itself refuses (queued from review 059)

**Governing:** RFC 014 (hardening), and the threat model's C-2a ("refuse rather than misread"). **Crate:**
`brygge-decode-cvs`. **Independent of batch R.** One review request. **A topic branch is required**
(real-`cvs` fixtures run only on CI's Linux job).

## Today

- `rcs.rs::apply_diff` applies an RCS edit script (`d<start> <count>` and `a<after> <count>`) to the source
  lines. **It tolerates scripts that CVS itself refuses:**
  - commands **out of order,** or **overlapping** (a `d` or `a` that refers to a line before what earlier
    commands have already consumed);
  - a `d` that reaches **past the end** of the source, and an `a` whose `after` is past it;
  - a `d` with a count of 0.
- These silently produce *some* text (review 059 confirmed that the M-1 cap left the output unchanged, so this
  leniency predates it), where the format's reference implementation refuses the file.
- **A misread, not a refusal, is exactly what C-2a forbids.** A crafted or corrupted `,v` could decode into
  content no CVS checkout would ever give.

## The rule

The script's line numbers refer to the **source** (the previous revision), 1-based. Commands must be
ascending and non-overlapping. With `consumed` = the source lines already copied or deleted (0 at the start):

| Command | Valid only when |
|---|---|
| `d<start> <count>` | `count ≥ 1`; `start ≥ consumed + 1` (not before what is consumed); `start + count − 1 ≤ source.len()` (no past-the-end delete; checked arithmetic) |
| `a<after> <count>` | `after ≥ consumed` (an `a` may follow a `d` that ends at the same line); `after ≤ source.len()` |

- **Anything else is `Error::Read`,** naming the command and the revision: for example "RCS edit script
  command `d9 2` is out of order or past the end of revision 1.3's source (12 lines, 10 consumed)".
- **Keep** M-1's caps and the `count`-based allocation bound. The new checks come first, and make the caps
  defence in depth.

## The oracle is CVS itself

**Before refusing anything,** prove that CVS refuses it too, so brygge's rule is CVS's rule, not brygge's
guess:
- in the real-`cvs` fixture tests (CI's Linux job), take a valid repository and **hand-corrupt one `,v`**
  for each case: out of order, overlapping, delete past the end, add past the end, a zero-count delete;
- for each, assert that **`cvs checkout -r <rev>` fails** on that revision (or, if CVS accepts one of them,
  **stop and report it**, because then brygge must accept it too, exactly as CVS does);
- and assert that brygge's decode gives `Read`, naming the command;
- **plus:** every valid real-`cvs` fixture still decodes byte-identically.

## Tests

1. **Unit,** with hand-written `,v` and no `cvs`: each invalid case gives `Read` naming the command. Each valid
   edge passes: an `a` right after a `d` on the same line, a delete of the last line, an add after the last
   line, and an add at 0 (before the first line).
2. **The oracle tests above** (CI, ubuntu-latest).
3. **Regression:**
   - every CVS fixture and the bench corpora (`cvs`, `cvs-revs`, `cvs-branches`) are byte-identical to `main`
     (two builds, compared with `cmp`);
   - the mutation tests pass at both depths (new refusals are typed errors);
   - the deep job runs on the topic branch, since this changes `crates/brygge-decode-cvs/src/**`.

## CHANGELOG `[Unreleased]`, Fixed

"CVS: an RCS edit script whose commands are out of order, overlap, or reach past the end of the previous
revision is refused, as CVS itself refuses it; brygge no longer decodes such a file into content no CVS
checkout would give."

## Review request

`.git-exclude/review-requests/044-rcs-edit-script-strictness.md`. Use the topic-branch routine: push, the
oracle tests run and pass on ubuntu-latest (named in the log), then fast-forward `main`.
