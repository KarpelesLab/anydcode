//! Float helpers `core` lacks. Encoders that must stay bit-exact without `std` use
//! these instead of the inherent `f64` methods; with `std` they forward to it.

/// Correctly rounded square root (IEEE 754 `sqrt`), bit-identical to `f64::sqrt`.
#[cfg(feature = "std")]
#[inline]
pub(crate) fn sqrt(x: f64) -> f64 {
    x.sqrt()
}

/// Correctly rounded square root (IEEE 754 `sqrt`), bit-identical to `f64::sqrt`.
#[cfg(not(feature = "std"))]
#[inline]
pub(crate) fn sqrt(x: f64) -> f64 {
    soft_sqrt(x)
}

/// Software `sqrt` for `no_std` builds.
///
/// Integer square root of the widened mantissa: a 53-bit root plus a round-to-nearest
/// decision from the remainder (an exact tie is impossible for a square root).
#[cfg(any(not(feature = "std"), test))]
fn soft_sqrt(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 || x.is_infinite() {
        return x;
    }
    let bits = x.to_bits();
    let raw_exp = ((bits >> 52) & 0x7FF) as i32;
    let frac = bits & ((1 << 52) - 1);
    // x = m · 2^e with m a 53-bit integer (subnormals normalized).
    let (mut m, mut e) = if raw_exp == 0 {
        (frac, -1074)
    } else {
        (frac | (1 << 52), raw_exp - 1075)
    };
    while m < (1 << 52) {
        m <<= 1;
        e -= 1;
    }
    // Widen to M in [2^104, 2^106) with an even exponent, so √M is a 53-bit integer part.
    let shift = if (e - 52) % 2 == 0 { 52 } else { 53 };
    let wide = u128::from(m) << shift;
    let mut q = wide.isqrt();
    if wide - q * q > q {
        q += 1;
    }
    let exp = (e - shift) / 2;
    (q as u64 as f64) * pow2(exp)
}

/// `2^k` for exponents in the normal range.
#[cfg(any(not(feature = "std"), test))]
fn pow2(k: i32) -> f64 {
    f64::from_bits(((k + 1023) as u64) << 52)
}

#[cfg(test)]
mod tests {
    #[test]
    fn sqrt_matches_std() {
        let mut x = 1.0e-30_f64;
        while x < 1.0e30 {
            for v in [x, x * 1.37, x * 2.0 - 1e-300, 0.666 * x, 1.5 * x] {
                assert_eq!(super::soft_sqrt(v).to_bits(), v.sqrt().to_bits(), "{v}");
            }
            x *= 3.1;
        }
        for n in 0..10_000u32 {
            let v = f64::from(n) * 0.666;
            assert_eq!(super::soft_sqrt(v).to_bits(), v.sqrt().to_bits());
        }
    }

    /// The whole `f64` range, not just the magnitudes encoders use: subnormals (whose
    /// mantissa must be normalized first), the extremes of the exponent, both
    /// exponent parities, and the special values.
    #[test]
    fn sqrt_matches_std_at_the_edges() {
        let same = |v: f64| {
            let (soft, hard) = (super::soft_sqrt(v), v.sqrt());
            assert_eq!(
                soft.to_bits(),
                hard.to_bits(),
                "sqrt({v:e}): {soft:e} vs {hard:e}"
            );
        };
        for v in [
            0.0,
            -0.0,
            f64::INFINITY,
            f64::MIN_POSITIVE,
            f64::MIN_POSITIVE / 2.0,
            f64::from_bits(1),
            f64::from_bits(2),
            f64::from_bits(3),
            f64::from_bits((1 << 52) - 1),
            f64::MAX,
            f64::MAX / 2.0,
            f64::EPSILON,
            1.0,
            1.0 + f64::EPSILON,
            1.0 - f64::EPSILON / 2.0,
            2.0,
            4.0,
            2.0 - f64::EPSILON,
            4.0 - 2.0 * f64::EPSILON,
            1e300,
            1e-300,
            1e-310,
            4.9e-324,
        ] {
            same(v);
        }
        assert_eq!(super::soft_sqrt(-0.0).to_bits(), (-0.0f64).to_bits());
        for nan in [f64::NAN, -1.0, -f64::MIN_POSITIVE, f64::NEG_INFINITY] {
            assert!(super::soft_sqrt(nan).is_nan(), "{nan}");
        }
        // Every exponent, with mantissas that sit on rounding boundaries.
        for exp in -1074..=1023 {
            let base = 2f64.powi(exp);
            for m in [
                1.0,
                1.25,
                1.5,
                1.999_999_999_999_999_8,
                1.000_000_000_000_000_2,
            ] {
                same(base * m);
            }
        }
        // Perfect squares are exact; their neighbours round correctly.
        for n in 1..2000u64 {
            let sq = (n * n) as f64;
            assert_eq!(super::soft_sqrt(sq), n as f64);
            same(f64::from_bits(sq.to_bits() - 1));
            same(f64::from_bits(sq.to_bits() + 1));
        }
        // A deterministic sweep of arbitrary bit patterns (positive, finite).
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let v = f64::from_bits(x >> 1);
            if v.is_finite() {
                same(v);
            }
        }
    }
}
