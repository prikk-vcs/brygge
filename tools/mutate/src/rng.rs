//! A small, deterministic pseudo-random generator (splitmix64), used only to pick mutation operators and
//! their arguments. It is not cryptographic and makes no such claim — determinism from a fixed seed is the
//! whole point, so a failure is reproducible from the seed index and iteration alone (RFC 014 D-1).

/// splitmix64 (Vigna & Steele), a simple, well-known, fast generator with no external crate.
#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// A generator seeded from `seed`. Two generators built from the same seed produce the same sequence.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next pseudo-random `u64`.
    #[must_use]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// The next pseudo-random `u32` (the low bits of a `next_u64`).
    #[must_use]
    pub fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }

    /// A pseudo-random byte.
    #[must_use]
    pub fn next_byte(&mut self) -> u8 {
        self.next_u64() as u8
    }

    /// A pseudo-random index in `0..bound`. Returns `0` when `bound == 0`.
    #[must_use]
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next_u64() % bound as u64) as usize
    }

    /// `true` with probability `1/n` (`n == 0` is always `false`).
    #[must_use]
    pub fn one_in(&mut self, n: u64) -> bool {
        n != 0 && self.next_u64().is_multiple_of(n)
    }
}

/// A stable seed for one (test, seed index, iteration) triple: a fixed constant (so the sequence is
/// specific to this tool, not to splitmix64's own default state), the test's name (so two tests with the
/// same fixture do not replay each other's mutations), the fixture's seed index, and the iteration. Given
/// unchanged inputs this always returns the same value, which is the whole point: a failure is
/// reproduced by re-running the same test — no seed needs to be copied anywhere by hand.
#[must_use]
pub fn seed_for(test_name: &str, seed_index: u64, iteration: u64) -> u64 {
    const FIXED_CONSTANT: u64 = 0xB7E1_5162_8AED_2A6B;
    let mut h = FIXED_CONSTANT;
    for b in test_name.as_bytes() {
        h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01B3);
    }
    h = h
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(seed_index);
    h = h
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(iteration);
    h
}

#[cfg(test)]
mod tests;
