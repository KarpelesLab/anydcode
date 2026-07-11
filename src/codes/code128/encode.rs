//! Code 128 encoding: [`Symbol`] → [`LinearPattern`].
//!
//! Two paths meet here. [`Code128Encoder::encode`] renders the exact symbol-value
//! sequence pinned in [`Code128Meta`], guaranteeing a lossless round-trip. The
//! `build*` methods take fresh input and choose an efficient code-set sequence,
//! pinning the result in the returned symbol's meta.

use super::Code128Meta;
use super::decode::reconstruct_segments;
use super::tables::{CODE_A, CODE_B, CODE_C, CodeSet, FNC1, PATTERNS, SHIFT, STOP, STOP_VALUE};
use crate::error::{Error, Result};
use crate::output::{Encoding, LinearPattern};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// The quiet zone Code 128 requires on each side, in narrow modules.
const QUIET_ZONE: usize = 10;

/// One element of fresh input to [`Code128Encoder::build`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code128Input {
    /// A literal data byte in `0..=127` (ASCII). Code sets A and B together cover it.
    Data(u8),
    /// FNC1: in GS1-128 first position it marks GS1 mode, elsewhere an AI separator.
    Fnc1,
}

/// Code 128 encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code128Encoder;

impl Code128Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        Code128Encoder
    }

    /// Build a plain Code 128 [`Symbol`] from fresh input, choosing an efficient
    /// code-set sequence. The returned symbol's [`Code128Meta`] pins that sequence.
    pub fn build(&self, input: &[Code128Input]) -> Result<Symbol> {
        self.build_impl(input, false)
    }

    /// Build a GS1-128 [`Symbol`]: a leading FNC1 is prepended automatically, and any
    /// [`Code128Input::Fnc1`] in `input` acts as an AI separator.
    pub fn build_gs1(&self, input: &[Code128Input]) -> Result<Symbol> {
        self.build_impl(input, true)
    }

    /// Convenience: build a plain Code 128 symbol from ASCII `text` (bytes `0..=127`).
    pub fn build_text(&self, text: &str) -> Result<Symbol> {
        let input: Vec<Code128Input> = text.bytes().map(Code128Input::Data).collect();
        self.build(&input)
    }

    fn build_impl(&self, input: &[Code128Input], gs1: bool) -> Result<Symbol> {
        for el in input {
            if let Code128Input::Data(b) = el
                && *b > 127
            {
                return Err(Error::invalid_data(
                    "Code 128 data byte out of range (only 0..=127 supported)",
                ));
            }
        }
        if input.is_empty() && !gs1 {
            return Err(Error::invalid_data("Code 128 input is empty"));
        }
        let symbols = plan_symbols(input, gs1);
        // Derive the GS1 flag from the planned sequence so build and decode agree.
        let (segments, detected_gs1) = reconstruct_segments(&symbols)?;
        let meta = Code128Meta {
            gs1: detected_gs1,
            symbols,
        };
        let symbology = if detected_gs1 {
            Symbology::Gs1_128
        } else {
            Symbology::Code128
        };
        Ok(Symbol::new(symbology, segments, SymbolMeta::Code128(meta)))
    }
}

impl Encode for Code128Encoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if !matches!(symbol.symbology, Symbology::Code128 | Symbology::Gs1_128) {
            return Err(Error::invalid_parameter(
                "Code128Encoder given a non-Code128 symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Code128(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Code 128 symbol missing Code128Meta",
                ));
            }
        };
        let pattern = render(&meta.symbols)?;
        Ok(Encoding::Linear(pattern))
    }
}

/// The modulo-103 check character for a Start-plus-data symbol sequence.
///
/// The Start value has weight 1; each subsequent value has weight equal to its
/// 1-based position after the Start.
pub(crate) fn checksum(symbols: &[u8]) -> u8 {
    let mut sum = symbols[0] as u32;
    for (k, &v) in symbols.iter().enumerate().skip(1) {
        sum += (k as u32) * v as u32;
    }
    (sum % 103) as u8
}

/// Append a symbol pattern (run-width string) to the module vector.
fn append_pattern(modules: &mut Vec<bool>, widths: &str) {
    let mut bar = true;
    for w in widths.bytes().map(|b| (b - b'0') as usize) {
        modules.extend(std::iter::repeat_n(bar, w));
        bar = !bar;
    }
}

/// Render a symbol-value sequence (Start + data) into a [`LinearPattern`].
fn render(symbols: &[u8]) -> Result<LinearPattern> {
    if symbols.is_empty() {
        return Err(Error::invalid_parameter(
            "Code 128 symbol sequence is empty",
        ));
    }
    if CodeSet::from_start(symbols[0]).is_none() {
        return Err(Error::invalid_parameter(
            "Code 128 sequence does not begin with a Start value",
        ));
    }
    for &v in symbols {
        if v >= STOP_VALUE {
            return Err(Error::invalid_parameter(
                "Code 128 data contains a Stop/out-of-range symbol value",
            ));
        }
    }
    let check = checksum(symbols);
    let mut modules = Vec::new();
    for &v in symbols {
        append_pattern(&mut modules, PATTERNS[v as usize]);
    }
    append_pattern(&mut modules, PATTERNS[check as usize]);
    append_pattern(&mut modules, STOP);
    Ok(LinearPattern {
        modules,
        quiet_zone: QUIET_ZONE,
    })
}

// ---------------------------------------------------------------------------
// Fresh-input code-set planner
// ---------------------------------------------------------------------------

fn is_digit_input(el: Code128Input) -> bool {
    matches!(el, Code128Input::Data(b) if b.is_ascii_digit())
}

