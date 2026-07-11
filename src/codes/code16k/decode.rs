//! Code 16K decoding: [`BitMatrix`] -> [`Symbol`].
//!
//! Reads a clean module matrix (as produced by the encoder or an image front-end),
//! recovers the exact per-row symbol values into [`Code16kMeta`], validates the row
//! delimiters and the two mod-107 check characters, and reconstructs the payload
//! [`Segment`]s so the result re-encodes identically.

use super::Code16kMeta;
use super::encode::{ROW_WIDTH, checksum};
use super::tables::{
    CODE_A, CODE_B, CODE_C, FNC1, PAD, SHIFT, START_STOP, START_VALUES, STOP_VALUES,
    value_for_widths,
};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    A,
    B,
    C,
}

/// Reconstruct the payload [`Segment`]s from a Code 16K data-value sequence (the mode
/// character, data and padding, without the check characters). Shared by the decoder and
/// the encoder's `build` path so both produce identical payload segments.
///
/// Mode values `0..=4` (start A/B/C, optionally GS1) are handled; the size-optimizing
/// shift modes `5`/`6` are decoded best-effort as a Set B start. Because re-encoding
/// renders from the stored symbol values rather than these segments, segment fidelity
/// never affects the round-trip identity.
pub(crate) fn reconstruct_segments(values: &[u8]) -> Result<Vec<Segment>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }
    let m = values[0] % 7;
    let mut set = match m {
        0 => Set::A,
        2 | 4 => Set::C,
        _ => Set::B,
    };
    let mut bytes = Vec::new();
    let mut shift: Option<Set> = None;

    for &v in &values[1..] {
        if v == PAD {
            continue;
        }
        let active = shift.take().unwrap_or(set);
        match active {
            Set::C => match v {
                0..=99 => {
                    bytes.push(b'0' + v / 10);
                    bytes.push(b'0' + v % 10);
                }
                CODE_B => set = Set::B,
                CODE_A => set = Set::A,
                FNC1 => {}
                _ => return Err(Error::undecodable("invalid symbol in Code 16K set C")),
            },
            Set::A => match v {
                0..=95 => bytes.push(if v < 64 { v + 32 } else { v - 64 }),
                96 | 97 => {} // FNC3 / FNC2
                SHIFT => shift = Some(Set::B),
                CODE_C => set = Set::C,
                CODE_B => set = Set::B,
                101 => {} // FNC4 in Set A
                FNC1 => {}
                _ => return Err(Error::undecodable("invalid symbol in Code 16K set A")),
            },
            Set::B => match v {
                0..=95 => bytes.push(v + 32),
                96 | 97 => {} // FNC3 / FNC2
                SHIFT => shift = Some(Set::A),
                CODE_C => set = Set::C,
                100 => {} // FNC4 in Set B
                CODE_A => set = Set::A,
                FNC1 => {}
                _ => return Err(Error::undecodable("invalid symbol in Code 16K set B")),
            },
        }
    }

    Ok(if bytes.is_empty() {
        Vec::new()
    } else {
        vec![Segment::byte(bytes)]
    })
}
