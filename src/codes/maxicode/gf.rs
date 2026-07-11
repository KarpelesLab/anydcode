//! GF(64) arithmetic and Reed–Solomon codes as used by MaxiCode (ISO/IEC 16023).
//!
//! The field is GF(2^6) with primitive polynomial `x^6 + x + 1` (`0x43`) and
//! generator element `α = 2`. MaxiCode's Reed–Solomon generator polynomial has its
//! first consecutive root at `α^1` (generator base 1), matching the ISO/IEC 16023
//! error-correction definition (and zint / BWIPP). Polynomials are coefficient
//! vectors, most-significant coefficient first.
//!
//! The systematic remainder produced by [`encode`] is exactly the check-symbol
//! sequence a spec-compliant MaxiCode places after its data symbols, so a real
//! symbol's `data || ecc` block evaluates to zero at every generator root — which is
//! what [`decode`] relies on to detect and correct errors.

/// Primitive polynomial for the MaxiCode field (`x^6 + x + 1`), with the `x^6` bit.
const PRIMITIVE: u16 = 0x43;

/// Index of the first consecutive root of the RS generator polynomial (`α^1`).
const GENERATOR_BASE: usize = 1;

/// Number of non-zero field elements (the multiplicative order), `2^6 - 1`.
const ORDER: usize = 63;

/// Precomputed exponent/log tables: `EXP[i] = α^i` and `LOG[EXP[i]] = i`.
struct Tables {
    exp: [u8; ORDER],
    log: [u8; 64],
}

const fn build_tables() -> Tables {
    let mut exp = [0u8; ORDER];
    let mut log = [0u8; 64];
    let mut x: u16 = 1;
    let mut i = 0;
    while i < ORDER {
        exp[i] = x as u8;
        log[x as usize] = i as u8;
        x <<= 1;
        if x & 0x40 != 0 {
            x ^= PRIMITIVE;
        }
        i += 1;
    }
    Tables { exp, log }
}

const TABLES: Tables = build_tables();

/// Multiply two field elements.
pub fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    let idx = (TABLES.log[a as usize] as usize + TABLES.log[b as usize] as usize) % ORDER;
    TABLES.exp[idx]
}

/// Divide `a` by `b`. Panics if `b == 0`.
pub fn div(a: u8, b: u8) -> u8 {
    assert!(b != 0, "division by zero in GF(64)");
    if a == 0 {
        return 0;
    }
    let idx = (TABLES.log[a as usize] as usize + ORDER - TABLES.log[b as usize] as usize) % ORDER;
    TABLES.exp[idx]
}

/// `α^n` for any non-negative exponent.
pub fn exp(n: usize) -> u8 {
    TABLES.exp[n % ORDER]
}

/// Multiplicative inverse of a non-zero element.
pub fn inv(a: u8) -> u8 {
    div(1, a)
}

/// Multiply two polynomials (coefficients most-significant first).
fn poly_mul(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; a.len() + b.len() - 1];
    for (i, &av) in a.iter().enumerate() {
        for (j, &bv) in b.iter().enumerate() {
            out[i + j] ^= mul(av, bv);
        }
    }
    out
}

/// The Reed–Solomon generator polynomial of degree `ec_len`,
/// `∏(x - α^i)` for `i` in `GENERATOR_BASE..GENERATOR_BASE + ec_len`.
pub fn generator(ec_len: usize) -> Vec<u8> {
    let mut g = vec![1u8];
    for i in 0..ec_len {
        g = poly_mul(&g, &[1, exp(GENERATOR_BASE + i)]);
    }
    g
}

/// Compute the `ec_len` Reed–Solomon check symbols for `data`.
///
/// This is the remainder of `data · x^ec_len` divided by the generator polynomial,
/// most-significant coefficient first — i.e. exactly the check symbols that follow
/// the data symbols in a MaxiCode message.
pub fn encode(data: &[u8], ec_len: usize) -> Vec<u8> {
    let gen_poly = generator(ec_len);
    let mut rem = vec![0u8; ec_len];
    for &d in data {
        let factor = d ^ rem[0];
        rem.remove(0);
        rem.push(0);
        if factor != 0 {
            for (i, &g) in gen_poly.iter().enumerate().skip(1) {
                rem[i - 1] ^= mul(g, factor);
            }
        }
    }
    rem
}

