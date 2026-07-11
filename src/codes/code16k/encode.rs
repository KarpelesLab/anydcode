//! Code 16K encoding: [`Symbol`] -> [`BitMatrix`], plus the fresh-input `build` path.
//!
//! [`Code16kEncoder::encode`] renders the exact symbol-value sequence pinned in
//! [`Code16kMeta`], guaranteeing a lossless round-trip. [`Code16kEncoder::build`] takes
//! fresh bytes, plans a Code 128 A/B code-set sequence, and pins the result in the
//! returned symbol's meta.

use super::Code16kMeta;
use super::decode::reconstruct_segments;
use super::tables::{
    C128_PATTERNS, CODE_A, CODE_B, PAD, SHIFT, START_STOP, START_VALUES, STOP_VALUES,
};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// The quiet zone Code 16K requires on each side, in narrow modules.
pub(crate) const QUIET_ZONE: usize = 10;

/// Fixed symbol width in modules: 7 (start) + 1 (guard) + 5*11 (chars) + 7 (stop).
pub(crate) const ROW_WIDTH: usize = 70;

/// Code 16K encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code16kEncoder;

impl Code16kEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Code16kEncoder
    }

    /// Build a Code 16K [`Symbol`] from fresh bytes (each `0..=127`).
    ///
    /// The planner uses Code 128 sets A and B (control characters latch to A, the rest
    /// to B); it does not use the Code C numeric-compression optimization, so digits
    /// encode one symbol each. The result is still a valid, minimal-size-family Code 16K
    /// symbol. The chosen symbol-value sequence is pinned in [`Code16kMeta`].
    pub fn build(&self, data: &[u8]) -> Result<Symbol> {
        self.build_rows(data, None)
    }

    /// Build with an explicit minimum row count (`2..=16`); the symbol grows to fit the
    /// data if `min_rows` is too small.
    pub fn build_rows(&self, data: &[u8], min_rows: Option<usize>) -> Result<Symbol> {
        if data.is_empty() {
            return Err(Error::invalid_data("Code 16K input is empty"));
        }
        for &b in data {
            if b > 127 {
                return Err(Error::invalid_data(
                    "Code 16K build supports bytes 0..=127 (extended ASCII not implemented)",
                ));
            }
        }
        let (m, body) = plan(data);
        // bar_characters counts the mode character plus the planned body.
        let bar_characters = 1 + body.len();
        let mut pads_needed = 5 - ((bar_characters + 2) % 5);
        if pads_needed == 5 {
            pads_needed = 0;
        }
        if bar_characters + pads_needed < 8 {
            pads_needed += 8 - (bar_characters + pads_needed);
        }
        let mut rows = (bar_characters + pads_needed).div_ceil(5);
        let mut extra_pads = 0;
        if let Some(min) = min_rows {
            if !(2..=16).contains(&min) {
                return Err(Error::invalid_parameter("Code 16K rows must be 2..=16"));
            }
            if min > rows {
                extra_pads = (min - rows) * 5;
                rows = min;
            }
        }
        if rows > 16 {
            return Err(Error::capacity("Code 16K data exceeds 16 rows"));
        }

        // values: mode char + body + pads, length rows*5 - 2 (the two checks are derived).
        let mut values = Vec::with_capacity(rows * 5 - 2);
        values.push(0); // placeholder for mode char
        values.extend_from_slice(&body);
        values.extend(std::iter::repeat_n(PAD, pads_needed + extra_pads));
        values[0] = 7 * (rows as u8 - 2) + m;
        debug_assert_eq!(values.len(), rows * 5 - 2);

        let meta = Code16kMeta { rows, values };
        let segments = reconstruct_segments(&meta.values)?;
        Ok(Symbol::new(
            Symbology::Code16k,
            segments,
            SymbolMeta::Code16k(meta),
        ))
    }
}

impl Encode for Code16kEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::Code16k {
            return Err(Error::invalid_parameter(
                "Code16kEncoder given a non-Code16k symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Code16k(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Code 16K symbol missing Code16kMeta",
                ));
            }
        };
        Ok(Encoding::Matrix(render(meta)?))
    }
}

