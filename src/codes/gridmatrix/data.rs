//! Grid Matrix data encodation: text/bytes ⇄ the mode-switched bitstream.
//!
//! Grid Matrix packs data as a stream of 7-bit codewords built from a bit sequence
//! that switches between encoding *modes*. This module implements the modes this
//! crate supports and the canonical segmentation that keeps the round-trip lossless.
//!
//! # Supported modes and coverage
//!
//! | Mode      | Content                     | Per-symbol size |
//! |-----------|-----------------------------|-----------------|
//! | `Numeral` | digits `0-9`                | 10 bits / 3 digits |
//! | `Upper`   | `A-Z` and space             | 5 bits |
//! | `Lower`   | `a-z` and space             | 5 bits |
//! | `Byte`    | arbitrary 8-bit bytes       | 8 bits |
//!
//! The mode-switch codewords, the 5-bit upper/lower alphabets, and the 3-digit/10-bit
//! numeral packing follow AIMD014 as implemented by zint's `gridmtx.c`
//! (`gm_mode_switch`, `gm_mode_len`, and the per-mode glyph logic).
//!
//! **Scope.** `Mixed` and `Chinese` (GB2312) modes are modelled in [`GmMode`] and
//! recognised by the decoder as *unsupported*, but are not produced by the encoder;
//! GB2312 double-byte encodation is left as future work. Two Grid Matrix `Byte`-mode
//! details are simplified into a self-consistent "anyd profile" that this encoder and
//! decoder agree on (and which differs from zint): a byte run carries a 9-bit *count*
//! (not `count - 1`), the otherwise-unused byte→byte control (`6`) marks a continued
//! run so a segment may exceed 511 bytes, and end-of-data from byte mode uses the
//! 4-bit control `0`. All other modes use the exact AIMD014 codeword values.

use super::tables::EcLevel;
use crate::error::{Error, Result};

/// A Grid Matrix data-encodation mode. The numeric [`GmMode::index`] matches the
/// specification's mode numbering (Chinese = 1 … Byte = 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GmMode {
    /// GB2312 Chinese double-byte mode (modelled, not produced by this encoder).
    Chinese,
    /// Digits `0-9`.
    Numeral,
    /// Lowercase `a-z` and space.
    Lower,
    /// Uppercase `A-Z` and space.
    Upper,
    /// The mixed control/symbol set (modelled, not produced by this encoder).
    Mixed,
    /// Arbitrary 8-bit bytes.
    Byte,
}

impl GmMode {
    /// The 1-based mode number used by the specification tables.
    pub fn index(self) -> u8 {
        match self {
            GmMode::Chinese => 1,
            GmMode::Numeral => 2,
            GmMode::Lower => 3,
            GmMode::Upper => 4,
            GmMode::Mixed => 5,
            GmMode::Byte => 6,
        }
    }
}

// -------------------------------------------------------------------------------------
// Bit I/O
// -------------------------------------------------------------------------------------

/// Append `value`'s low `len` bits to `bits`, most-significant bit first.
fn append(bits: &mut Vec<bool>, value: u32, len: usize) {
    for i in (0..len).rev() {
        bits.push((value >> i) & 1 != 0);
    }
}

/// A most-significant-first reader over a boolean bit sequence.
struct BitReader<'a> {
    bits: &'a [bool],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(bits: &'a [bool]) -> Self {
        BitReader { bits, pos: 0 }
    }

    fn read(&mut self, len: usize) -> Result<u32> {
        if self.pos + len > self.bits.len() {
            return Err(Error::undecodable("truncated Grid Matrix bitstream"));
        }
        let mut v = 0u32;
        for _ in 0..len {
            v = (v << 1) | (self.bits[self.pos] as u32);
            self.pos += 1;
        }
        Ok(v)
    }
}

// -------------------------------------------------------------------------------------
// Mode transitions (AIMD014 gm_mode_switch / gm_mode_len; byte details per module docs)
// -------------------------------------------------------------------------------------

