//! Code 49 decoding: [`BitMatrix`] -> [`Symbol`].
//!
//! Reads a clean module matrix, recovers each row's four symbol characters into the
//! code-character grid, revalidates every row and symbol check character by recomputing
//! the grid, and reconstructs the payload [`Segment`]s so the result re-encodes
//! identically.

use std::collections::HashMap;

use super::Code49Meta;
use super::encode::{ROW_WIDTH, compute_grid};
use super::tables::{ASCII_TO_INSET, EVEN_BITPATTERN, INSET, ODD_BITPATTERN, ROW_PARITY};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// Pad / numeric-shift codeword value.
const PAD: u8 = 48;
/// Shift 1 (`!`) and Shift 2 (`&`) INSET character bytes.
const SHIFT1: u8 = b'!';
const SHIFT2: u8 = b'&';

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
        let even_rev: HashMap<u16, u16> = EVEN_BITPATTERN
            .iter()
            .enumerate()
            .map(|(i, &p)| (p, i as u16))
            .collect();
        let odd_rev: HashMap<u16, u16> = ODD_BITPATTERN
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

/// Recover the base-49 data codewords from the grid (dropping check/mode cells and the
/// trailing pad characters).
fn extract_codewords(grid: &[u8], rows: usize) -> Vec<u8> {
    let mut cw = Vec::new();
    for r in 0..rows - 1 {
        for c in 0..7 {
            cw.push(grid[r * 8 + c]);
        }
    }
    // The last row holds data only in columns 0..2, and only when rows <= 6.
    if rows <= 6 {
        cw.push(grid[(rows - 1) * 8]);
        cw.push(grid[(rows - 1) * 8 + 1]);
    }
    while cw.last() == Some(&PAD) {
        cw.pop();
    }
    cw
}

/// Reconstruct the payload [`Segment`]s from a decoded [`Code49Meta`]. Shared by the
/// decoder and the encoder's `build` path so both produce identical segments.
///
/// The Numeric Encodation mode (leading codeword `48`) is not reconstructed; symbols
/// produced by this crate's encoder never use it. Because re-encoding renders from the
/// stored grid, segment fidelity never affects the round-trip identity.
pub(crate) fn reconstruct_segments(meta: &Code49Meta) -> Result<Vec<Segment>> {
    let mut codewords = extract_codewords(&meta.grid, meta.rows);
    let mode_char = meta.grid[(meta.rows - 1) * 8 + 6];
    let m = mode_char % 7;
    match m {
        0 => {}
        4 => codewords.insert(0, 43), // leading Shift 1
        5 => codewords.insert(0, 44), // leading Shift 2
        _ => {
            return Err(Error::undecodable(
                "Code 49 numeric-mode payload reconstruction is not implemented",
            ));
        }
    }

    // Reverse the Code 49 ASCII chart: (char1[, char2]) -> source byte.
    let mut rev: HashMap<(u8, u8), u8> = HashMap::new();
    for (b, entry) in ASCII_TO_INSET.iter().enumerate() {
        rev.insert((entry[0], entry[1]), b as u8);
    }

    // Codewords -> INSET characters.
    let mut chars = Vec::with_capacity(codewords.len());
    for &c in &codewords {
        let ch = *INSET
            .get(c as usize)
            .ok_or_else(|| Error::undecodable("Code 49 codeword out of chart range"))?;
        chars.push(ch);
    }

    let mut bytes = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c1 = chars[i];
        if c1 == SHIFT1 || c1 == SHIFT2 {
            let c2 = *chars
                .get(i + 1)
                .ok_or_else(|| Error::undecodable("Code 49 dangling shift character"))?;
            let b = *rev
                .get(&(c1, c2))
                .ok_or_else(|| Error::undecodable("Code 49 unknown shifted character"))?;
            bytes.push(b);
            i += 2;
        } else {
            match rev.get(&(c1, 0)) {
                Some(&b) => bytes.push(b),
                None => return Err(Error::undecodable("Code 49 unknown character")),
            }
            i += 1;
        }
    }

    Ok(if bytes.is_empty() {
        Vec::new()
    } else {
        vec![Segment::byte(bytes)]
    })
}
