//! Grid Matrix decoding: [`BitMatrix`] → [`Symbol`].
//!
//! The *structural* decoder consumes a clean, already-sampled module grid (as produced
//! by the encoder or an image front-end), recovers the version from the grid size and
//! the EC level from the layer-id rings, corrects errors with Reed–Solomon, and
//! reconstructs the exact segments and [`GridMatrixMeta`] so the result re-encodes
//! identically.

use super::data::decode_segments;
use super::data::to_segment;
use super::layout::{read_codewords, read_ec_level};
use super::tables::{Version, blocks};
use super::{GridMatrixMeta, gf};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;
use alloc::vec::Vec;

/// Grid Matrix structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct GridMatrixDecoder;

impl GridMatrixDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        GridMatrixDecoder
    }

    /// Decode a sampled Grid Matrix module grid into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        if matrix.width() != matrix.height() {
            return Err(Error::undecodable("Grid Matrix grid is not square"));
        }
        let size = matrix.width();
        if size < 18 || !(size - 6).is_multiple_of(12) {
            return Err(Error::undecodable("grid size is not a valid Grid Matrix"));
        }
        let version = Version::new(((size - 6) / 12) as u8)
            .ok_or_else(|| Error::undecodable("Grid Matrix version out of range"))?;

        let ec = read_ec_level(version, matrix)
            .ok_or_else(|| Error::undecodable("could not read Grid Matrix EC level"))?;
        // Version 1 has no level-1 variant: it would carry no EC codewords at all.
        if version.data_codewords(ec) == 0 {
            return Err(Error::undecodable(
                "Grid Matrix EC level is not defined for this version",
            ));
        }

        let cws = read_codewords(version, matrix);
        let data = deinterleave_and_correct(&cws, version, ec)?;

        // Data codewords → bitstream (7 bits each, MSB first).
        let mut bits = Vec::with_capacity(data.len() * 7);
        for &cw in &data {
            for j in 0..7 {
                bits.push((cw >> (6 - j)) & 1 != 0);
            }
        }

        let segs = decode_segments(&bits)?;
        let modes = segs.iter().map(|(m, _)| *m).collect();
        let segments = segs.into_iter().map(|(m, d)| to_segment(m, d)).collect();

        let meta = GridMatrixMeta {
            version,
            ec_level: ec,
            modes,
        };
        Ok(Symbol::new(
            Symbology::GridMatrix,
            segments,
            SymbolMeta::GridMatrix(meta),
        ))
    }
}

impl Decode for GridMatrixDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "Grid Matrix decode of a linear pattern",
            }),
        }
    }
}

/// De-interleave the codeword stream into RS blocks, correct each, and concatenate the
/// corrected data codewords (including padding) in stream order.
fn deinterleave_and_correct(cws: &[u8], version: Version, ec: super::EcLevel) -> Result<Vec<u8>> {
    let block_layout = blocks(version, ec);
    let num_blocks = block_layout.len();
    let mut data = Vec::new();
    for (i, blk) in block_layout.iter().enumerate() {
        let data_size = blk.data();
        let block: Vec<u8> = (0..blk.total).map(|j| cws[num_blocks * j + i]).collect();
        let corrected = gf::decode(&block, blk.ec).ok_or(Error::ErrorCorrectionFailed)?;
        data.extend_from_slice(&corrected[..data_size]);
    }
    Ok(data)
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;
    use crate::codes::gridmatrix::layout::render;
    use crate::codes::gridmatrix::{EcLevel, GridMatrixEncoder};
    use crate::traits::Encode;
    use alloc::vec;

    /// Version 1 has no level-1 variant (its table row holds no EC codewords at all),
    /// so layer ids claiming it must not yield an unprotected "successful" decode.
    #[test]
    fn rejects_version_1_level_1() {
        let version = Version::new(1).unwrap();
        // Upper 'A' then end-of-data: 0100 00000 11011 -> codewords 0x20, 0x1B.
        let mut cws = vec![0u8; version.total_codewords()];
        cws[0] = 0x20;
        cws[1] = 0x1B;
        let m = render(version, EcLevel::L1, &cws);
        assert!(GridMatrixDecoder::new().decode_matrix(&m).is_err());
    }

    /// In every usable version and EC level, every RS block corrects `ec / 2` codeword
    /// errors simultaneously; one more error in any block is reported, not mis-decoded.
    #[test]
    fn corrects_up_to_capacity_in_every_block() {
        let enc = GridMatrixEncoder::new();
        for v in 1..=13u8 {
            let version = Version::new(v).unwrap();
            for ec in EcLevel::ALL {
                if version.data_codewords(ec) == 0 {
                    continue;
                }
                let symbol = enc.build_sized(b"GM 2026", version, ec).unwrap();
                let Encoding::Matrix(clean) = enc.encode(&symbol).unwrap() else {
                    panic!("expected a matrix");
                };
                let cws = read_codewords(version, &clean);
                let layout = blocks(version, ec);
                let n = layout.len();
                // Codeword `j` of block `i` sits at `n * j + i`; spread the errors
                // evenly over the block's data and EC codewords.
                let hit = |cws: &mut [u8], i: usize, count: usize| {
                    for k in 0..count {
                        cws[n * (k * layout[i].total / count) + i] ^= 0x55;
                    }
                };

                let mut bad = cws.clone();
                for (i, blk) in layout.iter().enumerate() {
                    hit(&mut bad, i, blk.ec / 2);
                }
                let decoded = GridMatrixDecoder::new()
                    .decode_matrix(&render(version, ec, &bad))
                    .unwrap();
                assert_eq!(decoded, symbol, "v{v} {ec:?}");

                for (i, blk) in layout.iter().enumerate() {
                    let mut bad = cws.clone();
                    hit(&mut bad, i, blk.ec / 2 + 1);
                    let result = GridMatrixDecoder::new().decode_matrix(&render(version, ec, &bad));
                    assert!(result.is_err(), "v{v} {ec:?} block {i}");
                }
            }
        }
    }
}
