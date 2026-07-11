//! GF(256) arithmetic and Reed–Solomon codes as used by QR.
//!
//! The field is GF(2^8) with primitive polynomial `x^8 + x^4 + x^3 + x^2 + 1`
//! (`0x11D`) and generator element `α = 2`, matching ISO/IEC 18004. Polynomials are
//! represented as coefficient vectors, **most-significant coefficient first**.

/// Primitive polynomial for the QR field, without the implicit `x^8` bit.
const PRIMITIVE: u16 = 0x11D;

/// Precomputed exponent/log tables: `EXP[i] = α^i` (for `i` in `0..255`) and
/// `LOG[EXP[i]] = i`. `LOG[0]` is unused.
struct Tables {
    exp: [u8; 255],
    log: [u8; 256],
}

const fn build_tables() -> Tables {
    let mut exp = [0u8; 255];
    let mut log = [0u8; 256];
    let mut x: u16 = 1;
    let mut i = 0;
    while i < 255 {
        exp[i] = x as u8;
        log[x as usize] = i as u8;
        x <<= 1;
        if x & 0x100 != 0 {
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
    let idx = (TABLES.log[a as usize] as usize + TABLES.log[b as usize] as usize) % 255;
    TABLES.exp[idx]
}

/// Divide `a` by `b`. Panics if `b == 0`.
pub fn div(a: u8, b: u8) -> u8 {
    assert!(b != 0, "division by zero in GF(256)");
    if a == 0 {
        return 0;
    }
    let idx = (TABLES.log[a as usize] as usize + 255 - TABLES.log[b as usize] as usize) % 255;
    TABLES.exp[idx]
}

/// `α^n` for any non-negative exponent.
pub fn exp(n: usize) -> u8 {
    TABLES.exp[n % 255]
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
/// `∏(x - α^i)` for `i` in `0..ec_len`. Cached-free: cheap enough to build per block.
pub fn generator(ec_len: usize) -> Vec<u8> {
    let mut g = vec![1u8];
    for i in 0..ec_len {
        g = poly_mul(&g, &[1, exp(i)]);
    }
    g
}

/// Compute the `ec_len` Reed–Solomon error-correction codewords for `data`.
///
/// This is the remainder of `data · x^ec_len` divided by the generator polynomial.
pub fn encode(data: &[u8], ec_len: usize) -> Vec<u8> {
    let gen_poly = generator(ec_len);
    // Working buffer holds the running remainder; process message coefficients in turn.
    let mut rem = vec![0u8; ec_len];
    for &d in data {
        let factor = d ^ rem[0];
        rem.remove(0);
        rem.push(0);
        if factor != 0 {
            // rem ^= factor * gen[1..]  (gen[0] == 1 handles the leading term)
            for (i, &g) in gen_poly.iter().enumerate().skip(1) {
                rem[i - 1] ^= mul(g, factor);
            }
        }
    }
    rem
}

/// Attempt to correct up to `ec_len / 2` errors in `received` (data followed by EC
/// codewords), returning the corrected full codeword block, or `None` if the errors
/// exceed the correction capacity.
///
/// Implements syndrome computation, Berlekamp–Massey, Chien search and Forney.
pub fn decode(received: &[u8], ec_len: usize) -> Option<Vec<u8>> {
    let n = received.len();
    // Syndromes S_i = R(α^i) for i in 0..ec_len (the generator's roots are α^0..α^(n-1)).
    let mut syndromes = vec![0u8; ec_len];
    let mut all_zero = true;
    for (i, syn) in syndromes.iter_mut().enumerate() {
        let x = exp(i);
        // Horner over received, most-significant first.
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

    // Berlekamp–Massey to find the error-locator polynomial Λ(x), stored
    // least-significant-first: Λ(x) = Σ sigma[i]·x^i with sigma[0] == 1.
    let mut sigma = vec![1u8];
    let mut prev = vec![1u8]; // B(x): last Λ before the previous length change
    let mut l = 0usize; // current assumed number of errors (deg Λ)
    let mut m = 1usize; // steps since the last length change
    let mut b = 1u8; // discrepancy at that last change

    for n_iter in 0..ec_len {
        // Discrepancy δ = S_n + Σ_{i=1}^{l} Λ_i · S_{n-i}.
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
            // Λ(x) ← Λ(x) − (δ/b)·x^m·B(x)
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

    // Trim trailing zero coefficients so the degree reflects the true error count.
    while sigma.len() > 1 && *sigma.last().unwrap() == 0 {
        sigma.pop();
    }
    let num_errors = sigma.len() - 1;
    if num_errors > ec_len / 2 || num_errors == 0 {
        return None;
    }

    // Chien search: Λ(α^-j) = 0 marks an error at power j, i.e. received index n-1-j.
    let mut error_positions = Vec::new();
    for j in 0..n {
        let x_inv = exp((255 - (j % 255)) % 255);
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
        let x = exp(p % 255); // X = α^p
        let x_inv = inv(x); // X^-1
        // Ω(X^-1) = Σ omega[k]·(X^-1)^k.
        let mut omega_val = 0u8;
        let mut pw = 1u8;
        for &c in &omega {
            omega_val ^= mul(c, pw);
            pw = mul(pw, x_inv);
        }
        // Λ'(X^-1): GF(2) derivative keeps odd-degree terms → Σ_{k odd} Λ_k (X^-1)^{k-1}.
        let mut sigma_deriv = 0u8;
        let mut pw = 1u8; // (X^-1)^0 for k = 1
        for (k, &c) in sigma.iter().enumerate() {
            if k % 2 == 1 {
                sigma_deriv ^= mul(c, pw);
                pw = mul(pw, mul(x_inv, x_inv));
            }
        }
        if sigma_deriv == 0 {
            return None;
        }
        // First consecutive root α^0 (c = 0): e = X · Ω(X^-1) / Λ'(X^-1).
        corrected[pos] ^= mul(x, div(omega_val, sigma_deriv));
    }

    // Verify the correction actually satisfies all syndromes.
    for i in 0..ec_len {
        let mut s = 0u8;
        let x = exp(i);
        for &c in &corrected {
            s = mul(s, x) ^ c;
        }
        if s != 0 {
            return None;
        }
    }
    Some(corrected)
}

/// `dst ← dst − scale · x^shift · src`, where all polynomials are least-significant
/// first (index i is the coefficient of x^i). Grows `dst` by appending zeros.
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
        for a in 1u8..=255 {
            assert_eq!(mul(a, inv(a)), 1, "inverse of {a}");
        }
        // α^255 == α^0 == 1
        assert_eq!(exp(255), 1);
    }

    #[test]
    fn rs_roundtrip_clean() {
        let data = [0x10, 0x20, 0x0C, 0x56, 0x61, 0x80, 0xEC, 0x11];
        let ec = encode(&data, 10);
        let mut block = data.to_vec();
        block.extend_from_slice(&ec);
        assert_eq!(decode(&block, 10).as_deref(), Some(block.as_slice()));
    }

    #[test]
    fn rs_corrects_errors() {
        let data = [0x10, 0x20, 0x0C, 0x56, 0x61, 0x80, 0xEC, 0x11];
        let ec = encode(&data, 10);
        let mut block = data.to_vec();
        block.extend_from_slice(&ec);
        let clean = block.clone();
        // Introduce up to 5 errors (= ec_len/2).
        block[0] ^= 0xFF;
        block[3] ^= 0x0F;
        block[7] ^= 0xA5;
        block[11] ^= 0x33;
        block[16] ^= 0x01;
        assert_eq!(decode(&block, 10).as_deref(), Some(clean.as_slice()));
    }
}
