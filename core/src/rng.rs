//! A small, deterministic pseudo-random generator for procedural
//! background generation ([`crate::generator`]) — deliberately not the
//! `rand` crate: a saved background's seed must reproduce the exact same
//! visual forever ("the project must be able to regenerate the exact same
//! background"), and an external crate's algorithm is free to change
//! between its own versions in ways that would silently break every
//! previously-saved seed. [SplitMix64](https://prng.di.unimi.it/splitmix64.c)
//! (Vigna, 2015) is simple enough to pin down completely in a few lines,
//! good enough statistically for visual variety (not cryptographic use),
//! and is what this crate commits to for as long as project files need to
//! stay reproducible.

#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next raw 64-bit output.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// A float uniformly distributed in `0.0..1.0`.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// A float uniformly distributed in `lo..=hi`.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.next_f64() * (hi - lo)
    }

    /// An index uniformly distributed in `0..len`, or `0` if `len == 0` —
    /// callers index a possibly-empty slice with this rather than
    /// checking emptiness themselves at every call site.
    pub fn index(&mut self, len: usize) -> usize {
        if len == 0 {
            0
        } else {
            ((self.next_f64() * len as f64) as usize).min(len - 1)
        }
    }

    /// `true` with probability `p` (`0.0..=1.0`).
    pub fn chance(&mut self, p: f64) -> bool {
        self.next_f64() < p
    }

    /// A sub-generator seeded deterministically from this one plus a small
    /// integer tag — gives each generated element (or layer) its own
    /// independent stream without hand-threading a running `Rng` through
    /// every call site, while staying fully reproducible from the parent
    /// seed alone.
    pub fn fork(&mut self, tag: u64) -> Rng {
        Rng::new(self.next_u64() ^ tag.wrapping_mul(0xD1B5_4A32_D192_ED03))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_produces_the_same_sequence() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        let seq_a: Vec<u64> = (0..10).map(|_| a.next_u64()).collect();
        let seq_b: Vec<u64> = (0..10).map(|_| b.next_u64()).collect();
        assert_ne!(seq_a, seq_b);
    }

    #[test]
    fn next_f64_stays_within_unit_range() {
        let mut rng = Rng::new(7);
        for _ in 0..1000 {
            let v = rng.next_f64();
            assert!((0.0..1.0).contains(&v), "out of range: {v}");
        }
    }

    #[test]
    fn range_respects_bounds() {
        let mut rng = Rng::new(99);
        for _ in 0..1000 {
            let v = rng.range(-5.0, 5.0);
            assert!((-5.0..=5.0).contains(&v), "out of range: {v}");
        }
    }

    #[test]
    fn index_never_reaches_len() {
        let mut rng = Rng::new(3);
        for _ in 0..1000 {
            assert!(rng.index(7) < 7);
        }
    }

    #[test]
    fn index_of_zero_length_is_zero_not_a_panic() {
        let mut rng = Rng::new(3);
        assert_eq!(rng.index(0), 0);
    }

    #[test]
    fn fork_is_deterministic_for_the_same_parent_state_and_tag() {
        let mut a = Rng::new(11);
        let mut b = Rng::new(11);
        let mut fork_a = a.fork(5);
        let mut fork_b = b.fork(5);
        assert_eq!(fork_a.next_u64(), fork_b.next_u64());
    }

    #[test]
    fn fork_with_a_different_tag_diverges() {
        let mut a = Rng::new(11);
        let mut b = Rng::new(11);
        let mut fork_a = a.fork(5);
        let mut fork_b = b.fork(6);
        assert_ne!(fork_a.next_u64(), fork_b.next_u64());
    }
}