/// The two modulo-107 check characters for a Code 16K data-value sequence (mode char +
/// data + pads, without the checks). Ported from EN 12323 / zint.
pub(crate) fn checksum(values: &[u8]) -> (u8, u8) {
    let mut first: u32 = 0;
    let mut second: u32 = 0;
    for (i, &v) in values.iter().enumerate() {
        first += (i as u32 + 2) * v as u32;
        second += (i as u32 + 1) * v as u32;
    }
    let first_check = (first % 107) as u8;
    second += first_check as u32 * (values.len() as u32 + 1);
    let second_check = (second % 107) as u8;
    (first_check, second_check)
}

/// Render a validated [`Code16kMeta`] into its module matrix.
pub(crate) fn render(meta: &Code16kMeta) -> Result<BitMatrix> {
    let rows = meta.rows;
    if !(2..=16).contains(&rows) {
        return Err(Error::invalid_parameter("Code 16K rows must be 2..=16"));
    }
    if meta.values.len() != rows * 5 - 2 {
        return Err(Error::invalid_parameter(
            "Code 16K value count inconsistent with row count",
        ));
    }
    for &v in &meta.values {
        if v as usize >= C128_PATTERNS.len() {
            return Err(Error::invalid_parameter(
                "Code 16K symbol value out of range",
            ));
        }
    }
    let (c1, c2) = checksum(&meta.values);
    let mut full = meta.values.clone();
    full.push(c1);
    full.push(c2);

    let mut matrix = BitMatrix::new(ROW_WIDTH, rows, QUIET_ZONE);
    for row in 0..rows {
        let mut widths: Vec<u8> = Vec::with_capacity(39);
        widths.extend_from_slice(&START_STOP[START_VALUES[row]]);
        widths.push(1); // guard
        for i in 0..5 {
            let v = full[row * 5 + i];
            for b in C128_PATTERNS[v as usize].bytes() {
                widths.push(b - b'0');
            }
        }
        widths.extend_from_slice(&START_STOP[STOP_VALUES[row]]);

        // Render alternating dark/light across the row, starting dark.
        let mut x = 0usize;
        let mut dark = true;
        for w in widths {
            for _ in 0..w {
                if dark {
                    matrix.set(x, row, true);
                }
                x += 1;
            }
            dark = !dark;
        }
        debug_assert_eq!(x, ROW_WIDTH);
    }
    Ok(matrix)
}

// ---------------------------------------------------------------------------
// Fresh-input code-set planner (Code 128 A/B, no Code C compression)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    A,
    B,
}

fn representable(set: Set, b: u8) -> bool {
    match set {
        Set::A => b < 96,
        Set::B => (32..=127).contains(&b),
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
    }
}

/// Plan the mode value `m` (0 = start A, 1 = start B) and the body symbol values.
fn plan(data: &[u8]) -> (u8, Vec<u8>) {
    let mut set = if data[0] < 32 { Set::A } else { Set::B };
    let m = match set {
        Set::A => 0,
        Set::B => 1,
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if representable(set, b) {
            out.push(value_in(set, b));
            i += 1;
            continue;
        }
        let other = match set {
            Set::A => Set::B,
            Set::B => Set::A,
        };
        // Shift if the following character returns to the current set; else latch.
        let next_returns = match data.get(i + 1) {
            Some(&nb) => representable(set, nb),
            None => true,
        };
        if next_returns {
            out.push(SHIFT);
            out.push(value_in(other, b));
            i += 1;
        } else {
            out.push(match other {
                Set::A => CODE_A,
                Set::B => CODE_B,
            });
            set = other;
        }
    }
    (m, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent reference (zint 2.16.0, `zint --verbose --barcode=CODE16K
    /// --data=ABCD1234`): the data codewords are `[1,33,34,35,36,99,12,34]` with two
    /// mod-107 check characters `[11,40]`.
    #[test]
    fn zint_abcd1234_checksum() {
        let values = [1u8, 33, 34, 35, 36, 99, 12, 34];
        assert_eq!(checksum(&values), (11, 40));
    }
}