/// Switch codeword `(value, bit length)` to move from mode `from` (0 = start) to mode
/// `to` (both by [`GmMode::index`]). `None` for combinations this encoder never emits.
fn switch_code(from: u8, to: u8) -> Option<(u32, usize)> {
    match (from, to) {
        // From the initial neutral state.
        (0, 1) => Some((1, 4)),
        (0, 2) => Some((2, 4)),
        (0, 3) => Some((3, 4)),
        (0, 4) => Some((4, 4)),
        (0, 5) => Some((5, 4)),
        (0, 6) => Some((7, 4)),
        // From NUMERAL.
        (2, 1) => Some((1019, 10)),
        (2, 3) => Some((1020, 10)),
        (2, 4) => Some((1021, 10)),
        (2, 5) => Some((1022, 10)),
        (2, 6) => Some((1023, 10)),
        // From LOWER.
        (3, 1) => Some((28, 5)),
        (3, 2) => Some((29, 5)),
        (3, 4) => Some((30, 5)),
        (3, 5) => Some((124, 7)),
        (3, 6) => Some((126, 7)),
        // From UPPER.
        (4, 1) => Some((28, 5)),
        (4, 2) => Some((29, 5)),
        (4, 3) => Some((30, 5)),
        (4, 5) => Some((124, 7)),
        (4, 6) => Some((126, 7)),
        // From BYTE (4-bit controls; also serve as the byte-run terminator).
        (6, 1) => Some((1, 4)),
        (6, 2) => Some((2, 4)),
        (6, 3) => Some((3, 4)),
        (6, 4) => Some((4, 4)),
        (6, 5) => Some((5, 4)),
        _ => None,
    }
}

/// End-of-data terminator `(value, bit length)` for the given current mode.
fn eod_code(mode: u8) -> Option<(u32, usize)> {
    match mode {
        2 => Some((1018, 10)), // NUMERAL
        3 => Some((27, 5)),    // LOWER
        4 => Some((27, 5)),    // UPPER
        6 => Some((0, 4)),     // BYTE (anyd profile)
        _ => None,
    }
}

/// Continuation control inside byte mode: another length-prefixed run follows.
const BYTE_CONTINUE: u32 = 6;

/// Max bytes in one byte-mode length block (9-bit count).
const BYTE_BLOCK_MAX: usize = 511;

// -------------------------------------------------------------------------------------
// Per-mode payload encoding
// -------------------------------------------------------------------------------------

fn encode_alpha(bits: &mut Vec<bool>, data: &[u8], base: u8) -> Result<()> {
    for &b in data {
        let g = if b == b' ' {
            26
        } else if (base..base + 26).contains(&b) {
            b - base
        } else {
            return Err(Error::invalid_data(
                "byte not representable in Grid Matrix alpha mode",
            ));
        };
        append(bits, g as u32, 5);
    }
    Ok(())
}

fn encode_numeral(bits: &mut Vec<bool>, digits: &[u8]) -> Result<()> {
    if digits.is_empty() {
        return Err(Error::invalid_data("empty Grid Matrix numeral segment"));
    }
    if digits.iter().any(|b| !b.is_ascii_digit()) {
        return Err(Error::invalid_data(
            "non-digit in Grid Matrix numeral segment",
        ));
    }
    let n = digits.len();
    let last_len = ((n - 1) % 3) + 1; // 1..=3
    append(bits, (3 - last_len) as u32, 2); // 2-bit trailing-group indicator
    let mut i = 0;
    while i < n {
        let take = if n - i > 3 { 3 } else { n - i };
        let mut val = 0u32;
        for &d in &digits[i..i + take] {
            val = val * 10 + (d - b'0') as u32;
        }
        append(bits, val, 10);
        i += take;
    }
    Ok(())
}

