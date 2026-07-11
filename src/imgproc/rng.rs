//! A tiny explicit seeded pseudo-random generator.
//!
//! Randomized algorithms in this crate (e.g. [`ransac_line`]) must be reproducible,
//! so they take an explicit [`Prng`] seeded by the caller rather than reaching for a
//! global or OS source. This is a `xorshift64*` generator: fast, dependency-free and
//! deterministic for a given seed. It is **not** cryptographically secure.
//!
//! [`ransac_line`]: crate::imgproc::line::ransac_line

/// A deterministic `xorshift64*` pseudo-random generator.
#[derive(Debug, Clone)]
pub struct Prng {
    state: u64,
}

impl Prng {
    /// Create a generator from `seed`. Any seed value is accepted; the internal state
    /// is forced non-zero (a zero xorshift state is a fixed point).
    pub fn new(seed: u64) -> Self {
        // Mix the seed a little and guarantee a non-zero state.
        let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
        if state == 0 {
            state = 0xDEAD_BEEF_CAFE_F00D;
        }
        Prng { state }
    }

    /// Return the next 64-bit value and advance the state.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Return a value in `0..n` with negligible modulo bias. Panics if `n == 0`.
    pub fn below(&mut self, n: usize) -> usize {
        assert!(n > 0, "Prng::below requires n > 0");
        (self.next_u64() % (n as u64)) as usize
    }

    /// Return a float in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        // Use the top 53 bits for a uniformly-spaced double in [0, 1).
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_seed() {
        let mut a = Prng::new(42);
        let mut b = Prng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_differ() {
        let mut a = Prng::new(1);
        let mut b = Prng::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn zero_seed_is_usable() {
        let mut a = Prng::new(0);
        // Must not get stuck at zero.
        assert_ne!(a.next_u64(), 0);
        assert_ne!(a.next_u64(), a.next_u64());
    }

    #[test]
    fn below_in_range() {
        let mut r = Prng::new(7);
        for _ in 0..10_000 {
            let v = r.below(10);
            assert!(v < 10);
        }
    }

    #[test]
    fn next_f64_in_unit_interval() {
        let mut r = Prng::new(99);
        for _ in 0..10_000 {
            let v = r.next_f64();
            assert!((0.0..1.0).contains(&v));
        }
    }
}
