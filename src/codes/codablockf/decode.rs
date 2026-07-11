//! Codablock F decoding: [`BitMatrix`] -> [`Symbol`].
//!
//! Reads a clean module matrix, recovers each row's Code 128 codewords into
//! [`CodablockFMeta`], validates the per-row Code 128 check characters, the row
//! indicators and the K1/K2 data check characters, and reconstructs the payload
//! bytes so the result re-encodes identically.

use super::CodablockFMeta;
use super::encode::{Set, k1k2, row_check, sum_to_value};
use super::tables::{START_A, STOP, STOP_VALUE, value_for_widths};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// Codablock F structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodablockFDecoder;

impl CodablockFDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        CodablockFDecoder
    }

    /// Decode a clean Codablock F module matrix into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        let rows = matrix.height();
        if !(2..=44).contains(&rows) {
            return Err(Error::undecodable("Codablock F row count out of range"));
        }
        let width = matrix.width();
        // width = (columns-1)*11 + 13.
        if width < 24 || !(width - 13).is_multiple_of(11) {
            return Err(Error::undecodable("Codablock F row width is invalid"));
        }
        let columns = (width - 13) / 11 + 1;
        if !(9..=67).contains(&columns) {
            return Err(Error::undecodable("Codablock F column count out of range"));
        }

        // Parse every row into its columns-1 leading symbols (Start .. C128 check).
        let mut row_syms: Vec<Vec<u8>> = Vec::with_capacity(rows);
        for r in 0..rows {
            let vals = parse_row(matrix, r, columns)?;
            // Structural validation.
            if vals[0] != START_A {
                return Err(Error::undecodable("Codablock F row missing Start A"));
            }
            let set = sel_set(vals[1])?;
            let body_len = vals.len() - 1;
            if row_check(&vals[..body_len]) != vals[body_len] {
                return Err(Error::undecodable("Codablock F row check mismatch"));
            }
            let expect_ind = if r == 0 {
                (rows - 2) as u8
            } else {
                (r + 42) as u8
            };
            if vals[2] != sum_to_value(expect_ind, set) {
                return Err(Error::undecodable("Codablock F row indicator mismatch"));
            }
            row_syms.push(vals);
        }

        // Reconstruct payload and validate K1/K2 in the last row.
        let payload = reconstruct_payload(&row_syms, rows, columns)?;
        let (k1, k2) = k1k2(&payload);
        let last = &row_syms[rows - 1];
        let set = sel_set(last[1])?;
        // Data region of the last row is [3 .. columns-2); K1,K2 are its final two.
        let data_end = columns - 2;
        let set_at_k = set_after(&last[3..data_end - 2], set);
        if last[data_end - 2] != sum_to_value(k1, set_at_k)
            || last[data_end - 1] != sum_to_value(k2, set_at_k)
        {
            return Err(Error::undecodable("Codablock F K1/K2 check mismatch"));
        }

        // Store the full codeword grid (each row plus its Stop value).
        let mut codewords = Vec::with_capacity(rows * columns);
        for vals in &row_syms {
            codewords.extend_from_slice(vals);
            codewords.push(STOP_VALUE);
        }

        let meta = CodablockFMeta {
            rows,
            columns,
            codewords,
        };
        let segments = if payload.is_empty() {
            Vec::new()
        } else {
            vec![Segment::byte(payload)]
        };
        Ok(Symbol::new(
            Symbology::CodablockF,
            segments,
            SymbolMeta::CodablockF(meta),
        ))
    }
}

impl Decode for CodablockFDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "Codablock F decode of a linear pattern",
            }),
        }
    }
}

fn sel_set(sel: u8) -> Result<Set> {
    match sel {
        98 => Ok(Set::A),
        100 => Ok(Set::B),
        99 => Ok(Set::C),
        _ => Err(Error::undecodable("Codablock F invalid code-set selector")),
    }
}

/// Parse one matrix row into its `columns-1` leading Code 128 symbol values,
/// validating the trailing Stop pattern.
fn parse_row(matrix: &BitMatrix, r: usize, columns: usize) -> Result<Vec<u8>> {
    let width = matrix.width();
    if !matrix.get(0, r) {
        return Err(Error::undecodable("Codablock F row must start with a bar"));
    }
    let mut runs: Vec<u8> = Vec::new();
    let mut cur = true;
    let mut count = 0u16;
    for x in 0..width {
        let m = matrix.get(x, r);
        if m == cur {
            count += 1;
        } else {
            runs.push(count.min(255) as u8);
            cur = m;
            count = 1;
        }
    }
    runs.push(count.min(255) as u8);

    // Expect (columns-1)*6 char elements + 7 Stop elements.
    let expect = (columns - 1) * 6 + STOP.len();
    if runs.len() != expect {
        return Err(Error::undecodable(
            "Codablock F row has wrong element count",
        ));
    }
    let (char_runs, stop_runs) = runs.split_at((columns - 1) * 6);
    let stop_expected: Vec<u8> = STOP.bytes().map(|b| b - b'0').collect();
    if stop_runs != stop_expected.as_slice() {
        return Err(Error::undecodable("Codablock F Stop pattern not found"));
    }
    let mut vals = Vec::with_capacity(columns - 1);
    for group in char_runs.chunks(6) {
        let v = value_for_widths(group)
            .ok_or_else(|| Error::undecodable("unknown Codablock F symbol pattern"))?;
        vals.push(v);
    }
    Ok(vals)
}

/// Follow the code-set switches in a data-region slice, returning the resulting set.
fn set_after(slice: &[u8], mut set: Set) -> Set {
    for &v in slice {
        set = match (set, v) {
            (Set::A, 99) | (Set::B, 99) => Set::C,
            (Set::A, 100) => Set::B,
            (Set::B, 101) => Set::A,
            (Set::C, 100) => Set::B,
            (Set::C, 101) => Set::A,
            _ => set,
        };
    }
    set
}

/// Reconstruct the payload bytes from all rows (excluding K1/K2 in the last row).
pub(crate) fn reconstruct_payload(
    row_syms: &[Vec<u8>],
    rows: usize,
    columns: usize,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for (r, vals) in row_syms.iter().enumerate() {
        let mut set = sel_set(vals[1])?;
        let mut data_end = columns - 2; // exclusive; excludes C128 check and Stop
        if r == rows - 1 {
            data_end -= 2; // exclude K1, K2
        }
        let mut shift: Option<Set> = None;
        for &v in &vals[3..data_end] {
            let active = shift.take().unwrap_or(set);
            match active {
                Set::C => match v {
                    0..=99 => {
                        bytes.push(b'0' + v / 10);
                        bytes.push(b'0' + v % 10);
                    }
                    100 => set = Set::B,
                    101 => set = Set::A,
                    102 => {}
                    _ => return Err(Error::undecodable("invalid symbol in Codablock F set C")),
                },
                Set::A => match v {
                    0..=95 => bytes.push(if v < 64 { v + 32 } else { v - 64 }),
                    96 | 97 => {}
                    98 => shift = Some(Set::B),
                    99 => set = Set::C,
                    100 => set = Set::B,
                    101 => {} // FNC4
                    102 => {}
                    _ => return Err(Error::undecodable("invalid symbol in Codablock F set A")),
                },
                Set::B => match v {
                    0..=95 => bytes.push(v + 32),
                    96 | 97 => {}
                    98 => shift = Some(Set::A),
                    99 => set = Set::C,
                    100 => {} // FNC4
                    101 => set = Set::A,
                    102 => {}
                    _ => return Err(Error::undecodable("invalid symbol in Codablock F set B")),
                },
            }
        }
    }
    Ok(bytes)
}
