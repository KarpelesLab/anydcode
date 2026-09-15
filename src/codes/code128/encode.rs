//! Code 128 encoding: symbol-value sequence → modules.
//!
//! Two paths meet here. [`Code128Encoder::encode_into`] (heap-free) and the
//! [`Encode`](crate::traits::Encode) impl render the exact symbol-value sequence
//! pinned in [`Code128Meta`](super::Code128Meta), guaranteeing a lossless round-trip.
//! The `build*` methods (`alloc`) take fresh input and choose an efficient code-set
//! sequence, pinning the result in the returned symbol's meta.

#[cfg(feature = "alloc")]
use super::Code128Meta;
#[cfg(feature = "alloc")]
use super::tables::reconstruct_segments;
#[cfg(feature = "alloc")]
use super::tables::{CODE_A, CODE_B, CODE_C, FNC1, SHIFT};
use super::tables::{CodeSet, PATTERNS, STOP, STOP_VALUE, checksum};
use crate::error::{Error, Result};
use crate::output::LinearSink;
#[cfg(feature = "alloc")]
use crate::output::{Encoding, LinearPattern};
#[cfg(feature = "alloc")]
use crate::symbol::{Symbol, SymbolMeta};
#[cfg(feature = "alloc")]
use crate::symbology::Symbology;
#[cfg(feature = "alloc")]
use crate::traits::Encode;
#[cfg(feature = "alloc")]
use alloc::{vec, vec::Vec};

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

    /// Number of modules [`Code128Encoder::encode_into`] emits for a symbol-value
    /// sequence of `symbols_len` values (Start + data), excluding quiet zones. Exact:
    /// every symbol character, including the derived check, is 11 modules and the
    /// Stop pattern 13.
    pub const fn max_modules(symbols_len: usize) -> usize {
        (symbols_len + 1) * 11 + 13
    }

    /// Heap-free encoding: render a symbol-value sequence to `out`.
    ///
    /// `symbols` is the Start value (`103`/`104`/`105`) followed by every data symbol
    /// value, exactly as stored in [`Code128Meta::symbols`](super::Code128Meta); the
    /// modulo-103 check character and the Stop pattern are appended here. The
    /// sequence is validated before the first module is written. Size a
    /// [`LinearBuf`](crate::output::LinearBuf) with [`Code128Encoder::max_modules`].
    pub fn encode_into<S: LinearSink>(&self, symbols: &[u8], out: &mut S) -> Result<()> {
        let Some(&start) = symbols.first() else {
            return Err(Error::invalid_parameter(
                "Code 128 symbol sequence is empty",
            ));
        };
        if CodeSet::from_start(start).is_none() {
            return Err(Error::invalid_parameter(
                "Code 128 sequence does not begin with a Start value",
            ));
        }
        if symbols.iter().any(|&v| v >= STOP_VALUE) {
            return Err(Error::invalid_parameter(
                "Code 128 data contains a Stop/out-of-range symbol value",
            ));
        }
        let check = checksum(symbols);
        out.begin(QUIET_ZONE)?;
        for &v in symbols {
            push_widths(out, PATTERNS[v as usize])?;
        }
        push_widths(out, PATTERNS[check as usize])?;
        push_widths(out, STOP)
    }
}

#[cfg(feature = "alloc")]
impl Code128Encoder {
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

#[cfg(feature = "alloc")]
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
        let mut pattern = LinearPattern::new();
        self.encode_into(&meta.symbols, &mut pattern)?;
        Ok(Encoding::Linear(pattern))
    }
}

/// Append a symbol pattern (run-width string, starting with a bar) to `out`.
fn push_widths(out: &mut impl LinearSink, widths: &str) -> Result<()> {
    let mut bar = true;
    for w in widths.bytes().map(|b| (b - b'0') as usize) {
        out.push_run(bar, w)?;
        bar = !bar;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Fresh-input code-set planner
// ---------------------------------------------------------------------------

#[cfg(feature = "alloc")]
fn is_digit_input(el: Code128Input) -> bool {
    matches!(el, Code128Input::Data(b) if b.is_ascii_digit())
}

/// Number of consecutive digit inputs starting at `i`.
#[cfg(feature = "alloc")]
fn count_digits(input: &[Code128Input], i: usize) -> usize {
    input[i..]
        .iter()
        .take_while(|el| is_digit_input(**el))
        .count()
}

/// Whether byte `b` is representable directly in code set `set` (A or B).
#[cfg(feature = "alloc")]
fn representable(set: CodeSet, b: u8) -> bool {
    match set {
        CodeSet::A => b < 96,
        CodeSet::B => b >= 32,
        CodeSet::C => false,
    }
}

/// The symbol value of byte `b` in code set `set` (A or B). Assumes representable.
#[cfg(feature = "alloc")]
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
#[cfg(feature = "alloc")]
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

#[cfg(feature = "alloc")]
fn digit_val(el: Code128Input) -> u8 {
    match el {
        Code128Input::Data(b) => b - b'0',
        Code128Input::Fnc1 => unreachable!("digit_val on FNC1"),
    }
}

/// Emit one data character at `input[i]` in code set A or B, latching or shifting to
/// the other set when the character is not representable in the current one.
#[cfg(feature = "alloc")]
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

#[cfg(all(test, feature = "encode", feature = "decode"))]
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
        let symbols = [START_A, 48, 42, 42, 17, 18, 19, 35];
        let mut pattern = LinearPattern::new();
        Code128Encoder::new()
            .encode_into(&symbols, &mut pattern)
            .unwrap();

        let mut expected = LinearPattern::new();
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
            push_widths(&mut expected, widths).unwrap();
        }
        assert_eq!(pattern.modules, expected.modules);
        assert_eq!(pattern.quiet_zone, QUIET_ZONE);
    }
}
