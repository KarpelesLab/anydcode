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
use alloc::{vec, vec::Vec};

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

/// Multiplicative inverse of a non-zero element (Fermat: `a^(p-2)`).
fn inv(a: i32) -> i32 {
    pow(a, GF - 2)
}

/// Evaluate a least-significant-first polynomial at `x`.
fn eval_lsf(p: &[i32], x: i32) -> i32 {
    p.iter().rev().fold(0, |acc, &c| (acc * x + c) % GF)
}

/// Correct `wd` (`nd` data words followed by `nc` check words, laid out as by
/// [`rsencode`]) in place. `erased[i]` marks words known to be unreliable. Each
/// interleaved block recovers `e` erasures plus `t` errors while `e + 2t` does not
/// exceed its check-word count. Returns `false` if any block is beyond repair.
pub fn rsdecode(nd: usize, nc: usize, wd: &mut [u8], erased: &[bool]) -> bool {
    debug_assert_eq!(wd.len(), nd + nc);
    debug_assert_eq!(erased.len(), wd.len());
    let nw = nd + nc;
    let step = (nw + DC_GF as usize - 2) / (DC_GF as usize - 1);
    for start in 0..step.min(nw) {
        let nd_b = (nd.saturating_sub(start)).div_ceil(step);
        let nw_b = (nw - start).div_ceil(step);
        if nw_b <= nd_b {
            return false;
        }
        let idx: Vec<usize> = (0..nw_b).map(|i| start + i * step).collect();
        if !decode_block(wd, erased, &idx, nw_b - nd_b) {
            return false;
        }
    }
    true
}

