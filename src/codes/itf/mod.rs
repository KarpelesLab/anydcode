//! Interleaved 2 of 5 (ITF, ISO/IEC 16390).
//!
//! ITF is a continuous, self-checking numeric symbology that packs two digits
//! into one character: the five bars carry the first digit of a pair and the five
//! interleaved spaces carry the second. Each digit is one of the standard
//! "two-of-five" patterns (exactly two of the five elements are wide), weighted
//! 1-2-4-7-parity.
//!
//! # Representation
//!
//! [`LinearPattern::modules`] uses one boolean per narrow module (`true` = bar).
//! A narrow element is one module; a wide element is `WIDE` modules.
//!
//! # Options ([`ItfMeta`])
//!
//! - `check`: append a mod-10 (GTIN-style, 3-1 weighted) check digit.
//!
//! ITF requires an **even** number of encoded digits. This crate does not silently
//! pad: the caller must supply digits such that `len + check_digit` is even (prepend
//! a leading zero yourself if needed). Because a leading pad zero is indistinguishable
//! from a genuine leading zero, folding it into the payload is the only lossless
//! convention.
//!
//! Because the presence of a check digit is not encoded in the bars, the decoder is
//! told whether to expect one via [`ItfDecoder::with_check`]; the default decoder
//! treats every decoded digit as payload.
//!
//! # Validation
//!
//! Digit patterns and start/stop match the reference implementation
//! [zint](https://github.com/zint/zint) (`C25InterTable`, `stop_start`). The mod-10
//! check is hand-verified against GTIN-13 `1234567890123` → check digit `1`.

use crate::error::{Error, Result};
use crate::output::{Encoding, LinearPattern};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::{Decode, Encode};

/// Module width of a narrow element.
const NARROW: u32 = 1;
/// Module width of a wide element (zint uses a 3:1 ratio).
const WIDE: u32 = 3;
/// Quiet-zone width in narrow modules on each side.
const QUIET_ZONE: usize = 10;

/// Two-of-five element widths per digit (5 elements, wide = `WIDE`, narrow = `NARROW`).
///
/// Matches zint's `C25InterTable`; equivalently the ISO/IEC 16390 patterns weighted
/// 1-2-4-7 with a parity element.
const DIGIT_WIDTHS: [[u32; 5]; 10] = [
    [1, 1, 3, 3, 1],
    [3, 1, 1, 1, 3],
    [1, 3, 1, 1, 3],
    [3, 3, 1, 1, 1],
    [1, 1, 3, 1, 3],
    [3, 1, 3, 1, 1],
    [1, 3, 3, 1, 1],
    [1, 1, 1, 3, 3],
    [3, 1, 1, 3, 1],
    [1, 3, 1, 3, 1],
];

/// Parameters required to re-encode an Interleaved 2 of 5 symbol identically.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItfMeta {
    /// Whether a trailing mod-10 check digit is present.
    pub check: bool,
}

/// The mod-10 (3-1 weighted, GTIN-style) check digit for ASCII `digits`.
fn mod10(digits: &[u8]) -> u8 {
    let mut sum = 0u32;
    for (i, &d) in digits.iter().rev().enumerate() {
        let v = (d - b'0') as u32;
        sum += if i % 2 == 0 { v * 3 } else { v };
    }
    ((10 - sum % 10) % 10) as u8
}

/// Validate that every byte is an ASCII digit.
fn ensure_digits(digits: &[u8]) -> Result<()> {
    if digits.is_empty() {
        return Err(Error::invalid_data("ITF payload is empty"));
    }
    if digits.iter().all(u8::is_ascii_digit) {
        Ok(())
    } else {
        Err(Error::invalid_data("ITF payload must be ASCII digits"))
    }
}

/// Interleaved 2 of 5 encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct ItfEncoder;

impl ItfEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

    /// Build a symbol from `digits`, optionally appending a mod-10 check digit.
    ///
    /// Returns [`Error::InvalidData`] if the resulting digit count (including the
    /// check digit) is odd, since ITF encodes digits in pairs.
    pub fn build(&self, digits: &[u8], check: bool) -> Result<Symbol> {
        ensure_digits(digits)?;
        let total = digits.len() + usize::from(check);
        if !total.is_multiple_of(2) {
            return Err(Error::invalid_data(
                "ITF requires an even digit count; prepend a leading zero",
            ));
        }
        Ok(Symbol::new(
            Symbology::Itf,
            vec![Segment::numeric(digits.to_vec())],
            SymbolMeta::Itf(ItfMeta { check }),
        ))
    }
}

/// Append `width` copies of `bar` to `modules`.
fn push_run(modules: &mut Vec<bool>, bar: bool, width: u32) {
    modules.extend(std::iter::repeat_n(bar, width as usize));
}

impl Encode for ItfEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::Itf {
            return Err(Error::invalid_parameter(
                "ItfEncoder given a non-ITF symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Itf(m) => m,
            _ => return Err(Error::invalid_parameter("ITF symbol missing ItfMeta")),
        };
        let mut digits = symbol.payload_bytes();
        ensure_digits(&digits)?;
        if meta.check {
            digits.push(b'0' + mod10(&digits));
        }
        if !digits.len().is_multiple_of(2) {
            return Err(Error::invalid_data("ITF requires an even digit count"));
        }

        let mut modules = Vec::new();
        // Start pattern: narrow bar, narrow space, narrow bar, narrow space.
        for _ in 0..2 {
            push_run(&mut modules, true, NARROW);
            push_run(&mut modules, false, NARROW);
        }
        // Interleaved digit pairs: bars from the first digit, spaces from the second.
        for pair in digits.chunks_exact(2) {
            let a = (pair[0] - b'0') as usize;
            let b = (pair[1] - b'0') as usize;
            for (&bar, &space) in DIGIT_WIDTHS[a].iter().zip(&DIGIT_WIDTHS[b]) {
                push_run(&mut modules, true, bar);
                push_run(&mut modules, false, space);
            }
        }
        // Stop pattern: wide bar, narrow space, narrow bar.
        push_run(&mut modules, true, WIDE);
        push_run(&mut modules, false, NARROW);
        push_run(&mut modules, true, NARROW);

        Ok(Encoding::Linear(LinearPattern {
            modules,
            quiet_zone: QUIET_ZONE,
        }))
    }
}

