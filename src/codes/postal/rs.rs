//! A small Reed–Solomon encoder over `GF(2^m)`, used by the Australia Post
//! (`GF(64)`, primitive polynomial `0x43`) and Mailmark (`GF(32)`, primitive
//! polynomial `0x25`) 4-state codes.
//!
//! The implementation deliberately mirrors zint's `reedsol.c` (Cliff Hones /
//! Robin Stuart) so that the generated check symbols match the reference
//! implementation byte-for-byte: the generator polynomial is
//! `(x + α^index)(x + α^(index+1))…` and the systematic remainder is reversed
//! before being returned, exactly as zint does. Both Australia Post and Mailmark
//! use `index = 1`.

/// A Galois field `GF(2^m)` with a fixed primitive polynomial.
pub(super) struct Gf {
    /// Discrete-log table: `logt[x]` is the exponent `e` with `α^e == x`.
    logt: Vec<u8>,
    /// Antilog table, doubled in length so `alog[a + b]` never needs a modulo.
    alog: Vec<u8>,
}

impl Gf {
    /// Build the field of order `1 << m` with primitive polynomial `poly`.
    pub(super) fn new(poly: u16, m: u32) -> Gf {
        let size = 1usize << m;
        let mut logt = vec![0u8; size];
        let mut alog = vec![0u8; size * 2];
        let mut x = 1u16;
        for (i, slot) in alog.iter_mut().take(size - 1).enumerate() {
            *slot = x as u8;
            logt[x as usize] = i as u8;
            x <<= 1;
            if x & size as u16 != 0 {
                x ^= poly;
            }
        }
        for i in (size - 1)..(size * 2) {
            alog[i] = alog[i - (size - 1)];
        }
        Gf { logt, alog }
    }
}

/// A Reed–Solomon generator for `nsym` check symbols over `gf`.
pub(super) struct RsEncoder<'a> {
    gf: &'a Gf,
    /// Generator-polynomial coefficients (length `nsym + 1`).
    rspoly: Vec<u8>,
    /// Precomputed logs of `rspoly`.
    log_rspoly: Vec<usize>,
    nsym: usize,
}

impl<'a> RsEncoder<'a> {
    /// Prepare an encoder producing `nsym` check symbols, first generator root
    /// `α^index`.
    pub(super) fn new(gf: &'a Gf, nsym: usize, index: i32) -> RsEncoder<'a> {
        let mut rspoly = vec![0u8; nsym + 1];
        rspoly[0] = 1;
        for i in 1..=nsym {
            // Root exponent for this factor: index, index+1, … (see zint).
            let idx = index as usize + (i - 1);
            rspoly[i] = 1;
            for k in (1..i).rev() {
                if rspoly[k] != 0 {
                    rspoly[k] = gf.alog[gf.logt[rspoly[k] as usize] as usize + idx];
                }
                rspoly[k] ^= rspoly[k - 1];
            }
            rspoly[0] = gf.alog[gf.logt[rspoly[0] as usize] as usize + idx];
        }
        let log_rspoly = rspoly
            .iter()
            .map(|&c| gf.logt[c as usize] as usize)
            .collect();
        RsEncoder {
            gf,
            rspoly,
            log_rspoly,
            nsym,
        }
    }

    /// The `nsym` Reed–Solomon check symbols for `data`.
    pub(super) fn encode(&self, data: &[u8]) -> Vec<u8> {
        let nsym = self.nsym;
        let mut res = vec![0u8; nsym];
        for &d in data {
            let m = res[nsym - 1] ^ d;
            if m != 0 {
                let log_m = self.gf.logt[m as usize] as usize;
                for k in (1..nsym).rev() {
                    res[k] = if self.rspoly[k] != 0 {
                        res[k - 1] ^ self.gf.alog[log_m + self.log_rspoly[k]]
                    } else {
                        res[k - 1]
                    };
                }
                res[0] = self.gf.alog[log_m + self.log_rspoly[0]];
            } else {
                for k in (1..nsym).rev() {
                    res[k] = res[k - 1];
                }
                res[0] = 0;
            }
        }
        res.reverse();
        res
    }
}
