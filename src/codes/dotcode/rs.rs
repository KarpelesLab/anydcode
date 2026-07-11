//! Reed–Solomon error correction over the prime field GF(113), as used by DotCode
//! (AIM ISS DotCode Rev 4.0 Annex B).
//!
//! 113 is prime, so the field is the integers modulo 113 with ordinary modular
//! arithmetic. The generator polynomial of degree `nc` is `∏(x − 3^i)` for
//! `i` in `1..=nc` (primitive element `α = 3`), returned most-significant
//! coefficient first with a monic leading `1`. These coefficients match the
//! hard-coded `coefs[]` table in zint's `backend/dotcode.c`.
//!
//! The systematic encoder mirrors zint's `dc_rsencode()`, including its
//! interleaving of `step` Reed–Solomon blocks when the codeword count reaches the
//! field size. The check words are written in place into the tail of `wd`.

use super::tables::DC_GF;

const GF: i32 = DC_GF as i32;
/// Primitive element of GF(113) used to build the generator polynomial.
const ALPHA: i32 = 3;

/// `α^n mod 113`.
fn pow(mut base: i32, mut exp: i32) -> i32 {
    let mut acc = 1i32;
    base %= GF;
    while exp > 0 {
        if exp & 1 == 1 {
            acc = (acc * base) % GF;
        }
        base = (base * base) % GF;
        exp >>= 1;
    }
    acc
}

/// The generator polynomial of degree `nc`, `∏(x − 3^i)` for `i` in `1..=nc`.
///
/// Returned most-significant coefficient first: `out[0] == 1` (monic leading term
/// for `x^nc`) through `out[nc]` (the constant term). This is the layout consumed
/// by [`rsencode`] as `c` (so `c[j]` multiplies `x^(nc-j)`).
pub fn generator(nc: usize) -> Vec<i32> {
    let mut g = vec![1i32]; // g[0] is the highest-degree coefficient so far.
    for i in 1..=nc as i32 {
        let root = pow(ALPHA, i);
        let mut next = vec![0i32; g.len() + 1];
        for (j, &c) in g.iter().enumerate() {
            next[j] = (next[j] + c) % GF; // x * g
            next[j + 1] = (next[j + 1] + GF - (c * root) % GF) % GF; // − root * g
        }
        g = next;
    }
    g
}

/// Append `nc` Reed–Solomon check words to the `nd` data words at the front of
/// `wd`, in place. `wd` must have length `nd + nc`; the check words are written to
/// `wd[nd..nd + nc]`. Port of zint's `dc_rsencode()`.
pub fn rsencode(nd: usize, nc: usize, wd: &mut [u8]) {
    debug_assert_eq!(wd.len(), nd + nc);
    let nw = nd + nc;
    // Number of interleaved blocks needed so no block exceeds the field size.
    let step = (nw + DC_GF as usize - 2) / (DC_GF as usize - 1);

    for start in 0..step {
        let nd_b = (nd - start).div_ceil(step);
        let nw_b = (nw - start).div_ceil(step);
        let nc_b = nw_b - nd_b;
        // The check-word region for this block begins here and is stepped by `step`.
        let e_base = start + nd_b * step;

        let c = generator(nc_b); // c[0] == 1, length nc_b + 1

        // Working error/check accumulator, one entry per check word.
        let mut e = vec![0i32; nc_b];
        for i in 0..nd_b {
            let k = (wd[start + i * step] as i32 + e[0]) % GF;
            let mut next = vec![0i32; nc_b];
            for j in 0..nc_b - 1 {
                next[j] = (GF - (c[j + 1] * k) % GF + e[j + 1]) % GF;
            }
            next[nc_b - 1] = (GF - (c[nc_b] * k) % GF) % GF;
            e = next;
        }
        for (i, &ev) in e.iter().enumerate() {
            wd[e_base + i * step] = if ev != 0 { (GF - ev) as u8 } else { 0 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generator polynomials must match zint's hard-coded `coefs[]` (dotcode.c),
    /// which are themselves the AIM ISS DotCode Rev 4.0 Annex B coefficients.
    #[test]
    fn generator_matches_zint_coefs() {
        assert_eq!(generator(3), vec![1, 74, 12, 62]);
        assert_eq!(generator(4), vec![1, 106, 7, 107, 63]);
        assert_eq!(generator(5), vec![1, 89, 13, 101, 52, 59]);
        assert_eq!(generator(6), vec![1, 38, 107, 3, 99, 6, 42]);
        assert_eq!(generator(7), vec![1, 111, 56, 17, 92, 1, 28, 15]);
    }

    /// A clean data block plus its check words must decode to all-zero syndromes,
    /// i.e. evaluate to 0 at every generator root `3^i`.
    #[test]
    fn check_words_zero_syndromes() {
        let data = [50u8, 12, 100, 7, 33, 90];
        let nc = 3 + data.len() / 2;
        let mut wd = data.to_vec();
        wd.extend(std::iter::repeat_n(0u8, nc));
        rsencode(data.len(), nc, &mut wd);
        for i in 1..=nc as i32 {
            let root = pow(ALPHA, i);
            let mut s = 0i32;
            for &c in &wd {
                s = (s * root + c as i32) % GF;
            }
            assert_eq!(s, 0, "syndrome at root 3^{i} is non-zero");
        }
    }
}
