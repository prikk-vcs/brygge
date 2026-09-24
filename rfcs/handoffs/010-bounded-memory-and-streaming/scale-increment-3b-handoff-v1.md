# Handoff — 0.2.0, RFC 010 increment 3b: CVS clustering overlap in near-linear time

**Governing:** RFC 010 as amended 2026-09-24 (increment 3b, added from review 030). RFC 007's
`span-overlap-v1` confidence rule, which must not change by a single value.

**The problem, as measured (request 025 §5):** after increment 3, `cluster::reconstruct` is most of the CVS
decode time and grows ~4× per doubling of revisions: 53 ms, then 195 ms, then 948 ms, then **4.0 s** at
10k, 20k, 40k and 100k revisions. In `finalize` (`cluster.rs`), each changeset's overlap count scans
**every** changeset's date range for each of its paths: O(changesets² × paths).

## The change

- **The rule, unchanged.** For changeset `i`, with range `[earliest_i, latest_i]` widened by `window`
  (saturating) to `[lo_i, hi_i]`, the overlap count is the number of `i`'s paths `p` for which **some other**
  changeset `j ≠ i` touches `p` and has `latest_j ≥ lo_i` and `earliest_j ≤ hi_i`. Keep the inclusive
  bounds, the saturation, and `j ≠ i`.
- **Index it per path.**
  - Build once, for each path, the list of changesets touching it, with their `(earliest, latest,
    index)`.
  - Answer "does some `j ≠ i` in `p`'s list satisfy both bounds?" without scanning all changesets. For
    example, sort each list by `earliest`, and binary-search the prefix with `earliest_j ≤ hi_i`. Keep, for
    each prefix, the two largest `latest` values with their indices, so that `i` itself can be excluded.
    Any structure is fine if it answers exactly the same predicate.
- **The target:** O(total path-touches × log) overall.
- **Nothing else changes:** the clustering, the ordering, the splits, the time score, and the formula.

## Acceptance

- **Byte-identical:**
  - every CVS test fixture and the bench corpora (`cvs`, `cvs-revs`), compared by `cmp` between the
    two-build CLI outputs, as in request 025 §4;
  - **a property test** comparing the old overlap function (kept in the test module as the oracle) with
    the new one on generated inputs: random paths, dates and windows, **including `window = 0`, equal
    dates, `i64` extremes (saturation) and many changesets on one path**. Several thousand cases, fixed
    seed.
- **The A/B on `cvs-revs`** (200, 1,000, 5,000): the clustering time grows ~linearly (or ×log), and the
  total at 5,000 per file drops well below the 5.6 s of increment 3. The peak is not higher.
- **`tools/bench/README.md`:** a "0.2.0 increment 3b" section. **CHANGELOG `[Unreleased]`:** one line.

## Review request

`.git-exclude/review-request/026-increment-3b-cvs-clustering.md`. After approval, commit and push, and append
CI.
