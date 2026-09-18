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

/// Quiet-zone width in narrow modules on each side.
#[cfg(feature = "encode")]
const QUIET_ZONE: usize = 10;
/// Start guard bit stream (byte 0x5F, LSB first).
const START_BITS: [bool; 8] = [true, true, true, true, true, false, true, false];
/// Stop guard bit stream (byte 0xFA, LSB first).
const STOP_BITS: [bool; 8] = [false, true, false, true, true, true, true, true];

/// Parameters required to re-encode a Telepen symbol identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TelepenMeta {
    /// Whether a trailing modulo-127 check character is present.
    pub check: bool,
}

/// The even-parity byte for an ASCII value (parity in bit 7).
#[cfg(feature = "encode")]
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

/// A byte's 8 bits, LSB first.
#[cfg(feature = "encode")]
fn byte_bits(byte: u8) -> impl Iterator<Item = bool> {
    (0..8).map(move |i| (byte >> i) & 1 == 1)
}

/// Convert a bit stream to (bar, space) element-width pairs, passed to `emit` in
/// order. Streaming: only the length of the current `0 1* 0` block is buffered.
///
/// Every closed group renders at exactly two modules per bit. Even-parity bytes
/// contain an even number of zeros, so a whole symbol's blocks all close and its `n`
/// bits yield `2 * n` modules.
#[cfg(feature = "encode")]
fn bits_to_widths(
    bits: impl Iterator<Item = bool>,
    mut emit: impl FnMut(usize, usize) -> Result<()>,
) -> Result<()> {
    // Close a block holding `ones` one-bits between its two zeros.
    fn close(emit: &mut impl FnMut(usize, usize) -> Result<()>, ones: usize) -> Result<()> {
        match ones {
            0 => emit(3, 1), // "00": wide bar, narrow space
            1 => emit(3, 3), // "010": wide bar, wide space
            _ => {
                emit(1, 3)?; // leading "01": narrow bar, wide space
                for _ in 0..ones - 2 {
                    emit(1, 1)?;
                }
                emit(1, 3) // trailing "10": narrow bar, wide space
            }
        }
    }
    // `Some(ones)` while inside a block opened by a zero bit.
    let mut block: Option<usize> = None;
    for bit in bits {
        block = match (block, bit) {
            (None, true) => {
                emit(1, 1)?; // "1": narrow bar, narrow space
                None
            }
            (None, false) => Some(0),
            (Some(ones), true) => Some(ones + 1),
            (Some(ones), false) => {
                close(&mut emit, ones)?;
                None
            }
        };
    }
    match block {
        // A stream ending inside a block renders as if the block were closed.
        Some(ones) => close(&mut emit, ones),
        None => Ok(()),
    }
}

/// Reverse of [`bits_to_widths`]: element widths back to the bit stream.
#[cfg(feature = "decode")]
fn widths_to_bits(widths: &[u32]) -> Result<Vec<bool>> {
    if !widths.len().is_multiple_of(2) {
        return Err(Error::undecodable("odd Telepen element count"));
    }
    let mut bits = Vec::new();
    let mut in_block = false;
    for pair in widths.as_chunks::<2>().0 {
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
#[cfg(feature = "encode")]
#[derive(Debug, Default, Clone, Copy)]
pub struct TelepenEncoder;

#[cfg(feature = "encode")]
impl TelepenEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

    /// Number of modules [`TelepenEncoder::encode_into`] emits for `data_len` payload
    /// bytes, excluding quiet zones (exact: every bit of the start, data, check and
    /// stop bytes renders as two modules).
    pub const fn max_modules(data_len: usize, meta: &TelepenMeta) -> usize {
        (data_len + meta.check as usize + 2) * 16
    }

    /// Heap-free encoding: write the symbol for ASCII `data` (bytes `0..=127`) under
    /// `meta` to `out`.
    ///
    /// The input is validated before the first module is written. Size a
    /// [`LinearBuf`](crate::output::LinearBuf) with [`TelepenEncoder::max_modules`].
    pub fn encode_into<S: LinearSink>(
        &self,
        data: &[u8],
        meta: &TelepenMeta,
        out: &mut S,
    ) -> Result<()> {
        if !data.iter().all(|&b| b < 128) {
            return Err(Error::invalid_data(
                "Telepen full-ASCII data must be 0..=127",
            ));
        }
        let check = meta.check.then(|| check_value(data));
        let bits = START_BITS
            .iter()
            .copied()
            .chain(data.iter().flat_map(|&b| byte_bits(even_parity(b))))
            .chain(check.into_iter().flat_map(|c| byte_bits(even_parity(c))))
            .chain(STOP_BITS);

        out.begin(QUIET_ZONE)?;
        bits_to_widths(bits, |bar, space| {
            out.push_run(true, bar)?;
            out.push_run(false, space)
        })
    }
}

#[cfg(all(feature = "alloc", feature = "encode"))]
impl TelepenEncoder {
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

#[cfg(all(feature = "alloc", feature = "encode"))]
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
        let mut pattern = LinearPattern::new();
        self.encode_into(&symbol.payload_bytes(), meta, &mut pattern)?;
        Ok(Encoding::Linear(pattern))
    }
}

/// Telepen decoder (full-ASCII mode).
#[cfg(feature = "decode")]
#[derive(Debug, Default, Clone, Copy)]
pub struct TelepenDecoder {
    check: bool,
}

#[cfg(feature = "decode")]
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

#[cfg(feature = "decode")]
impl Decode for TelepenDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        let pattern = match encoding {
            Encoding::Linear(p) => p,
            Encoding::Matrix(_) => {
                return Err(Error::invalid_parameter("Telepen expects a linear pattern"));
            }
        };
        let mut widths = rle(&pattern.modules)?;
        // The stop character closes on a narrow space that merges into the quiet
        // zone; a row cropped to its last bar simply lacks it.
        if !widths.len().is_multiple_of(2) {
            widths.push(1);
        }
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
        for group in body.as_chunks::<8>().0 {
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

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    #[test]
    fn check_reference_abc() {
        // "ABC" = 65+66+67 = 198; 198 mod 127 = 71; check = 127-71 = 56 ('8').
        assert_eq!(check_value(b"ABC"), 56);
    }

    /// Collect the element widths of a bit stream.
    fn widths(bits: &[bool]) -> Vec<usize> {
        let mut out = Vec::new();
        bits_to_widths(bits.iter().copied(), |bar, space| {
            out.extend([bar, space]);
            Ok(())
        })
        .unwrap();
        out
    }

    #[test]
    fn start_stop_guard_shapes() {
        // Start renders as five narrow (1,1) pairs then one wide (3,3) pair.
        let start = widths(&START_BITS);
        assert_eq!(start, vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 3, 3]);
        // Stop renders as one wide (3,3) pair then five narrow (1,1) pairs.
        let stop = widths(&STOP_BITS);
        assert_eq!(stop, vec![3, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
    }

    #[test]
    fn block_shapes() {
        // "00", "010", "0110", "01110" and a trailing unterminated block.
        assert_eq!(widths(&[false, false]), vec![3, 1]);
        assert_eq!(widths(&[false, true, false]), vec![3, 3]);
        assert_eq!(widths(&[false, true, true, false]), vec![1, 3, 1, 3]);
        assert_eq!(
            widths(&[false, true, true, true, false]),
            vec![1, 3, 1, 1, 1, 3]
        );
        assert_eq!(widths(&[true, false, true]), vec![1, 1, 3, 3]);
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
