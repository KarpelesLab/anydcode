//! Code 39 (ISO/IEC 16388) encoder and structural decoder.
//!
//! Code 39 is a discrete, self-checking linear symbology using two element widths
//! (narrow and wide). Every character is framed by a `*` start/stop character and
//! separated from its neighbour by a single narrow inter-character gap. Each of the
//! 43 data characters is nine elements wide (five bars, four spaces) of which exactly
//! three are wide.
//!
//! This module supports:
//! - the standard 43-character set (`0-9 A-Z - . SPACE $ / + %`),
//! - the **full-ASCII extension**, in which every one of the 128 ASCII bytes is
//!   represented by one or two base characters using the `$ % / +` shift prefixes,
//! - an optional **mod-43 check character** appended before the stop character.
//!
//! [`Code39Meta`] records the two lossless-round-trip decisions (full-ASCII on/off
//! and check-digit present/absent). Because neither decision is encoded in-band, the
//! [`Code39Decoder`] is told which to expect; this mirrors how a physical scanner is
//! configured per application.
//!
//! ## Pattern table source
//!
//! The per-character module patterns (`wide = 2` narrow modules, 12 modules per
//! character) are taken verbatim from Bar Code Island's Code 39 specification
//! (<http://www.barcodeisland.com/code39.phtml>), cross-checked against the Code 39
//! article on Wikipedia. The full-ASCII table and the mod-43 algorithm come from the
//! same sources.

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

/// The 43 data characters in value order (`value == index`), each with its 12-module
/// bar pattern (`1` = bar, `0` = space; `wide = 2` narrow modules).
const TABLE: [(u8, &str); 43] = [
    (b'0', "101001101101"),
    (b'1', "110100101011"),
    (b'2', "101100101011"),
    (b'3', "110110010101"),
    (b'4', "101001101011"),
    (b'5', "110100110101"),
    (b'6', "101100110101"),
    (b'7', "101001011011"),
    (b'8', "110100101101"),
    (b'9', "101100101101"),
    (b'A', "110101001011"),
    (b'B', "101101001011"),
    (b'C', "110110100101"),
    (b'D', "101011001011"),
    (b'E', "110101100101"),
    (b'F', "101101100101"),
    (b'G', "101010011011"),
    (b'H', "110101001101"),
    (b'I', "101101001101"),
    (b'J', "101011001101"),
    (b'K', "110101010011"),
    (b'L', "101101010011"),
    (b'M', "110110101001"),
    (b'N', "101011010011"),
    (b'O', "110101101001"),
    (b'P', "101101101001"),
    (b'Q', "101010110011"),
    (b'R', "110101011001"),
    (b'S', "101101011001"),
    (b'T', "101011011001"),
    (b'U', "110010101011"),
    (b'V', "100110101011"),
    (b'W', "110011010101"),
    (b'X', "100101101011"),
    (b'Y', "110010110101"),
    (b'Z', "100110110101"),
    (b'-', "100101011011"),
    (b'.', "110010101101"),
    (b' ', "100110101101"),
    (b'$', "100100100101"),
    (b'/', "100100101001"),
    (b'+', "100101001001"),
    (b'%', "101001001001"),
];

/// The `*` start/stop character pattern (12 modules).
const START_STOP: &str = "100101101101";

/// The value (0..=42) of a base Code 39 character, or `None` if outside the set.
#[cfg(feature = "encode")]
fn char_value(b: u8) -> Option<u8> {
    TABLE.iter().position(|&(c, _)| c == b).map(|i| i as u8)
}

/// A shift pair or self-mapping character in the full-ASCII scheme.
#[cfg(feature = "encode")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fa {
    /// A byte that maps to a single base character (the byte itself).
    Single(u8),
    /// A byte that maps to a shift prefix (`$ % / +`) plus a letter.
    Pair(u8, u8),
}