/// Interleaved 2 of 5 decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct ItfDecoder {
    /// Whether a trailing mod-10 check digit is expected (and verified/stripped).
    check: bool,
}

impl ItfDecoder {
    /// A new decoder that treats all decoded digits as payload.
    pub fn new() -> Self {
        Self { check: false }
    }

    /// A decoder that expects, verifies and strips a trailing mod-10 check digit.
    pub fn with_check(check: bool) -> Self {
        Self { check }
    }
}

/// Run-length encode `modules` into element widths, starting with a bar.
fn rle(modules: &[bool]) -> Result<Vec<u32>> {
    if modules.is_empty() || !modules[0] {
        return Err(Error::undecodable("linear pattern must start with a bar"));
    }
    let mut runs = Vec::new();
    let mut cur = modules[0];
    let mut len = 0u32;
    for &m in modules {
        if m == cur {
            len += 1;
        } else {
            runs.push(len);
            cur = m;
            len = 1;
        }
    }
    runs.push(len);
    Ok(runs)
}

/// Recover a digit from a 5-element width pattern (wide = width > 1).
fn digit_from_widths(w: &[u32]) -> Result<u8> {
    for (d, pat) in DIGIT_WIDTHS.iter().enumerate() {
        if w.iter().zip(pat).all(|(&a, &b)| (a > 1) == (b > 1)) {
            return Ok(d as u8);
        }
    }
    Err(Error::undecodable("invalid ITF digit pattern"))
}

impl Decode for ItfDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        let pattern = match encoding {
            Encoding::Linear(p) => p,
            Encoding::Matrix(_) => {
                return Err(Error::invalid_parameter("ITF expects a linear pattern"));
            }
        };
        let runs = rle(&pattern.modules)?;
        // start(4) + stop(3) + N*10, N >= 1.
        if runs.len() < 17 || (runs.len() - 7) % 10 != 0 {
            return Err(Error::undecodable("malformed ITF structure"));
        }
        // Start: four narrow elements.
        if runs[..4].iter().any(|&w| w > 1) {
            return Err(Error::undecodable("bad ITF start pattern"));
        }
        // Stop: wide bar, narrow space, narrow bar.
        let stop = &runs[runs.len() - 3..];
        if stop[0] <= 1 || stop[1] > 1 || stop[2] > 1 {
            return Err(Error::undecodable("bad ITF stop pattern"));
        }

        let mut digits = Vec::new();
        let pairs = (runs.len() - 7) / 10;
        for g in 0..pairs {
            let group = &runs[4 + g * 10..4 + g * 10 + 10];
            let bars = [group[0], group[2], group[4], group[6], group[8]];
            let spaces = [group[1], group[3], group[5], group[7], group[9]];
            digits.push(b'0' + digit_from_widths(&bars)?);
            digits.push(b'0' + digit_from_widths(&spaces)?);
        }

        if self.check {
            let (data, chk) = digits.split_at(digits.len() - 1);
            if chk[0] != b'0' + mod10(data) {
                return Err(Error::undecodable("ITF check digit mismatch"));
            }
            digits.truncate(digits.len() - 1);
        }

        Ok(Symbol::new(
            Symbology::Itf,
            vec![Segment::numeric(digits)],
            SymbolMeta::Itf(ItfMeta { check: self.check }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mod10_matches_gtin_reference() {
        // GTIN-13 1234567890123 has published check digit 1.
        assert_eq!(mod10(b"1234567890123"), 1);
    }

    #[test]
    fn start_and_stop_shape() {
        let enc = ItfEncoder::new();
        let sym = enc.build(b"1234", false).unwrap();
        let Encoding::Linear(p) = enc.encode(&sym).unwrap() else {
            panic!("expected linear");
        };
        let runs = rle(&p.modules).unwrap();
        // start = 4 narrow, stop = wide/narrow/narrow.
        assert_eq!(&runs[..4], &[1, 1, 1, 1]);
        let n = runs.len();
        assert_eq!(&runs[n - 3..], &[WIDE, 1, 1]);
    }

    #[test]
    fn roundtrip_with_and_without_check() {
        let enc = ItfEncoder::new();
        for (digits, check) in [
            (&b"1234"[..], false),
            (&b"123456"[..], false),
            (&b"12345"[..], true), // 5 data + check = 6 (even)
        ] {
            let sym = enc.build(digits, check).unwrap();
            let encoding = enc.encode(&sym).unwrap();
            let dec = ItfDecoder::with_check(check);
            let back = dec.decode(&encoding).unwrap();
            assert_eq!(back.segments, sym.segments);
            assert_eq!(back.meta, sym.meta);
            assert_eq!(enc.encode(&back).unwrap(), encoding);
        }
    }

    #[test]
    fn odd_count_rejected() {
        assert!(ItfEncoder::new().build(b"123", false).is_err());
    }
}
