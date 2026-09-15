//! Codabar (a.k.a. NW-7, USD-4, Code 2 of 7).
//!
//! A discrete symbology over the character set `0-9 - $ : / . +`, framed by a
//! start and a stop character each chosen from `A B C D`. Every character is seven
//! elements (four bars, three spaces); adjacent characters are separated by a single
//! narrow inter-character gap.
//!
//! # Representation
//!
//! [`LinearPattern::modules`] uses one boolean per narrow module (`true` = bar); a
//! wide element spans `WIDE` modules. The start/stop pair is carried in the bars
//! and recovered on decode, and is also recorded in [`CodabarMeta`].
//!
//! # Validation
//!
//! The seven-element patterns match ZXing's `CodaBarReader.CHARACTER_ENCODINGS`
//! table (each is a 7-bit value, MSB first, `1` = wide).

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
const NARROW: usize = 1;
/// Module width of a wide element.
#[cfg(feature = "encode")]
const WIDE: usize = 3;
/// Modules in the widest character: every Codabar character has at most three wide
/// elements among its seven.
#[cfg(feature = "encode")]
const MAX_CHAR_MODULES: usize = 4 * NARROW + 3 * WIDE;
/// Quiet-zone width in narrow modules on each side.
#[cfg(feature = "encode")]
const QUIET_ZONE: usize = 10;

/// Data + start/stop alphabet, index-aligned with [`ENCODINGS`].
const ALPHABET: &[u8] = b"0123456789-$:/.+ABCD";

/// Seven-element width patterns as 7-bit values (bit 6 = first element, `1` = wide).
///
/// Source: ZXing `CodaBarReader.CHARACTER_ENCODINGS`.
const ENCODINGS: [u8; 20] = [
    0x03, 0x06, 0x09, 0x60, 0x12, 0x42, 0x21, 0x24, 0x30, 0x48, // 0-9
    0x0C, 0x18, 0x45, 0x51, 0x54, 0x15, // - $ : / . +
    0x1A, 0x29, 0x0B, 0x0E, // A B C D
];

/// Parameters required to re-encode a Codabar symbol identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodabarMeta {
    /// Start character, one of `A B C D`.
    pub start: u8,
    /// Stop character, one of `A B C D`.
    pub stop: u8,
}

impl Default for CodabarMeta {
    fn default() -> Self {
        CodabarMeta {
            start: b'A',
            stop: b'B',
        }
    }
}

/// The index of `c` in [`ALPHABET`], if present.
#[cfg(feature = "encode")]
fn index_of(c: u8) -> Option<usize> {
    ALPHABET.iter().position(|&a| a == c)
}

/// Whether `c` is a valid start/stop character.
fn is_guard(c: u8) -> bool {
    matches!(c, b'A' | b'B' | b'C' | b'D')
}

/// Whether `c` is a valid data character.
fn is_data(c: u8) -> bool {
    matches!(c, b'0'..=b'9' | b'-' | b'$' | b':' | b'/' | b'.' | b'+')
}

/// Codabar encoder.
#[cfg(feature = "encode")]
#[derive(Debug, Default, Clone, Copy)]
pub struct CodabarEncoder;

#[cfg(feature = "encode")]
impl CodabarEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

    /// Upper bound on the modules [`CodabarEncoder::encode_into`] emits for
    /// `data_len` payload characters, excluding quiet zones (the exact width depends
    /// on how many wide elements each character has).
    pub const fn max_modules(data_len: usize) -> usize {
        (data_len + 2) * (MAX_CHAR_MODULES + 1) - 1
    }

    /// Heap-free encoding: write `meta.start`, `data`, `meta.stop` to `out`.
    ///
    /// `meta.start`/`meta.stop` must be one of `A B C D` and every data byte one of
    /// `0-9 - $ : / . +`. The input is validated before the first module is written.
    /// Size a [`LinearBuf`](crate::output::LinearBuf) with
    /// [`CodabarEncoder::max_modules`].
    pub fn encode_into<S: LinearSink>(
        &self,
        data: &[u8],
        meta: &CodabarMeta,
        out: &mut S,
    ) -> Result<()> {
        if !is_guard(meta.start) || !is_guard(meta.stop) {
            return Err(Error::invalid_data(
                "Codabar start/stop must be A, B, C or D",
            ));
        }
        if !data.iter().all(|&c| is_data(c)) {
            return Err(Error::invalid_data("invalid Codabar data character"));
        }

        out.begin(QUIET_ZONE)?;
        push_char(out, meta.start)?;
        for &c in data {
            out.push(false)?; // narrow inter-character gap
            push_char(out, c)?;
        }
        out.push(false)?;
        push_char(out, meta.stop)
    }
}