/// Number of consecutive digit inputs starting at `i`.
fn count_digits(input: &[Code128Input], i: usize) -> usize {
    input[i..]
        .iter()
        .take_while(|el| is_digit_input(**el))
        .count()
}

/// Whether byte `b` is representable directly in code set `set` (A or B).
fn representable(set: CodeSet, b: u8) -> bool {
    match set {
        CodeSet::A => b < 96,
        CodeSet::B => b >= 32,
        CodeSet::C => false,
    }
}

/// The symbol value of byte `b` in code set `set` (A or B). Assumes representable.
fn value_in(set: CodeSet, b: u8) -> u8 {
    match set {
        CodeSet::A => {
            if b < 32 {
                b + 64
            } else {
                b - 32
            }
        }
        CodeSet::B => b - 32,
        CodeSet::C => unreachable!("value_in called with code set C"),
    }
}

/// Choose an efficient symbol-value sequence for the input.
fn plan_symbols(input: &[Code128Input], gs1: bool) -> Vec<u8> {
    let len = input.len();

    // Pick the starting code set.
    let lead_digits = count_digits(input, 0);
    let start = if (lead_digits >= 2 && lead_digits == len && lead_digits.is_multiple_of(2))
        || lead_digits >= 4
    {
        CodeSet::C
    } else {
        match input.first() {
            Some(Code128Input::Data(b)) if *b < 32 => CodeSet::A,
            _ => CodeSet::B,
        }
    };

    let mut out = vec![start.start_value()];
    if gs1 {
        out.push(FNC1);
    }
    let mut set = start;
    let mut i = 0;
    while i < len {
        match input[i] {
            Code128Input::Fnc1 => {
                out.push(FNC1);
                i += 1;
            }
            Code128Input::Data(b) => {
                if set == CodeSet::C {
                    if is_digit_input(input[i]) && i + 1 < len && is_digit_input(input[i + 1]) {
                        let d0 = digit_val(input[i]);
                        let d1 = digit_val(input[i + 1]);
                        out.push(d0 * 10 + d1);
                        i += 2;
                    } else {
                        // Leave Code C for the character at i.
                        if b < 32 {
                            out.push(CODE_A);
                            set = CodeSet::A;
                        } else {
                            out.push(CODE_B);
                            set = CodeSet::B;
                        }
                    }
                } else {
                    let run = count_digits(input, i);
                    let at_end = i + run == len;
                    let want_c = run >= 6 || (run >= 4 && at_end);
                    if want_c {
                        if !run.is_multiple_of(2) {
                            // Emit the odd leading digit in the current set first.
                            out.push(value_in(set, b));
                            i += 1;
                        }
                        out.push(CODE_C);
                        set = CodeSet::C;
                    } else {
                        emit_char(&mut out, &mut set, input, i);
                        i += 1;
                    }
                }
            }
        }
    }
    out
}

fn digit_val(el: Code128Input) -> u8 {
    match el {
        Code128Input::Data(b) => b - b'0',
        Code128Input::Fnc1 => unreachable!("digit_val on FNC1"),
    }
}

/// Emit one data character at `input[i]` in code set A or B, latching or shifting to
/// the other set when the character is not representable in the current one.
fn emit_char(out: &mut Vec<u8>, set: &mut CodeSet, input: &[Code128Input], i: usize) {
    let b = match input[i] {
        Code128Input::Data(b) => b,
        Code128Input::Fnc1 => unreachable!("emit_char on FNC1"),
    };
    if representable(*set, b) {
        out.push(value_in(*set, b));
        return;
    }
    let other = match *set {
        CodeSet::A => CodeSet::B,
        CodeSet::B => CodeSet::A,
        CodeSet::C => CodeSet::B,
    };
    // Shift when the following character returns to the current set; otherwise latch.
    let next_returns = match input.get(i + 1) {
        Some(Code128Input::Data(nb)) => representable(*set, *nb),
        Some(Code128Input::Fnc1) | None => true,
    };
    if next_returns {
        out.push(SHIFT);
        out.push(value_in(other, b));
    } else {
        out.push(match other {
            CodeSet::A => CODE_A,
            CodeSet::B => CODE_B,
            CodeSet::C => unreachable!(),
        });
        *set = other;
        out.push(value_in(other, b));
    }
}

#[cfg(test)]
mod tests {
    use super::super::tables::START_A;
    use super::*;

    /// Independent reference: Wikipedia "Code 128" worked example. Encoding
    /// "PJJ123C" with Start Code A gives the symbol values [103,48,42,42,17,18,19,35]
    /// and a modulo-103 check character of 54.
    #[test]
    fn wikipedia_pjj123c_checksum() {
        let symbols = [START_A, 48, 42, 42, 17, 18, 19, 35];
        assert_eq!(checksum(&symbols), 54);
    }

    /// The same example rendered to modules must equal the concatenation of the
    /// documented Start A / data / check(54) / Stop patterns.
    #[test]
    fn wikipedia_pjj123c_modules() {
        let symbols = vec![START_A, 48, 42, 42, 17, 18, 19, 35];
        let pattern = render(&symbols).unwrap();

        let mut expected = Vec::new();
        // Start A, P, J, J, 1, 2, 3, C, check=54, Stop.
        for widths in [
            "211412",  // Start A (103)
            "313121",  // P (48)
            "112133",  // J (42)
            "112133",  // J (42)
            "123221",  // 1 (17)
            "223211",  // 2 (18)
            "221132",  // 3 (19)
            "131321",  // C (35)
            "311123",  // check (54)
            "2331112", // Stop
        ] {
            append_pattern(&mut expected, widths);
        }
        assert_eq!(pattern.modules, expected);
        assert_eq!(pattern.quiet_zone, QUIET_ZONE);
    }
}
