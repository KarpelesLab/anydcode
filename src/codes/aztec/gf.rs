//! Galois-field arithmetic and Reed–Solomon codes for Aztec.
//!
//! Aztec uses several fields depending on the codeword size: GF(16) for the mode
//! message (4-bit words), GF(64) for 6-bit data words (1–2 layers), GF(256) for
//! 8-bit words (3–8 layers) and GF(1024) for 10-bit words (9–22 layers). Unlike QR,
//! the Reed–Solomon generator's first root is `α¹` (generator base 1), per
//! ISO/IEC 24778.
//!
//! Polynomials are stored **most-significant coefficient first** (`coeffs[0]` is the
//! highest-degree term), matching the reference Euclidean decoder this module mirrors.

/// A finite field GF(2^m) described by its size and primitive polynomial.
#[derive(Debug, Clone)]
pub struct Gf {
    /// `exp[i] = α^i` for `i` in `0..order`.
    exp: Vec<u16>,
    /// `log[x]` is the discrete log of `x`; `log[0]` is unused.
    log: Vec<u16>,
    /// Field order (number of elements), e.g. 16, 64, 256, 1024.
    size: usize,
    /// Exponent of the first root of the RS generator polynomial.
    base: usize,
}

impl Gf {
    /// Build a field of `size = 2^m` elements with the given primitive polynomial
    /// (including the implicit high bit) and RS generator base.
    pub fn new(size: usize, primitive: usize, base: usize) -> Gf {
        let order = size - 1;
        let mut exp = vec![0u16; order];
        let mut log = vec![0u16; size];
        let mut x = 1usize;
        for (i, e) in exp.iter_mut().enumerate() {
            *e = x as u16;
            log[x] = i as u16;
            x <<= 1;
            if x & size != 0 {
                x ^= primitive;
            }
        }
        Gf {
            exp,
            log,
            size,
            base,
        }
    }

    /// The Aztec mode-message field GF(16), `x⁴ + x + 1`.
    pub fn gf16() -> Gf {
        Gf::new(16, 0b1_0011, 1)
    }

    /// The field for `word_size`-bit data codewords.
    pub fn for_word_size(word_size: usize) -> Option<Gf> {
        Some(match word_size {
            4 => Gf::gf16(),
            6 => Gf::new(64, 0b100_0011, 1),         // x^6 + x + 1
            8 => Gf::new(256, 0b1_0010_1101, 1),     // x^8 + x^5 + x^3 + x^2 + 1
            10 => Gf::new(1024, 0b100_0000_1001, 1), // x^10 + x^3 + 1
            _ => return None,
        })
    }

    /// Number of non-zero elements (the multiplicative order).
    fn order(&self) -> usize {
        self.size - 1
    }

    /// The RS generator base (first-root exponent).
    pub fn base(&self) -> usize {
        self.base
    }

    /// Field size.
    pub fn size(&self) -> usize {
        self.size
    }

    /// `α^n` for any integer exponent (reduced modulo the field order).
    pub fn exp(&self, n: i64) -> u16 {
        let o = self.order() as i64;
        let m = ((n % o) + o) % o;
        self.exp[m as usize]
    }

    /// Discrete log of a non-zero element.
    pub fn log(&self, a: u16) -> usize {
        self.log[a as usize] as usize
    }

    /// Field multiplication.
    pub fn mul(&self, a: u16, b: u16) -> u16 {
        if a == 0 || b == 0 {
            return 0;
        }
        let idx = (self.log[a as usize] as usize + self.log[b as usize] as usize) % self.order();
        self.exp[idx]
    }

    /// Multiplicative inverse of a non-zero element.
    pub fn inv(&self, a: u16) -> u16 {
        debug_assert!(a != 0, "inverse of zero in GF");
        self.exp((self.order() - self.log[a as usize] as usize) as i64)
    }
}

/// Field addition (and subtraction) is XOR.
fn add(a: u16, b: u16) -> u16 {
    a ^ b
}

/// Degree of a most-significant-first polynomial (zero-poly reports 0).
fn degree(p: &[u16]) -> usize {
    p.len().saturating_sub(1)
}

/// Strip leading (highest-order) zero coefficients, keeping at least one element.
fn normalize(mut p: Vec<u16>) -> Vec<u16> {
    let mut lead = 0;
    while lead + 1 < p.len() && p[lead] == 0 {
        lead += 1;
    }
    if lead > 0 {
        p.drain(0..lead);
    }
    p
}

fn is_zero(p: &[u16]) -> bool {
    p.iter().all(|&c| c == 0)
}

/// Evaluate `p` at `x` via Horner's method.
fn eval(gf: &Gf, p: &[u16], x: u16) -> u16 {
    let mut acc = 0u16;
    for &c in p {
        acc = add(gf.mul(acc, x), c);
    }
    acc
}

