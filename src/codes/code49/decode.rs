//! Code 49 decoding: [`BitMatrix`] -> [`Symbol`].
//!
//! Reads a clean module matrix, recovers each row's four symbol characters into the
//! code-character grid, revalidates every row and symbol check character by recomputing
//! the grid, and reconstructs the payload [`Segment`]s so the result re-encodes
//! identically.

use alloc::collections::BTreeMap;
use alloc::vec;

use super::Code49Meta;
use super::tables::{EVEN_BITPATTERN, ODD_BITPATTERN, ROW_PARITY};
use super::tables::{ROW_WIDTH, compute_grid, extract_codewords, reconstruct_segments};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// Code 49 structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code49Decoder;

impl Code49Decoder {
    /// A new decoder.
    pub fn new() -> Self {
        Code49Decoder
    }

    /// Decode a clean Code 49 module matrix into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        let rows = matrix.height();
        if !(2..=8).contains(&rows) {
            return Err(Error::undecodable("Code 49 row count out of range"));
        }
        if matrix.width() != ROW_WIDTH {
            return Err(Error::undecodable("Code 49 row width is not 70 modules"));
        }

        // Reverse lookups: 16-bit pattern -> symbol-character value.
        let even_rev: BTreeMap<u16, u16> = EVEN_BITPATTERN
            .iter()
            .enumerate()
            .map(|(i, &p)| (p, i as u16))
            .collect();
        let odd_rev: BTreeMap<u16, u16> = ODD_BITPATTERN
            .iter()
            .enumerate()
            .map(|(i, &p)| (p, i as u16))
            .collect();

        let mut grid = vec![0u8; rows * 8];
        for i in 0..rows {
            let bits = row_bits(matrix, i)?;
            // Start "10" and stop "1111".
            if !bits[0] || bits[1] {
                return Err(Error::undecodable("Code 49 start pattern not found"));
            }
            if !bits[66..70].iter().all(|&b| b) {
                return Err(Error::undecodable("Code 49 stop pattern not found"));
            }
            for j in 0..4 {
                let mut pat: u16 = 0;
                for k in 0..16 {
                    pat = (pat << 1) | bits[2 + j * 16 + k] as u16;
                }
                let even = i == rows - 1 || ROW_PARITY[i][j];
                let w = if even { &even_rev } else { &odd_rev }
                    .get(&pat)
                    .copied()
                    .ok_or_else(|| Error::undecodable("unknown Code 49 symbol pattern"))?;
                grid[i * 8 + 2 * j] = (w / 49) as u8;
                grid[i * 8 + 2 * j + 1] = (w % 49) as u8;
            }
        }

        // Validate the mode/row-count character against the row count.
        let mode_char = grid[(rows - 1) * 8 + 6];
        if mode_char / 7 + 2 != rows as u8 {
            return Err(Error::undecodable("Code 49 mode/row-count mismatch"));
        }
        let m = mode_char % 7;

        // Recompute the whole grid from the recovered codewords and compare; this
        // revalidates every row and symbol check character at once.
        let codewords = extract_codewords(&grid, rows);
        let (rrows, rgrid) = compute_grid(&codewords, m, Some(rows))
            .map_err(|_| Error::undecodable("Code 49 grid failed to revalidate"))?;
        if rrows != rows || rgrid != grid {
            return Err(Error::undecodable("Code 49 check characters mismatch"));
        }

        let meta = Code49Meta { rows, grid };
        let segments = reconstruct_segments(&meta)?;
        Ok(Symbol::new(
            Symbology::Code49,
            segments,
            SymbolMeta::Code49(meta),
        ))
    }
}

impl Decode for Code49Decoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "Code 49 decode of a linear pattern",
            }),
        }
    }
}

/// The 70 module bits of one row.
fn row_bits(matrix: &BitMatrix, row: usize) -> Result<[bool; ROW_WIDTH]> {
    let mut bits = [false; ROW_WIDTH];
    for (x, b) in bits.iter_mut().enumerate() {
        *b = matrix.get(x, row);
    }
    Ok(bits)
}
