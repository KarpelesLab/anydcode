//! Galois-field arithmetic and Reed–Solomon codes for Han Xin Code (ISO/IEC 20830).
//!
//! Han Xin uses **two** fields, so this module is written against a runtime-built
//! [`GaloisField`] rather than the single hard-coded table QR uses:
//!
//! - **Data codewords** live in GF(2^8) with primitive polynomial `0x163`
//!   (`x^8 + x^6 + x^5 + x + 1`) — see zint `hanxin.c` `hx_add_ecc`
//!   (`zint_rs_init_gf(&rs, 0x163)`). This is a *different* field from QR's `0x11D`,
//!   so the tables are deliberately not shared.
//! - **Function information** lives in GF(2^4) with primitive polynomial `0x13`
//!   (`x^4 + x + 1`) — zint `hx_set_function_info` (`zint_rs_init_gf(&rs, 0x13)`).
//!
//! Both use generator element `α = 2` and, per zint `zint_rs_init_code(&rs, nsym, 1)`,
//! a **first consecutive root of `α^1`** (unlike QR's `α^0`). Polynomials are
//! most-significant coefficient first.

/// A binary Galois field GF(2^m) with a fixed primitive polynomial.
#[derive(Debug, Clone)]
pub struct GaloisField {
    /// `exp[i] = α^i` for `i` in `0..order`.
    exp: Vec<u8>,
    /// `log[α^i] = i`; `log[0]` is unused.
    log: Vec<u8>,
    /// Multiplicative order of the field, `2^m - 1`.
    order: usize,
}

impl GaloisField {
    /// Build the field of the given `size` (`2^m`, e.g. 256 or 16) using `primitive`
    /// (the reduction polynomial including its high `x^m` bit, e.g. `0x163`).
    pub fn new(primitive: u16, size: usize) -> Self {
        let order = size - 1;
        let mut exp = vec![0u8; order];
        let mut log = vec![0u8; size];
        let mut x: u16 = 1;
        for (i, e) in exp.iter_mut().enumerate() {
            *e = x as u8;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & size as u16 != 0 {
                x ^= primitive;
            }
        }
        GaloisField { exp, log, order }
    }

    /// The GF(2^8) field used for Han Xin data codewords (primitive `0x163`).
    pub fn data() -> Self {
        GaloisField::new(0x163, 256)
    }

    /// The GF(2^4) field used for Han Xin function information (primitive `0x13`).
    pub fn function() -> Self {
        GaloisField::new(0x13, 16)
    }

    /// `α^n` for any non-negative exponent.
    fn exp_of(&self, n: usize) -> u8 {
        self.exp[n % self.order]
    }

