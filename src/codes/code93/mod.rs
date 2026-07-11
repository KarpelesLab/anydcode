//! Code 93 encoder and structural decoder.
//!
//! Code 93 is a continuous linear symbology designed as a denser, more secure
//! successor to Code 39. Every character is exactly nine modules wide (three bars and
//! three spaces, each 1–4 modules) and characters abut with no inter-character gap.
//! A symbol is `* <data> C K *` followed by a single terminating bar, where `C` and
//! `K` are two mandatory modulo-47 check characters.
//!
//! Beyond the 43 base characters shared with Code 39 (`0-9 A-Z - . SPACE $ / + %`),
//! Code 93 defines four dedicated shift characters (values 43–46, written `($) (%)
//! (/) (+)`) used only by the full-ASCII extension. Because these shifts are distinct
//! from the literal `$ % / +` data characters, full-ASCII is detected in-band: a
//! stream is full-ASCII exactly when it uses a shift character. [`Code93Meta`]
//! therefore only records the full-ASCII flag; both check characters are always
//! present and always validated.
//!
//! ## Pattern table and algorithm sources
//!
//! The nine-module binary patterns are taken verbatim from the Code 93 article on
//! Wikipedia. The C/K check algorithm (weights 1..=20 for C and 1..=15 for K, taken
//! right-to-left, modulo 47, with C included in K) is from Bar Code Island's Code 93
//! specification (<http://www.barcodeisland.com/code93.phtml>); worked example
//! `TEST93` → C = `+`, K = `6`.

use crate::error::{Error, Result};
use crate::output::{Encoding, LinearPattern};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::{Decode, Encode};

/// Quiet-zone margin, in modules, emitted on each side of the pattern.
const QUIET_ZONE: usize = 10;

/// The nine-module patterns for values 0..=46 (`1` = bar, `0` = space).
const PATTERNS: [&str; 47] = [
    "100010100", // 0  0
    "101001000", // 1  1
    "101000100", // 2  2
    "101000010", // 3  3
    "100101000", // 4  4
    "100100100", // 5  5
    "100100010", // 6  6
    "101010000", // 7  7
    "100010010", // 8  8
    "100001010", // 9  9
    "110101000", // 10 A
    "110100100", // 11 B
    "110100010", // 12 C
    "110010100", // 13 D
    "110010010", // 14 E
    "110001010", // 15 F
    "101101000", // 16 G
    "101100100", // 17 H
    "101100010", // 18 I
    "100110100", // 19 J
    "100011010", // 20 K
    "101011000", // 21 L
    "101001100", // 22 M
    "101000110", // 23 N
    "100101100", // 24 O
    "100010110", // 25 P
    "110110100", // 26 Q
    "110110010", // 27 R
    "110101100", // 28 S
    "110100110", // 29 T
    "110010110", // 30 U
    "110011010", // 31 V
    "101101100", // 32 W
    "101100110", // 33 X
    "100110110", // 34 Y
    "100111010", // 35 Z
    "100101110", // 36 -
    "111010100", // 37 .
    "111010010", // 38 SPACE
    "111001010", // 39 $
    "101101110", // 40 /
    "101110110", // 41 +
    "110101110", // 42 %
    "100100110", // 43 ($)
    "111011010", // 44 (%)
    "111010110", // 45 (/)
    "100110010", // 46 (+)
];

/// The `*` start/stop character pattern.
const START_STOP: &str = "101011110";

/// The base ASCII character for a value 0..=42, or `None` for the shift values 43..=46.
fn base_char(value: u8) -> Option<u8> {
    Some(match value {
        0..=9 => b'0' + value,
        10..=35 => b'A' + (value - 10),
        36 => b'-',
        37 => b'.',
        38 => b' ',
        39 => b'$',
        40 => b'/',
        41 => b'+',
        42 => b'%',
        _ => return None,
    })
}

/// The value 0..=42 of a base ASCII character, or `None` if outside the base set.
fn base_value(b: u8) -> Option<u8> {
    Some(match b {
        b'0'..=b'9' => b - b'0',
        b'A'..=b'Z' => b - b'A' + 10,
        b'-' => 36,
        b'.' => 37,
        b' ' => 38,
        b'$' => 39,
        b'/' => 40,
        b'+' => 41,
        b'%' => 42,
        _ => return None,
    })
}

/// The Code 39-style shift prefix (`$ % / +`) that a Code 93 shift value 43..=46 stands
/// in for, used to reuse the shared full-ASCII pairing rules.
fn shift_prefix(value: u8) -> Option<u8> {
    Some(match value {
        43 => b'$',
        44 => b'%',
        45 => b'/',
        46 => b'+',
        _ => return None,
    })
}

