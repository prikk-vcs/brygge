//! The quick/deep test harness (RFC 014 D-2/D-3): for each seed, mutate it a number of times and call the
//! module's `check` closure on the result, under a per-call time budget, with every call wrapped so a
//! panic fails the test with full reproduction details instead of taking the rest of the run down with it.

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::ops::{self, Boundary, InterestingPosition};
use crate::rng::{SplitMix64, seed_for};

/// One seed fixture and the positions in it worth probing with boundary values.
#[derive(Debug, Clone, Copy)]
pub struct Seed<'a> {
    /// The fixture's raw bytes, unmodified.
    pub bytes: &'a [u8],
    /// The byte offsets in `bytes` worth probing with boundary values.
    pub positions: &'a [InterestingPosition],
}

impl<'a> Seed<'a> {
    /// A seed with its declared interesting positions.
    #[must_use]
    pub fn new(bytes: &'a [u8], positions: &'a [InterestingPosition]) -> Self {
        Self { bytes, positions }
    }

    /// A seed with no declared interesting positions — generic operators only.
    #[must_use]
    pub fn generic(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            positions: &[],
        }
    }
}

/// What a module's `check` closure reports for one mutated input.
#[derive(Debug)]
pub enum Outcome {
    /// Accepted as a typed error, or as an `Ok` within every declared ceiling. Nothing to report.
    Ok,
    /// A typed-but-wrong result: an `Ok` whose output exceeds its declared ceiling, a check that should
    /// have refused but didn't, or any other rule the module itself is responsible for. A panic is caught
    /// separately by the harness and never needs this variant.
    Violation(String),
}

/// The two depths (RFC 014 D-2). `Depth::from_env` reads `BRYGGE_MUTATION_DEPTH`, which is how both the
/// ordinary test suite (unset: quick) and the CI deep job (`deep`) select one, with no other plumbing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// The default: runs in every `cargo test`, everywhere.
    Quick,
    /// `quick_iterations_per_seed * deep_factor` per seed, under a memory cap, in the CI deep job and at
    /// each release cut.
    Deep,
}

impl Depth {
    /// Reads `BRYGGE_MUTATION_DEPTH`: `"deep"` selects [`Depth::Deep`]; anything else, including unset,
    /// selects [`Depth::Quick`] (so an ordinary `cargo test` always runs the quick depth).
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("BRYGGE_MUTATION_DEPTH") {
            Ok(v) if v == "deep" => Depth::Deep,
            _ => Depth::Quick,
        }
    }
}

/// Mutate `seed` by a short chain of 1-3 operators (format-aware boundary writes when `seed` declares
/// positions, generic byte operators otherwise, occasionally spliced against `other`), returning the
/// mutated bytes and the chain of operator names, for reporting on failure.
fn mutate(seed: &Seed<'_>, other: &[u8], rng: &mut SplitMix64) -> (Vec<u8>, Vec<String>) {
    let chain_len = 1 + rng.below(3);
    let mut data = seed.bytes.to_vec();
    let mut chain = Vec::with_capacity(chain_len);
    for _ in 0..chain_len {
        if !seed.positions.is_empty() && rng.one_in(2) {
            let idx = rng.below(seed.positions.len());
            let Some(pos) = seed.positions.get(idx) else {
                continue;
            };
            let bounds = ops::boundaries_for(pos);
            let Some(&b) = bounds.get(rng.below(bounds.len())) else {
                continue;
            };
            data = ops::apply_boundary(&data, pos, b);
            chain.push(format!("boundary@{}:{}", pos.offset, describe(b)));
        } else if rng.one_in(8) && !data.is_empty() {
            let (d, name) = ops::apply_truncate(&data, rng);
            data = d;
            chain.push(name);
        } else {
            let (d, name) = ops::apply_generic(&data, other, rng);
            data = d;
            chain.push(name);
        }
    }
    (data, chain)
}

