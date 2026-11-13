//! GF(929) arithmetic and Reed–Solomon codes as used by PDF417.
//!
//! 929 is prime, so the field is simply the integers modulo 929 with ordinary
//! modular addition and multiplication — no polynomial basis is needed. The
//! generator element is `α = 3`, a primitive root of 929, and the Reed–Solomon
//! generator polynomial has roots `α^1 .. α^k` (note: the first root is `α^1`, not
//! `α^0`), matching ISO/IEC 15438 §4.10.
//!
//! Codeword blocks are treated as polynomials with the first codeword as the
//! highest-degree coefficient and the last (error-correction) codeword as the
//! constant term, so a clean block evaluates to zero at every generator root.

/// The field modulus.
use alloc::{vec, vec::Vec};

pub const MOD: u32 = 929;

/// The primitive element used to build the generator polynomial.
const ALPHA: u32 = 3;

/// Add two field elements.
#[inline]
fn add(a: u32, b: u32) -> u32 {
    (a + b) % MOD
}

/// Subtract `b` from `a`.
#[inline]
fn sub(a: u32, b: u32) -> u32 {
    (a + MOD - b % MOD) % MOD
}

/// Multiply two field elements.
#[inline]
fn mul(a: u32, b: u32) -> u32 {
    (a * b) % MOD
}

/// Raise `base` to `exp` in the field.
fn pow(base: u32, mut exp: u32) -> u32 {
    let mut acc = 1u32;
    let mut b = base % MOD;
    while exp > 0 {
        if exp & 1 == 1 {
            acc = mul(acc, b);
        }
        b = mul(b, b);
        exp >>= 1;
    }
    acc
}

/// Multiplicative inverse of a non-zero element (Fermat: `a^(p-2)`).
fn inv(a: u32) -> u32 {
    debug_assert!(!a.is_multiple_of(MOD), "inverse of zero in GF(929)");
    pow(a, MOD - 2)
}

/// `α^n`.
#[inline]
fn exp(n: u32) -> u32 {
    pow(ALPHA, n)
}

/// The Reed–Solomon generator polynomial of degree `k`, `∏(x - α^i)` for
/// `i` in `1..=k`, returned as coefficients least-significant first (constant term
/// at index 0; the monic leading coefficient at index `k` is `1`).
pub fn generator(k: usize) -> Vec<u32> {
    let mut g = vec![1u32];
    for i in 1..=k as u32 {
        let root = exp(i);
        // Multiply g(x) by (x - root): new[j] = g[j-1] - root*g[j].
        let mut next = vec![0u32; g.len() + 1];
        for (j, &c) in g.iter().enumerate() {
            next[j] = add(next[j], mul(c, sub(0, root)));
            next[j + 1] = add(next[j + 1], c);
        }
        g = next;
    }
    g
}

/// Compute the `k` Reed–Solomon error-correction codewords for `data`.
///
/// The returned codewords are appended to `data` (in order) to form a block that is
/// divisible by the generator polynomial. This is the algorithm of ISO/IEC 15438
/// §4.10.
pub fn encode(data: &[u32], k: usize) -> Vec<u32> {
    // Coefficients of the generator excluding the monic leading term, indexed by
    // power: `coeff[j]` multiplies `x^j`.
    let g = generator(k);
    let mut e = vec![0u32; k];
    for &d in data {
        let t = add(d % MOD, e[k - 1]);
        // e(x) <- (e(x) shifted) - t * (g(x) without leading term)
        let mut next = vec![0u32; k];
        for j in (1..k).rev() {
            next[j] = sub(e[j - 1], mul(t, g[j]));
        }
        next[0] = sub(0, mul(t, g[0]));
        e = next;
    }
    // The check codewords are the negated remainder, emitted most-significant first.
    let mut out = Vec::with_capacity(k);
    for j in (0..k).rev() {
        out.push(if e[j] == 0 { 0 } else { MOD - e[j] });
    }
    out
}

/// Attempt to correct up to `k / 2` errors in `received` (data followed by the `k`
/// error-correction codewords), returning the corrected block, or `None` if the
/// errors exceed the correction capacity.
///
/// This is [`decode_with_erasures`] with no erasures.
pub fn decode(received: &[u32], k: usize) -> Option<Vec<u32>> {
    decode_with_erasures(received, k, &[]).map(|(block, _)| block)
}