/// Full-ASCII Code 39 encoding of a byte (the canonical scheme). Returns `None` for
/// bytes `>= 128`.
#[cfg(feature = "encode")]
fn fa_encode(b: u8) -> Option<Fa> {
    let pair = match b {
        0 => (b'%', b'U'),
        1..=26 => (b'$', b'A' + (b - 1)),
        27..=31 => (b'%', b'A' + (b - 27)),
        32 => return Some(Fa::Single(b' ')),
        b'!' => (b'/', b'A'),
        b'"' => (b'/', b'B'),
        b'#' => (b'/', b'C'),
        b'$' => (b'/', b'D'),
        b'%' => (b'/', b'E'),
        b'&' => (b'/', b'F'),
        b'\'' => (b'/', b'G'),
        b'(' => (b'/', b'H'),
        b')' => (b'/', b'I'),
        b'*' => (b'/', b'J'),
        b'+' => (b'/', b'K'),
        b',' => (b'/', b'L'),
        b'-' => return Some(Fa::Single(b'-')),
        b'.' => return Some(Fa::Single(b'.')),
        b'/' => (b'/', b'O'),
        b'0'..=b'9' => return Some(Fa::Single(b)),
        b':' => (b'/', b'Z'),
        b';' => (b'%', b'F'),
        b'<' => (b'%', b'G'),
        b'=' => (b'%', b'H'),
        b'>' => (b'%', b'I'),
        b'?' => (b'%', b'J'),
        b'@' => (b'%', b'V'),
        b'A'..=b'Z' => return Some(Fa::Single(b)),
        b'[' => (b'%', b'K'),
        b'\\' => (b'%', b'L'),
        b']' => (b'%', b'M'),
        b'^' => (b'%', b'N'),
        b'_' => (b'%', b'O'),
        b'`' => (b'%', b'W'),
        b'a'..=b'z' => (b'+', b'A' + (b - b'a')),
        b'{' => (b'%', b'P'),
        b'|' => (b'%', b'Q'),
        b'}' => (b'%', b'R'),
        b'~' => (b'%', b'S'),
        127 => (b'%', b'T'),
        _ => return None,
    };
    Some(Fa::Pair(pair.0, pair.1))
}