fn encode_byte(bits: &mut Vec<bool>, data: &[u8]) {
    let mut i = 0;
    loop {
        let chunk = (data.len() - i).min(BYTE_BLOCK_MAX);
        append(bits, chunk as u32, 9);
        for &b in &data[i..i + chunk] {
            append(bits, b as u32, 8);
        }
        i += chunk;
        if i >= data.len() {
            break;
        }
        append(bits, BYTE_CONTINUE, 4);
    }
}

/// Encode a sequence of `(mode, bytes)` segments into the Grid Matrix data bitstream,
/// including the initial mode selector and the end-of-data terminator.
pub fn encode_segments(segs: &[(GmMode, Vec<u8>)]) -> Result<Vec<bool>> {
    if segs.is_empty() {
        return Err(Error::invalid_parameter("no Grid Matrix segments"));
    }
    let mut bits = Vec::new();
    let mut current = 0u8; // start state
    for (mode, data) in segs {
        let to = mode.index();
        let (code, len) = switch_code(current, to).ok_or(Error::Unsupported {
            what: "Grid Matrix mode transition (Chinese/Mixed not supported)",
        })?;
        append(&mut bits, code, len);
        current = to;
        match mode {
            GmMode::Numeral => encode_numeral(&mut bits, data)?,
            GmMode::Upper => encode_alpha(&mut bits, data, b'A')?,
            GmMode::Lower => encode_alpha(&mut bits, data, b'a')?,
            GmMode::Byte => encode_byte(&mut bits, data),
            GmMode::Chinese | GmMode::Mixed => {
                return Err(Error::Unsupported {
                    what: "Grid Matrix Chinese/Mixed encodation",
                });
            }
        }
    }
    let (code, len) = eod_code(current).ok_or(Error::Unsupported {
        what: "Grid Matrix end-of-data for this mode",
    })?;
    append(&mut bits, code, len);
    Ok(bits)
}

// -------------------------------------------------------------------------------------
// Decoding
// -------------------------------------------------------------------------------------

/// The mode selected by the initial 4-bit selector.
fn start_mode(sel: u32) -> Result<GmMode> {
    match sel {
        1 => unsupported_mode(),
        2 => Ok(GmMode::Numeral),
        3 => Ok(GmMode::Lower),
        4 => Ok(GmMode::Upper),
        5 => unsupported_mode(),
        7 => Ok(GmMode::Byte),
        _ => Err(Error::undecodable("invalid Grid Matrix mode selector")),
    }
}

fn unsupported_mode() -> Result<GmMode> {
    Err(Error::Unsupported {
        what: "Grid Matrix Chinese/Mixed encodation",
    })
}

/// Mode reached by a 4-bit control from byte mode (1..=5).
fn byte_switch_mode(ctrl: u32) -> Result<GmMode> {
    match ctrl {
        1 => unsupported_mode(),
        2 => Ok(GmMode::Numeral),
        3 => Ok(GmMode::Lower),
        4 => Ok(GmMode::Upper),
        5 => unsupported_mode(),
        _ => Err(Error::undecodable("invalid Grid Matrix byte-mode control")),
    }
}

/// One resolved control after a mode's payload: either end-of-data or a new mode.
enum Next {
    Eod,
    Mode(GmMode),
}

fn decode_alpha(reader: &mut BitReader<'_>, base: u8) -> Result<(Vec<u8>, Next)> {
    let mut data = Vec::new();
    loop {
        let v = reader.read(5)?;
        if v <= 26 {
            data.push(if v == 26 { b' ' } else { base + v as u8 });
        } else {
            let next = match v {
                27 => Next::Eod,
                28 => Next::Mode(unsupported_mode()?),
                29 => Next::Mode(GmMode::Numeral),
                30 => Next::Mode(if base == b'A' {
                    GmMode::Lower
                } else {
                    GmMode::Upper
                }),
                31 => {
                    let code = 124 + reader.read(2)?;
                    match code {
                        124 => Next::Mode(unsupported_mode()?),
                        126 => Next::Mode(GmMode::Byte),
                        _ => {
                            // 125 = shift char (not emitted by this encoder).
                            return Err(Error::Unsupported {
                                what: "Grid Matrix shift character",
                            });
                        }
                    }
                }
                _ => unreachable!("5-bit value out of range"),
            };
            return Ok((data, next));
        }
    }
}