/// Attempt to correct up to `ec_len / 2` errors in `received` (data followed by EC
/// symbols), returning the corrected full block, or `None` if the errors exceed the
/// correction capacity.
///
/// Implements syndrome computation, Berlekamp–Massey, Chien search and Forney, with
/// the generator's first root at `α^GENERATOR_BASE`.
pub fn decode(received: &[u8], ec_len: usize) -> Option<Vec<u8>> {
    let n = received.len();
    // Syndromes S_i = R(α^(base + i)) for i in 0..ec_len.
    let mut syndromes = vec![0u8; ec_len];
    let mut all_zero = true;
    for (i, syn) in syndromes.iter_mut().enumerate() {
        let x = exp(GENERATOR_BASE + i);
        let mut s = 0u8;
        for &c in received {
            s = mul(s, x) ^ c;
        }
        *syn = s;
        if s != 0 {
            all_zero = false;
        }
    }
    if all_zero {
        return Some(received.to_vec());
    }

    // Berlekamp–Massey to find the error-locator polynomial Λ(x), least-significant
    // coefficient first with sigma[0] == 1.
    let mut sigma = vec![1u8];
    let mut prev = vec![1u8];
    let mut l = 0usize;
    let mut m = 1usize;
    let mut b = 1u8;

    for n_iter in 0..ec_len {
        let mut delta = syndromes[n_iter];
        for i in 1..=l {
            if i < sigma.len() {
                delta ^= mul(sigma[i], syndromes[n_iter - i]);
            }
        }
        if delta == 0 {
            m += 1;
        } else if 2 * l <= n_iter {
            let t = sigma.clone();
            sub_shift(&mut sigma, &prev, div(delta, b), m);
            l = n_iter + 1 - l;
            prev = t;
            b = delta;
            m = 1;
        } else {
            sub_shift(&mut sigma, &prev, div(delta, b), m);
            m += 1;
        }
    }

    while sigma.len() > 1 && *sigma.last().unwrap() == 0 {
        sigma.pop();
    }
    let num_errors = sigma.len() - 1;
    if num_errors > ec_len / 2 || num_errors == 0 {
        return None;
    }

    // Chien search: Λ(α^-j) = 0 marks an error at power j → received index n-1-j.
    let mut error_positions = Vec::new();
    for j in 0..n {
        let x_inv = exp((ORDER - (j % ORDER)) % ORDER);
        let mut val = 0u8;
        let mut pw = 1u8;
        for &coef in &sigma {
            val ^= mul(coef, pw);
            pw = mul(pw, x_inv);
        }
        if val == 0 {
            error_positions.push(n - 1 - j);
        }
    }
    if error_positions.len() != num_errors {
        return None;
    }

    // Forney: Ω(x) = [S(x)·Λ(x)] mod x^ec_len, with S(x) = Σ S_i x^i (both LSF).
    let mut omega = vec![0u8; ec_len];
    for (i, &s) in syndromes.iter().enumerate() {
        for (k, &sg) in sigma.iter().enumerate() {
            if i + k < ec_len {
                omega[i + k] ^= mul(s, sg);
            }
        }
    }

    let mut corrected = received.to_vec();
    for &pos in &error_positions {
        let p = n - 1 - pos; // error power
        let x = exp(p % ORDER); // X = α^p
        let x_inv = inv(x); // X^-1
        // Ω(X^-1).
        let mut omega_val = 0u8;
        let mut pw = 1u8;
        for &c in &omega {
            omega_val ^= mul(c, pw);
            pw = mul(pw, x_inv);
        }
        // Λ'(X^-1): GF(2) derivative keeps odd-degree terms.
        let mut sigma_deriv = 0u8;
        let mut pw = 1u8;
        for (k, &c) in sigma.iter().enumerate() {
            if k % 2 == 1 {
                sigma_deriv ^= mul(c, pw);
                pw = mul(pw, mul(x_inv, x_inv));
            }
        }
        if sigma_deriv == 0 {
            return None;
        }
        // First consecutive root is α^1 (base 1), so the X^(1-base) factor is 1:
        // e = Ω(X^-1) / Λ'(X^-1).
        corrected[pos] ^= div(omega_val, sigma_deriv);
    }

    // Verify the correction satisfies all syndromes.
    for i in 0..ec_len {
        let mut s = 0u8;
        let x = exp(GENERATOR_BASE + i);
        for &c in &corrected {
            s = mul(s, x) ^ c;
        }
        if s != 0 {
            return None;
        }
    }
    Some(corrected)
}

/// `dst ← dst − scale · x^shift · src`, all polynomials least-significant first.
fn sub_shift(dst: &mut Vec<u8>, src: &[u8], scale: u8, shift: usize) {
    let needed = src.len() + shift;
    if dst.len() < needed {
        dst.resize(needed, 0);
    }
    for (i, &s) in src.iter().enumerate() {
        dst[i + shift] ^= mul(scale, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_axioms() {
        assert_eq!(mul(0, 5), 0);
        assert_eq!(mul(1, 5), 5);
        for a in 1u8..64 {
            assert_eq!(mul(a, inv(a)), 1, "inverse of {a}");
        }
        assert_eq!(exp(ORDER), 1);
        // 6-bit field: every element is < 64.
        for i in 0..ORDER {
            assert!(exp(i) < 64);
        }
    }

    #[test]
    fn rs_roundtrip_clean() {
        let data = [34u8, 20, 45, 20, 17, 18, 2, 18, 7, 0];
        let ec = encode(&data, 10);
        let mut block = data.to_vec();
        block.extend_from_slice(&ec);
        // A freshly encoded block must satisfy all parity checks.
        assert_eq!(decode(&block, 10).as_deref(), Some(block.as_slice()));
    }

    #[test]
    fn rs_corrects_errors() {
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let ec = encode(&data, 20);
        let mut block = data.to_vec();
        block.extend_from_slice(&ec);
        let clean = block.clone();
        // Up to ec_len/2 = 10 errors correctable; flip 6 spread out.
        for k in 0..6 {
            block[k * 4] ^= 0x2A;
        }
        assert_eq!(decode(&block, 20).as_deref(), Some(clean.as_slice()));
    }
}