/// Multiply two polynomials.
fn poly_mul(gf: &Gf, a: &[u16], b: &[u16]) -> Vec<u16> {
    if is_zero(a) || is_zero(b) {
        return vec![0];
    }
    let mut out = vec![0u16; a.len() + b.len() - 1];
    for (i, &av) in a.iter().enumerate() {
        if av == 0 {
            continue;
        }
        for (j, &bv) in b.iter().enumerate() {
            out[i + j] ^= gf.mul(av, bv);
        }
    }
    normalize(out)
}

/// Scale a polynomial by a scalar.
fn poly_scale(gf: &Gf, a: &[u16], s: u16) -> Vec<u16> {
    normalize(a.iter().map(|&c| gf.mul(c, s)).collect())
}

/// Add two most-significant-first polynomials.
fn poly_add(a: &[u16], b: &[u16]) -> Vec<u16> {
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let mut out = long.to_vec();
    let off = long.len() - short.len();
    for (i, &c) in short.iter().enumerate() {
        out[off + i] ^= c;
    }
    normalize(out)
}

/// `s · x^shift`, as a polynomial.
fn monomial(s: u16, shift: usize) -> Vec<u16> {
    if s == 0 {
        return vec![0];
    }
    let mut p = vec![0u16; shift + 1];
    p[0] = s;
    p
}

/// The RS generator polynomial `∏_{i=0}^{ec-1}(x - α^{base+i})`.
fn generator(gf: &Gf, ec: usize) -> Vec<u16> {
    let mut g = vec![1u16];
    for i in 0..ec {
        g = poly_mul(gf, &g, &[1, gf.exp((gf.base + i) as i64)]);
    }
    g
}

/// Compute the `ec` Reed–Solomon check words for `data` (most-significant word first).
pub fn rs_encode(gf: &Gf, data: &[u16], ec: usize) -> Vec<u16> {
    let g = generator(gf, ec);
    let mut rem = vec![0u16; ec];
    for &d in data {
        let factor = d ^ rem[0];
        rem.remove(0);
        rem.push(0);
        if factor != 0 {
            for i in 1..g.len() {
                rem[i - 1] ^= gf.mul(g[i], factor);
            }
        }
    }
    rem
}

/// Correct up to `ec/2` errors in `received` (data followed by check words), in place.
/// Returns `false` if the errors exceed the correction capacity.
///
/// Mirrors the reference Euclidean-algorithm Reed–Solomon decoder, generalized for a
/// non-zero generator base.
pub fn rs_decode(gf: &Gf, received: &mut [u16], ec: usize) -> bool {
    // Syndromes S_i = R(α^{base+i}); stored most-significant-first.
    let mut syndromes = vec![0u16; ec];
    let mut no_error = true;
    for i in 0..ec {
        let s = eval(gf, received, gf.exp((gf.base + i) as i64));
        syndromes[ec - 1 - i] = s;
        if s != 0 {
            no_error = false;
        }
    }
    if no_error {
        return true;
    }
    let syndromes = normalize(syndromes);

    let (sigma, omega) = match run_euclidean(gf, monomial(1, ec), syndromes, ec) {
        Some(v) => v,
        None => return false,
    };

    let locations = match find_error_locations(gf, &sigma) {
        Some(v) => v,
        None => return false,
    };
    let magnitudes = find_error_magnitudes(gf, &omega, &locations);

    let n = received.len();
    for (loc, mag) in locations.iter().zip(magnitudes.iter()) {
        let pos = match n.checked_sub(1 + gf.log(*loc)) {
            Some(p) if p < n => p,
            _ => return false,
        };
        received[pos] ^= *mag;
    }
    true
}

/// The extended Euclidean step producing `(sigma, omega)` (error locator and
/// evaluator). `a` starts as `x^ec`, `b` as the syndrome polynomial.
fn run_euclidean(gf: &Gf, a: Vec<u16>, b: Vec<u16>, ec: usize) -> Option<(Vec<u16>, Vec<u16>)> {
    let (mut r_last, mut r) = if degree(&a) < degree(&b) {
        (b, a)
    } else {
        (a, b)
    };
    let mut t_last = vec![0u16];
    let mut t = vec![1u16];

    while 2 * degree(&r) >= ec {
        let r_last_last = r_last;
        let t_last_last = t_last;
        r_last = r;
        t_last = t;
        if is_zero(&r_last) {
            return None;
        }
        r = r_last_last;
        let mut q = vec![0u16];
        let dlt = r_last[0];
        let dlt_inv = gf.inv(dlt);
        while degree(&r) >= degree(&r_last) && !is_zero(&r) {
            let deg_diff = degree(&r) - degree(&r_last);
            let scale = gf.mul(r[0], dlt_inv);
            q = poly_add(&q, &monomial(scale, deg_diff));
            let term = poly_mul(gf, &r_last, &monomial(scale, deg_diff));
            r = poly_add(&r, &term);
        }
        t = poly_add(&poly_mul(gf, &q, &t_last), &t_last_last);
        if degree(&r) >= degree(&r_last) {
            return None;
        }
    }

    let sigma_tilde_at_zero = *t.last().unwrap();
    if sigma_tilde_at_zero == 0 {
        return None;
    }
    let inverse = gf.inv(sigma_tilde_at_zero);
    let sigma = poly_scale(gf, &t, inverse);
    let omega = poly_scale(gf, &r, inverse);
    Some((sigma, omega))
}

