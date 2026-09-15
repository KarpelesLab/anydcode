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
#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "decode")]
use alloc::vec::Vec;

/// Module width of a narrow element.
#[cfg(feature = "encode")]
const NARROW: u32 = 1;
/// Module width of a wide element (zint uses a 3:1 ratio).
#[cfg(feature = "encode")]
const WIDE: u32 = 3;
/// Quiet-zone width in narrow modules on each side.
#[cfg(feature = "encode")]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
#[cfg(feature = "encode")]
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
#[cfg(feature = "encode")]
#[derive(Debug, Default, Clone, Copy)]
pub struct ItfEncoder;

#[cfg(feature = "encode")]
impl ItfEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

    /// Number of modules [`ItfEncoder::encode_into`] emits for `data_len` payload
    /// digits, excluding quiet zones (exact).
    pub const fn max_modules(data_len: usize, meta: &ItfMeta) -> usize {
        // start (4) + 9 modules per digit + stop (5).
        4 + 9 * (data_len + meta.check as usize) + 5
    }

    /// Heap-free encoding: write the symbol for ASCII `digits` under `meta` to `out`.
    ///
    /// `digits` must be non-empty ASCII digits whose count (plus the check digit, if
    /// `meta.check`) is even. The input is validated before the first module is
    /// written. Size a [`LinearBuf`](crate::output::LinearBuf) with
    /// [`ItfEncoder::max_modules`].
    pub fn encode_into<S: LinearSink>(
        &self,
        digits: &[u8],
        meta: &ItfMeta,
        out: &mut S,
    ) -> Result<()> {
        ensure_digits(digits)?;
        if !(digits.len() + usize::from(meta.check)).is_multiple_of(2) {
            return Err(Error::invalid_data("ITF requires an even digit count"));
        }
        let check = meta.check.then(|| b'0' + mod10(digits));
        let mut all = digits.iter().copied().chain(check);

        out.begin(QUIET_ZONE)?;
        // Start pattern: narrow bar, narrow space, narrow bar, narrow space.
        for _ in 0..2 {
            push_run(out, true, NARROW)?;
            push_run(out, false, NARROW)?;
        }
        // Interleaved digit pairs: bars from the first digit, spaces from the second.
        while let (Some(a), Some(b)) = (all.next(), all.next()) {
            let a = (a - b'0') as usize;
            let b = (b - b'0') as usize;
            for (&bar, &space) in DIGIT_WIDTHS[a].iter().zip(&DIGIT_WIDTHS[b]) {
                push_run(out, true, bar)?;
                push_run(out, false, space)?;
            }
        }
        // Stop pattern: wide bar, narrow space, narrow bar.
        push_run(out, true, WIDE)?;
        push_run(out, false, NARROW)?;
        push_run(out, true, NARROW)
    }
}

#[cfg(all(feature = "alloc", feature = "encode"))]
impl ItfEncoder {
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

/// Append `width` copies of `bar` to `out`.
#[cfg(feature = "encode")]
fn push_run(out: &mut impl LinearSink, bar: bool, width: u32) -> Result<()> {
    out.push_run(bar, width as usize)
}

#[cfg(all(feature = "alloc", feature = "encode"))]
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
        let mut pattern = LinearPattern::new();
        self.encode_into(&symbol.payload_bytes(), meta, &mut pattern)?;
        Ok(Encoding::Linear(pattern))
    }
}

/// Interleaved 2 of 5 decoder.
#[cfg(feature = "decode")]
#[derive(Debug, Default, Clone, Copy)]
pub struct ItfDecoder {
    /// Whether a trailing mod-10 check digit is expected (and verified/stripped).
    check: bool,
}

#[cfg(feature = "decode")]
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
#[cfg(feature = "decode")]
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
#[cfg(feature = "decode")]
fn digit_from_widths(w: &[u32]) -> Result<u8> {
    for (d, pat) in DIGIT_WIDTHS.iter().enumerate() {
        if w.iter().zip(pat).all(|(&a, &b)| (a > 1) == (b > 1)) {
            return Ok(d as u8);
        }
    }
    Err(Error::undecodable("invalid ITF digit pattern"))
}

#[cfg(feature = "decode")]
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

#[cfg(all(test, feature = "encode", feature = "decode"))]
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
