//! Mutation tests for the RCS `,v` reader (RFC 014 D-1): `parse_rcs`, then `content_of` and
//! `contents_of_many`, must never panic on a mutated `,v` file, must never reconstruct content wildly
//! larger than the mutated input could plausibly have produced, and must never run for long — a mutated
//! file is tiny, and nothing it can claim should cost more than a constant multiple of its own size.
//!
//! Found and fixed building this module, in order of severity:
//! - **an actual infinite loop**, no boundary value needed — the harness's plain truncation operator hit
//!   it on the very first seed, within the first few hundred mutations: a `,v` file truncated right after
//!   an unterminated `symbols` or `branches` list looped forever, advancing `cur.pos` one byte past the
//!   end of the buffer on every iteration with no bound and no termination condition. Fixed by refusing
//!   at end of input instead of blindly advancing; regression tests
//!   `a_truncated_symbols_list_is_a_read_error_not_an_infinite_loop` and
//!   `a_truncated_branches_list_is_a_read_error_not_an_infinite_loop` in `rcs::tests`.
//! - two attacker-controlled ed-command fields with no ceiling of their own (RUSTSEC-2026-0344's class):
//!   - an `a<n> <count>` command's `count` was used to `Vec::with_capacity(count)` directly — a claimed
//!     count of billions, with few actual lines following, would try to reserve that much before the
//!     loop's own bound ever ran. Fixed in [`super::apply_diff`] by capping the pre-allocation to what
//!     remains of the script; regression test
//!     `an_ed_add_commands_claimed_count_does_not_preallocate_past_the_script` in `rcs::tests`.
//!   - `d<n> <count>`/`a<n> <count>`'s `n` drove the copy-unchanged-lines loop directly, with no bound
//!     tied to the source's actual (already-bounded) length — confirmed by hand that a single
//!     `d99999999999 1` command on a two-line source hung the decoder for minutes. Fixed in the same
//!     function by capping that loop's bound at `source.len()`; regression test
//!     `an_ed_commands_claimed_line_number_cannot_spin_the_apply_loop` in `rcs::tests`.
//!
//! The ed-command positions below exercise the latter two fixes at every depth; the truncation operator
//! (built into every mutation chain) now exercises the first.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeSet;
use std::time::Duration;

use brygge_mutate::harness::{self, Outcome, Seed};
use brygge_mutate::ops::{Encoding, InterestingPosition};

use super::parse_rcs;

const QUICK_ITERATIONS: u64 = 250;
const DEEP_FACTOR: u64 = 400;
const TIME_BUDGET: Duration = Duration::from_secs(2);

/// A hand-written `,v` with three trunk revisions, two of them real ed-style reverse deltas carrying
/// explicit `d<n> <count>` / `a<n> <count>` commands — the fields this module targets. (Independent of
/// `rcs::tests`'s own `SAMPLE`, which is private to that module.)
const SAMPLE: &[u8] = b"head\t1.3;\n\
access;\n\
symbols\n\
\tREL_1:1.2;\n\
locks; strict;\n\
comment\t@# @;\n\
\n\
\n\
1.3\n\
date\t2024.01.03.12.00.00;\tauthor alice;\tstate Exp;\n\
branches;\n\
next\t1.2;\n\
\n\
1.2\n\
date\t2024.01.02.12.00.00;\tauthor alice;\tstate Exp;\n\
branches;\n\
next\t1.1;\n\
\n\
1.1\n\
date\t2024.01.01.12.00.00;\tauthor bob;\tstate Exp;\n\
branches;\n\
next\t;\n\
\n\
\n\
desc\n\
@@\n\
\n\
\n\
1.3\n\
log\n\
@third@\n\
text\n\
@line one\nline two changed\nline three\n@\n\
\n\
\n\
1.2\n\
log\n\
@second@\n\
text\n\
@d2 1\na2 1\nline two\n@\n\
\n\
\n\
1.1\n\
log\n\
@first@\n\
text\n\
@d3 1\n@\n";

fn find(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
        .unwrap_or_else(|| {
            panic!(
                "fixture marker {:?} not found",
                String::from_utf8_lossy(needle)
            )
        })
}

/// Both numbers of each of the three ed commands in `SAMPLE` (`d2 1`, `a2 1`, `d3 1`): the line number
/// (`n`, right after the command letter) and the claimed count (right after the space) — both
/// attacker-controlled fields with no declared ceiling of their own, now bounded (by the two fixes above)
/// by what the script and source actually contain.
fn interesting_positions() -> Vec<InterestingPosition> {
    [&b"d2 1"[..], b"a2 1", b"d3 1"]
        .into_iter()
        .flat_map(|marker| {
            let at = find(SAMPLE, marker);
            // marker is "<op><n> <count>": the line number is one byte in, the count three bytes in.
            [at + 1, at + 3]
        })
        .map(|offset| InterestingPosition {
            offset,
            encoding: Encoding::Decimal { len: 1 },
            ceiling: None,
        })
        .collect()
}

/// A generous amplification bound: no single reconstructed revision should be wildly larger than the
/// mutated file it came from. The mutation operators themselves grow a seed by at most a few dozen bytes
/// per call (short inserted/duplicated runs, chain length at most 3), so a factor of 64 has ample margin
/// before it would ever fire on legitimate growth, while still catching a genuine amplification bug.
fn amplification_ceiling(input_len: usize) -> usize {
    input_len.saturating_mul(64).max(4096)
}

fn check(bytes: &[u8]) -> Outcome {
    let ceiling = amplification_ceiling(bytes.len());
    let file = match parse_rcs(bytes) {
        Ok(f) => f,
        Err(_) => return Outcome::Ok,
    };
    // "`content_of` on a sample", not every revision (RFC 014 D-1's own wording): the head is always
    // present, and `contents_of_many` below already exercises every revision in one combined pass.
    match file.content_of(&file.head) {
        Ok(content) if content.len() > ceiling => {
            return Outcome::Violation(format!(
                "content_of(head={}) reconstructed {} bytes from a {}-byte file (ceiling {ceiling})",
                file.head.to_dotted(),
                content.len(),
                bytes.len()
            ));
        }
        Ok(_) | Err(_) => {}
    }
    let wanted: BTreeSet<_> = file.revisions.keys().cloned().collect();
    if let Ok(contents) = file.contents_of_many(&wanted) {
        let total: usize = contents.values().map(Vec::len).sum();
        if total > ceiling {
            return Outcome::Violation(format!(
                "contents_of_many reconstructed {total} bytes in total from a {}-byte file (ceiling {ceiling})",
                bytes.len()
            ));
        }
    }
    Outcome::Ok
}

#[test]
fn parsing_and_reconstructing_a_mutated_rcs_file_never_panics_and_never_amplifies() {
    let positions = interesting_positions();
    let seeds = [Seed::new(SAMPLE, &positions)];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "parsing_and_reconstructing_a_mutated_rcs_file_never_panics_and_never_amplifies",
        &seeds,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, bytes| check(bytes),
    );
}
