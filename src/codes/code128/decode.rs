//! Code 128 decoding: [`LinearPattern`] → [`Symbol`].
//!
//! This is the structural decoder: it consumes a clean module row (as produced by the
//! encoder or an image-sampling front-end), recovers the exact symbol-value sequence
//! into [`Code128Meta`], validates the modulo-103 check character, and reconstructs the
//! payload [`Segment`]s so the result re-encodes identically.

use super::Code128Meta;
use super::tables::{CodeSet, STOP_VALUE, value_for_widths};
use super::tables::{checksum, reconstruct_segments};
use crate::error::{Error, Result};
use crate::output::{Encoding, LinearPattern};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;
use alloc::vec::Vec;

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

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;
    use alloc::vec;

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
