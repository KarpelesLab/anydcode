//! MaxiCode decoding: [`BitMatrix`] → [`Symbol`].
//!
//! This is the *structural* decoder: it consumes a clean, already-sampled 30×33
//! module grid, corrects errors with Reed–Solomon over GF(64), determines the mode,
//! reconstructs the Structured Carrier primary (modes 2/3), and re-runs the code-set
//! state machine to recover the segments and the exact [`MaxiCodeMeta`] needed to
//! re-encode identically.

use super::tables::{self, Cw, decode_body};
use super::{
    Carrier, MaxiCodeMeta, TOTAL_CW, body_len, correct_primary, correct_secondary, read_codewords,
    secondary_lengths,
};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;
use alloc::{format, string::String};

/// MaxiCode structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MaxiCodeDecoder;

impl MaxiCodeDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        MaxiCodeDecoder
    }

    /// Decode a sampled MaxiCode module grid into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        if matrix.width() != tables::WIDTH || matrix.height() != tables::HEIGHT {
            return Err(Error::undecodable("MaxiCode grid must be 30×33 modules"));
        }
        let mut cw = read_codewords(matrix);

        // Correct the primary first so the mode codeword is trustworthy.
        correct_primary(&mut cw)?;
        let mode = cw[0] & 0x0F;
        if !(2..=6).contains(&mode) {
            return Err(Error::undecodable("MaxiCode mode out of range (2..=6)"));
        }
        correct_secondary(&mut cw, mode)?;

        let (carrier, body) = if mode <= 3 {
            let carrier = if mode == 2 {
                decode_primary_2(&cw)?
            } else {
                decode_primary_3(&cw)?
            };
            (Some(carrier), cw[20..104].to_vec())
        } else {
            let (sec, _) = secondary_lengths(mode);
            let mut body = cw[1..10].to_vec();
            body.extend_from_slice(&cw[20..20 + sec]);
            (None, body)
        };
        debug_assert_eq!(body.len(), body_len(mode));

        let segments = decode_body(&body);
        Ok(Symbol::new(
            Symbology::MaxiCode,
            segments,
            SymbolMeta::MaxiCode(MaxiCodeMeta {
                mode,
                carrier,
                body,
            }),
        ))
    }
}

impl Decode for MaxiCodeDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "MaxiCode decode of a linear pattern",
            }),
        }
    }
}

/// Reconstruct the numeric Structured Carrier (Mode 2) primary fields.
fn decode_primary_2(cw: &[u8; TOTAL_CW]) -> Result<Carrier> {
    let g = |i: usize| cw[i] as u32;
    let pcn = ((g(0) >> 4) & 0x03)
        | (g(1) << 2)
        | (g(2) << 8)
        | (g(3) << 14)
        | (g(4) << 20)
        | ((g(5) & 0x0F) << 26);
    let len = (((g(5) >> 4) & 0x03) | ((g(6) & 0x0F) << 2)) as usize;
    if len == 0 || len > 9 {
        return Err(Error::undecodable(
            "MaxiCode mode 2 postcode length invalid",
        ));
    }
    // The length field counts the postcode's digits: a value that needs more is not
    // a postcode of that length (and would re-encode with a different length).
    if pcn >= 10u32.pow(len as u32) {
        return Err(Error::undecodable(
            "MaxiCode mode 2 postcode exceeds its length field",
        ));
    }
    let (country, service) = country_and_service(cw)?;
    Ok(Carrier {
        postcode: format!("{pcn:0len$}"),
        country,
        service,
    })
}

/// The 10-bit country code and service class shared by modes 2 and 3. Both are
/// three-digit numbers, so `1000..=1023` cannot come from a Structured Carrier.
fn country_and_service(cw: &[u8; TOTAL_CW]) -> Result<(u16, u16)> {
    let g = |i: usize| cw[i] as u16;
    let country = ((g(6) >> 4) & 0x03) | (g(7) << 2) | ((g(8) & 0x03) << 8);
    let service = ((g(8) >> 2) & 0x0F) | (g(9) << 4);
    if country > 999 || service > 999 {
        return Err(Error::undecodable(
            "MaxiCode country code or service class above 999",
        ));
    }
    Ok((country, service))
}