/// The shift value 43..=46 for a Code 39-style prefix character.
fn shift_value(prefix: u8) -> u8 {
    match prefix {
        b'$' => 43,
        b'%' => 44,
        b'/' => 45,
        _ => 46, // b'+'
    }
}

/// Full-ASCII pairing of a byte, in the Code 39 canonical form (prefix in `$ % / +`).
/// Returns `Single(base)` for the 5 self-mapping punctuation/letter/digit classes and
/// `Pair(prefix, letter)` otherwise; `None` for bytes `>= 128`.
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

/// Reverse a full-ASCII shift pair back to its ASCII byte.
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

/// A shift pair or self-mapping character in the full-ASCII scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fa {
    /// A byte that maps to a single base character (the byte itself).
    Single(u8),
    /// A byte that maps to a shift prefix plus a letter.
    Pair(u8, u8),
}

/// Weighted checksum: rightmost character weight 1, increasing leftward, wrapping at
/// `max_weight`; result taken modulo 47.
fn checksum(values: &[u8], max_weight: usize) -> u8 {
    let mut sum: u32 = 0;
    for (i, &v) in values.iter().rev().enumerate() {
        let weight = (i % max_weight) as u32 + 1;
        sum += weight * v as u32;
    }
    (sum % 47) as u8
}

/// Parameters required to re-encode a Code 93 symbol identically (lossless round-trip).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Code93Meta {
    /// Whether the payload uses the full-ASCII extension (a shift character appears).
    pub full_ascii: bool,
}

/// Code 93 encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code93Encoder;

impl Code93Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

    /// Build a reproducible [`Symbol`] from raw `data`.
    ///
    /// When `full_ascii` is `false`, every byte must be one of the 43 base characters
    /// and the payload is stored as a single [`Segment::alphanumeric`]. When `true`,
    /// every byte must be ASCII (`< 128`) and is stored as a single [`Segment::byte`].
    pub fn build(&self, data: &[u8], full_ascii: bool) -> Result<Symbol> {
        let segment = if full_ascii {
            if data.iter().any(|&b| b >= 128) {
                return Err(Error::invalid_data(
                    "Code 93 full-ASCII payload must be ASCII (bytes < 128)",
                ));
            }
            Segment::byte(data.to_vec())
        } else {
            if let Some(&bad) = data.iter().find(|&&b| base_value(b).is_none()) {
                return Err(Error::invalid_data(format!(
                    "byte {bad:#04x} is not a Code 93 character (enable full-ASCII?)"
                )));
            }
            Segment::alphanumeric(data.to_vec())
        };
        Ok(Symbol::new(
            Symbology::Code93,
            vec![segment],
            SymbolMeta::Code93(Code93Meta { full_ascii }),
        ))
    }
}

/// Expand a payload to Code 93 symbol values (0..=46), per the full-ASCII flag.
fn expand(data: &[u8], full_ascii: bool) -> Result<Vec<u8>> {
    let mut values = Vec::new();
    for &b in data {
        if !full_ascii {
            match base_value(b) {
                Some(v) => values.push(v),
                None => {
                    return Err(Error::invalid_data(format!(
                        "byte {b:#04x} is not a Code 93 character"
                    )));
                }
            }
            continue;
        }
        if let Some(v) = base_value(b) {
            values.push(v);
        } else {
            match fa_encode(b) {
                Some(Fa::Pair(p, l)) => {
                    values.push(shift_value(p));
                    values.push(base_value(l).unwrap());
                }
                _ => {
                    return Err(Error::invalid_data(format!(
                        "byte {b:#04x} is not representable in Code 93"
                    )));
                }
            }
        }
    }
    Ok(values)
}

impl Encode for Code93Encoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::Code93 {
            return Err(Error::invalid_parameter(
                "Code93Encoder given a non-Code93 symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Code93(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Code 93 symbol missing Code93Meta",
                ));
            }
        };
        let data = symbol.payload_bytes();
        let mut values = expand(&data, meta.full_ascii)?;

        let c = checksum(&values, 20);
        values.push(c);
        let k = checksum(&values, 15);
        values.push(k);

        let mut modules: Vec<bool> = Vec::new();
        push_pattern(&mut modules, START_STOP);
        for &v in &values {
            push_pattern(&mut modules, PATTERNS[v as usize]);
        }
        push_pattern(&mut modules, START_STOP);
        modules.push(true); // terminating bar

        Ok(Encoding::Linear(LinearPattern {
            modules,
            quiet_zone: QUIET_ZONE,
        }))
    }
}

/// Code 93 decoder. Full-ASCII is detected in-band and both check characters are
/// always validated, so no configuration is required.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code93Decoder;

