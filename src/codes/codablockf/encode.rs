//! Codablock F encoding: [`Symbol`] -> [`BitMatrix`], plus the fresh-input `build` path.
//!
//! [`CodablockFEncoder::encode`] renders the exact Code 128 codeword grid pinned in
//! [`CodablockFMeta`], guaranteeing a lossless round-trip. [`CodablockFEncoder::build`]
//! lays fresh bytes out into rows and pins the resulting grid.

use super::CodablockFMeta;
use super::tables::{C128_PATTERNS, START_A, STOP, STOP_VALUE};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// The quiet zone Codablock F requires on each side, in narrow modules.
pub(crate) const QUIET_ZONE: usize = 10;

/// Code-set selector symbol placed second in every row (after Start A).
const SEL_A: u8 = 98;
const SEL_B: u8 = 100;
const SEL_C: u8 = 99;
/// Latch symbols used inside a row's data region.
const CODE_A: u8 = 101;
const CODE_B: u8 = 100;
const CODE_C: u8 = 99;

/// Codablock F encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodablockFEncoder;

impl CodablockFEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        CodablockFEncoder
    }

    /// Build a Codablock F [`Symbol`] from fresh bytes (each `0..=127`).
    ///
    /// The layout uses Code 128 sets A and B (control characters start a row in A, the
    /// rest in B; extended-ASCII/Code C compression are not used) and appends a
    /// dedicated final row carrying the K1/K2 data check characters. Column count is
    /// chosen automatically for a roughly square symbol. The full codeword grid is
    /// pinned in [`CodablockFMeta`].
    pub fn build(&self, data: &[u8]) -> Result<Symbol> {
        if data.is_empty() {
            return Err(Error::invalid_data("Codablock F input is empty"));
        }
        for &b in data {
            if b > 127 {
                return Err(Error::invalid_data(
                    "Codablock F build supports bytes 0..=127 (extended ASCII not implemented)",
                ));
            }
        }
        let (k1, k2) = k1k2(data);

        // Choose a column count for a roughly-square symbol.
        let mut best: Option<(usize, usize, usize)> = None; // (score, use_columns, rows)
        for use_columns in 4..=62usize {
            let rows = encode_data_rows(data, use_columns).len() + 1;
            if rows > 44 {
                continue;
            }
            let columns = use_columns + 5;
            let score = rows.abs_diff(columns);
            if best.is_none_or(|(best_score, _, _)| score < best_score) {
                best = Some((score, use_columns, rows));
            }
        }
        let (_, use_columns, rows) =
            best.ok_or_else(|| Error::capacity("Codablock F data exceeds the 44-row maximum"))?;

        let grid = build_grid(data, use_columns, rows, k1, k2);
        let columns = use_columns + 5;
        let meta = CodablockFMeta {
            rows,
            columns,
            codewords: grid,
        };
        Ok(Symbol::new(
            Symbology::CodablockF,
            vec![Segment::byte(data.to_vec())],
            SymbolMeta::CodablockF(meta),
        ))
    }
}

impl Encode for CodablockFEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::CodablockF {
            return Err(Error::invalid_parameter(
                "CodablockFEncoder given a non-CodablockF symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::CodablockF(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Codablock F symbol missing CodablockFMeta",
                ));
            }
        };
        Ok(Encoding::Matrix(render(meta)?))
    }
}

/// The two data check characters K1 and K2 (Codablock F Annex F), over the raw source.
pub(crate) fn k1k2(data: &[u8]) -> (u8, u8) {
    let mut s1: u32 = 0;
    let mut s2: u32 = 0;
    for (i, &b) in data.iter().enumerate() {
        s1 = (s1 + (i as u32 + 1) * b as u32) % 86;
        s2 = (s2 + i as u32 * b as u32) % 86;
    }
    (s1 as u8, s2 as u8)
}

/// Encode a row-indicator / check sum into a Code 128 value in the given set.
/// Sets A and B use the same (Set B) mapping per the Codablock F spec; Set C is direct.
pub(crate) fn sum_to_value(sum: u8, set: Set) -> u8 {
    match set {
        Set::C => sum,
        _ => {
            if sum <= 31 {
                sum + 64
            } else if sum <= 47 {
                sum - 32
            } else {
                sum - 22
            }
        }
    }
}

/// The modulo-103 Code 128 row check over the symbols preceding the check position.
pub(crate) fn row_check(syms: &[u8]) -> u8 {
    let mut sum = syms[0] as u32 % 103;
    for (pos, &v) in syms.iter().enumerate().skip(1) {
        sum = (sum + v as u32 * pos as u32) % 103;
    }
    sum as u8
}

/// Render a validated [`CodablockFMeta`] into its module matrix.
pub(crate) fn render(meta: &CodablockFMeta) -> Result<BitMatrix> {
    let (rows, columns) = (meta.rows, meta.columns);
    if !(2..=44).contains(&rows) || !(9..=67).contains(&columns) {
        return Err(Error::invalid_parameter(
            "Codablock F row/column count out of range",
        ));
    }
    if meta.codewords.len() != rows * columns {
        return Err(Error::invalid_parameter(
            "Codablock F codeword count inconsistent with geometry",
        ));
    }
    // Each of the first columns-1 symbols is 11 modules; the Stop pattern is 13.
    let width = (columns - 1) * 11 + 13;

    let mut matrix = BitMatrix::new(width, rows, QUIET_ZONE);
    for r in 0..rows {
        let mut widths: Vec<u8> = Vec::with_capacity(columns * 6 + 1);
        for c in 0..columns - 1 {
            let v = meta.codewords[r * columns + c];
            if v as usize >= C128_PATTERNS.len() {
                return Err(Error::invalid_parameter(
                    "Codablock F symbol value out of range",
                ));
            }
            for b in C128_PATTERNS[v as usize].bytes() {
                widths.push(b - b'0');
            }
        }
        for b in STOP.bytes() {
            widths.push(b - b'0');
        }

        let mut x = 0usize;
        let mut dark = true;
        for w in widths {
            for _ in 0..w {
                if dark {
                    matrix.set(x, r, true);
                }
                x += 1;
            }
            dark = !dark;
        }
        debug_assert_eq!(x, width);
    }
    Ok(matrix)
}