/// Errors-and-erasures decoding of one block: the words `wd[idx[..]]`, first word
/// the highest-degree coefficient, generator roots `3^1..=3^nc`.
fn decode_block(wd: &mut [u8], erased: &[bool], idx: &[usize], nc: usize) -> bool {
    let n = idx.len();
    // The locator of the word at block position `j` is `α^(n-1-j)`.
    let locator = |j: usize| pow(ALPHA, (n - 1 - j) as i32);

    // More erasures than check words leave nothing to verify the rest against (an
    // all-erased block would otherwise pass as the all-zero codeword).
    if idx.iter().filter(|&&p| erased[p]).count() > nc {
        return false;
    }

    // Syndromes S_1..S_nc (stored 0-based).
    let syn: Vec<i32> = (1..=nc as i32)
        .map(|i| {
            let root = pow(ALPHA, i);
            idx.iter()
                .fold(0, |s, &p| (s * root + wd[p] as i32 % GF) % GF)
        })
        .collect();
    if syn.iter().all(|&s| s == 0) {
        return true;
    }

    // Erasure locator Γ(x) = ∏(1 − X_e·x), least-significant coefficient first.
    let mut lambda = vec![1i32];
    for j in (0..n).filter(|&j| erased[idx[j]]) {
        let x = locator(j);
        let mut next = vec![0i32; lambda.len() + 1];
        for (k, &c) in lambda.iter().enumerate() {
            next[k] = (next[k] + c) % GF;
            next[k + 1] = (next[k + 1] + GF - (c * x) % GF) % GF;
        }
        lambda = next;
    }
    let erasures = lambda.len() - 1;

    // Berlekamp–Massey, seeded with the erasure locator.
    let mut prev = lambda.clone();
    let mut l = erasures;
    for r in erasures + 1..=nc {
        let delta = lambda
            .iter()
            .enumerate()
            .filter(|&(j, _)| j < r)
            .fold(0, |d, (j, &c)| (d + c * syn[r - 1 - j]) % GF);
        prev.insert(0, 0); // B(x) ← x·B(x)
        if delta == 0 {
            continue;
        }
        let mut next = lambda.clone();
        if next.len() < prev.len() {
            next.resize(prev.len(), 0);
        }
        for (k, &b) in prev.iter().enumerate() {
            next[k] = (next[k] + GF - (delta * b) % GF) % GF;
        }
        if 2 * l < r + erasures {
            let scale = inv(delta);
            // The un-shifted B is the old Λ scaled by 1/Δ; the shift comes next round.
            prev = lambda.iter().map(|&c| (c * scale) % GF).collect();
            l = r + erasures - l;
        }
        lambda = next;
    }
    while lambda.len() > 1 && lambda[lambda.len() - 1] == 0 {
        lambda.pop();
    }
    let degree = lambda.len() - 1;
    if degree == 0 || degree != l || 2 * degree > nc + erasures {
        return false;
    }

    // Chien search over the block's positions.
    let roots: Vec<usize> = (0..n)
        .filter(|&j| eval_lsf(&lambda, inv(locator(j))) == 0)
        .collect();
    if roots.len() != degree {
        return false;
    }

    // Forney: Ω(x) = S(x)·Λ(x) mod x^nc, e_k = −Ω(X_k⁻¹) / Λ′(X_k⁻¹).
    let mut omega = vec![0i32; nc];
    for (i, &s) in syn.iter().enumerate() {
        for (k, &c) in lambda.iter().enumerate() {
            if i + k < nc {
                omega[i + k] = (omega[i + k] + s * c) % GF;
            }
        }
    }
    let deriv: Vec<i32> = lambda
        .iter()
        .enumerate()
        .skip(1)
        .map(|(k, &c)| (c * (k as i32 % GF)) % GF)
        .collect();
    for &j in &roots {
        let x_inv = inv(locator(j));
        let denom = eval_lsf(&deriv, x_inv);
        if denom == 0 {
            return false;
        }
        let err = (GF - (eval_lsf(&omega, x_inv) * inv(denom)) % GF) % GF;
        wd[idx[j]] = ((wd[idx[j]] as i32 % GF + GF - err) % GF) as u8;
    }

    // The repaired block must be a codeword.
    (1..=nc as i32).all(|i| {
        let root = pow(ALPHA, i);
        idx.iter().fold(0, |s, &p| (s * root + wd[p] as i32) % GF) == 0
    })
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    /// Every interleaved block repairs any mix of `e` erasures and `t` errors with
    /// `e + 2t` up to its check-word count, and reports one error more.
    #[test]
    fn corrects_errors_and_erasures_up_to_capacity() {
        let mut seed = 0xD07C_0DE5_0113_0001u64;
        let mut next = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        // 300 data words interleave into several blocks (463 > 112 words).
        for nd in [1usize, 2, 9, 40, 74, 75, 150, 300] {
            let nc = 3 + nd / 2;
            let nw = nd + nc;
            let step = nw.div_ceil(112);
            let mut clean: Vec<u8> = (0..nd).map(|_| next(113) as u8).collect();
            clean.extend(core::iter::repeat_n(0u8, nc));
            rsencode(nd, nc, &mut clean);
            // Check words of block 0, the block the damage goes into.
            let nc_b = nw.div_ceil(step) - nd.div_ceil(step);
            for erasures in [0, 1, nc_b / 2, nc_b] {
                let errors = (nc_b - erasures) / 2;
                for over in [false, true] {
                    let mut wd = clean.clone();
                    let mut erased = vec![false; nw];
                    // Damage block 0 only (positions 0, step, 2·step, …).
                    let count = erasures + errors + usize::from(over);
                    if count > nw.div_ceil(step) {
                        continue;
                    }
                    for k in 0..count {
                        let p = k * step;
                        wd[p] = ((wd[p] as usize + 1 + next(112)) % 113) as u8;
                        erased[p] = k < erasures;
                    }
                    let ok = rsdecode(nd, nc, &mut wd, &erased);
                    if over {
                        assert!(!ok || wd != clean, "nd={nd} e={erasures}");
                    } else {
                        assert!(ok, "nd={nd} e={erasures} t={errors}");
                        assert_eq!(wd, clean, "nd={nd} e={erasures} t={errors}");
                    }
                }
            }
        }
    }

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
        wd.extend(core::iter::repeat_n(0u8, nc));
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
