//! Code 11 (USD-8) encoder and structural decoder.
//!
//! Code 11 is a discrete linear symbology used mainly in telecommunications labelling.
//! It encodes the digits `0-9` and the dash `-`, framed by a shared start/stop
//! character (three bars and two spaces per character, narrow/wide element widths, a
//! single narrow inter-character gap). One or two modulo-11 check characters are
//! appended: conventionally one (`C`) for short messages and two (`C`, `K`) once the
//! data reaches ten characters. [`Code11Meta`] records how many were used so the
//! symbol re-encodes losslessly; the [`Code11Decoder`] is told the same count since it
//! is not encoded in-band.
//!
//! ## Sources
//!
//! Element patterns are from the Code 11 article on Wikipedia. The check-digit
//! algorithm (C: weights 1..=10 right-to-left, K: weights 1..=9 including C, both
//! modulo 11, dash = 10) is from Bar Code Island's Code 11 specification
//! (<http://www.barcodeisland.com/code11.phtml>); worked example `123-45` → C = `5`,
//! K = `2`, and the Wikipedia example `012345` → C = `2`.

// With neither `encode` nor `decode` only the metadata types remain; their shared
// helpers are then unused.
#![cfg_attr(
    not(any(feature = "encode", feature = "decode")),
    allow(dead_code, unused_imports)
)]

use crate::error::{Error, Result};
#[cfg(feature = "alloc")]
use crate::output::Encoding;
#[cfg(all(feature = "alloc", feature = "encode"))]
use crate::output::LinearPattern;
#[cfg(feature = "encode")]
use crate::output::LinearSink;
#[cfg(feature = "alloc")]
use crate::segment::Segment;
#[cfg(feature = "alloc")]
use crate::symbol::{Symbol, SymbolMeta};
#[cfg(feature = "alloc")]
use crate::symbology::Symbology;
#[cfg(feature = "decode")]
use crate::traits::Decode;
#[cfg(all(feature = "alloc", feature = "encode"))]
use crate::traits::Encode;
#[cfg(all(feature = "alloc", feature = "encode"))]
use alloc::format;
#[cfg(feature = "decode")]
use alloc::string::String;
#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "decode")]
use alloc::vec::Vec;

/// Quiet-zone margin, in narrow modules, emitted on each side of the pattern.
#[cfg(feature = "encode")]
const QUIET_ZONE: usize = 10;

/// Element patterns for values 0..=10 (`0`-`9` then `-`); `1` = bar, `0` = space,
/// `wide = 2` narrow modules.
const PATTERNS: [&str; 11] = [
    "101011",  // 0
    "1101011", // 1
    "1001011", // 2
    "1100101", // 3
    "1011011", // 4
    "1101101", // 5
    "1001101", // 6
    "1010011", // 7
    "1101001", // 8
    "110101",  // 9
    "101101",  // - (value 10)
];

/// The shared start/stop character pattern.
const START_STOP: &str = "1011001";

/// The value 0..=10 of a Code 11 data character, or `None` if outside the set.
#[cfg(feature = "encode")]
fn char_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'-' => Some(10),
        _ => None,
    }
}

/// The ASCII character for a value 0..=10.
#[cfg(feature = "decode")]
fn value_char(v: u8) -> u8 {
    if v == 10 { b'-' } else { b'0' + v }
}

/// Weighted modulo-11 checksum: rightmost character weight 1, increasing leftward,
/// wrapping at `max_weight`.
#[cfg(feature = "decode")]
fn checksum(values: &[u8], max_weight: usize) -> u8 {
    let mut sum: u32 = 0;
    for (i, &v) in values.iter().rev().enumerate() {
        let weight = (i % max_weight) as u32 + 1;
        sum += weight * v as u32;
    }
    (sum % 11) as u8
}

/// Parameters required to re-encode a Code 11 symbol identically (lossless round-trip).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Code11Meta {
    /// Number of modulo-11 check characters appended (0, 1 = `C`, or 2 = `C` and `K`).
    pub check_count: u8,
}

/// Code 11 encoder.
#[cfg(feature = "encode")]
#[derive(Debug, Default, Clone, Copy)]
pub struct Code11Encoder;

