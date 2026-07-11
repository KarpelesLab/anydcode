//! Code 128 decoding: [`LinearPattern`] → [`Symbol`].
//!
//! This is the structural decoder: it consumes a clean module row (as produced by the
//! encoder or an image-sampling front-end), recovers the exact symbol-value sequence
//! into [`Code128Meta`], validates the modulo-103 check character, and reconstructs the
//! payload [`Segment`]s so the result re-encodes identically.

use super::Code128Meta;
use super::encode::checksum;
use super::tables::{CODE_A, CODE_B, CODE_C, CodeSet, FNC1, SHIFT, STOP_VALUE, value_for_widths};
use crate::error::{Error, Result};
use crate::output::{Encoding, LinearPattern};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// GS1 AI separator byte used when a non-leading FNC1 is decoded into the payload.
const GS: u8 = 0x1D;

/// Code 128 structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code128Decoder;

impl Code128Decoder {
    /// A new decoder.
    pub fn new() -> Self {
        Code128Decoder
    }

    /// Decode a clean Code 128 linear pattern into a [`Symbol`].
    pub fn decode_linear(&self, pattern: &LinearPattern) -> Result<Symbol> {
        let runs = run_lengths(&pattern.modules)?;
        // The final seven runs are the Stop pattern; the rest are 6-run symbol chars.
        if runs.len() < 6 + 7 {
            return Err(Error::undecodable("Code 128 pattern too short"));
        }
        let (value_runs, stop_runs) = runs.split_at(runs.len() - 7);
        let stop_expected = [2usize, 3, 3, 1, 1, 1, 2];
        if stop_runs != stop_expected {
            return Err(Error::undecodable("Code 128 Stop pattern not found"));
        }
        if !value_runs.len().is_multiple_of(6) {
            return Err(Error::undecodable(
                "Code 128 module count is not a whole number of characters",
            ));
        }

        // Each group of six runs is one symbol character (Start, data..., check).
        let mut values = Vec::with_capacity(value_runs.len() / 6);
        for group in value_runs.chunks(6) {
            let v = value_for_widths(group)
                .ok_or_else(|| Error::undecodable("unknown Code 128 symbol pattern"))?;
            values.push(v);
        }

        // Split off the trailing check character and validate it.
        let check = values
            .pop()
            .ok_or_else(|| Error::undecodable("missing Code 128 check character"))?;
        if values.is_empty() {
            return Err(Error::undecodable("Code 128 has no Start character"));
        }
        if CodeSet::from_start(values[0]).is_none() {
            return Err(Error::undecodable("Code 128 does not begin with a Start"));
        }
        if values.iter().any(|&v| v >= STOP_VALUE) {
            return Err(Error::undecodable(
                "Code 128 data contains a reserved value",
            ));
        }
        if checksum(&values) != check {
            return Err(Error::undecodable("Code 128 check character mismatch"));
        }

        let (segments, gs1) = reconstruct_segments(&values)?;
        let symbology = if gs1 {
            Symbology::Gs1_128
        } else {
            Symbology::Code128
        };
        let meta = Code128Meta {
            gs1,
            symbols: values,
        };
        Ok(Symbol::new(symbology, segments, SymbolMeta::Code128(meta)))
    }
}

impl Decode for Code128Decoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Linear(p) => self.decode_linear(p),
            Encoding::Matrix(_) => Err(Error::Unsupported {
                what: "Code 128 decode of a matrix",
            }),
        }
    }
}

/// Convert a module row into element run lengths, starting with a bar.
fn run_lengths(modules: &[bool]) -> Result<Vec<usize>> {
    if modules.is_empty() {
        return Err(Error::undecodable("empty Code 128 module row"));
    }
    if !modules[0] {
        return Err(Error::undecodable("Code 128 row must start with a bar"));
    }
    let mut runs = Vec::new();
    let mut cur = modules[0];
    let mut count = 0usize;
    for &m in modules {
        if m == cur {
            count += 1;
        } else {
            runs.push(count);
            cur = m;
            count = 1;
        }
    }
    runs.push(count);
    Ok(runs)
}

/// Reconstruct the human-readable payload [`Segment`]s from a Start-plus-data symbol
/// sequence, and report whether the symbol is GS1-128 (leading FNC1).
///
/// Shared by the decoder and the encoder's `build` path so both produce identical
/// payload segments for a given symbol-value sequence.
pub(crate) fn reconstruct_segments(values: &[u8]) -> Result<(Vec<Segment>, bool)> {
    let mut set = CodeSet::from_start(values[0])
        .ok_or_else(|| Error::undecodable("Code 128 sequence has no Start"))?;
    let gs1 = values.get(1) == Some(&FNC1);

    let mut bytes = Vec::new();
    let mut shift: Option<CodeSet> = None;
    let mut first_data = true;

    for &v in &values[1..] {
        let active = shift.take().unwrap_or(set);
        let is_leading = first_data;
        first_data = false;

        match active {
            CodeSet::C => match v {
                0..=99 => {
                    bytes.push(b'0' + v / 10);
                    bytes.push(b'0' + v % 10);
                }
                CODE_B => set = CodeSet::B,
                CODE_A => set = CodeSet::A,
                FNC1 => push_fnc1(&mut bytes, is_leading),
                _ => return Err(Error::undecodable("invalid symbol in Code C")),
            },
            CodeSet::A => match v {
                0..=95 => bytes.push(if v < 64 { v + 32 } else { v - 64 }),
                96 | 97 => {} // FNC3 / FNC2: no payload byte
                SHIFT => shift = Some(CodeSet::B),
                CODE_C => set = CodeSet::C,
                CODE_B => set = CodeSet::B,
                101 => {} // FNC4 (Code A): extended latch, no payload byte
                FNC1 => push_fnc1(&mut bytes, is_leading),
                _ => return Err(Error::undecodable("invalid symbol in Code A")),
            },
            CodeSet::B => match v {
                0..=95 => bytes.push(v + 32),
                96 | 97 => {} // FNC3 / FNC2
                SHIFT => shift = Some(CodeSet::A),
                CODE_C => set = CodeSet::C,
                100 => {} // FNC4 (Code B)
                CODE_A => set = CodeSet::A,
                FNC1 => push_fnc1(&mut bytes, is_leading),
                _ => return Err(Error::undecodable("invalid symbol in Code B")),
            },
        }
    }

    let segments = if bytes.is_empty() {
        Vec::new()
    } else {
        vec![Segment::byte(bytes)]
    };
    Ok((segments, gs1))
}

/// Push the payload effect of an FNC1: the leading (GS1-mode) FNC1 emits nothing; a
/// later FNC1 is an AI separator, rendered as the GS control byte.
fn push_fnc1(bytes: &mut Vec<u8>, is_leading: bool) {
    if !is_leading {
        bytes.push(GS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_lengths_basic() {
        let modules = vec![true, true, false, true];
        assert_eq!(run_lengths(&modules).unwrap(), vec![2, 1, 1]);
    }

    #[test]
    fn reconstruct_rejects_non_start() {
        // 0 is not a Start value.
        assert!(reconstruct_segments(&[0, 1]).is_err());
    }
}
