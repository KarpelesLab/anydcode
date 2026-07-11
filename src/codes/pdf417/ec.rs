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
/// Implements syndrome computation, Berlekamp–Massey, a Chien search and Forney's
/// formula over GF(929).
pub fn decode(received: &[u32], k: usize) -> Option<Vec<u32>> {
    let n = received.len();
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
        return Some(received.to_vec());
    }

    // Berlekamp–Massey: find the error-locator polynomial Λ(x), least-significant
    // first with Λ_0 == 1.
    let mut sigma = vec![1u32];
    let mut prev = vec![1u32];
    let mut l = 0usize;
    let mut m = 1usize;
    let mut b = 1u32;

    for i in 0..k {
        let mut delta = syndromes[i];
        for j in 1..=l {
            if j < sigma.len() {
                delta = add(delta, mul(sigma[j], syndromes[i - j]));
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
    if num_errors == 0 || num_errors > k / 2 {
        return None;
    }

    // Chien search: Λ(α^-p) == 0 marks an error at power p (received index n-1-p).
    let mut positions = Vec::new();
    for p in 0..n {
        let x_inv = exp(((MOD - 1) - (p as u32 % (MOD - 1))) % (MOD - 1));
        let mut val = 0u32;
        let mut pw = 1u32;
        for &coef in &sigma {
            val = add(val, mul(coef, pw));
            pw = mul(pw, x_inv);
        }
        if val == 0 {
            positions.push(n - 1 - p);
        }
    }
    if positions.len() != num_errors {
        return None;
    }

    // Ω(x) = [S(x)·Λ(x)] mod x^k, with S(x) = Σ S_{i+1} x^i (least-significant first).
    let mut omega = vec![0u32; k];
    for (i, &s) in syndromes.iter().enumerate() {
        for (j, &sg) in sigma.iter().enumerate() {
            if i + j < k {
                omega[i + j] = add(omega[i + j], mul(s, sg));
            }
        }
    }

    // Forney's formula. The error magnitude at position `pos` (power p, X = α^p) is
    // e = ± Ω(X^-1) / Λ'(X^-1), where the unlabelled sign depends on the sign
    // conventions of Ω and the C = R − E relation. Both fixed roots (first root α^1)
    // remove any X^{1-b} factor. Rather than track the convention, compute the
    // magnitude and pick the sign that makes every syndrome vanish.
    let mut magnitudes = Vec::with_capacity(positions.len());
    for &pos in &positions {
        let p = (n - 1 - pos) as u32;
        let x_inv = inv(exp(p));
        // Ω(X^-1).
        let mut omega_val = 0u32;
        let mut pw = 1u32;
        for &c in &omega {
            omega_val = add(omega_val, mul(c, pw));
            pw = mul(pw, x_inv);
        }
        // Λ'(X^-1) = Σ_{j≥1} j·Λ_j (X^-1)^{j-1} (formal derivative in GF(929)).
        let mut deriv = 0u32;
        let mut pw = 1u32;
        for (j, &coef) in sigma.iter().enumerate().skip(1) {
            deriv = add(deriv, mul(mul((j as u32) % MOD, coef), pw));
            pw = mul(pw, x_inv);
        }
        if deriv == 0 {
            return None;
        }
        magnitudes.push(mul(omega_val, inv(deriv)));
    }

    for &sign_add in &[false, true] {
        let mut corrected = received.to_vec();
        for (&pos, &mag) in positions.iter().zip(&magnitudes) {
            corrected[pos] = if sign_add {
                add(corrected[pos], mag)
            } else {
                sub(corrected[pos], mag)
            };
        }
        if syndromes_vanish(&corrected, k) {
            return Some(corrected);
        }
    }
    None
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

#[cfg(test)]
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