#[cfg(feature = "encode")]
impl Code11Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

    /// Upper bound on the modules [`Code11Encoder::encode_into`] emits for `data_len`
    /// payload bytes, excluding quiet zones (characters are 6 or 7 modules wide, so
    /// the exact count depends on the data). Values of `check_count` above 2 are
    /// treated as 2.
    pub const fn max_modules(data_len: usize, meta: &Code11Meta) -> usize {
        let checks = if meta.check_count > 2 {
            2
        } else {
            meta.check_count as usize
        };
        // start/stop + (gap + widest character) per data/check character + gap + stop.
        (data_len + checks) * 8 + 15
    }

    /// Heap-free encoding: write the symbol for `data` (digits and `-`) under `meta`
    /// to `out`.
    ///
    /// `meta.check_count` must be 0, 1, or 2. The input is validated before the first
    /// module is written. Size a [`LinearBuf`](crate::output::LinearBuf) with
    /// [`Code11Encoder::max_modules`].
    pub fn encode_into<S: LinearSink>(
        &self,
        data: &[u8],
        meta: &Code11Meta,
        out: &mut S,
    ) -> Result<()> {
        if meta.check_count > 2 {
            return Err(Error::invalid_parameter(
                "Code 11 check_count must be 0, 1 or 2",
            ));
        }
        if data.iter().any(|&b| char_value(b).is_none()) {
            return Err(Error::invalid_data("byte is not a Code 11 character"));
        }

        // Weighted modulo-11 checks computed in one pass: the rightmost data
        // character has weight 1 for C (wrapping at 10) and weight 2 for K (wrapping
        // at 9, C itself taking weight 1).
        let n = data.len();
        let (mut c_sum, mut k_sum) = (0u32, 0u32);
        for (i, &b) in data.iter().enumerate() {
            let v = u32::from(char_value(b).expect("validated Code 11 character"));
            let from_right = n - 1 - i;
            c_sum += ((from_right % 10) as u32 + 1) * v;
            k_sum += (((from_right + 1) % 9) as u32 + 1) * v;
        }
        let c = (c_sum % 11) as u8;
        let k = ((k_sum + u32::from(c)) % 11) as u8;

        out.begin(QUIET_ZONE)?;
        push_pattern(out, START_STOP)?;
        let emit = |out: &mut S, v: u8| -> Result<()> {
            out.push(false)?;
            push_pattern(out, PATTERNS[v as usize])
        };
        for &b in data {
            emit(out, char_value(b).expect("validated Code 11 character"))?;
        }
        if meta.check_count >= 1 {
            emit(out, c)?;
        }
        if meta.check_count == 2 {
            emit(out, k)?;
        }
        out.push(false)?;
        push_pattern(out, START_STOP)
    }
}

#[cfg(all(feature = "alloc", feature = "encode"))]
impl Code11Encoder {
    /// Build a reproducible [`Symbol`] from raw `data` (digits and `-`).
    ///
    /// `check_count` must be 0, 1, or 2. The payload is stored as a single
    /// [`Segment::numeric`] when it is all digits, otherwise [`Segment::alphanumeric`]
    /// (the dash is not a numeric byte).
    pub fn build(&self, data: &[u8], check_count: u8) -> Result<Symbol> {
        if check_count > 2 {
            return Err(Error::invalid_parameter(
                "Code 11 check_count must be 0, 1 or 2",
            ));
        }
        if let Some(&bad) = data.iter().find(|&&b| char_value(b).is_none()) {
            return Err(Error::invalid_data(format!(
                "byte {bad:#04x} is not a Code 11 character (digits and '-' only)"
            )));
        }
        let segment = if data.iter().all(|b| b.is_ascii_digit()) {
            Segment::numeric(data.to_vec())
        } else {
            Segment::alphanumeric(data.to_vec())
        };
        Ok(Symbol::new(
            Symbology::Code11,
            vec![segment],
            SymbolMeta::Code11(Code11Meta { check_count }),
        ))
    }
}

#[cfg(all(feature = "alloc", feature = "encode"))]
impl Encode for Code11Encoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::Code11 {
            return Err(Error::invalid_parameter(
                "Code11Encoder given a non-Code11 symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Code11(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Code 11 symbol missing Code11Meta",
                ));
            }
        };
        let mut pattern = LinearPattern::new();
        self.encode_into(&symbol.payload_bytes(), meta, &mut pattern)?;
        Ok(Encoding::Linear(pattern))
    }
}

/// Code 11 decoder. The number of check characters is not encoded in-band, so the
/// decoder is told how many to strip and validate (default 1).
#[cfg(feature = "decode")]
#[derive(Debug, Clone, Copy)]
pub struct Code11Decoder {
    check_count: u8,
}

#[cfg(feature = "decode")]
impl Default for Code11Decoder {
    fn default() -> Self {
        Code11Decoder { check_count: 1 }
    }
}

#[cfg(feature = "decode")]
impl Code11Decoder {
    /// A new decoder expecting a single (`C`) check character.
    pub fn new() -> Self {
        Self::default()
    }

    /// Expect and validate `count` (0, 1, or 2) check characters.
    pub fn with_check_count(mut self, count: u8) -> Self {
        self.check_count = count;
        self
    }
}