#[cfg(all(feature = "alloc", feature = "encode"))]
impl CodabarEncoder {
    /// Build a symbol from `data` framed by `start`/`stop` (each one of `A B C D`).
    pub fn build(&self, start: u8, data: &[u8], stop: u8) -> Result<Symbol> {
        if !is_guard(start) || !is_guard(stop) {
            return Err(Error::invalid_data(
                "Codabar start/stop must be A, B, C or D",
            ));
        }
        if !data.iter().all(|&c| is_data(c)) {
            return Err(Error::invalid_data("invalid Codabar data character"));
        }
        Ok(Symbol::new(
            Symbology::Codabar,
            vec![Segment::byte(data.to_vec())],
            SymbolMeta::Codabar(CodabarMeta { start, stop }),
        ))
    }
}

/// Append the seven elements of the (validated) character `c` to `out`.
#[cfg(feature = "encode")]
fn push_char(out: &mut impl LinearSink, c: u8) -> Result<()> {
    let value = ENCODINGS[index_of(c).expect("validated Codabar character")];
    for i in 0..7 {
        let wide = (value >> (6 - i)) & 1 == 1;
        let width = if wide { WIDE } else { NARROW };
        out.push_run(i % 2 == 0, width)?;
    }
    Ok(())
}

#[cfg(all(feature = "alloc", feature = "encode"))]
impl Encode for CodabarEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::Codabar {
            return Err(Error::invalid_parameter(
                "CodabarEncoder given a non-Codabar symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Codabar(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Codabar symbol missing CodabarMeta",
                ));
            }
        };
        let mut pattern = LinearPattern::new();
        self.encode_into(&symbol.payload_bytes(), meta, &mut pattern)?;
        Ok(Encoding::Linear(pattern))
    }
}

/// Codabar decoder.
#[cfg(feature = "decode")]
#[derive(Debug, Default, Clone, Copy)]
pub struct CodabarDecoder;

#[cfg(feature = "decode")]
impl CodabarDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
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

/// Recover a character from its seven element widths (wide = width > 1).
#[cfg(feature = "decode")]
fn char_from_widths(w: &[u32]) -> Result<u8> {
    let mut value = 0u8;
    for (i, &width) in w.iter().enumerate() {
        if width > 1 {
            value |= 1 << (6 - i);
        }
    }
    ENCODINGS
        .iter()
        .position(|&e| e == value)
        .map(|idx| ALPHABET[idx])
        .ok_or_else(|| Error::undecodable("invalid Codabar character pattern"))
}

#[cfg(feature = "decode")]
impl Decode for CodabarDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        let pattern = match encoding {
            Encoding::Linear(p) => p,
            Encoding::Matrix(_) => {
                return Err(Error::invalid_parameter("Codabar expects a linear pattern"));
            }
        };
        let runs = rle(&pattern.modules)?;
        // Each character is 7 elements; characters are joined by 1 gap element.
        // Total elements = 7k + (k - 1) = 8k - 1.
        if (runs.len() + 1) % 8 != 0 {
            return Err(Error::undecodable("malformed Codabar structure"));
        }
        let k = (runs.len() + 1) / 8;
        if k < 2 {
            return Err(Error::undecodable("Codabar needs start and stop"));
        }

        let mut chars = Vec::with_capacity(k);
        for c in 0..k {
            let base = c * 8;
            chars.push(char_from_widths(&runs[base..base + 7])?);
            // The gap element after every non-final character must be a narrow space.
            if c + 1 < k && runs[base + 7] > 1 {
                return Err(Error::undecodable("Codabar inter-character gap not narrow"));
            }
        }

        let start = chars[0];
        let stop = chars[k - 1];
        if !is_guard(start) || !is_guard(stop) {
            return Err(Error::undecodable("Codabar start/stop not A-D"));
        }
        let data = &chars[1..k - 1];
        if !data.iter().all(|&c| is_data(c)) {
            return Err(Error::undecodable("Codabar data character out of range"));
        }

        Ok(Symbol::new(
            Symbology::Codabar,
            vec![Segment::byte(data.to_vec())],
            SymbolMeta::Codabar(CodabarMeta { start, stop }),
        ))
    }
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_various() {
        let enc = CodabarEncoder::new();
        let cases: &[(u8, &[u8], u8)] = &[
            (b'A', b"1234567890", b'B'),
            (b'C', b"123-456", b'D'),
            (b'A', b"$12.34", b'A'),
            (b'B', b"01:23/45+6", b'C'),
        ];
        for &(start, data, stop) in cases {
            let sym = enc.build(start, data, stop).unwrap();
            let encoding = enc.encode(&sym).unwrap();
            let back = CodabarDecoder::new().decode(&encoding).unwrap();
            assert_eq!(back.segments, sym.segments);
            assert_eq!(back.meta, sym.meta);
            assert_eq!(enc.encode(&back).unwrap(), encoding);
        }
    }

    #[test]
    fn rejects_bad_input() {
        let enc = CodabarEncoder::new();
        assert!(enc.build(b'E', b"12", b'A').is_err()); // bad start
        assert!(enc.build(b'A', b"12X", b'A').is_err()); // bad data
    }
}