/// Reverse a full-ASCII shift pair (`prefix` in `$ % / +`, plus `letter`) back to its
/// ASCII byte, or `None` if the pair is not a valid combination.
#[cfg(feature = "decode")]
fn fa_decode_pair(prefix: u8, letter: u8) -> Option<u8> {
    match prefix {
        b'$' => letter.is_ascii_uppercase().then_some(1 + (letter - b'A')),
        b'+' => letter
            .is_ascii_uppercase()
            .then_some(b'a' + (letter - b'A')),
        b'%' => match letter {
            b'A'..=b'E' => Some(27 + (letter - b'A')),
            b'F'..=b'J' => Some(b';' + (letter - b'F')),
            b'K'..=b'O' => Some(b'[' + (letter - b'K')),
            b'P'..=b'S' => Some(b'{' + (letter - b'P')),
            b'T' => Some(127),
            b'U' => Some(0),
            b'V' => Some(b'@'),
            b'W' => Some(b'`'),
            _ => None,
        },
        b'/' => match letter {
            b'A'..=b'L' => Some(b'!' + (letter - b'A')),
            b'O' => Some(b'/'),
            b'Z' => Some(b':'),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a base character acts as a full-ASCII shift prefix.
#[cfg(feature = "decode")]
fn is_shift(b: u8) -> bool {
    matches!(b, b'$' | b'%' | b'/' | b'+')
}

/// Parameters required to re-encode a Code 39 symbol identically (lossless round-trip).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Code39Meta {
    /// Whether the payload was encoded with the full-ASCII extension.
    pub full_ascii: bool,
    /// Whether a mod-43 check character was appended before the stop character.
    pub check_digit: bool,
}

/// Code 39 encoder.
#[cfg(feature = "encode")]
#[derive(Debug, Default, Clone, Copy)]
pub struct Code39Encoder;

#[cfg(feature = "encode")]
impl Code39Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

    /// Number of modules [`Code39Encoder::encode_into`] emits for `data_len` payload
    /// bytes, excluding quiet zones: an upper bound when `full_ascii` is on (bytes
    /// that need a shift pair take two characters).
    pub const fn max_modules(data_len: usize, meta: &Code39Meta) -> usize {
        let chars = if meta.full_ascii {
            2 * data_len
        } else {
            data_len
        };
        (chars + meta.check_digit as usize + 2) * 13 - 1
    }

    /// Heap-free encoding: write the symbol for `data` under `meta` to `out`.
    ///
    /// Without full-ASCII every byte must be one of the 43 base characters; with it,
    /// every byte must be ASCII. The input is validated before the first module is
    /// written. Size a [`LinearBuf`](crate::output::LinearBuf) with
    /// [`Code39Encoder::max_modules`].
    pub fn encode_into<S: LinearSink>(
        &self,
        data: &[u8],
        meta: &Code39Meta,
        out: &mut S,
    ) -> Result<()> {
        let valid = if meta.full_ascii {
            data.iter().all(|&b| b < 128)
        } else {
            data.iter().all(|&b| char_value(b).is_some())
        };
        if !valid {
            return Err(Error::invalid_data(if meta.full_ascii {
                "byte is not representable in Code 39"
            } else {
                "byte is not a Code 39 character"
            }));
        }

        // Emit: * gap v gap v ... gap *
        out.begin(QUIET_ZONE)?;
        push_pattern(out, START_STOP)?;
        let mut sum = 0u32;
        let mut emit = |out: &mut S, c: u8| -> Result<()> {
            let v = char_value(c).expect("validated Code 39 character");
            sum += u32::from(v);
            out.push(false)?;
            push_pattern(out, TABLE[v as usize].1)
        };
        for &b in data {
            if meta.full_ascii {
                match fa_encode(b).expect("validated ASCII byte") {
                    Fa::Single(c) => emit(out, c)?,
                    Fa::Pair(p, l) => {
                        emit(out, p)?;
                        emit(out, l)?;
                    }
                }
            } else {
                emit(out, b)?;
            }
        }
        if meta.check_digit {
            out.push(false)?;
            push_pattern(out, TABLE[(sum % 43) as usize].1)?;
        }
        out.push(false)?;
        push_pattern(out, START_STOP)
    }
}

#[cfg(all(feature = "alloc", feature = "encode"))]
impl Code39Encoder {
    /// Build a reproducible [`Symbol`] from raw `data`.
    ///
    /// When `full_ascii` is `false`, every byte must be one of the 43 base characters;
    /// the payload is stored as a single [`Segment::alphanumeric`]. When `full_ascii`
    /// is `true`, every byte must be ASCII (`< 128`) and the payload is stored as a
    /// single [`Segment::byte`]. `check_digit` requests a mod-43 check character.
    pub fn build(&self, data: &[u8], full_ascii: bool, check_digit: bool) -> Result<Symbol> {
        let segment = if full_ascii {
            if data.iter().any(|&b| b >= 128) {
                return Err(Error::invalid_data(
                    "Code 39 full-ASCII payload must be ASCII (bytes < 128)",
                ));
            }
            Segment::byte(data.to_vec())
        } else {
            if let Some(&bad) = data.iter().find(|&&b| char_value(b).is_none()) {
                return Err(Error::invalid_data(format!(
                    "byte {bad:#04x} is not a Code 39 character (enable full-ASCII?)"
                )));
            }
            Segment::alphanumeric(data.to_vec())
        };
        let meta = Code39Meta {
            full_ascii,
            check_digit,
        };
        Ok(Symbol::new(
            Symbology::Code39,
            vec![segment],
            SymbolMeta::Code39(meta),
        ))
    }
}

#[cfg(all(feature = "alloc", feature = "encode"))]
impl Encode for Code39Encoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::Code39 {
            return Err(Error::invalid_parameter(
                "Code39Encoder given a non-Code39 symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Code39(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Code 39 symbol missing Code39Meta",
                ));
            }
        };
        let mut pattern = LinearPattern::new();
        self.encode_into(&symbol.payload_bytes(), meta, &mut pattern)?;
        Ok(Encoding::Linear(pattern))
    }
}

/// Code 39 decoder.
///
/// Because neither the full-ASCII decision nor check-digit presence is encoded
/// in-band, the decoder is configured with what to expect (defaults: both off).
#[cfg(feature = "decode")]
#[derive(Debug, Default, Clone, Copy)]
pub struct Code39Decoder {
    full_ascii: bool,
    check_digit: bool,
}

#[cfg(feature = "decode")]
impl Code39Decoder {
    /// A new decoder (no full-ASCII interpretation, no check character).
    pub fn new() -> Self {
        Self::default()
    }

    /// Interpret the payload using the full-ASCII extension.
    pub fn with_full_ascii(mut self, yes: bool) -> Self {
        self.full_ascii = yes;
        self
    }

    /// Expect and validate a trailing mod-43 check character.
    pub fn with_check_digit(mut self, yes: bool) -> Self {
        self.check_digit = yes;
        self
    }
}

