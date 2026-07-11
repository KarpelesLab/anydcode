//! PDF417 decoding: [`BitMatrix`] → [`Symbol`].
//!
//! This is the *structural* decoder: it consumes a clean, already-sampled module
//! grid (as produced by the encoder or an image-sampling front-end) and recovers the
//! exact segments and [`Pdf417Meta`], so the result re-encodes identically.
//!
//! Geometry is read directly from the grid: the column count from the width
//! (`17·c + 69`) and the row count from the height (`rows · ROW_HEIGHT`). The
//! error-correction level is not read from the row indicators; instead each
//! candidate level is tried and the one whose Reed–Solomon decode both succeeds and
//! yields a consistent symbol-length descriptor is selected.

use std::collections::HashMap;

use super::encode::ROW_HEIGHT;
use super::tables::CODEWORD_PATTERNS;
use super::{EcLevel, Pdf417Meta, compaction, ec};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// PDF417 structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Pdf417Decoder;

impl Pdf417Decoder {
    /// A new decoder.
    pub fn new() -> Self {
        Pdf417Decoder
    }

    /// Decode a sampled PDF417 module grid into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        let width = matrix.width();
        let height = matrix.height();

        if width < 69 + 17 || !(width - 69).is_multiple_of(17) {
            return Err(Error::undecodable("width is not a valid PDF417 geometry"));
        }
        let cols = (width - 69) / 17;
        if !(1..=30).contains(&cols) {
            return Err(Error::undecodable("PDF417 column count out of range"));
        }
        if !height.is_multiple_of(ROW_HEIGHT) {
            return Err(Error::undecodable(
                "height is not a multiple of the row height",
            ));
        }
        let rows = height / ROW_HEIGHT;
        if !(3..=90).contains(&rows) {
            return Err(Error::undecodable("PDF417 row count out of range"));
        }

        let reverse = build_reverse_tables();
        let mut codewords = Vec::with_capacity(rows * cols);
        for y in 0..rows {
            let cluster = y % 3;
            let sample_row = y * ROW_HEIGHT;
            for col in 0..cols {
                let x0 = 34 + 17 * col; // start(17) + left indicator(17)
                let pattern = read_pattern(matrix, x0, sample_row);
                // An unrecognised pattern is a damaged symbol character; substitute a
                // sentinel so Reed–Solomon sees (and can repair) it as a codeword error.
                let cw = reverse[cluster].get(&pattern).copied().unwrap_or(0);
                codewords.push(cw);
            }
        }

        let total = rows * cols;
        let (ec_level, corrected) = resolve_ec(&codewords, total)?;
        let k = ec_level.ec_codewords();
        let n = total - k;
        let segments = compaction::decode_segments(&corrected[..n])?;

        let meta = Pdf417Meta {
            rows,
            columns: cols,
            ec_level,
            variant: super::Pdf417Variant::Standard,
        };
        Ok(Symbol::new(
            Symbology::Pdf417,
            segments,
            SymbolMeta::Pdf417(meta),
        ))
    }
}

impl Decode for Pdf417Decoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "PDF417 decode of a linear pattern",
            }),
        }
    }
}

/// Read a 17-module symbol character starting at `x0` on module row `y` into a
/// 17-bit pattern (most-significant module first).
fn read_pattern(matrix: &BitMatrix, x0: usize, y: usize) -> u32 {
    let mut pattern = 0u32;
    for j in 0..17 {
        if matrix.get(x0 + j, y) {
            pattern |= 1 << (16 - j);
        }
    }
    pattern
}

/// Build the inverse `pattern -> codeword` maps for the three clusters.
fn build_reverse_tables() -> [HashMap<u32, u32>; 3] {
    std::array::from_fn(|cluster| {
        CODEWORD_PATTERNS[cluster]
            .iter()
            .enumerate()
            .map(|(cw, &pat)| (pat, cw as u32))
            .collect()
    })
}

/// Determine the error-correction level and correct the codeword block.
///
/// A clean block is divisible by every generator polynomial of degree ≤ the true
/// `k`, so the symbol-length descriptor (`corrected[0] == total - k`) is the
/// discriminator that pins the actual level.
fn resolve_ec(codewords: &[u32], total: usize) -> Result<(EcLevel, Vec<u32>)> {
    for level in 0..=8u8 {
        let ec_level = EcLevel::new(level).unwrap();
        let k = ec_level.ec_codewords();
        if total < k + 1 {
            continue;
        }
        if let Some(corrected) = ec::decode(codewords, k)
            && corrected[0] as usize == total - k
            && corrected[0] > 0
        {
            return Ok((ec_level, corrected));
        }
    }
    Err(Error::ErrorCorrectionFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::pdf417::Pdf417Encoder;
    use crate::segment::Segment;
    use crate::traits::Encode;

    #[test]
    fn reverse_tables_are_bijective() {
        let rev = build_reverse_tables();
        for (cluster, map) in rev.iter().enumerate() {
            assert_eq!(map.len(), 929, "cluster {cluster} lost patterns");
        }
    }

    #[test]
    fn geometry_recovered_from_matrix() {
        let enc = Pdf417Encoder::new();
        let level = EcLevel::new(1).unwrap();
        let symbol = enc
            .build(vec![Segment::alphanumeric(b"HELLO PDF417".to_vec())], level)
            .unwrap();
        let Encoding::Matrix(m) = enc.encode(&symbol).unwrap() else {
            panic!("expected matrix");
        };
        let decoded = Pdf417Decoder::new().decode_matrix(&m).unwrap();
        assert_eq!(decoded.meta, symbol.meta);
        assert_eq!(decoded.segments, symbol.segments);
    }
}