/// Attempt to correct `received` (data followed by the `k` error-correction
/// codewords) given the indices of the codewords known to be missing or unreliable.
///
/// An *erasure* — a codeword whose position is known, whatever value stands in for it
/// in `received` — costs one check codeword to fill in; an *error*, whose position must
/// be found as well, costs two. Any `e` errors and `f` erasures with `2e + f <= k` are
/// corrected. Returns the corrected block and the number of errors found outside the
/// erased positions, or `None` if the damage exceeds that capacity (or `erasures`
/// names a position twice or one outside the block).
///
/// The result is always a genuine codeword, but the more of the `k` check codewords
/// the correction consumes, the less remains to tell a block that was damaged from
/// one that was never a codeword: with `f = k` erasures *every* input is "corrected".
/// A caller that must not accept garbage should leave some of them unused.
///
/// Implements syndrome computation, the erasure locator, Berlekamp–Massey on the
/// modified syndromes, a Chien search and Forney's formula over GF(929).
pub fn decode_with_erasures(
    received: &[u32],
    k: usize,
    erasures: &[usize],
) -> Option<(Vec<u32>, usize)> {
    let n = received.len();
    let f = erasures.len();
    if f > k || erasures.iter().any(|&pos| pos >= n) {
        return None;
    }
    // Syndromes S_j = R(α^j) for j in 1..=k.
    let mut syndromes = vec![0u32; k];
    let mut all_zero = true;
    for (idx, syn) in syndromes.iter_mut().enumerate() {
        let root = exp((idx + 1) as u32);
        // Horner over received, most-significant coefficient first.
        let mut s = 0u32;
        for &c in received {
            s = add(mul(s, root), c % MOD);
        }
        *syn = s;
        if s != 0 {
            all_zero = false;
        }
    }
    if all_zero {
        return Some((received.iter().map(|&c| c % MOD).collect(), 0));
    }

    // Erasure locator Γ(x) = ∏ (1 − X_i·x), X_i = α^p for an erasure at power p
    // (received index n-1-p); least-significant first, like every polynomial below.
    let mut gamma = vec![1u32];
    for &pos in erasures {
        let x = exp(((n - 1 - pos) % (MOD as usize - 1)) as u32);
        gamma.push(0);
        for j in (1..gamma.len()).rev() {
            gamma[j] = sub(gamma[j], mul(x, gamma[j - 1]));
        }
    }
    // A repeated position gives Γ a double root, and Forney's formula nothing to
    // divide by; it is refused there.

    // Modified syndromes T(x) = [S(x)·Γ(x)] mod x^k, with S(x) = Σ S_{i+1} x^i. Its
    // coefficients from x^f up obey the recurrence of the *error* locator alone.
    let modified = mul_mod_xk(&syndromes, &gamma, k);
    let tail = &modified[f..];

    // Berlekamp–Massey: find the error-locator polynomial Λ(x) with Λ_0 == 1.
    let mut sigma = vec![1u32];
    let mut prev = vec![1u32];
    let mut l = 0usize;
    let mut m = 1usize;
    let mut b = 1u32;

    for i in 0..tail.len() {
        let mut delta = tail[i];
        for j in 1..=l {
            if j < sigma.len() {
                delta = add(delta, mul(sigma[j], tail[i - j]));
            }
        }
        if delta == 0 {
            m += 1;
        } else if 2 * l <= i {
            let t = sigma.clone();
            sub_shift(&mut sigma, &prev, mul(delta, inv(b)), m);
            l = i + 1 - l;
            prev = t;
            b = delta;
            m = 1;
        } else {
            sub_shift(&mut sigma, &prev, mul(delta, inv(b)), m);
            m += 1;
        }
    }

    while sigma.len() > 1 && *sigma.last().unwrap() == 0 {
        sigma.pop();
    }
    let num_errors = sigma.len() - 1;
    if (num_errors == 0 && f == 0) || 2 * num_errors + f > k {
        return None;
    }

    // Chien search: Λ(α^-p) == 0 marks an error at power p (received index n-1-p).
    let mut positions = Vec::new();
    if num_errors > 0 {
        for p in 0..n {
            let x_inv = exp(((MOD - 1) - (p as u32 % (MOD - 1))) % (MOD - 1));
            if eval(&sigma, x_inv) == 0 {
                positions.push(n - 1 - p);
            }
        }
    }
    if positions.len() != num_errors {
        return None;
    }
    positions.extend_from_slice(erasures);

    // The combined locator Ψ(x) = Λ(x)·Γ(x) and Ω(x) = [S(x)·Ψ(x)] mod x^k.
    let psi = mul_mod_xk(&sigma, &gamma, k + 1);
    let omega = mul_mod_xk(&syndromes, &psi, k);

    // Forney's formula. The error magnitude at position `pos` (power p, X = α^p) is
    // e = ± Ω(X^-1) / Ψ'(X^-1), where the unlabelled sign depends on the sign
    // conventions of Ω and the C = R − E relation. Both fixed roots (first root α^1)
    // remove any X^{1-b} factor. Rather than track the convention, compute the
    // magnitude and pick the sign that makes every syndrome vanish.
    let mut magnitudes = Vec::with_capacity(positions.len());
    for &pos in &positions {
        let p = ((n - 1 - pos) % (MOD as usize - 1)) as u32;
        let x_inv = inv(exp(p));
        let omega_val = eval(&omega, x_inv);
        // Ψ'(X^-1) = Σ_{j≥1} j·Ψ_j (X^-1)^{j-1} (formal derivative in GF(929)).
        let mut deriv = 0u32;
        let mut pw = 1u32;
        for (j, &coef) in psi.iter().enumerate().skip(1) {
            deriv = add(deriv, mul(mul((j as u32) % MOD, coef), pw));
            pw = mul(pw, x_inv);
        }
        if deriv == 0 {
            return None;
        }
        magnitudes.push(mul(omega_val, inv(deriv)));
    }

    for &sign_add in &[false, true] {
        let mut corrected: Vec<u32> = received.iter().map(|&c| c % MOD).collect();
        for (&pos, &mag) in positions.iter().zip(&magnitudes) {
            corrected[pos] = if sign_add {
                add(corrected[pos], mag)
            } else {
                sub(corrected[pos], mag)
            };
        }
        if syndromes_vanish(&corrected, k) {
            return Some((corrected, num_errors));
        }
    }
    None
}