/// Reconstruct the alphanumeric Structured Carrier (Mode 3) primary fields.
fn decode_primary_3(cw: &[u8; TOTAL_CW]) -> Result<Carrier> {
    let g = |i: usize| cw[i] as u32;
    let vals = [
        ((g(5) >> 4) & 0x03) | ((g(6) & 0x0F) << 2), // p0
        ((g(4) >> 4) & 0x03) | ((g(5) & 0x0F) << 2), // p1
        ((g(3) >> 4) & 0x03) | ((g(4) & 0x0F) << 2), // p2
        ((g(2) >> 4) & 0x03) | ((g(3) & 0x0F) << 2), // p3
        ((g(1) >> 4) & 0x03) | ((g(2) & 0x0F) << 2), // p4
        ((g(0) >> 4) & 0x03) | ((g(1) & 0x0F) << 2), // p5
    ];
    let sets = tables::sets();
    let mut postcode = String::with_capacity(6);
    for v in vals {
        // Each of the six positions holds a Code Set A character; a shift, latch or
        // pad value there is not one (dropping it would shorten the postcode).
        let Cw::Byte(b) = sets[0][(v & 0x3F) as usize] else {
            return Err(Error::undecodable(
                "MaxiCode mode 3 postcode holds a non-character codeword",
            ));
        };
        postcode.push(b as char);
    }
    let (country, service) = country_and_service(cw)?;
    Ok(Carrier {
        postcode,
        country,
        service,
    })
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;
    use crate::codes::maxicode::{MaxiCodeEncoder, add_error_correction, render_matrix};
    use crate::traits::Encode;
    use alloc::vec::Vec;

    /// A Structured Carrier symbol with the given primary data codewords (`cw[0]` is
    /// completed with the mode) and an all-pad secondary, with valid Reed–Solomon.
    fn structured(mode: u8, primary: [u8; 10]) -> BitMatrix {
        let mut cw = [33u8; TOTAL_CW];
        cw[..10].copy_from_slice(&primary);
        cw[0] = (cw[0] & 0x30) | mode;
        add_error_correction(&mut cw, mode);
        render_matrix(&cw)
    }

    /// A primary message can hold values no Structured Carrier has: a 10-bit country
    /// or service above 999, a Mode 2 postcode with more digits than its length
    /// field, a Mode 3 postcode "character" that is a shift or pad. Those are not
    /// readable symbols: whatever does decode must re-encode to the same modules.
    #[test]
    fn structured_primary_decodes_only_what_reencodes() {
        let enc = MaxiCodeEncoder::new();
        let dec = MaxiCodeDecoder::new();
        // Mode 2: postcode 12345 (binary 0x3039) declared as 3 digits long.
        let pcn = 12345u32;
        let mut p = [0u8; 10];
        p[0] = ((pcn & 0x03) << 4) as u8;
        p[1] = ((pcn >> 2) & 0x3F) as u8;
        p[2] = ((pcn >> 8) & 0x3F) as u8;
        p[5] = 3 << 4;
        assert!(dec.decode_matrix(&structured(2, p)).is_err());
        // The same postcode with a length that holds it reads back.
        p[5] = (5 & 0x03) << 4;
        p[6] = 5 >> 2;
        let sym = dec.decode_matrix(&structured(2, p)).unwrap();
        let SymbolMeta::MaxiCode(meta) = &sym.meta else {
            panic!("expected MaxiCodeMeta");
        };
        assert_eq!(meta.carrier.as_ref().unwrap().postcode, "12345");
        // Country 1023 / service 1023 are not three-digit numbers.
        let mut bad = p;
        bad[6] |= 0x30;
        bad[7] = 0x3F;
        bad[8] |= 0x03;
        assert!(dec.decode_matrix(&structured(2, bad)).is_err());
        let mut bad = p;
        bad[8] |= 0x3C;
        bad[9] = 0x3F;
        assert!(dec.decode_matrix(&structured(2, bad)).is_err());
        // Mode 3: all six postcode values 63 (Latch B in Code Set A).
        let all = [0x30, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x0F, 0, 0, 0];
        assert!(dec.decode_matrix(&structured(3, all)).is_err());

        // Random primaries: an accepted one always re-encodes identically.
        let mut seed = 0x3A81_C0DE_0002_0003u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut accepted = 0;
        for round in 0..600 {
            let mode = 2 + (round % 2) as u8;
            let mut p = [0u8; 10];
            for c in &mut p {
                *c = (next() % 64) as u8;
            }
            if round % 3 == 0 {
                // Keep the numeric fields small so plenty of primaries are valid.
                p[7] &= 0x0F;
                p[9] &= 0x0F;
                p[4] = 0;
                p[5] &= 0x30;
                p[6] = (p[6] & 0x30) | 2;
            }
            let m = structured(mode, p);
            if let Ok(sym) = dec.decode_matrix(&m) {
                accepted += 1;
                assert_eq!(
                    enc.encode(&sym),
                    Ok(Encoding::Matrix(m)),
                    "mode {mode} primary {p:?}"
                );
            }
        }
        assert!(accepted > 50, "only {accepted} primaries accepted");
    }

    /// The primary block corrects 5 codeword errors and each secondary interleave 10
    /// (Standard EC) or 14 (Enhanced EC, mode 5) simultaneously; one more error in
    /// any block is reported rather than mis-decoded.
    #[test]
    fn corrects_up_to_capacity_in_every_block() {
        let enc = MaxiCodeEncoder::new();
        for mode in [4u8, 5, 6] {
            let symbol = enc
                .build_mode(b"MAXICODE BLOCK CAPACITY 2026", mode)
                .unwrap();
            let Encoding::Matrix(clean) = enc.encode(&symbol).unwrap() else {
                panic!("expected a matrix");
            };
            let cw = read_codewords(&clean);
            let t2 = secondary_lengths(mode).1 / 4;
            // Codeword indices of the primary block and of each secondary interleave.
            let primary: Vec<usize> = (0..20).collect();
            let even: Vec<usize> = (20..TOTAL_CW).step_by(2).collect();
            let odd: Vec<usize> = (21..TOTAL_CW).step_by(2).collect();
            let blocks = [(&primary, 5usize), (&even, t2), (&odd, t2)];
            let hit = |cw: &mut [u8; TOTAL_CW], idx: &[usize], count: usize| {
                for k in 0..count {
                    cw[idx[k * idx.len() / count]] ^= 0x2A;
                }
            };

            let mut bad = cw;
            for (idx, t) in blocks {
                hit(&mut bad, idx, t);
            }
            let decoded = MaxiCodeDecoder::new()
                .decode_matrix(&render_matrix(&bad))
                .unwrap();
            assert_eq!(decoded, symbol, "mode {mode}");

            for (b, (idx, t)) in blocks.into_iter().enumerate() {
                let mut bad = cw;
                hit(&mut bad, idx, t + 1);
                let result = MaxiCodeDecoder::new().decode_matrix(&render_matrix(&bad));
                assert!(result.is_err(), "mode {mode} block {b}");
            }
        }
    }
}