    /// Multiply two field elements.
    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            return 0;
        }
        let idx = (self.log[a as usize] as usize + self.log[b as usize] as usize) % self.order;
        self.exp[idx]
    }

    /// Divide `a` by `b`. Panics if `b == 0`.
    fn div(&self, a: u8, b: u8) -> u8 {
        assert!(b != 0, "division by zero in GF");
        if a == 0 {
            return 0;
        }
        let idx = (self.log[a as usize] as usize + self.order - self.log[b as usize] as usize)
            % self.order;
        self.exp[idx]
    }

    /// Multiplicative inverse of a non-zero element.
    fn inv(&self, a: u8) -> u8 {
        self.div(1, a)
    }

    /// `base` raised to a (possibly negative) integer power.
    fn pow(&self, base: u8, exp: i32) -> u8 {
        if base == 0 {
            return 0;
        }
        let l = self.log[base as usize] as i64;
        let o = self.order as i64;
        let mut idx = (l * exp as i64) % o;
        if idx < 0 {
            idx += o;
        }
        self.exp[idx as usize]
    }

    /// Multiply two polynomials (coefficients most-significant first).
    fn poly_mul(&self, a: &[u8], b: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; a.len() + b.len() - 1];
        for (i, &av) in a.iter().enumerate() {
            for (j, &bv) in b.iter().enumerate() {
                out[i + j] ^= self.mul(av, bv);
            }
        }
        out
    }

    /// Reed–Solomon generator polynomial `∏(x − α^(fcr+i))` for `i` in `0..nsym`,
    /// most-significant coefficient first.
    fn generator(&self, nsym: usize, fcr: usize) -> Vec<u8> {
        let mut g = vec![1u8];
        for i in 0..nsym {
            g = self.poly_mul(&g, &[1, self.exp_of(fcr + i)]);
        }
        g
    }

    /// Compute `nsym` Reed–Solomon EC codewords for `data`, most-significant first.
    ///
    /// This is the remainder of `data · x^nsym` modulo the generator polynomial and,
    /// with `fcr = 1`, reproduces zint's `zint_rs_encode` output byte-for-byte.
    pub fn rs_encode(&self, data: &[u8], nsym: usize, fcr: usize) -> Vec<u8> {
        let gen_poly = self.generator(nsym, fcr);
        let mut rem = vec![0u8; nsym];
        for &d in data {
            let factor = d ^ rem[0];
            rem.remove(0);
            rem.push(0);
            if factor != 0 {
                for (i, &g) in gen_poly.iter().enumerate().skip(1) {
                    rem[i - 1] ^= self.mul(g, factor);
                }
            }
        }
        rem
    }

    /// Attempt to correct up to `nsym / 2` errors in `received` (data followed by EC
    /// codewords), returning the corrected block, or `None` if uncorrectable.
    ///
    /// Syndrome + Berlekamp–Massey + Chien + Forney, parameterised by the first
    /// consecutive root `fcr` (Han Xin uses `fcr = 1`).
    pub fn rs_decode(&self, received: &[u8], nsym: usize, fcr: usize) -> Option<Vec<u8>> {
        let n = received.len();
        // Syndromes S_i = R(α^(fcr+i)).
        let mut syn = vec![0u8; nsym];
        let mut all_zero = true;
        for (i, s) in syn.iter_mut().enumerate() {
            let x = self.exp_of(fcr + i);
            let mut acc = 0u8;
            for &c in received {
                acc = self.mul(acc, x) ^ c;
            }
            *s = acc;
            if acc != 0 {
                all_zero = false;
            }
        }
        if all_zero {
            return Some(received.to_vec());
        }

        // Berlekamp–Massey (Λ least-significant-first, Λ_0 = 1).
        let mut sigma = vec![1u8];
        let mut prev = vec![1u8];
        let mut l = 0usize;
        let mut m = 1usize;
        let mut b = 1u8;
        for it in 0..nsym {
            let mut delta = syn[it];
            for i in 1..=l {
                if i < sigma.len() {
                    delta ^= self.mul(sigma[i], syn[it - i]);
                }
            }
            if delta == 0 {
                m += 1;
            } else if 2 * l <= it {
                let t = sigma.clone();
                self.sub_shift(&mut sigma, &prev, self.div(delta, b), m);
                l = it + 1 - l;
                prev = t;
                b = delta;
                m = 1;
            } else {
                self.sub_shift(&mut sigma, &prev, self.div(delta, b), m);
                m += 1;
            }
        }

        while sigma.len() > 1 && *sigma.last().unwrap() == 0 {
            sigma.pop();
        }
        let num_errors = sigma.len() - 1;
        if num_errors > nsym / 2 || num_errors == 0 {
            return None;
        }

        // Chien search: Λ(α^-j) = 0 → error at received index n-1-j.
        let mut positions = Vec::new();
        for j in 0..n {
            let x_inv = self.exp_of((self.order - (j % self.order)) % self.order);
            let mut val = 0u8;
            let mut pw = 1u8;
            for &c in &sigma {
                val ^= self.mul(c, pw);
                pw = self.mul(pw, x_inv);
            }
            if val == 0 {
                positions.push(n - 1 - j);
            }
        }
        if positions.len() != num_errors {
            return None;
        }

        // Forney: Ω(x) = [S(x)·Λ(x)] mod x^nsym (both LSF).
        let mut omega = vec![0u8; nsym];
        for (i, &s) in syn.iter().enumerate() {
            for (k, &sg) in sigma.iter().enumerate() {
                if i + k < nsym {
                    omega[i + k] ^= self.mul(s, sg);
                }
            }
        }

        let mut corrected = received.to_vec();
        for &pos in &positions {
            let p = n - 1 - pos;
            let x = self.exp_of(p % self.order);
            let x_inv = self.inv(x);
            let mut omega_val = 0u8;
            let mut pw = 1u8;
            for &c in &omega {
                omega_val ^= self.mul(c, pw);
                pw = self.mul(pw, x_inv);
            }
            // Λ'(X^-1): GF(2) derivative keeps odd-degree terms.
            let mut deriv = 0u8;
            let mut pw = 1u8;
            for (k, &c) in sigma.iter().enumerate() {
                if k % 2 == 1 {
                    deriv ^= self.mul(c, pw);
                    pw = self.mul(pw, self.mul(x_inv, x_inv));
                }
            }
            if deriv == 0 {
                return None;
            }
            // e = X^(1-fcr) · Ω(X^-1) / Λ'(X^-1).
            let scale = self.pow(x, 1 - fcr as i32);
            corrected[pos] ^= self.mul(scale, self.div(omega_val, deriv));
        }

        // Verify the correction satisfies every syndrome.
        for i in 0..nsym {
            let x = self.exp_of(fcr + i);
            let mut acc = 0u8;
            for &c in &corrected {
                acc = self.mul(acc, x) ^ c;
            }
            if acc != 0 {
                return None;
            }
        }
        Some(corrected)
    }

    /// `dst ← dst − scale · x^shift · src`, all least-significant first.
    fn sub_shift(&self, dst: &mut Vec<u8>, src: &[u8], scale: u8, shift: usize) {
        let needed = src.len() + shift;
        if dst.len() < needed {
            dst.resize(needed, 0);
        }
        for (i, &s) in src.iter().enumerate() {
            dst[i + shift] ^= self.mul(scale, s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_field_axioms() {
        let gf = GaloisField::data();
        assert_eq!(gf.mul(0, 5), 0);
        assert_eq!(gf.mul(1, 5), 5);
        for a in 1u8..=255 {
            assert_eq!(gf.mul(a, gf.inv(a)), 1, "inverse of {a}");
        }
        assert_eq!(gf.exp_of(255), 1);
    }

    #[test]
    fn function_field_axioms() {
        let gf = GaloisField::function();
        for a in 1u8..=15 {
            assert_eq!(gf.mul(a, gf.inv(a)), 1, "inverse of {a}");
        }
        assert_eq!(gf.exp_of(15), 1);
    }

    #[test]
    fn rs_roundtrip_clean() {
        let gf = GaloisField::data();
        let data = [0x11u8, 0xEC, 0x04, 0xFF, 0x40, 0x00, 0x00, 0x00, 0x00];
        let ec = gf.rs_encode(&data, 16, 1);
        let mut block = data.to_vec();
        block.extend_from_slice(&ec);
        assert_eq!(
            gf.rs_decode(&block, 16, 1).as_deref(),
            Some(block.as_slice())
        );
    }

    #[test]
    fn rs_corrects_errors() {
        let gf = GaloisField::data();
        let data = [0x11u8, 0xEC, 0x04, 0xFF, 0x40, 0x00, 0x00, 0x00, 0x00];
        let ec = gf.rs_encode(&data, 16, 1);
        let mut block = data.to_vec();
        block.extend_from_slice(&ec);
        let clean = block.clone();
        // 16 EC codewords correct up to 8 errors.
        block[0] ^= 0xFF;
        block[4] ^= 0x0F;
        block[9] ^= 0xA5;
        block[14] ^= 0x33;
        block[20] ^= 0x01;
        assert_eq!(
            gf.rs_decode(&block, 16, 1).as_deref(),
            Some(clean.as_slice())
        );
    }

    #[test]
    fn function_rs_roundtrip() {
        let gf = GaloisField::function();
        let data = [0x2, 0x1, 0x0];
        let ec = gf.rs_encode(&data, 4, 1);
        assert_eq!(ec.len(), 4);
        let mut block = data.to_vec();
        block.extend_from_slice(&ec);
        assert_eq!(
            gf.rs_decode(&block, 4, 1).as_deref(),
            Some(block.as_slice())
        );
    }
}
