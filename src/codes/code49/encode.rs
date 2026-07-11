//! Code 49 encoding: [`Symbol`] -> [`BitMatrix`], plus the fresh-input `build` path.
//!
//! [`Code49Encoder::encode`] renders the exact code-character grid pinned in
//! [`Code49Meta`], guaranteeing a lossless round-trip. [`Code49Encoder::build`] takes
//! fresh bytes, plans a base-49 codeword stream, lays out the grid (computing the row
//! and symbol check characters), and pins it.

use super::Code49Meta;
use super::decode::reconstruct_segments;
use super::tables::{
    ASCII_TO_INSET, EVEN_BITPATTERN, INSET, ODD_BITPATTERN, ROW_PARITY, X_WEIGHT, Y_WEIGHT,
    Z_WEIGHT,
};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// The quiet zone Code 49 requires on each side, in narrow modules.
pub(crate) const QUIET_ZONE: usize = 10;

/// Symbol width in modules: start "10" (2) + 4 characters * 16 + stop "1111" (4).
pub(crate) const ROW_WIDTH: usize = 70;

/// Pad / numeric-shift codeword value.
const PAD: u8 = 48;

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

/// Lay the codewords into the code-character grid and fill in every check character.
/// Ported from the zint reference implementation (`backend/code49.c`).
pub(crate) fn compute_grid(
    codewords: &[u8],
    m: u8,
    min_rows: Option<usize>,
) -> Result<(usize, Vec<u8>)> {
    let count = codewords.len();
    let mut grid: Vec<[i64; 8]> = Vec::new();
    let mut pad_count = 0usize;

    let mut rows = 0usize;
    loop {
        let mut row = [0i64; 8];
        for (i, slot) in row.iter_mut().enumerate().take(7) {
            let idx = rows * 7 + i;
            if idx < count {
                *slot = codewords[idx] as i64;
            } else {
                *slot = PAD as i64;
                pad_count += 1;
            }
        }
        grid.push(row);
        rows += 1;
        if rows * 7 >= count {
            break;
        }
    }

    if rows == 1 || rows > 6 || pad_count < 5 {
        grid.push([PAD as i64; 8]);
        rows += 1;
    }

    if let Some(min) = min_rows {
        if !(2..=8).contains(&min) {
            return Err(Error::invalid_parameter("Code 49 rows must be 2..=8"));
        }
        while rows < min {
            grid.push([PAD as i64; 8]);
            rows += 1;
        }
    }
    if rows > 8 {
        return Err(Error::capacity("Code 49 data exceeds 8 rows"));
    }

    // Row-count and mode character.
    grid[rows - 1][6] = 7 * (rows as i64 - 2) + m as i64;

    // Row check characters for all but the last row.
    for row in grid.iter_mut().take(rows - 1) {
        let sum: i64 = row[0..7].iter().sum();
        row[7] = sum % 49;
    }

    // Symbol check characters (X, Y, Z), modulo 2401.
    let mut posn = 0usize;
    let mode_char = grid[rows - 1][6];
    let mut x_count = mode_char * 20;
    let mut y_count = mode_char * 16;
    let mut z_count = mode_char * 38;
    for row in grid.iter().take(rows - 1) {
        for j in 0..4 {
            let local = row[2 * j] * 49 + row[2 * j + 1];
            x_count += X_WEIGHT[posn] as i64 * local;
            y_count += Y_WEIGHT[posn] as i64 * local;
            z_count += Z_WEIGHT[posn] as i64 * local;
            posn += 1;
        }
    }

    if rows > 6 {
        z_count %= 2401;
        grid[rows - 1][0] = z_count / 49;
        grid[rows - 1][1] = z_count % 49;
    }

    let local = grid[rows - 1][0] * 49 + grid[rows - 1][1];
    x_count += X_WEIGHT[posn] as i64 * local;
    y_count += Y_WEIGHT[posn] as i64 * local;
    posn += 1;

    y_count %= 2401;
    grid[rows - 1][2] = y_count / 49;
    grid[rows - 1][3] = y_count % 49;

    let local = grid[rows - 1][2] * 49 + grid[rows - 1][3];
    x_count += X_WEIGHT[posn] as i64 * local;

    x_count %= 2401;
    grid[rows - 1][4] = x_count / 49;
    grid[rows - 1][5] = x_count % 49;

    // Last row check character.
    let sum: i64 = grid[rows - 1][0..7].iter().sum();
    grid[rows - 1][7] = sum % 49;

    // Flatten to bytes.
    let mut flat = Vec::with_capacity(rows * 8);
    for row in &grid {
        for &v in row {
            flat.push(v as u8);
        }
    }
    Ok((rows, flat))
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
        bits.extend(std::iter::repeat_n(true, 4));
        debug_assert_eq!(bits.len(), ROW_WIDTH);
        for (x, &b) in bits.iter().enumerate() {
            if b {
                matrix.set(x, i, true);
            }
        }
    }
    Ok(matrix)
}

#[cfg(test)]
mod tests {
    use super::*;

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