fn decode_numeral(reader: &mut BitReader<'_>) -> Result<(Vec<u8>, Next)> {
    let p = reader.read(2)? as usize;
    let last_len = 3 - p; // 1..=3
    let mut groups = Vec::new();
    let control = loop {
        let v = reader.read(10)?;
        if v <= 999 {
            groups.push(v);
        } else {
            break v;
        }
    };
    let mut data = Vec::new();
    let count = groups.len();
    for (k, &g) in groups.iter().enumerate() {
        let len = if k + 1 == count { last_len } else { 3 };
        let mut buf = [0u8; 3];
        let mut vv = g;
        for slot in buf[..len].iter_mut().rev() {
            *slot = b'0' + (vv % 10) as u8;
            vv /= 10;
        }
        data.extend_from_slice(&buf[..len]);
    }
    let next = match control {
        1018 => Next::Eod,
        1019 => Next::Mode(unsupported_mode()?),
        1020 => Next::Mode(GmMode::Lower),
        1021 => Next::Mode(GmMode::Upper),
        1022 => Next::Mode(unsupported_mode()?),
        1023 => Next::Mode(GmMode::Byte),
        _ => return Err(Error::undecodable("invalid Grid Matrix numeral control")),
    };
    Ok((data, next))
}

fn decode_byte(reader: &mut BitReader<'_>) -> Result<(Vec<u8>, Next)> {
    let mut data = Vec::new();
    loop {
        let count = reader.read(9)? as usize;
        for _ in 0..count {
            data.push(reader.read(8)? as u8);
        }
        let ctrl = reader.read(4)?;
        if ctrl == BYTE_CONTINUE {
            continue;
        }
        let next = if ctrl == 0 {
            Next::Eod
        } else {
            Next::Mode(byte_switch_mode(ctrl)?)
        };
        return Ok((data, next));
    }
}

/// Decode the Grid Matrix data bitstream into `(mode, bytes)` segments, stopping at
/// the end-of-data terminator.
pub fn decode_segments(bits: &[bool]) -> Result<Vec<(GmMode, Vec<u8>)>> {
    let mut reader = BitReader::new(bits);
    let mut mode = start_mode(reader.read(4)?)?;
    let mut segs = Vec::new();
    loop {
        let (data, next) = match mode {
            GmMode::Numeral => decode_numeral(&mut reader)?,
            GmMode::Upper => decode_alpha(&mut reader, b'A')?,
            GmMode::Lower => decode_alpha(&mut reader, b'a')?,
            GmMode::Byte => decode_byte(&mut reader)?,
            GmMode::Chinese | GmMode::Mixed => {
                return Err(Error::Unsupported {
                    what: "Grid Matrix Chinese/Mixed encodation",
                });
            }
        };
        segs.push((mode, data));
        match next {
            Next::Eod => break,
            Next::Mode(m) => mode = m,
        }
    }
    Ok(segs)
}

// -------------------------------------------------------------------------------------
// Canonical segmentation (deterministic; reproduced exactly by the decoder)
// -------------------------------------------------------------------------------------

/// The mode a run *starting* with byte `b` is assigned.
fn start_class(b: u8) -> GmMode {
    if b.is_ascii_digit() {
        GmMode::Numeral
    } else if b.is_ascii_uppercase() {
        GmMode::Upper
    } else if b.is_ascii_lowercase() {
        GmMode::Lower
    } else if b == b' ' {
        GmMode::Upper // deterministic tie-break for a leading space
    } else {
        GmMode::Byte
    }
}

