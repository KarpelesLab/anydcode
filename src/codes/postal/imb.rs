//! USPS Intelligent Mail Barcode (IMb / OneCode / USPS4CB): 4-state, 65 bars.
//!
//! Implements the six-step encoding of USPS-B-3200 (Intelligent Mail Barcode
//! 4-State Specification, Rev H, 2015):
//!
//! 1. Conversion of the routing (ZIP) and 20-digit tracking fields into a
//!    102-bit binary value.
//! 2. An 11-bit CRC frame check sequence (generator polynomial `0x0F35`).
//! 3. Base conversion of the binary value into ten codewords (`mod 636` then
//!    `mod 1365`).
//! 4. Folding the orientation bit and the CRC's most-significant bit into
//!    codewords J and A.
//! 5. Mapping codewords to 13-bit characters via the algorithmically generated
//!    "5 of 13" (1287-entry) and "2 of 13" (78-entry) tables, then complementing
//!    each character selected by a CRC bit.
//! 6. Assembling the 65 bars from the character bits per the bar-to-character map
//!    (USPS-B-3200 Table 22 / Appendix D Table IV).
//!
//! Decoding inverts every step and re-verifies the CRC, so it recovers the exact
//! tracking and routing digits. Verified against Examples 1–4 of the spec's
//! Appendix C.

use super::BarState;
use crate::error::{Error, Result};
use crate::segment::Segment;
use std::sync::OnceLock;

/// Bar-to-character map (USPS-B-3200 Table 22): for each of the 65 bars, the
/// `(descender character, descender bit, ascender character, ascender bit)`.
/// Characters are indexed `A=0 … J=9`; bits are `0` (LSB) … `12` (MSB).
const BAR_MAP: [(u8, u8, u8, u8); 65] = [
    (7, 2, 4, 3),
    (1, 10, 0, 0),
    (9, 12, 2, 8),
    (5, 5, 6, 11),
    (8, 9, 3, 1),
    (0, 1, 5, 12),
    (2, 5, 1, 8),
    (4, 4, 9, 11),
    (6, 3, 8, 10),
    (3, 9, 7, 6),
    (5, 11, 1, 4),
    (8, 5, 2, 12),
    (9, 10, 0, 2),
    (7, 1, 6, 7),
    (3, 6, 4, 9),
    (0, 3, 8, 6),
    (6, 4, 2, 7),
    (1, 1, 9, 9),
    (7, 10, 5, 2),
    (4, 0, 3, 8),
    (6, 2, 0, 4),
    (8, 11, 1, 0),
    (9, 8, 3, 12),
    (2, 6, 7, 7),
    (5, 1, 4, 10),
    (1, 12, 6, 9),
    (7, 3, 8, 0),
    (5, 8, 9, 7),
    (4, 6, 2, 10),
    (3, 4, 0, 5),
    (8, 4, 5, 7),
    (7, 11, 1, 9),
    (6, 0, 9, 6),
    (0, 6, 4, 8),
    (2, 1, 3, 2),
    (5, 9, 8, 12),
    (4, 11, 6, 1),
    (9, 5, 7, 4),
    (3, 3, 1, 2),
    (0, 7, 2, 0),
    (1, 3, 4, 1),
    (6, 10, 3, 5),
    (8, 7, 9, 4),
    (2, 11, 5, 6),
    (0, 8, 7, 12),
    (4, 2, 8, 1),
    (5, 10, 3, 0),
    (9, 3, 0, 9),
    (6, 5, 2, 4),
    (7, 8, 1, 7),
    (5, 0, 4, 5),
    (2, 3, 0, 10),
    (6, 12, 9, 2),
    (3, 11, 1, 6),
    (8, 8, 7, 9),
    (5, 4, 0, 11),
    (1, 5, 2, 2),
    (9, 1, 4, 12),
    (8, 3, 6, 6),
    (7, 0, 3, 7),
    (4, 7, 7, 5),
    (0, 12, 1, 11),
    (2, 9, 9, 0),
    (6, 8, 5, 3),
    (3, 10, 8, 2),
];

