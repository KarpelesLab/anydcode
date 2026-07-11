//! Grid Matrix decoding: [`BitMatrix`] → [`Symbol`].
//!
//! The *structural* decoder consumes a clean, already-sampled module grid (as produced
//! by the encoder or an image front-end), recovers the version from the grid size and
//! the EC level from the layer-id rings, corrects errors with Reed–Solomon, and
//! reconstructs the exact segments and [`GridMatrixMeta`] so the result re-encodes
//! identically.

use super::data::decode_segments;
use super::encode::to_segment;
use super::layout::{read_codewords, read_ec_level};
use super::tables::{Version, blocks};
use super::{GridMatrixMeta, gf};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

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