#[cfg(feature = "decode")]
impl Decode for Code39Decoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        let pattern = match encoding {
            Encoding::Linear(p) => p,
            Encoding::Matrix(_) => {
                return Err(Error::invalid_parameter("Code 39 expects a linear pattern"));
            }
        };
        let runs = run_lengths(&pattern.modules);
        if runs.is_empty() {
            return Err(Error::undecodable("empty Code 39 pattern"));
        }
        // Each character is 9 runs; characters are joined by a single gap run.
        if !(runs.len() + 1).is_multiple_of(10) {
            return Err(Error::undecodable("malformed Code 39 structure"));
        }
        let nchars = (runs.len() + 1) / 10;
        if nchars < 2 {
            return Err(Error::undecodable("Code 39 missing start/stop"));
        }
        let narrow = runs.iter().map(|&(_, len)| len).min().unwrap();

        let mut values: Vec<u8> = Vec::new();
        for c in 0..nchars {
            let base = c * 10;
            let bits = rebuild_bits(&runs[base..base + 9], narrow);
            let is_start_stop = c == 0 || c == nchars - 1;
            if bits == START_STOP {
                if !is_start_stop {
                    return Err(Error::undecodable("unexpected Code 39 start/stop"));
                }
                continue;
            }
            if is_start_stop {
                return Err(Error::undecodable("Code 39 not framed by start/stop"));
            }
            match TABLE.iter().position(|&(_, p)| p == bits) {
                Some(v) => values.push(v as u8),
                None => return Err(Error::undecodable("unknown Code 39 character")),
            }
        }

        if self.check_digit {
            let check = values
                .pop()
                .ok_or_else(|| Error::undecodable("Code 39 missing check character"))?;
            let sum: u32 = values.iter().map(|&v| v as u32).sum();
            if (sum % 43) as u8 != check {
                return Err(Error::undecodable("Code 39 check character mismatch"));
            }
        }

        let base_chars: Vec<u8> = values.iter().map(|&v| TABLE[v as usize].0).collect();
        let segment = if self.full_ascii {
            Segment::byte(collapse_full_ascii(&base_chars)?)
        } else {
            Segment::alphanumeric(base_chars)
        };

        let meta = Code39Meta {
            full_ascii: self.full_ascii,
            check_digit: self.check_digit,
        };
        Ok(Symbol::new(
            Symbology::Code39,
            vec![segment],
            SymbolMeta::Code39(meta),
        ))
    }
}

/// Collapse a sequence of base characters into ASCII bytes using the full-ASCII rules.
#[cfg(feature = "decode")]
fn collapse_full_ascii(base: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < base.len() {
        let c = base[i];
        if is_shift(c) {
            let letter = *base
                .get(i + 1)
                .ok_or_else(|| Error::undecodable("dangling Code 39 shift character"))?;
            let byte = fa_decode_pair(c, letter)
                .ok_or_else(|| Error::undecodable("invalid Code 39 full-ASCII pair"))?;
            out.push(byte);
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    Ok(out)
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
/// classifying each element against the narrowest run. Symbols are printed with a
/// wide:narrow ratio from 2:1 to 3:1, so an element of two or three narrow widths is
/// wide (two modules in the canonical tables); anything wider is not an element of
/// this symbology and yields a string that matches no character.
#[cfg(feature = "decode")]
fn rebuild_bits(runs: &[(bool, usize)], narrow: usize) -> String {
    let mut s = String::new();
    for &(bar, len) in runs {
        let width = match (len + narrow / 2) / narrow {
            1 => 1,
            2 | 3 => 2,
            _ => return String::new(),
        };
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
    fn patterns_are_well_formed() {
        for (ch, bits) in TABLE {
            assert_eq!(bits.len(), 12, "char {} pattern length", ch as char);
            assert!(bits.starts_with('1') && bits.ends_with('1'));
        }
        assert_eq!(START_STOP.len(), 12);
    }

    #[test]
    fn full_ascii_is_a_bijection() {
        for b in 0u8..128 {
            let base: Vec<u8> = match fa_encode(b).unwrap() {
                Fa::Single(c) => vec![c],
                Fa::Pair(p, l) => vec![p, l],
            };
            let back = collapse_full_ascii(&base).unwrap();
            assert_eq!(back, vec![b], "byte {b:#04x} did not round-trip");
        }
    }
}
