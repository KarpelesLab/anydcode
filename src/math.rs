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
}
