//! Telepen (full-ASCII mode).
//!
//! Telepen is unusual: it does not assign an independent pattern to each character
//! but encodes a *bit stream*. Each data byte is an ASCII value (0..=127) given even
//! parity in bit 7, emitted least-significant-bit first. The concatenated stream —
//! start, data, check, stop — is then divided into single `1` bits and blocks of the
//! form `0 1* 0`, each mapped to a bar/space pair (narrow = 1 module, wide = 3):
//!
//! | group        | bar   | space |
//! |--------------|-------|-------|
//! | `1`          | narrow| narrow|
//! | `00`         | wide  | narrow|
//! | `010`        | wide  | wide  |
//! | `01`(lead)   | narrow| wide  |
//! | `10`(trail)  | narrow| wide  |
//!
//! Start byte `0x5F` and stop byte `0xFA` are themselves already even-parity, and
//! render as the documented "five narrow pairs + wide pair" (start) and "wide pair +
//! five narrow pairs" (stop).
//!
//! A modulo-127 check character is appended: `check = (127 - (Σ data / mod 127)) mod
//! 127`, so `(Σ data + check) mod 127 == 0`. It is optional and recorded in
//! [`TelepenMeta`]; the decoder is told whether to expect it via
//! [`TelepenDecoder::with_check`].
//!
//! Numeric compression mode is **not** implemented (full-ASCII only).
//!
//! # Validation
//!
//! The start/stop element sequences are hand-verified to reproduce the documented
//! Telepen guard patterns (see unit tests). The mod-127 check algorithm matches the
//! reference implementation [zint](https://github.com/zint/zint) (`telepen.c`); it is
//! hand-verified for `"ABC"` → check value `56` (`'8'`).

use crate::error::{Error, Result};
use crate::output::{Encoding, LinearPattern};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::{Decode, Encode};

/// Quiet-zone width in narrow modules on each side.
const QUIET_ZONE: usize = 10;
/// Start guard bit stream (byte 0x5F, LSB first).
const START_BITS: [bool; 8] = [true, true, true, true, true, false, true, false];
/// Stop guard bit stream (byte 0xFA, LSB first).
const STOP_BITS: [bool; 8] = [false, true, false, true, true, true, true, true];

/// Parameters required to re-encode a Telepen symbol identically.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TelepenMeta {
    /// Whether a trailing modulo-127 check character is present.
    pub check: bool,
}

/// The even-parity byte for an ASCII value (parity in bit 7).
fn even_parity(value: u8) -> u8 {
    if value.count_ones() % 2 == 1 {
        value | 0x80
    } else {
        value
    }
}

/// The modulo-127 check value for `data`.
fn check_value(data: &[u8]) -> u8 {
    let sum: u32 = data.iter().map(|&b| b as u32).sum();
    ((127 - (sum % 127)) % 127) as u8
}

/// Append a byte's 8 bits, LSB first, to `bits`.
fn push_byte(bits: &mut Vec<bool>, byte: u8) {
    for i in 0..8 {
        bits.push((byte >> i) & 1 == 1);
    }
}

/// Convert a bit stream to alternating (bar, space, ...) element widths.
fn bits_to_widths(bits: &[bool]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bits.len() {
        if bits[i] {
            out.push(1); // narrow bar
            out.push(1); // narrow space
            i += 1;
        } else {
            let mut j = i + 1;
            while j < bits.len() && bits[j] {
                j += 1;
            }
            let ones = j - (i + 1);
            match ones {
                0 => {
                    out.push(3); // wide bar
                    out.push(1); // narrow space
                }
                1 => {
                    out.push(3); // wide bar
                    out.push(3); // wide space
                }
                _ => {
                    out.push(1); // leading "01": narrow bar
                    out.push(3); // wide space
                    for _ in 0..ones - 2 {
                        out.push(1);
                        out.push(1);
                    }
                    out.push(1); // trailing "10": narrow bar
                    out.push(3); // wide space
                }
            }
            i = j + 1;
        }
    }
    out
}

/// Render alternating (bar, space, ...) element widths into modules.
fn widths_to_modules(widths: &[u32]) -> Vec<bool> {
    let mut modules = Vec::new();
    for (i, &w) in widths.iter().enumerate() {
        modules.extend(std::iter::repeat_n(i % 2 == 0, w as usize));
    }
    modules
}

/// Reverse of [`bits_to_widths`]: element widths back to the bit stream.
fn widths_to_bits(widths: &[u32]) -> Result<Vec<bool>> {
    if !widths.len().is_multiple_of(2) {
        return Err(Error::undecodable("odd Telepen element count"));
    }
    let mut bits = Vec::new();
    let mut in_block = false;
    for pair in widths.chunks_exact(2) {
        match (pair[0] > 1, pair[1] > 1) {
            (false, false) => bits.push(true), // "1"
            (true, false) => {
                // "00"
                bits.push(false);
                bits.push(false);
            }
            (true, true) => {
                // "010"
                bits.push(false);
                bits.push(true);
                bits.push(false);
            }
            (false, true) => {
                // narrow bar, wide space: leading "01" or trailing "10"
                if in_block {
                    bits.push(true);
                    bits.push(false);
                    in_block = false;
                } else {
                    bits.push(false);
                    bits.push(true);
                    in_block = true;
                }
            }
        }
    }
    if in_block {
        return Err(Error::undecodable("unterminated Telepen block"));
    }
    Ok(bits)
}