/// Whether byte `b` may extend an ongoing run in `mode`.
fn fits(mode: GmMode, b: u8) -> bool {
    match mode {
        GmMode::Numeral => b.is_ascii_digit(),
        GmMode::Upper => b.is_ascii_uppercase() || b == b' ',
        GmMode::Lower => b.is_ascii_lowercase() || b == b' ',
        GmMode::Byte => {
            !(b.is_ascii_digit() || b.is_ascii_uppercase() || b.is_ascii_lowercase() || b == b' ')
        }
        GmMode::Chinese | GmMode::Mixed => false,
    }
}

/// Split raw `data` into the canonical sequence of `(mode, bytes)` segments this
/// encoder produces. Greedy maximal runs guarantee no two adjacent segments share a
/// mode, so the decoder reconstructs the identical segmentation from the mode
/// switches in the bitstream — the property that makes the round-trip lossless.
pub fn canonical_segments(data: &[u8]) -> Vec<(GmMode, Vec<u8>)> {
    if data.is_empty() {
        return vec![(GmMode::Byte, Vec::new())];
    }
    let mut segs = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let mode = start_class(data[i]);
        let start = i;
        i += 1;
        while i < data.len() && fits(mode, data[i]) {
            i += 1;
        }
        segs.push((mode, data[start..i].to_vec()));
    }
    segs
}

/// The recommended EC level for a symbol of the given version (zint's defaults):
/// strongest for the smallest symbols, easing off as capacity grows.
pub fn recommended_ec(version: u8) -> EcLevel {
    match version {
        1 => EcLevel::L5,
        2 | 3 => EcLevel::L4,
        _ => EcLevel::L3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(segs: &[(GmMode, Vec<u8>)]) {
        let bits = encode_segments(segs).unwrap();
        let back = decode_segments(&bits).unwrap();
        assert_eq!(back, segs, "segment bitstream round trip");
    }

    #[test]
    fn bitstream_round_trips_each_mode() {
        roundtrip(&[(GmMode::Numeral, b"1234567".to_vec())]);
        roundtrip(&[(GmMode::Upper, b"HELLO WORLD".to_vec())]);
        roundtrip(&[(GmMode::Lower, b"grid matrix".to_vec())]);
        roundtrip(&[(GmMode::Byte, vec![0, 1, 2, 255, 128, 127])]);
        roundtrip(&[(GmMode::Byte, Vec::new())]);
    }

    #[test]
    fn bitstream_round_trips_mixed_sequences() {
        roundtrip(&[
            (GmMode::Upper, b"ABC ".to_vec()),
            (GmMode::Numeral, b"12".to_vec()),
            (GmMode::Lower, b"xyz".to_vec()),
            (GmMode::Byte, vec![0x80, 0x00, 0xFF]),
        ]);
    }

    #[test]
    fn byte_run_exceeds_one_block() {
        let big: Vec<u8> = (0..600).map(|i| (i % 256) as u8).collect();
        roundtrip(&[(GmMode::Byte, big)]);
    }

    #[test]
    fn numeral_all_group_boundaries() {
        for n in 1..=13usize {
            let digits: Vec<u8> = (0..n).map(|i| b'0' + (i % 10) as u8).collect();
            roundtrip(&[(GmMode::Numeral, digits)]);
        }
    }

    #[test]
    fn canonical_segmentation_is_deterministic_and_disjoint() {
        let segs = canonical_segments(b"AB12cd!! 34");
        // No two adjacent segments share a mode.
        for w in segs.windows(2) {
            assert_ne!(w[0].0, w[1].0);
        }
        // Concatenated bytes equal the input.
        let flat: Vec<u8> = segs.iter().flat_map(|(_, d)| d.clone()).collect();
        assert_eq!(flat, b"AB12cd!! 34");
    }
}
