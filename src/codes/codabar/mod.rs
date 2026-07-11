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

use crate::error::{Error, Result};
use crate::output::{Encoding, LinearPattern};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::{Decode, Encode};

/// Module width of a narrow element.
const NARROW: u32 = 1;
/// Module width of a wide element.
const WIDE: u32 = 3;
/// Quiet-zone width in narrow modules on each side.
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// The index of `c` in [`ALPHABET`], or an error.
fn index_of(c: u8) -> Result<usize> {
    ALPHABET.iter().position(|&a| a == c).ok_or_else(|| {
        Error::invalid_data(format!("character {:?} is not valid in Codabar", c as char))
    })
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
#[derive(Debug, Default, Clone, Copy)]
pub struct CodabarEncoder;

impl CodabarEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

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

/// Append the seven elements of `c` to `modules`.
fn push_char(modules: &mut Vec<bool>, c: u8) -> Result<()> {
    let value = ENCODINGS[index_of(c)?];
    for i in 0..7 {
        let wide = (value >> (6 - i)) & 1 == 1;
        let width = if wide { WIDE } else { NARROW };
        let bar = i % 2 == 0;
        modules.extend(std::iter::repeat_n(bar, width as usize));
    }
    Ok(())
}

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
        let data = symbol.payload_bytes();

        let mut chars = Vec::with_capacity(data.len() + 2);
        chars.push(meta.start);
        chars.extend_from_slice(&data);
        chars.push(meta.stop);

        let mut modules = Vec::new();
        for (i, &c) in chars.iter().enumerate() {
            if i > 0 {
                modules.push(false); // narrow inter-character gap
            }
            push_char(&mut modules, c)?;
        }

        Ok(Encoding::Linear(LinearPattern {
            modules,
            quiet_zone: QUIET_ZONE,
        }))
    }
}

/// Codabar decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodabarDecoder;

impl CodabarDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
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

/// Recover a character from its seven element widths (wide = width > 1).
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

#[cfg(test)]
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