/// The generated "5 of 13" and "2 of 13" codeword→character tables.
struct Tables {
    /// 1287 entries; codeword `0..=1286` → 13-bit character.
    five: Vec<u16>,
    /// 78 entries; codeword `1287..=1364` (index `−1287`) → 13-bit character.
    two: Vec<u16>,
}

/// The cached character tables, generated once per process.
fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| Tables {
        five: init_nof13(5, 1287),
        two: init_nof13(2, 78),
    })
}

/// Reverse the low 13 bits of `value`.
fn reverse13(value: u16) -> u16 {
    let mut reverse = 0u16;
    let mut v = value;
    for _ in 0..13 {
        reverse = (reverse << 1) | (v & 1);
        v >>= 1;
    }
    reverse
}

/// Generate an "N of 13" table (USPS-B-3200 Appendix D): the 13-bit values with
/// exactly `n` bits set, ordered so that a value and its bit-reversal are paired
/// from opposite ends of the table.
fn init_nof13(n: u32, table_length: usize) -> Vec<u16> {
    let mut table = vec![0u16; table_length];
    let mut lower = 0usize;
    let mut upper = table_length - 1;
    for count in 0u16..8192 {
        if count.count_ones() != n {
            continue;
        }
        let reverse = reverse13(count);
        if reverse < count {
            continue;
        }
        if count == reverse {
            table[upper] = count;
            upper = upper.wrapping_sub(1);
        } else {
            table[lower] = count;
            lower += 1;
            table[lower] = reverse;
            lower += 1;
        }
    }
    table
}

/// The 13-bit character for a codeword `0..=1364`.
fn codeword_to_char(codeword: u16) -> u16 {
    let t = tables();
    if (codeword as usize) < t.five.len() {
        t.five[codeword as usize]
    } else {
        t.two[codeword as usize - t.five.len()]
    }
}

/// The codeword `0..=1364` for a 13-bit character, if it is a valid one.
fn char_to_codeword(character: u16) -> Option<u16> {
    let t = tables();
    match character.count_ones() {
        5 => t
            .five
            .iter()
            .position(|&c| c == character)
            .map(|p| p as u16),
        2 => t
            .two
            .iter()
            .position(|&c| c == character)
            .map(|p| (p + t.five.len()) as u16),
        _ => None,
    }
}

/// Validate the IMb tracking and routing fields.
pub(super) fn validate(tracking: &[u8], routing: &[u8]) -> Result<()> {
    if tracking.len() != 20 || !tracking.iter().all(u8::is_ascii_digit) {
        return Err(Error::invalid_data("IMb tracking code must be 20 digits"));
    }
    // Barcode Identifier's second digit is constrained to 0..=4.
    if tracking[1] > b'4' {
        return Err(Error::invalid_data(
            "IMb barcode identifier second digit must be 0-4",
        ));
    }
    if !matches!(routing.len(), 0 | 5 | 9 | 11) || !routing.iter().all(u8::is_ascii_digit) {
        return Err(Error::invalid_data(
            "IMb routing code must be 0, 5, 9 or 11 digits",
        ));
    }
    Ok(())
}

/// Fold a run of ASCII digits into an integer.
fn parse_digits(digits: &[u8]) -> u128 {
    digits
        .iter()
        .fold(0u128, |acc, &d| acc * 10 + (d - b'0') as u128)
}

/// Step 1: the routing code's integer value with its length-dependent offset.
fn routing_value(routing: &[u8]) -> u128 {
    match routing.len() {
        5 => parse_digits(routing) + 1,
        9 => parse_digits(routing) + 100_000 + 1,
        11 => parse_digits(routing) + 1_000_000_000 + 100_000 + 1,
        _ => 0,
    }
}

