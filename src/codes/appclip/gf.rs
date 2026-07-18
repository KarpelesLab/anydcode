//! Galois-field arithmetic and Reed–Solomon coding for the App Clip codec.
//!
//! The codec uses two fields — GF(16) with polynomial `0x13` and first consecutive
//! root 0 for the metadata block, and GF(256) with `0x11D` and fcr 1 for the gaps and
//! arcs blocks. The parity counts are tiny (2–4 symbols), so the decoder corrects at
//! most one or two symbol errors by direct search rather than Berlekamp–Massey.

/// Arithmetic over GF(2^n) for `size` 16 or 256.
pub(super) struct Gf {
    size: usize,
    /// First consecutive root of the generator polynomial.
    pub fcr: usize,
    exp: Vec<usize>,
    log: Vec<usize>,
}

impl Gf {
    pub fn new(primitive: usize, size: usize, fcr: usize) -> Gf {
        let mut exp = vec![0usize; size * 2];
        let mut log = vec![0usize; size];
        let mut x = 1usize;
        for (i, e) in exp.iter_mut().enumerate().take(size) {
            *e = x;
            log[x] = i;
            x <<= 1;
            if x >= size {
                x ^= primitive;
                x &= size - 1;
            }
        }
        for i in size..size * 2 {
            exp[i] = exp[i - size + 1];
        }
        Gf {
            size,
            fcr,
            exp,
            log,
        }
    }

    /// GF(16), x⁴+x+1, fcr 0 — the metadata field.
    pub fn f16() -> Gf {
        Gf::new(0x13, 16, 0)
    }

    /// GF(256), x⁸+x⁴+x³+x²+1, fcr 1 — the gaps/arcs field.
    pub fn f256() -> Gf {
        Gf::new(0x11D, 256, 1)
    }

    #[inline]
    pub fn exp(&self, a: usize) -> usize {
        self.exp[a]
    }

    #[inline]
    pub fn mul(&self, a: usize, b: usize) -> usize {
        if a == 0 || b == 0 {
            return 0;
        }
        self.exp[self.log[a] + self.log[b]]
    }

    #[inline]
    pub fn inv(&self, a: usize) -> usize {
        self.exp[self.size - 1 - self.log[a]]
    }

    /// α^exponent with the exponent reduced modulo the group order.
    pub fn pow(&self, exponent: isize) -> usize {
        let order = (self.size - 1) as isize;
        let mut e = exponent % order;
        if e < 0 {
            e += order;
        }
        self.exp[e as usize]
    }
}

/// Systematic RS encode: returns `[data..., parity...]` with `num_parity` symbols of
/// parity from the generator `g(x) = ∏ (x + α^(fcr+i))`.
pub(super) fn rs_encode(gf: &Gf, data: &[usize], num_parity: usize) -> Vec<usize> {
    // Build the generator polynomial, highest degree first (leading coefficient 1).
    let mut genpoly = vec![1usize];
    for i in 0..num_parity {
        let root = gf.exp(gf.fcr + i);
        let mut next = vec![0usize; genpoly.len() + 1];
        next[..genpoly.len()].copy_from_slice(&genpoly);
        for (j, &g) in genpoly.iter().enumerate() {
            next[j + 1] ^= gf.mul(g, root);
        }
        genpoly = next;
    }

    let mut out = vec![0usize; data.len() + num_parity];
    out[..data.len()].copy_from_slice(data);
    // Polynomial long division of data·x^num_parity by the generator.
    for i in 0..data.len() {
        let coef = out[i];
        if coef != 0 {
            for j in 1..=num_parity {
                out[i + j] ^= gf.mul(genpoly[j], coef);
            }
        }
    }
    out[..data.len()].copy_from_slice(data);
    out
}

/// RS decode by syndrome check plus direct search for one (or, with ≥4 parity
/// symbols, two) symbol errors. Returns the corrected codeword.
pub(super) fn rs_decode(gf: &Gf, codeword: &[usize], num_parity: usize) -> Option<Vec<usize>> {
    if num_parity == 0 {
        return Some(codeword.to_vec());
    }
    let syn = syndromes(gf, codeword, num_parity);
    if syn.iter().all(|&s| s == 0) {
        return Some(codeword.to_vec());
    }
    if let Some(fixed) = correct_single(gf, codeword, &syn) {
        return Some(fixed);
    }
    if num_parity >= 4
        && let Some(fixed) = correct_double(gf, codeword, &syn)
    {
        return Some(fixed);
    }
    None
}