#[cfg(feature = "decode")]
impl Decode for Code11Decoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        if self.check_count > 2 {
            return Err(Error::invalid_parameter(
                "Code 11 check_count must be 0, 1 or 2",
            ));
        }
        let pattern = match encoding {
            Encoding::Linear(p) => p,
            Encoding::Matrix(_) => {
                return Err(Error::invalid_parameter("Code 11 expects a linear pattern"));
            }
        };
        let runs = run_lengths(&pattern.modules);
        if runs.is_empty() {
            return Err(Error::undecodable("empty Code 11 pattern"));
        }
        // Each character is 5 runs; characters are joined by a single gap run.
        if !(runs.len() + 1).is_multiple_of(6) {
            return Err(Error::undecodable("malformed Code 11 structure"));
        }
        let nchars = (runs.len() + 1) / 6;
        if nchars < 2 {
            return Err(Error::undecodable("Code 11 missing start/stop"));
        }
        let narrow = runs.iter().map(|&(_, len)| len).min().unwrap();

        let mut values: Vec<u8> = Vec::new();
        for c in 0..nchars {
            let base = c * 6;
            let bits = rebuild_bits(&runs[base..base + 5], narrow);
            let is_start_stop = c == 0 || c == nchars - 1;
            if bits == START_STOP {
                if !is_start_stop {
                    return Err(Error::undecodable("unexpected Code 11 start/stop"));
                }
                continue;
            }
            if is_start_stop {
                return Err(Error::undecodable("Code 11 not framed by start/stop"));
            }
            match PATTERNS.iter().position(|&p| p == bits) {
                Some(v) => values.push(v as u8),
                None => return Err(Error::undecodable("unknown Code 11 character")),
            }
        }

        if values.len() < self.check_count as usize {
            return Err(Error::undecodable("Code 11 missing check characters"));
        }
        // Split checks off the right, validating C first then K.
        if self.check_count == 2 {
            let k = values.pop().unwrap();
            let c = values.pop().unwrap();
            if checksum(&values, 10) != c {
                return Err(Error::undecodable("Code 11 C check character mismatch"));
            }
            let mut with_c = values.clone();
            with_c.push(c);
            if checksum(&with_c, 9) != k {
                return Err(Error::undecodable("Code 11 K check character mismatch"));
            }
        } else if self.check_count == 1 {
            let c = values.pop().unwrap();
            if checksum(&values, 10) != c {
                return Err(Error::undecodable("Code 11 C check character mismatch"));
            }
        }

        let data: Vec<u8> = values.iter().map(|&v| value_char(v)).collect();
        let segment = if data.iter().all(|b| b.is_ascii_digit()) {
            Segment::numeric(data)
        } else {
            Segment::alphanumeric(data)
        };

        Ok(Symbol::new(
            Symbology::Code11,
            vec![segment],
            SymbolMeta::Code11(Code11Meta {
                check_count: self.check_count,
            }),
        ))
    }
}

/// Append a `1`/`0` pattern string to `out` as bars/spaces.
#[cfg(feature = "encode")]
fn push_pattern(out: &mut impl LinearSink, pattern: &str) -> Result<()> {
    pattern.bytes().try_for_each(|b| out.push(b == b'1'))
}

/// Run-length encode a module row into `(is_bar, length)` runs.
#[cfg(feature = "decode")]
fn run_lengths(modules: &[bool]) -> Vec<(bool, usize)> {
    let mut runs = Vec::new();
    let mut iter = modules.iter().copied();
    if let Some(first) = iter.next() {
        let mut cur = first;
        let mut len = 1usize;
        for m in iter {
            if m == cur {
                len += 1;
            } else {
                runs.push((cur, len));
                cur = m;
                len = 1;
            }
        }
        runs.push((cur, len));
    }
    runs
}

/// Rebuild the canonical `1`/`0` module string of a character from its runs,
/// normalising element widths against the narrowest run.
#[cfg(feature = "decode")]
fn rebuild_bits(runs: &[(bool, usize)], narrow: usize) -> String {
    let mut s = String::new();
    for &(bar, len) in runs {
        let width = (len + narrow / 2) / narrow;
        let ch = if bar { '1' } else { '0' };
        for _ in 0..width {
            s.push(ch);
        }
    }
    s
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    #[test]
    fn patterns_start_and_end_with_a_bar() {
        for (v, bits) in PATTERNS.iter().enumerate() {
            assert!(bits.starts_with('1') && bits.ends_with('1'), "value {v}");
        }
        assert!(START_STOP.starts_with('1') && START_STOP.ends_with('1'));
    }

    #[test]
    fn reference_checksums() {
        // Wikipedia: 012345 => C = 2.
        let v: Vec<u8> = b"012345".iter().map(|&b| char_value(b).unwrap()).collect();
        assert_eq!(checksum(&v, 10), 2);

        // Bar Code Island: 123-45 => C = 5, K = 2.
        let v: Vec<u8> = b"123-45".iter().map(|&b| char_value(b).unwrap()).collect();
        let c = checksum(&v, 10);
        assert_eq!(c, 5);
        let mut with_c = v.clone();
        with_c.push(c);
        assert_eq!(checksum(&with_c, 9), 2);
    }
}