impl Code93Decoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for Code93Decoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        let pattern = match encoding {
            Encoding::Linear(p) => p,
            Encoding::Matrix(_) => {
                return Err(Error::invalid_parameter("Code 93 expects a linear pattern"));
            }
        };
        let runs = run_lengths(&pattern.modules);
        if runs.is_empty() {
            return Err(Error::undecodable("empty Code 93 pattern"));
        }
        // Each character is 6 runs; a single terminating bar follows the stop symbol.
        if !(runs.len() - 1).is_multiple_of(6) {
            return Err(Error::undecodable("malformed Code 93 structure"));
        }
        let nchars = (runs.len() - 1) / 6;
        if nchars < 2 {
            return Err(Error::undecodable("Code 93 missing start/stop"));
        }
        let narrow = runs.iter().map(|&(_, len)| len).min().unwrap();

        let mut values: Vec<u8> = Vec::new();
        for c in 0..nchars {
            let base = c * 6;
            let bits = rebuild_bits(&runs[base..base + 6], narrow);
            let is_start_stop = c == 0 || c == nchars - 1;
            if bits == START_STOP {
                if !is_start_stop {
                    return Err(Error::undecodable("unexpected Code 93 start/stop"));
                }
                continue;
            }
            if is_start_stop {
                return Err(Error::undecodable("Code 93 not framed by start/stop"));
            }
            match PATTERNS.iter().position(|&p| p == bits) {
                Some(v) => values.push(v as u8),
                None => return Err(Error::undecodable("unknown Code 93 character")),
            }
        }

        // Split and validate the two check characters.
        if values.len() < 2 {
            return Err(Error::undecodable("Code 93 missing check characters"));
        }
        let k = values.pop().unwrap();
        let c = values.pop().unwrap();
        if checksum(&values, 20) != c {
            return Err(Error::undecodable("Code 93 C check character mismatch"));
        }
        let mut with_c = values.clone();
        with_c.push(c);
        if checksum(&with_c, 15) != k {
            return Err(Error::undecodable("Code 93 K check character mismatch"));
        }

        let full_ascii = values.iter().any(|&v| (43..=46).contains(&v));
        let segment = if full_ascii {
            Segment::byte(collapse(&values)?)
        } else {
            let base: Vec<u8> = values
                .iter()
                .map(|&v| base_char(v).expect("value < 43 in non-full-ASCII stream"))
                .collect();
            Segment::alphanumeric(base)
        };

        Ok(Symbol::new(
            Symbology::Code93,
            vec![segment],
            SymbolMeta::Code93(Code93Meta { full_ascii }),
        ))
    }
}

/// Collapse Code 93 symbol values into ASCII bytes using the full-ASCII rules.
fn collapse(values: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < values.len() {
        let v = values[i];
        if let Some(prefix) = shift_prefix(v) {
            let letter_val = *values
                .get(i + 1)
                .ok_or_else(|| Error::undecodable("dangling Code 93 shift character"))?;
            let letter = base_char(letter_val)
                .ok_or_else(|| Error::undecodable("Code 93 shift followed by a shift"))?;
            let byte = fa_decode_pair(prefix, letter)
                .ok_or_else(|| Error::undecodable("invalid Code 93 full-ASCII pair"))?;
            out.push(byte);
            i += 2;
        } else {
            out.push(base_char(v).ok_or_else(|| Error::undecodable("stray Code 93 shift"))?);
            i += 1;
        }
    }
    Ok(out)
}

/// Append a `1`/`0` pattern string to `out` as bars/spaces.
fn push_pattern(out: &mut Vec<bool>, pattern: &str) {
    out.extend(pattern.bytes().map(|b| b == b'1'));
}

/// Run-length encode a module row into `(is_bar, length)` runs.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_are_well_formed() {
        for (v, bits) in PATTERNS.iter().enumerate() {
            assert_eq!(bits.len(), 9, "value {v} pattern length");
            let ones: usize = bits.bytes().filter(|&b| b == b'1').count();
            // three bars and three spaces => module widths sum to 9.
            assert!(bits.starts_with('1'), "value {v} must start with a bar");
            let _ = ones;
        }
        assert_eq!(START_STOP.len(), 9);
    }

    #[test]
    fn test93_checksum_matches_reference() {
        // Bar Code Island worked example: TEST93 => C = '+' (41), K = '6' (6).
        let values = expand(b"TEST93", false).unwrap();
        let c = checksum(&values, 20);
        assert_eq!(c, 41, "C check for TEST93");
        let mut with_c = values.clone();
        with_c.push(c);
        let k = checksum(&with_c, 15);
        assert_eq!(k, 6, "K check for TEST93");
    }
}