fn syndromes(gf: &Gf, codeword: &[usize], num_parity: usize) -> Vec<usize> {
    (0..num_parity)
        .map(|si| {
            let root = gf.pow((gf.fcr + si) as isize);
            codeword.iter().fold(0usize, |acc, &sym| gf.mul(acc, root) ^ sym)
        })
        .collect()
}

/// α^((fcr+si)·(n-1-pos)): the contribution of an error of magnitude 1 at `pos`.
fn error_term(gf: &Gf, n: usize, pos: usize, si: usize) -> usize {
    gf.pow(((gf.fcr + si) * (n - 1 - pos)) as isize)
}

fn correct_single(gf: &Gf, codeword: &[usize], syn: &[usize]) -> Option<Vec<usize>> {
    let n = codeword.len();
    for pos in 0..n {
        let a0 = error_term(gf, n, pos, 0);
        if a0 == 0 {
            continue;
        }
        let mag = gf.mul(syn[0], gf.inv(a0));
        if mag == 0 {
            continue;
        }
        if syn
            .iter()
            .enumerate()
            .all(|(si, &s)| gf.mul(mag, error_term(gf, n, pos, si)) == s)
        {
            let mut out = codeword.to_vec();
            out[pos] ^= mag;
            if syndromes(gf, &out, syn.len()).iter().all(|&s| s == 0) {
                return Some(out);
            }
        }
    }
    None
}

fn correct_double(gf: &Gf, codeword: &[usize], syn: &[usize]) -> Option<Vec<usize>> {
    let n = codeword.len();
    if syn.len() < 2 {
        return None;
    }
    for p in 0..n {
        for q in p + 1..n {
            let (a0, b0) = (error_term(gf, n, p, 0), error_term(gf, n, q, 0));
            let (a1, b1) = (error_term(gf, n, p, 1), error_term(gf, n, q, 1));
            let det = gf.mul(a0, b1) ^ gf.mul(a1, b0);
            if det == 0 {
                continue;
            }
            let inv = gf.inv(det);
            let ep = gf.mul(gf.mul(syn[0], b1) ^ gf.mul(syn[1], b0), inv);
            let eq = gf.mul(gf.mul(a0, syn[1]) ^ gf.mul(a1, syn[0]), inv);
            if ep == 0 || eq == 0 {
                continue;
            }
            let ok = syn.iter().enumerate().all(|(si, &s)| {
                gf.mul(ep, error_term(gf, n, p, si)) ^ gf.mul(eq, error_term(gf, n, q, si)) == s
            });
            if !ok {
                continue;
            }
            let mut out = codeword.to_vec();
            out[p] ^= ep;
            out[q] ^= eq;
            if syndromes(gf, &out, syn.len()).iter().all(|&s| s == 0) {
                return Some(out);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_basics() {
        let gf = Gf::f256();
        assert_eq!(gf.exp(0), 1);
        assert_eq!(gf.exp(1), 2);
        assert_eq!(gf.exp(255), gf.exp(0));
        assert_eq!(gf.mul(0, 5), 0);
        assert_eq!(gf.mul(1, 5), 5);
    }

    #[test]
    fn rs_roundtrip_with_errors() {
        let gf = Gf::f256();
        let data = [12usize, 34, 56, 78, 90, 11, 22, 33, 44];
        let mut cw = rs_encode(&gf, &data, 4);
        assert_eq!(rs_decode(&gf, &cw, 4).unwrap()[..9], data);
        cw[3] ^= 0x5a; // single error
        assert_eq!(rs_decode(&gf, &cw, 4).unwrap()[..9], data);
        cw[10] ^= 0x11; // second error
        assert_eq!(rs_decode(&gf, &cw, 4).unwrap()[..9], data);
    }

    #[test]
    fn zero_data_zero_parity() {
        let gf = Gf::f256();
        let cw = rs_encode(&gf, &[0, 0, 0, 0, 0], 2);
        assert_eq!(cw, vec![0; 7]);
    }
}