fn describe(b: Boundary) -> &'static str {
    match b {
        Boundary::Zero => "zero",
        Boundary::One => "one",
        Boundary::Max => "max",
        Boundary::CeilingMinus1 => "ceiling-1",
        Boundary::Ceiling => "ceiling",
        Boundary::CeilingPlus1 => "ceiling+1",
        Boundary::Overflow32 => "overflow32",
        Boundary::Overflow64 => "overflow64",
    }
}

fn find_target_dir(manifest_dir: &Path) -> PathBuf {
    let mut dir = manifest_dir.to_path_buf();
    for _ in 0..8 {
        let candidate = dir.join("target");
        if candidate.is_dir() {
            return candidate;
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => break,
        }
    }
    std::env::temp_dir()
}

fn save_finding(
    manifest_dir: &str,
    test_name: &str,
    seed_index: u64,
    iteration: u64,
    bytes: &[u8],
) -> PathBuf {
    let dir = find_target_dir(Path::new(manifest_dir)).join("mutation-findings");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{test_name}-seed{seed_index}-iter{iteration}.bin"));
    let _ = std::fs::write(&path, bytes);
    path
}

/// Run `check` against mutations of every seed in `seeds`, at the depth [`Depth::from_env`] selects.
///
/// `quick_iterations_per_seed` is how many mutations each seed gets at [`Depth::Quick`]; at
/// [`Depth::Deep`] every seed instead gets `quick_iterations_per_seed * deep_factor`. `time_budget` is
/// the per-call ceiling: exceeding it is reported exactly like a ceiling violation.
///
/// `manifest_dir` is the calling crate's `env!("CARGO_MANIFEST_DIR")`, used only to locate the workspace
/// `target/` directory a finding is saved under.
///
/// `check` receives the index of the seed being mutated (its position in `seeds`), so a module whose
/// check needs seed-specific context (a different "base" buffer per seed, a different resign prefix, …)
/// can look it up without the harness needing to know what that context is.
///
/// # Panics
/// On the first finding: a caught panic from `check`, a reported [`Outcome::Violation`], or a call that
/// ran longer than `time_budget`. The panic message names the test, the seed index, the iteration, the
/// operator chain, and the file the mutated input was saved to — enough to turn into a regression test
/// verbatim.
pub fn run(
    manifest_dir: &str,
    test_name: &str,
    seeds: &[Seed<'_>],
    quick_iterations_per_seed: u64,
    deep_factor: u64,
    time_budget: Duration,
    check: impl Fn(usize, &[u8]) -> Outcome,
) {
    if seeds.is_empty() {
        return;
    }
    let iterations = match Depth::from_env() {
        Depth::Quick => quick_iterations_per_seed,
        Depth::Deep => quick_iterations_per_seed.saturating_mul(deep_factor.max(1)),
    };
    for (seed_index, seed) in seeds.iter().enumerate() {
        let seed_index_u64 = seed_index as u64;
        let other = seeds
            .get((seed_index + 1) % seeds.len())
            .map_or(&[][..], |s| s.bytes);
        for iteration in 0..iterations {
            let mut rng = SplitMix64::new(seed_for(test_name, seed_index_u64, iteration));
            let (mutated, chain) = mutate(seed, other, &mut rng);
            let start = Instant::now();
            let outcome = panic::catch_unwind(AssertUnwindSafe(|| check(seed_index, &mutated)));
            let elapsed = start.elapsed();

            let violation = match &outcome {
                Err(_) => Some("panicked".to_string()),
                Ok(Outcome::Violation(msg)) => Some(msg.clone()),
                Ok(Outcome::Ok) if elapsed > time_budget => Some(format!(
                    "exceeded the {time_budget:?} time budget ({elapsed:?})"
                )),
                Ok(Outcome::Ok) => None,
            };

            if let Some(detail) = violation {
                let saved =
                    save_finding(manifest_dir, test_name, seed_index_u64, iteration, &mutated);
                panic!(
                    "mutation finding in {test_name}: seed {seed_index_u64}, iteration {iteration}, \
                     operator chain {chain:?}: {detail}. Mutated input saved to {}",
                    saved.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
