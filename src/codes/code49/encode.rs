//! Code 49 encoding: [`Symbol`] -> [`BitMatrix`], plus the fresh-input `build` path.
//!
//! [`Code49Encoder::encode`] renders the exact code-character grid pinned in
//! [`Code49Meta`], guaranteeing a lossless round-trip. [`Code49Encoder::build`] takes
//! fresh bytes, plans a base-49 codeword stream, lays out the grid (computing the row
//! and symbol check characters), and pins it.

use super::Code49Meta;
use super::tables::{ASCII_TO_INSET, EVEN_BITPATTERN, INSET, ODD_BITPATTERN, ROW_PARITY};
use super::tables::{ROW_WIDTH, compute_grid, reconstruct_segments};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;
use alloc::vec::Vec;

/// The quiet zone Code 49 requires on each side, in narrow modules.
pub(crate) const QUIET_ZONE: usize = 10;

/// Code 49 encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code49Encoder;

impl Code49Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        Code49Encoder
    }

    /// Build a Code 49 [`Symbol`] from fresh bytes (each `0..=127`).
    ///
    /// Every source byte encodes as one or two base-49 codewords via the Code 49 ASCII
    /// chart; the Numeric Encodation optimization (5-digit blocks) is not used, so the
    /// symbol is valid but not maximally dense. The resulting code-character grid,
    /// including all row and symbol check characters, is pinned in [`Code49Meta`].
    pub fn build(&self, data: &[u8]) -> Result<Symbol> {
        self.build_rows(data, None)
    }

    /// Build with an explicit minimum row count (`2..=8`).
    pub fn build_rows(&self, data: &[u8], min_rows: Option<usize>) -> Result<Symbol> {
        if data.is_empty() {
            return Err(Error::invalid_data("Code 49 input is empty"));
        }
        for &b in data {
            if b > 127 {
                return Err(Error::invalid_data(
                    "Code 49 supports bytes 0..=127 (extended ASCII not allowed)",
                ));
            }
        }
        let (codewords, m) = plan_codewords(data);
        if codewords.len() > 49 {
            return Err(Error::capacity("Code 49 data exceeds 49 codewords"));
        }
        let (rows, grid) = compute_grid(&codewords, m, min_rows)?;
        let meta = Code49Meta { rows, grid };
        let segments = reconstruct_segments(&meta)?;
        Ok(Symbol::new(
            Symbology::Code49,
            segments,
            SymbolMeta::Code49(meta),
        ))
    }
}

impl Encode for Code49Encoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::Code49 {
            return Err(Error::invalid_parameter(
                "Code49Encoder given a non-Code49 symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Code49(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Code 49 symbol missing Code49Meta",
                ));
            }
        };
        Ok(Encoding::Matrix(render(meta)?))
    }
}

/// The INSET position (base-49 codeword) of an ASCII/meta character.
fn inset_pos(c: u8) -> Option<u8> {
    INSET.iter().position(|&x| x == c).map(|p| p as u8)
}

/// Plan the base-49 codeword stream and the starting-mode value `M`.
fn plan_codewords(data: &[u8]) -> (Vec<u8>, u8) {
    let mut intermediate: Vec<u8> = Vec::new();
    for &b in data {
        let entry = ASCII_TO_INSET[b as usize];
        intermediate.push(entry[0]);
        if entry[1] != 0 {
            intermediate.push(entry[1]);
        }
    }
    let mut codewords: Vec<u8> = intermediate
        .iter()
        .map(|&c| inset_pos(c).expect("INSET chart yields valid codewords"))
        .collect();
    // Starting-mode extraction: a leading Shift 1 / Shift 2 becomes the mode value.
    let m = match codewords.first() {
        Some(48) => 2,
        Some(43) => 4,
        Some(44) => 5,
        _ => 0,
    };
    if m != 0 {
        codewords.remove(0);
    }
    (codewords, m)
}

/// Render a validated [`Code49Meta`] into its module matrix.
pub(crate) fn render(meta: &Code49Meta) -> Result<BitMatrix> {
    let rows = meta.rows;
    if !(2..=8).contains(&rows) {
        return Err(Error::invalid_parameter("Code 49 rows must be 2..=8"));
    }
    if meta.grid.len() != rows * 8 {
        return Err(Error::invalid_parameter(
            "Code 49 grid size inconsistent with row count",
        ));
    }
    for &v in &meta.grid {
        if v > 48 {
            return Err(Error::invalid_parameter(
                "Code 49 codeword value out of range",
            ));
        }
    }

    let mut matrix = BitMatrix::new(ROW_WIDTH, rows, QUIET_ZONE);
    for (i, chunk) in meta.grid.chunks(8).enumerate() {
        let mut bits: Vec<bool> = Vec::with_capacity(ROW_WIDTH);
        // Start character "10".
        bits.push(true);
        bits.push(false);
        // The last row uses even parity for every column.
        let parity = if i == rows - 1 {
            [true; 4]
        } else {
            ROW_PARITY[i]
        };
        for (j, &even) in parity.iter().enumerate() {
            let w = chunk[2 * j] as usize * 49 + chunk[2 * j + 1] as usize;
            let pat = if even {
                EVEN_BITPATTERN[w]
            } else {
                ODD_BITPATTERN[w]
            };
            for bit in (0..16).rev() {
                bits.push((pat >> bit) & 1 == 1);
            }
        }
        // Stop character "1111".
        bits.extend(core::iter::repeat_n(true, 4));
        debug_assert_eq!(bits.len(), ROW_WIDTH);
        for (x, &b) in bits.iter().enumerate() {
            if b {
                matrix.set(x, i, true);
            }
        }
    }
    Ok(matrix)
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;
    use alloc::vec;

    /// Independent reference (zint 2.16.0, `zint --verbose --barcode=CODE49
    /// --data=ABCD1234`): the code-character grid is
    /// `[[10,11,12,13,1,2,3,3],[4,48,26,9,17,31,0,37]]`.
    #[test]
    fn zint_abcd1234_grid() {
        let (codewords, m) = plan_codewords(b"ABCD1234");
        assert_eq!(codewords, vec![10, 11, 12, 13, 1, 2, 3, 4]);
        assert_eq!(m, 0);
        let (rows, grid) = compute_grid(&codewords, m, None).unwrap();
        assert_eq!(rows, 2);
        let expected: Vec<u8> = vec![10, 11, 12, 13, 1, 2, 3, 3, 4, 48, 26, 9, 17, 31, 0, 37];
        assert_eq!(grid, expected);
    }
}
