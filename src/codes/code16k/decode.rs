//! Code 16K decoding: [`BitMatrix`] -> [`Symbol`].
//!
//! Reads a clean module matrix (as produced by the encoder or an image front-end),
//! recovers the exact per-row symbol values into [`Code16kMeta`], validates the row
//! delimiters and the two mod-107 check characters, and reconstructs the payload
//! [`Segment`]s so the result re-encodes identically.

use super::Code16kMeta;
use super::tables::{ROW_WIDTH, checksum, reconstruct_segments};
use super::tables::{START_STOP, START_VALUES, STOP_VALUES, value_for_widths};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;
use alloc::vec::Vec;

/// Code 16K structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code16kDecoder;

impl Code16kDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Code16kDecoder
    }

    /// Decode a clean Code 16K module matrix into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        let rows = matrix.height();
        if !(2..=16).contains(&rows) {
            return Err(Error::undecodable("Code 16K row count out of range"));
        }
        if matrix.width() != ROW_WIDTH {
            return Err(Error::undecodable("Code 16K row width is not 70 modules"));
        }

        let mut full = Vec::with_capacity(rows * 5);
        for row in 0..rows {
            let runs = row_runs(matrix, row)?;
            if runs.len() != 39 {
                return Err(Error::undecodable("Code 16K row has wrong element count"));
            }
            // Verify the start/stop delimiters encode this row's number.
            if runs[0..4] != START_STOP[START_VALUES[row]] {
                return Err(Error::undecodable("Code 16K start delimiter mismatch"));
            }
            if runs[35..39] != START_STOP[STOP_VALUES[row]] {
                return Err(Error::undecodable("Code 16K stop delimiter mismatch"));
            }
            if runs[4] != 1 {
                return Err(Error::undecodable("Code 16K guard bar mismatch"));
            }
            for i in 0..5 {
                let widths = &runs[5 + i * 6..5 + i * 6 + 6];
                let v = value_for_widths(widths)
                    .ok_or_else(|| Error::undecodable("unknown Code 16K symbol pattern"))?;
                full.push(v);
            }
        }

        // Split the two trailing check characters and validate them.
        let check2 = full.pop().unwrap();
        let check1 = full.pop().unwrap();
        let (c1, c2) = checksum(&full);
        if (c1, c2) != (check1, check2) {
            return Err(Error::undecodable("Code 16K check character mismatch"));
        }

        // Verify the mode character agrees with the row count.
        let mode = full[0];
        if mode / 7 + 2 != rows as u8 {
            return Err(Error::undecodable("Code 16K mode/row-count mismatch"));
        }

        let meta = Code16kMeta { rows, values: full };
        let segments = reconstruct_segments(&meta.values)?;
        Ok(Symbol::new(
            Symbology::Code16k,
            segments,
            SymbolMeta::Code16k(meta),
        ))
    }
}

impl Decode for Code16kDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "Code 16K decode of a linear pattern",
            }),
        }
    }
}

/// Element run lengths of one row, starting with the (dark) first element.
fn row_runs(matrix: &BitMatrix, row: usize) -> Result<[u8; 39]> {
    if !matrix.get(0, row) {
        return Err(Error::undecodable("Code 16K row must start with a bar"));
    }
    let mut runs = [0u8; 39];
    let mut idx = 0usize;
    let mut cur = true;
    let mut count = 0u16;
    for x in 0..ROW_WIDTH {
        let m = matrix.get(x, row);
        if m == cur {
            count += 1;
        } else {
            if idx >= 39 {
                return Err(Error::undecodable("Code 16K row has too many elements"));
            }
            runs[idx] = count as u8;
            idx += 1;
            cur = m;
            count = 1;
        }
    }
    if idx != 38 {
        return Err(Error::undecodable("Code 16K row has too few elements"));
    }
    runs[38] = count as u8;
    Ok(runs)
}