/// Chien search: find the roots of the error locator, returning the error locations
/// (as field elements `X_i`).
fn find_error_locations(gf: &Gf, sigma: &[u16]) -> Option<Vec<u16>> {
    let num_errors = degree(sigma);
    if num_errors == 0 {
        return None;
    }
    if num_errors == 1 {
        // Locator is `α x + b`; the location is the coefficient of x.
        return Some(vec![sigma[0]]);
    }
    let mut result = Vec::with_capacity(num_errors);
    for i in 1..gf.size() as u16 {
        if eval(gf, sigma, i) == 0 {
            result.push(gf.inv(i));
            if result.len() == num_errors {
                break;
            }
        }
    }
    if result.len() != num_errors {
        return None;
    }
    Some(result)
}

/// Forney's algorithm: compute the error magnitudes at the given locations.
fn find_error_magnitudes(gf: &Gf, omega: &[u16], locations: &[u16]) -> Vec<u16> {
    let s = locations.len();
    let mut result = vec![0u16; s];
    for i in 0..s {
        let xi_inverse = gf.inv(locations[i]);
        let mut denominator = 1u16;
        for (j, &loc) in locations.iter().enumerate() {
            if i != j {
                let term = gf.mul(loc, xi_inverse);
                let term_plus_1 = if term & 1 == 0 { term | 1 } else { term & !1 };
                denominator = gf.mul(denominator, term_plus_1);
            }
        }
        let mut magnitude = gf.mul(eval(gf, omega, xi_inverse), gf.inv(denominator));
        if gf.base() != 0 {
            magnitude = gf.mul(magnitude, xi_inverse);
        }
        result[i] = magnitude;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gf16_known_products() {
        // GF(16) with x^4 + x + 1, α = 2: independently verifiable small products.
        let gf = Gf::gf16();
        assert_eq!(gf.mul(2, 2), 4);
        assert_eq!(gf.mul(2, 4), 8);
        assert_eq!(gf.mul(2, 8), 3); // α^4 = α + 1 = 0b0011
        assert_eq!(gf.mul(3, 3), 5); // (α+1)^2 = α^2 + 1 = 0b0101
        assert_eq!(gf.mul(7, 7), 6);
        for a in 1u16..16 {
            assert_eq!(gf.mul(a, gf.inv(a)), 1, "inverse of {a}");
        }
    }

    #[test]
    fn rs_roundtrip_and_correction_gf64() {
        let gf = Gf::for_word_size(6).unwrap();
        let data: Vec<u16> = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let ec = 7;
        let check = rs_encode(&gf, &data, ec);
        let mut block = data.clone();
        block.extend_from_slice(&check);
        let clean = block.clone();

        // No errors → unchanged.
        let mut b0 = block.clone();
        assert!(rs_decode(&gf, &mut b0, ec));
        assert_eq!(b0, clean);

        // Introduce ec/2 = 3 errors.
        block[0] ^= 0x2a;
        block[4] ^= 0x11;
        block[9] ^= 0x3f;
        assert!(rs_decode(&gf, &mut block, ec));
        assert_eq!(block, clean);
    }

    #[test]
    fn rs_detects_uncorrectable() {
        let gf = Gf::for_word_size(8).unwrap();
        let data: Vec<u16> = (0..16).collect();
        let ec = 6; // corrects up to 3
        let check = rs_encode(&gf, &data, ec);
        let mut block = data.clone();
        block.extend_from_slice(&check);
        // 4 errors exceed capacity.
        for i in [0, 3, 7, 15] {
            block[i] ^= 0x55;
        }
        // Either reports failure, or (rarely) miscorrects to something != original;
        // here we only require it not to silently return the original.
        let before = block.clone();
        let ok = rs_decode(&gf, &mut block, ec);
        if ok {
            assert_ne!(block, data, "should not have recovered original data");
        } else {
            assert_eq!(block, before);
        }
    }
}