/// `p(x)` at `x`, coefficients least-significant first.
fn eval(p: &[u32], x: u32) -> u32 {
    p.iter().rev().fold(0, |acc, &c| add(mul(acc, x), c))
}

/// `[a(x)·b(x)] mod x^k`, least-significant first; the result has `k` coefficients.
fn mul_mod_xk(a: &[u32], b: &[u32], k: usize) -> Vec<u32> {
    let mut out = vec![0u32; k];
    for (i, &x) in a.iter().enumerate().take(k) {
        if x == 0 {
            continue;
        }
        for (j, &y) in b.iter().enumerate().take(k - i) {
            out[i + j] = add(out[i + j], mul(x, y));
        }
    }
    out
}

/// Whether every syndrome `R(α^j)`, `j` in `1..=k`, is zero for `block`.
fn syndromes_vanish(block: &[u32], k: usize) -> bool {
    for idx in 0..k {
        let root = exp((idx + 1) as u32);
        let mut s = 0u32;
        for &c in block {
            s = add(mul(s, root), c);
        }
        if s != 0 {
            return false;
        }
    }
    true
}

/// `dst ← dst − scale · x^shift · src`, all polynomials least-significant first.
fn sub_shift(dst: &mut Vec<u32>, src: &[u32], scale: u32, shift: usize) {
    let needed = src.len() + shift;
    if dst.len() < needed {
        dst.resize(needed, 0);
    }
    for (i, &s) in src.iter().enumerate() {
        dst[i + shift] = sub(dst[i + shift], mul(scale, s));
    }
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    /// The generator polynomials must match the coefficient tables documented in
    /// ISO/IEC 15438 §4.10 (reproduced e.g. in ZXing's `PDF417ErrorCorrection`).
    #[test]
    fn generator_matches_iso_tables() {
        // level -> (k, expected coefficients, least-significant first, no leading 1).
        let k2 = generator(2);
        assert_eq!(&k2[..2], &[27, 917]);
        let k4 = generator(4);
        assert_eq!(&k4[..4], &[522, 568, 723, 809]);
        let k8 = generator(8);
        assert_eq!(&k8[..8], &[237, 308, 436, 284, 646, 653, 428, 379]);
        // Every generator is monic of degree k.
        assert_eq!(*k2.last().unwrap(), 1);
        assert_eq!(*k8.last().unwrap(), 1);
    }

    #[test]
    fn rs_roundtrip_clean() {
        let data = [7u32, 100, 200, 300, 400, 500, 600];
        let ec = encode(&data, 8);
        let mut block = data.to_vec();
        block.extend_from_slice(&ec);
        assert_eq!(decode(&block, 8).as_deref(), Some(block.as_slice()));
    }

    #[test]
    fn rs_corrects_errors() {
        let data = [7u32, 100, 200, 300, 400, 500, 600, 111, 222, 333];
        let ec = encode(&data, 16); // corrects up to 8 errors
        let mut block = data.to_vec();
        block.extend_from_slice(&ec);
        let clean = block.clone();
        // Inject 8 errors across data and EC regions.
        block[0] = (block[0] + 5) % MOD;
        block[2] = (block[2] + 900) % MOD;
        block[5] = 0;
        block[7] = 928;
        block[9] = (block[9] + 1) % MOD;
        block[11] = (block[11] + 400) % MOD;
        block[14] = (block[14] + 77) % MOD;
        block[17] = (block[17] + 200) % MOD;
        assert_eq!(decode(&block, 16).as_deref(), Some(clean.as_slice()));
    }

    /// Every error-correction size in use (the PDF417 levels and MicroPDF417's odd
    /// counts) corrects exactly `k / 2` random errors anywhere in the block, and one
    /// error more is never silently "corrected" into a different block that still
    /// claims to be the original.
    #[test]
    fn rs_corrects_up_to_capacity_for_every_size() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n as u64) as usize
        };
        for k in [2usize, 4, 7, 8, 9, 13, 16, 21, 32, 50, 64, 128, 512] {
            for _ in 0..4 {
                let data_len = (1 + next(60)).min(928 - k);
                let data: Vec<u32> = (0..data_len).map(|_| next(929) as u32).collect();
                let mut clean = data.clone();
                clean.extend_from_slice(&encode(&data, k));

                let mut inject = |count: usize| {
                    let mut block = clean.clone();
                    let mut hit = vec![false; block.len()];
                    let mut done = 0;
                    while done < count {
                        let pos = next(block.len());
                        if !hit[pos] {
                            hit[pos] = true;
                            block[pos] = (block[pos] + 1 + next(928) as u32) % MOD;
                            done += 1;
                        }
                    }
                    block
                };

                let t = k / 2;
                let block = inject(t);
                assert_eq!(
                    decode(&block, k).as_deref(),
                    Some(clean.as_slice()),
                    "k={k}"
                );
                if t < clean.len() {
                    let block = inject(t + 1);
                    if let Some(out) = decode(&block, k) {
                        // Beyond capacity a decode may land on another valid codeword,
                        // but it must be a genuine one.
                        assert!(syndromes_vanish(&out, k), "k={k}");
                    }
                }
            }
        }
    }

    /// A small deterministic generator for the randomised tests.
    fn xorshift(seed: u64) -> impl FnMut(usize) -> usize {
        let mut state = seed;
        move |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n as u64) as usize
        }
    }

    /// `count` distinct positions below `len`, none of them in `taken`.
    fn pick(next: &mut impl FnMut(usize) -> usize, len: usize, count: usize) -> Vec<usize> {
        let mut all: Vec<usize> = (0..len).collect();
        for i in 0..count {
            let j = i + next(len - i);
            all.swap(i, j);
        }
        all.truncate(count);
        all
    }

    #[test]
    fn rs_fills_in_erasures() {
        let data = [5u32, 100, 200, 300, 400];
        let mut clean = data.to_vec();
        clean.extend_from_slice(&encode(&data, 4));
        // As many erasures as check codewords, whatever stands in for them.
        for stand_in in [0u32, 17, 928] {
            let mut block = clean.clone();
            for pos in [1, 4, 6, 8] {
                block[pos] = stand_in;
            }
            let (out, errors) = decode_with_erasures(&block, 4, &[1, 4, 6, 8]).expect("decodes");
            assert_eq!((out, errors), (clean.clone(), 0));
            // The same damage as errors of unknown position is twice too much.
            assert_ne!(decode(&block, 4).as_deref(), Some(clean.as_slice()));
        }
        // Two erasures and one error: 2·1 + 2 = 4.
        let mut block = clean.clone();
        block[0] = 0;
        block[3] = 0;
        block[7] = (block[7] + 1) % MOD;
        assert_eq!(
            decode_with_erasures(&block, 4, &[0, 3]),
            Some((clean.clone(), 1))
        );
        // An erased position that happens to hold the right value costs nothing more.
        assert_eq!(
            decode_with_erasures(&clean, 4, &[2, 5]),
            Some((clean.clone(), 0))
        );
        // Malformed erasure lists are refused, not indexed.
        let mut block = clean.clone();
        block[2] = (block[2] + 1) % MOD;
        assert_eq!(decode_with_erasures(&block, 4, &[2, 2]), None);
        assert_eq!(decode_with_erasures(&block, 4, &[clean.len()]), None);
        assert_eq!(decode_with_erasures(&block, 4, &[0, 1, 2, 3, 4]), None);
    }

    /// For every error-correction size in use, any mix of `e` errors and `f` erasures
    /// with `2e + f == k` (or `k - 1`) is corrected and the errors counted; one error
    /// more is refused or lands on some other genuine codeword, never on a non-codeword.
    #[test]
    fn rs_corrects_errors_and_erasures_up_to_capacity() {
        let mut next = xorshift(0x9e37_79b9_7f4a_7c15);
        for k in [2usize, 4, 7, 8, 9, 13, 16, 21, 32, 50, 64, 128, 512] {
            for round in 0..6 {
                let data_len = (1 + next(60)).min(928 - k);
                let data: Vec<u32> = (0..data_len).map(|_| next(929) as u32).collect();
                let mut clean = data.clone();
                clean.extend_from_slice(&encode(&data, k));
                let n = clean.len();

                // Rounds 0 and 1 are the extremes: no erasures, nothing but erasures.
                let f = match round {
                    0 => 0,
                    1 => k,
                    _ => next(k + 1),
                }
                .min(n);
                let e = ((k - f) / 2).min(n - f);
                let damage = |errors: usize, next: &mut dyn FnMut(usize) -> usize| {
                    let mut next = |m: usize| next(m);
                    let hit = pick(&mut next, n, f + errors);
                    let mut block = clean.clone();
                    for &pos in &hit[..f] {
                        block[pos] = next(929) as u32;
                    }
                    for &pos in &hit[f..] {
                        block[pos] = (block[pos] + 1 + next(928) as u32) % MOD;
                    }
                    (block, hit[..f].to_vec())
                };

                let (block, erased) = damage(e, &mut next);
                assert_eq!(
                    decode_with_erasures(&block, k, &erased),
                    Some((clean.clone(), e)),
                    "k={k} f={f} e={e}"
                );
                if f + e < n {
                    let (block, erased) = damage(e + 1, &mut next);
                    if let Some((out, _)) = decode_with_erasures(&block, k, &erased) {
                        assert!(syndromes_vanish(&out, k), "k={k} f={f}");
                        if 2 * (e + 1) + f > k + 1 {
                            // Too far from `clean` to be mistaken for it.
                            assert_ne!(out, clean, "k={k} f={f}");
                        }
                    }
                }
            }
        }
    }

    /// Arbitrary input — any values, any erasure list — never panics, and whatever is
    /// returned is a codeword that differs from the input only where allowed.
    #[test]
    fn rs_survives_garbage() {
        let mut next = xorshift(0x1234_5678_9abc_def1);
        for round in 0..400 {
            let k = [0usize, 1, 2, 3, 4, 8, 16, 32, 64][next(9)];
            let n = next(80);
            // Values beyond the field too: the decoder reduces what it is given.
            let block: Vec<u32> = (0..n).map(|_| next(1200) as u32).collect();
            let erased: Vec<usize> = (0..next(k + 3)).map(|_| next(n + 2)).collect();
            if round % 2 == 0 {
                let _ = decode(&block, k);
            }
            if let Some((out, errors)) = decode_with_erasures(&block, k, &erased) {
                assert_eq!(out.len(), n);
                assert!(syndromes_vanish(&out, k));
                let changed = (0..n)
                    .filter(|&i| out[i] != block[i] % MOD && !erased.contains(&i))
                    .count();
                assert_eq!(changed, errors);
                assert!(2 * errors + erased.len() <= k);
            }
        }
    }

    #[test]
    fn rs_detects_uncorrectable() {
        let data = [1u32, 2, 3, 4];
        let ec = encode(&data, 4); // corrects up to 2 errors
        let mut block = data.to_vec();
        block.extend_from_slice(&ec);
        // Three errors exceed capacity.
        block[0] = (block[0] + 1) % MOD;
        block[1] = (block[1] + 1) % MOD;
        block[2] = (block[2] + 1) % MOD;
        assert_eq!(decode(&block, 4), None);
    }
}