/// Step 1: fold routing and tracking fields into the 102-bit binary value.
fn to_binary(tracking: &[u8], routing: &[u8]) -> u128 {
    let mut v = routing_value(routing);
    v = v * 10 + (tracking[0] - b'0') as u128;
    v = v * 5 + (tracking[1] - b'0') as u128;
    for &d in &tracking[2..20] {
        v = v * 10 + (d - b'0') as u128;
    }
    v
}

/// Step 2: the 11-bit CRC frame check sequence over the 13-byte binary value
/// (USPS-B-3200 Appendix D, `USPS_MSB_Math_CRC11GenerateFrameCheckSequence`).
fn crc11(bytes: &[u8; 13]) -> u16 {
    const GENERATOR: u16 = 0x0F35;
    let mut fcs = 0x07FFu16;

    // Most significant byte, skipping its two most significant (data-free) bits.
    let mut data = (bytes[0] as u16) << 5;
    for _ in 2..8 {
        if (fcs ^ data) & 0x400 != 0 {
            fcs = (fcs << 1) ^ GENERATOR;
        } else {
            fcs <<= 1;
        }
        fcs &= 0x7FF;
        data <<= 1;
    }
    // Remaining twelve bytes.
    for &byte in &bytes[1..13] {
        let mut data = (byte as u16) << 3;
        for _ in 0..8 {
            if (fcs ^ data) & 0x400 != 0 {
                fcs = (fcs << 1) ^ GENERATOR;
            } else {
                fcs <<= 1;
            }
            fcs &= 0x7FF;
            data <<= 1;
        }
    }
    fcs
}

/// The low 13 bytes of `value`'s big-endian representation (bits 96..103 of the
/// leading byte are zero, matching the CRC routine's "right justified" input).
fn binary_bytes(value: u128) -> [u8; 13] {
    let be = value.to_be_bytes();
    let mut out = [0u8; 13];
    out.copy_from_slice(&be[3..16]);
    out
}

/// Steps 3–4: the ten codewords (A..J → index 0..9), with orientation folded
/// into J and the CRC's top bit folded into A.
fn to_codewords(mut value: u128, fcs: u16) -> [u16; 10] {
    let mut cw = [0u16; 10];
    cw[9] = (value % 636) as u16;
    value /= 636;
    for slot in cw.iter_mut().take(9).skip(1).rev() {
        *slot = (value % 1365) as u16;
        value /= 1365;
    }
    cw[0] = value as u16; // 0..=658

    cw[9] *= 2; // orientation bit (reserved LSB, 0 for the standard orientation)
    if fcs & 0x400 != 0 {
        cw[0] += 659;
    }
    cw
}

/// Step 5: codewords → 13-bit characters, complementing those selected by the
/// low ten CRC bits.
fn to_characters(cw: &[u16; 10], fcs: u16) -> [u16; 10] {
    let mut chars = [0u16; 10];
    for (i, slot) in chars.iter_mut().enumerate() {
        let mut c = codeword_to_char(cw[i]);
        if fcs & (1 << i) != 0 {
            c ^= 0x1FFF;
        }
        *slot = c;
    }
    chars
}

/// Step 6: assemble the 65 bars from the character bits.
fn to_bars(chars: &[u16; 10]) -> Vec<BarState> {
    BAR_MAP
        .iter()
        .map(|&(dc, db, ac, ab)| {
            let descender = chars[dc as usize] & (1 << db) != 0;
            let ascender = chars[ac as usize] & (1 << ab) != 0;
            BarState::from_parts(ascender, descender)
        })
        .collect()
}

/// Encode the tracking and routing fields into the 65-bar sequence.
pub(super) fn encode(tracking: &[u8], routing: &[u8]) -> Result<Vec<BarState>> {
    validate(tracking, routing)?;
    let binary = to_binary(tracking, routing);
    let fcs = crc11(&binary_bytes(binary));
    let cw = to_codewords(binary, fcs);
    let chars = to_characters(&cw, fcs);
    Ok(to_bars(&chars))
}