// ---------------------------------------------------------------------------
// Fresh-input layout
// ---------------------------------------------------------------------------

/// A Code 128 code set (A/B used for data, C only for filler switches).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Set {
    /// Code Set A.
    A,
    /// Code Set B.
    B,
    /// Code Set C.
    C,
}

fn representable(set: Set, b: u8) -> bool {
    match set {
        Set::A => b < 96,
        Set::B => (32..=127).contains(&b),
        Set::C => false,
    }
}

fn value_in(set: Set, b: u8) -> u8 {
    match set {
        Set::A => {
            if b < 32 {
                b + 64
            } else {
                b - 32
            }
        }
        Set::B => b - 32,
        Set::C => unreachable!(),
    }
}

/// Distribute data bytes into rows, each row's data region exactly `use_columns`
/// symbols. Returns, per data row, the starting set and the data-region symbol values.
fn encode_data_rows(data: &[u8], use_columns: usize) -> Vec<(Set, Vec<u8>)> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let start_set = if data[i] < 32 { Set::A } else { Set::B };
        let mut set = start_set;
        let mut row: Vec<u8> = Vec::with_capacity(use_columns);
        while i < data.len() {
            let b = data[i];
            if representable(set, b) {
                if row.len() + 1 > use_columns {
                    break;
                }
                row.push(value_in(set, b));
                i += 1;
            } else {
                if row.len() + 2 > use_columns {
                    break;
                }
                let other = match set {
                    Set::A => Set::B,
                    Set::B => Set::A,
                    Set::C => Set::B,
                };
                row.push(match other {
                    Set::A => CODE_A,
                    Set::B => CODE_B,
                    Set::C => CODE_C,
                });
                set = other;
                row.push(value_in(set, b));
                i += 1;
            }
        }
        // Pad the data region to use_columns with alternating filler switches.
        while row.len() < use_columns {
            if set == Set::C {
                row.push(CODE_B);
                set = Set::B;
            } else {
                row.push(CODE_C);
                set = Set::C;
            }
        }
        rows.push((start_set, row));
    }
    rows
}

/// Build the full Code 128 codeword grid (rows * columns).
fn build_grid(data: &[u8], use_columns: usize, rows: usize, k1: u8, k2: u8) -> Vec<u8> {
    let columns = use_columns + 5;
    let data_rows = encode_data_rows(data, use_columns);
    debug_assert_eq!(data_rows.len() + 1, rows);

    let mut grid = Vec::with_capacity(rows * columns);
    for (r, (start_set, data_region)) in data_rows.iter().enumerate() {
        let rowind = if r == 0 {
            (rows - 2) as u8
        } else {
            (r + 42) as u8
        };
        emit_row(&mut grid, *start_set, rowind, data_region);
    }
    // Final dedicated check row: fillers then K1, K2.
    let mut check_region: Vec<u8> = Vec::with_capacity(use_columns);
    let mut set = Set::B;
    while check_region.len() < use_columns - 2 {
        if set == Set::C {
            check_region.push(CODE_B);
            set = Set::B;
        } else {
            check_region.push(CODE_C);
            set = Set::C;
        }
    }
    check_region.push(sum_to_value(k1, set));
    check_region.push(sum_to_value(k2, set));
    let last = rows - 1;
    let rowind = (last + 42) as u8;
    emit_row(&mut grid, Set::B, rowind, &check_region);

    debug_assert_eq!(grid.len(), rows * columns);
    grid
}

/// Append one complete row (start, selector, indicator, data, C128 check, stop).
fn emit_row(grid: &mut Vec<u8>, start_set: Set, rowind: u8, data_region: &[u8]) {
    let base = grid.len();
    grid.push(START_A);
    grid.push(match start_set {
        Set::A => SEL_A,
        Set::B => SEL_B,
        Set::C => SEL_C,
    });
    grid.push(sum_to_value(rowind, start_set));
    grid.extend_from_slice(data_region);
    let check = row_check(&grid[base..]);
    grid.push(check);
    grid.push(STOP_VALUE);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent reference (zint 2.16.0): K1/K2 for "ABCDEFGHIJKL" are 52 and 66.
    #[test]
    fn zint_abcdefghijkl_k1k2() {
        assert_eq!(k1k2(b"ABCDEFGHIJKL"), (52, 66));
        // Encoded into Set B they become symbol values 30 and 44 (zint row 3).
        assert_eq!(sum_to_value(52, Set::B), 30);
        assert_eq!(sum_to_value(66, Set::B), 44);
    }

    /// Independent reference: the Code 128 row check of the first row of the
    /// "ABCDEFGHIJKL" symbol (start, CodeB, row indicator, A B C D) is 34.
    #[test]
    fn zint_row0_check() {
        let syms = [103u8, 100, 66, 33, 34, 35, 36];
        assert_eq!(row_check(&syms), 34);
    }
}