/// Telepen encoder (full-ASCII mode).
#[derive(Debug, Default, Clone, Copy)]
pub struct TelepenEncoder;

impl TelepenEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

    /// Build a symbol from ASCII `data` (bytes 0..=127), optionally with a check char.
    pub fn build(&self, data: &[u8], check: bool) -> Result<Symbol> {
        if !data.iter().all(|&b| b < 128) {
            return Err(Error::invalid_data(
                "Telepen full-ASCII data must be 0..=127",
            ));
        }
        Ok(Symbol::new(
            Symbology::Telepen,
            vec![Segment::byte(data.to_vec())],
            SymbolMeta::Telepen(TelepenMeta { check }),
        ))
    }
}

impl Encode for TelepenEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::Telepen {
            return Err(Error::invalid_parameter(
                "TelepenEncoder given a non-Telepen symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Telepen(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Telepen symbol missing TelepenMeta",
                ));
            }
        };
        let data = symbol.payload_bytes();
        if !data.iter().all(|&b| b < 128) {
            return Err(Error::invalid_data(
                "Telepen full-ASCII data must be 0..=127",
            ));
        }

        let mut bits = Vec::new();
        bits.extend_from_slice(&START_BITS);
        for &b in &data {
            push_byte(&mut bits, even_parity(b));
        }
        if meta.check {
            push_byte(&mut bits, even_parity(check_value(&data)));
        }
        bits.extend_from_slice(&STOP_BITS);

        Ok(Encoding::Linear(LinearPattern {
            modules: widths_to_modules(&bits_to_widths(&bits)),
            quiet_zone: QUIET_ZONE,
        }))
    }
}

/// Telepen decoder (full-ASCII mode).
#[derive(Debug, Default, Clone, Copy)]
pub struct TelepenDecoder {
    check: bool,
}

impl TelepenDecoder {
    /// A new decoder that expects no check character.
    pub fn new() -> Self {
        Self { check: false }
    }

    /// A decoder that expects, verifies and strips a modulo-127 check character.
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

impl Decode for TelepenDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        let pattern = match encoding {
            Encoding::Linear(p) => p,
            Encoding::Matrix(_) => {
                return Err(Error::invalid_parameter("Telepen expects a linear pattern"));
            }
        };
        let widths = rle(&pattern.modules)?;
        let bits = widths_to_bits(&widths)?;

        // Strip the 8-bit start and stop guards.
        if bits.len() < 16 || bits[..8] != START_BITS || bits[bits.len() - 8..] != STOP_BITS {
            return Err(Error::undecodable("bad Telepen guards"));
        }
        let body = &bits[8..bits.len() - 8];
        if body.len() % 8 != 0 {
            return Err(Error::undecodable("Telepen body not byte-aligned"));
        }

        let mut bytes = Vec::with_capacity(body.len() / 8);
        for group in body.chunks_exact(8) {
            let mut e = 0u8;
            for (i, &bit) in group.iter().enumerate() {
                if bit {
                    e |= 1 << i;
                }
            }
            if !e.count_ones().is_multiple_of(2) {
                return Err(Error::undecodable("Telepen parity error"));
            }
            bytes.push(e & 0x7F);
        }

        if self.check {
            let Some((&chk, data)) = bytes.split_last() else {
                return Err(Error::undecodable("Telepen missing check character"));
            };
            if chk != check_value(data) {
                return Err(Error::undecodable("Telepen check mismatch"));
            }
            bytes.truncate(bytes.len() - 1);
        }

        Ok(Symbol::new(
            Symbology::Telepen,
            vec![Segment::byte(bytes)],
            SymbolMeta::Telepen(TelepenMeta { check: self.check }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_reference_abc() {
        // "ABC" = 65+66+67 = 198; 198 mod 127 = 71; check = 127-71 = 56 ('8').
        assert_eq!(check_value(b"ABC"), 56);
    }

    #[test]
    fn start_stop_guard_shapes() {
        // Start renders as five narrow (1,1) pairs then one wide (3,3) pair.
        let start = bits_to_widths(&START_BITS);
        assert_eq!(start, vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 3, 3]);
        // Stop renders as one wide (3,3) pair then five narrow (1,1) pairs.
        let stop = bits_to_widths(&STOP_BITS);
        assert_eq!(stop, vec![3, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
    }

    #[test]
    fn roundtrip_with_and_without_check() {
        let enc = TelepenEncoder::new();
        for check in [false, true] {
            for data in [&b"ABC"[..], b"Hello, World!", b"12345", b"\x00\x7f"] {
                let sym = enc.build(data, check).unwrap();
                let encoding = enc.encode(&sym).unwrap();
                let back = TelepenDecoder::with_check(check).decode(&encoding).unwrap();
                assert_eq!(back.segments, sym.segments);
                assert_eq!(back.meta, sym.meta);
                assert_eq!(enc.encode(&back).unwrap(), encoding);
            }
        }
    }
}