/// Decode a 65-bar IMb into `(variant, [tracking, routing] segments)`.
pub(super) fn decode(bars: &[BarState]) -> Result<(super::PostalVariant, Vec<Segment>)> {
    if bars.len() != 65 {
        return Err(Error::undecodable("IMb must have exactly 65 bars"));
    }
    // Step 6 inverse: scatter each bar's ascender/descender into the characters.
    let mut chars = [0u16; 10];
    for (bar, &(dc, db, ac, ab)) in bars.iter().zip(BAR_MAP.iter()) {
        if bar.has_descender() {
            chars[dc as usize] |= 1 << db;
        }
        if bar.has_ascender() {
            chars[ac as usize] |= 1 << ab;
        }
    }

    // Step 5 inverse: recover codewords and the low ten CRC bits.
    let mut cw = [0u16; 10];
    let mut fcs = 0u16;
    for (i, slot) in cw.iter_mut().enumerate() {
        let c = chars[i];
        let (negated, base) = match c.count_ones() {
            5 | 2 => (false, c),
            8 | 11 => (true, c ^ 0x1FFF),
            _ => return Err(Error::undecodable("IMb character is not 2/5-of-13")),
        };
        if negated {
            fcs |= 1 << i;
        }
        *slot = char_to_codeword(base)
            .ok_or_else(|| Error::undecodable("IMb character not in conversion table"))?;
    }

    // Steps 3–4 inverse: undo orientation and the CRC top bit.
    if cw[9] & 1 != 0 {
        return Err(Error::undecodable(
            "IMb orientation bit set (reversed read)",
        ));
    }
    let cw_j = cw[9] / 2;
    if cw[0] >= 659 {
        fcs |= 0x400;
        cw[0] -= 659;
    }

    // Rebuild the 102-bit binary value and re-verify the CRC.
    let mut value = cw[0] as u128;
    for &c in &cw[1..9] {
        value = value * 1365 + c as u128;
    }
    value = value * 636 + cw_j as u128;
    if crc11(&binary_bytes(value)) != fcs {
        return Err(Error::undecodable("IMb CRC mismatch"));
    }

    let (tracking, routing) = from_binary(value);
    Ok((
        super::PostalVariant::IntelligentMail,
        vec![Segment::numeric(tracking), Segment::numeric(routing)],
    ))
}

/// Step 1 inverse: recover the ASCII tracking (20 digits) and routing fields.
fn from_binary(mut value: u128) -> (Vec<u8>, Vec<u8>) {
    let mut tracking = [0u8; 20];
    for slot in tracking[2..20].iter_mut().rev() {
        *slot = b'0' + (value % 10) as u8;
        value /= 10;
    }
    tracking[1] = b'0' + (value % 5) as u8;
    value /= 5;
    tracking[0] = b'0' + (value % 10) as u8;
    value /= 10;

    let routing = decode_routing(value);
    (tracking.to_vec(), routing)
}

/// Recover the routing digits from the routing integer value.
fn decode_routing(value: u128) -> Vec<u8> {
    let (width, zip) = if value == 0 {
        (0, 0)
    } else if value <= 100_000 {
        (5, value - 1)
    } else if value <= 1_000_100_000 {
        (9, value - 100_001)
    } else {
        (11, value - 1_000_100_001)
    };
    fixed_width_digits(zip, width)
}

/// The ASCII decimal representation of `value` in exactly `width` digits.
fn fixed_width_digits(value: u128, width: usize) -> Vec<u8> {
    if width == 0 {
        return Vec::new();
    }
    let mut out = vec![b'0'; width];
    let text = value.to_string();
    let bytes = text.as_bytes();
    out[width - bytes.len()..].copy_from_slice(bytes);
    out
}
