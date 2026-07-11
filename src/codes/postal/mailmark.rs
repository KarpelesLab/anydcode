//! Royal Mail Mailmark 4-state barcode (Barcode C and Barcode L).
//!
//! Implemented from the "Royal Mail Mailmark barcode C/L encoding and decoding
//! instructions" (Sept 2015). The human-readable payload is the fixed-width
//! field string:
//!
//! ```text
//! Format(1) VersionID(1) Class(1) SupplyChainID(N) ItemID(8) Postcode+DPS(9)
//! ```
//!
//! where the Supply Chain ID is 2 digits for Barcode C (22 characters, 66 bars)
//! and 6 digits for Barcode L (26 characters, 78 bars). Encoding:
//!
//! 1. The destination post code + DPS is packed into an integer per its format
//!    (one of six patterns, or the `XY11` international designation).
//! 2. All fields are folded into a single Consolidated Data Value (CDV).
//! 3. The CDV is split into base-30 / base-32 *data numbers*.
//! 4. Reed–Solomon check numbers are appended (`GF(32)`, primitive polynomial
//!    `0x25`, generator roots `α^1…`).
//! 5. Data/check numbers become 6-bit *symbols* (even / odd bit-parity tables),
//!    are permuted into *extender groups*, and each group's bits select three
//!    bars.
//!
//! The CDV fits comfortably in a `u128` (its maximum is well under `10^28`), so
//! no bespoke big-integer type is needed. Bar states use the `A/D/F/T`
//! convention shared by the other 4-state codes.
//!
//! Reference: verified against the worked examples in the Royal Mail encoding
//! and decoding instructions (reproduced in zint's `test_mailmark`), see
//! `tests/postal_extra.rs`.

use super::rs::{Gf, RsEncoder};
use super::{BarState, PostalVariant};
use crate::error::{Error, Result};

/// Post code format patterns (Table 3), one per `postcode_type` `1..=6`.
const POSTCODE_FORMAT: [&[u8; 9]; 6] = [
    b"ANANLLNLS",
    b"AANNLLNLS",
    b"AANNNLLNL",
    b"AANANLLNL",
    b"ANNLLNLSS",
    b"ANNNLLNLS",
];

/// Character sets used inside the post-code patterns.
const SET_A: &[u8; 26] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const SET_L: &[u8; 20] = b"ABDEFGHJLNPQRSTUWXYZ";
const SET_N: &[u8; 10] = b"0123456789";

/// Data/check symbols with odd bit-parity (Table 5), indexed by number `0..=31`.
const SYMBOL_ODD: [u8; 32] = [
    0x01, 0x02, 0x04, 0x07, 0x08, 0x0B, 0x0D, 0x0E, 0x10, 0x13, 0x15, 0x16, 0x19, 0x1A, 0x1C, 0x1F,
    0x20, 0x23, 0x25, 0x26, 0x29, 0x2A, 0x2C, 0x2F, 0x31, 0x32, 0x34, 0x37, 0x38, 0x3B, 0x3D, 0x3E,
];

/// Data symbols with even bit-parity (Table 5), indexed by number `0..=29`.
const SYMBOL_EVEN: [u8; 30] = [
    0x03, 0x05, 0x06, 0x09, 0x0A, 0x0C, 0x0F, 0x11, 0x12, 0x14, 0x17, 0x18, 0x1B, 0x1D, 0x1E, 0x21,
    0x22, 0x24, 0x27, 0x28, 0x2B, 0x2D, 0x2E, 0x30, 0x33, 0x35, 0x36, 0x39, 0x3A, 0x3C,
];

/// Extender-group permutation for Barcode C (symbol index → extender index).
const EXTENDER_C: [usize; 22] = [
    3, 5, 7, 11, 13, 14, 16, 17, 19, 0, 1, 2, 4, 6, 8, 9, 10, 12, 15, 18, 20, 21,
];

/// Extender-group permutation for Barcode L.
const EXTENDER_L: [usize; 26] = [
    2, 5, 7, 8, 13, 14, 15, 16, 21, 22, 23, 0, 1, 3, 4, 6, 9, 10, 11, 12, 17, 18, 19, 20, 24, 25,
];

/// The two Mailmark 4-state formats.
#[derive(Clone, Copy)]
struct Kind {
    /// Field-string length (22 or 26).
    length: usize,
    /// Supply Chain ID digit width.
    chain_width: usize,
    /// `data_top` for the base-32 split.
    data_top: usize,
    /// `data_step`: numbers `0..=data_step` use base 30, above use base 32.
    data_step: usize,
    /// Number of Reed–Solomon check symbols.
    check_count: usize,
    /// CDV multiplier applied after Item ID (`100` for C, `1_000_000` for L).
    item_mul: u128,
}

impl Kind {
    /// The kind for a given field-string length.
    fn from_length(length: usize) -> Kind {
        if length == 22 {
            Kind {
                length: 22,
                chain_width: 2,
                data_top: 15,
                data_step: 8,
                check_count: 6,
                item_mul: 100,
            }
        } else {
            Kind {
                length: 26,
                chain_width: 6,
                data_top: 18,
                data_step: 10,
                check_count: 7,
                item_mul: 1_000_000,
            }
        }
    }

    /// The extender permutation for this kind.
    fn extender(&self) -> &'static [usize] {
        if self.length == 22 {
            &EXTENDER_C
        } else {
            &EXTENDER_L
        }
    }
}

/// `'0'..='9' -> 0..=9`, `'A'..='Z' -> 10..=35`.
fn ctoi(c: u8) -> Option<u32> {
    if c.is_ascii_digit() {
        Some((c - b'0') as u32)
    } else if c.is_ascii_uppercase() {
        Some((c - b'A' + 10) as u32)
    } else {
        None
    }
}

/// Inverse of [`ctoi`] for `0..=35`.
fn itoc(v: u32) -> u8 {
    if v < 10 {
        b'0' + v as u8
    } else {
        b'A' + (v - 10) as u8
    }
}

/// Position of `byte` in `set`.
fn posn(set: &[u8], byte: u8) -> Option<u32> {
    set.iter().position(|&c| c == byte).map(|p| p as u32)
}

/// Normalise the input: upper-case and space-pad to 22 (short) or 26 (long).
/// Returns the canonical field string.
pub(super) fn canonical(source: &[u8]) -> Result<Vec<u8>> {
    if source.len() > 26 {
        return Err(Error::capacity("Mailmark input too long (maximum 26)"));
    }
    if source.len() < 14 {
        return Err(Error::capacity("Mailmark input too short (minimum 14)"));
    }
    let mut s: Vec<u8> = source.iter().map(|b| b.to_ascii_uppercase()).collect();
    if s.len() < 22 {
        s.resize(22, b' ');
    } else if s.len() > 22 && s.len() < 26 {
        s.resize(26, b' ');
    }
    if s.iter()
        .any(|&b| !(b.is_ascii_uppercase() || b.is_ascii_digit() || b == b' '))
    {
        return Err(Error::invalid_data(
            "Mailmark input must be alphanumerics and space only",
        ));
    }
    Ok(s)
}

/// Detect the post-code type from the 9-character field (`1..=7`).
fn postcode_type(pc: &[u8; 9]) -> usize {
    if &pc[..4] == b"XY11" && pc == b"XY11     " {
        7
    } else if pc[7] == b' ' {
        5
    } else if pc[8] == b' ' {
        if pc[1].is_ascii_digit() {
            if pc[2].is_ascii_digit() { 6 } else { 1 }
        } else {
            2
        }
    } else if pc[3].is_ascii_digit() {
        3
    } else {
        4
    }
}

/// The cumulative offset and mixed-radix span for each post-code type.
fn postcode_offset(t: usize) -> (u128, u128) {
    match t {
        1 => (1, 5_408_000_000),
        2 => (5_408_000_001, 5_408_000_000),
        3 => (10_816_000_001, 54_080_000_000),
        4 => (64_896_000_001, 140_608_000_000),
        5 => (205_504_000_001, 208_000_000),
        6 => (205_712_000_001, 2_080_000_000),
        _ => (0, 0),
    }
}

/// Pack the 9-character post-code field into its integer value.
fn postcode_to_int(pc: &[u8; 9]) -> Result<u128> {
    let t = postcode_type(pc);
    if t == 7 {
        return Ok(0);
    }
    let pattern = POSTCODE_FORMAT[t - 1];
    let mut b: u128 = 0;
    for i in 0..9 {
        match pattern[i] {
            b'A' => {
                let v = posn(SET_A, pc[i]).ok_or_else(|| bad_postcode(pc))?;
                b = b * 26 + v as u128;
            }
            b'L' => {
                let v = posn(SET_L, pc[i]).ok_or_else(|| bad_postcode(pc))?;
                b = b * 20 + v as u128;
            }
            b'N' => {
                let v = posn(SET_N, pc[i]).ok_or_else(|| bad_postcode(pc))?;
                b = b * 10 + v as u128;
            }
            _ => {} // 'S' contributes a zero
        }
    }
    let (offset, _) = postcode_offset(t);
    Ok(b + offset)
}

/// Recover the 9-character post-code field from its integer value.
fn int_to_postcode(value: u128) -> [u8; 9] {
    if value == 0 {
        return *b"XY11     ";
    }
    let mut t = 7;
    let mut bval = 0u128;
    for cand in 1..=6 {
        let (offset, span) = postcode_offset(cand);
        if value >= offset && value < offset + span {
            t = cand;
            bval = value - offset;
            break;
        }
    }
    let pattern = POSTCODE_FORMAT[t - 1];
    let mut pc = [b' '; 9];
    for i in (0..9).rev() {
        match pattern[i] {
            b'A' => {
                pc[i] = SET_A[(bval % 26) as usize];
                bval /= 26;
            }
            b'L' => {
                pc[i] = SET_L[(bval % 20) as usize];
                bval /= 20;
            }
            b'N' => {
                pc[i] = SET_N[(bval % 10) as usize];
                bval /= 10;
            }
            _ => pc[i] = b' ',
        }
    }
    pc
}

/// Build an "invalid post code" error.
fn bad_postcode(pc: &[u8; 9]) -> Error {
    Error::invalid_data(format!(
        "Mailmark invalid post code \"{}\"",
        String::from_utf8_lossy(pc)
    ))
}

/// Parsed Mailmark fields.
struct Fields {
    format: u32,
    version_id: u32,
    mail_class: u32,
    supply_chain_id: u128,
    item_id: u128,
    postcode: u128,
}

/// Parse a canonical field string into its fields.
fn parse_fields(s: &[u8], kind: Kind) -> Result<Fields> {
    let length = kind.length;
    let format = ctoi(s[0]).filter(|&f| f <= 4).ok_or_else(|| {
        Error::invalid_data("Mailmark Format (1st character) out of range (0 to 4)")
    })?;
    let version_id = ctoi(s[1])
        .filter(|&v| (1..=4).contains(&v))
        .ok_or_else(|| {
            Error::invalid_data("Mailmark Version ID (2nd character) out of range (1 to 4)")
        })?
        - 1;
    let mail_class = ctoi(s[2]).filter(|&c| c <= 14).ok_or_else(|| {
        Error::invalid_data("Mailmark Class (3rd character) out of range (0 to 9, A to E)")
    })?;

    let mut supply_chain_id: u128 = 0;
    for &c in &s[3..length - 17] {
        let d = c
            .is_ascii_digit()
            .then(|| (c - b'0') as u128)
            .ok_or_else(|| Error::invalid_data("Mailmark Supply Chain ID must be digits"))?;
        supply_chain_id = supply_chain_id * 10 + d;
    }
    let mut item_id: u128 = 0;
    for &c in &s[length - 17..length - 9] {
        let d = c
            .is_ascii_digit()
            .then(|| (c - b'0') as u128)
            .ok_or_else(|| Error::invalid_data("Mailmark Item ID must be 8 digits"))?;
        item_id = item_id * 10 + d;
    }
    let mut postcode = [b' '; 9];
    postcode.copy_from_slice(&s[length - 9..length]);
    let postcode = postcode_to_int(&postcode)?;

    Ok(Fields {
        format,
        version_id,
        mail_class,
        supply_chain_id,
        item_id,
        postcode,
    })
}

/// Fold the fields into the Consolidated Data Value.
fn to_cdv(f: &Fields, kind: Kind) -> u128 {
    let mut cdv = f.postcode;
    cdv = cdv * 100_000_000 + f.item_id;
    cdv = cdv * kind.item_mul + f.supply_chain_id;
    cdv = cdv * 15 + f.mail_class as u128;
    cdv = cdv * 5 + f.format as u128;
    cdv = cdv * 4 + f.version_id as u128;
    cdv
}

/// Split the CDV into data numbers `data[0..=data_top]`.
fn to_data_numbers(mut cdv: u128, kind: Kind) -> Vec<u8> {
    let mut data = vec![0u8; kind.data_top + 1 + kind.check_count];
    for j in (kind.data_step + 1..=kind.data_top).rev() {
        data[j] = (cdv % 32) as u8;
        cdv /= 32;
    }
    for j in (0..=kind.data_step).rev() {
        data[j] = (cdv % 30) as u8;
        cdv /= 30;
    }
    data
}

/// The `GF(32)` Reed–Solomon check numbers for `data[0..=data_top]`.
fn rs_check(data: &[u8], kind: Kind) -> Vec<u8> {
    let gf = Gf::new(0x25, 5);
    RsEncoder::new(&gf, kind.check_count, 1).encode(&data[..=kind.data_top])
}

/// Encode a Mailmark field string (canonical or raw) into its bar sequence.
pub(super) fn encode(source: &[u8]) -> Result<Vec<BarState>> {
    let s = canonical(source)?;
    let kind = Kind::from_length(s.len());
    let fields = parse_fields(&s, kind)?;
    let cdv = to_cdv(&fields, kind);

    let mut data = to_data_numbers(cdv, kind);
    let check = rs_check(&data, kind);
    data[kind.data_top + 1..].copy_from_slice(&check);

    // Numbers → symbols (even table for the base-30 numbers, odd otherwise).
    for slot in data.iter_mut().take(kind.data_step + 1) {
        *slot = SYMBOL_EVEN[*slot as usize];
    }
    for slot in data.iter_mut().skip(kind.data_step + 1) {
        *slot = SYMBOL_ODD[*slot as usize];
    }

    // Symbols → extender groups.
    let mut extender = vec![0u8; kind.length];
    for (i, &sym) in data.iter().enumerate() {
        extender[kind.extender()[i]] = sym;
    }

    // Extender groups → bars.
    let mut bars = Vec::with_capacity(kind.length * 3);
    for (i, &group) in extender.iter().enumerate() {
        let mut e = group;
        for _ in 0..3 {
            bars.push(match (e & 0x24, i % 2) {
                (0x24, _) => BarState::Full,
                (0x20, 0) => BarState::Ascender,
                (0x20, _) => BarState::Descender,
                (0x04, 0) => BarState::Descender,
                (0x04, _) => BarState::Ascender,
                _ => BarState::Tracker,
            });
            e <<= 1;
        }
    }
    Ok(bars)
}

/// Decode a Mailmark bar sequence (66 or 78 bars) into its canonical field
/// string. Returns `Err` if the length, symbols or Reed–Solomon check fail.
pub(super) fn decode(bars: &[BarState]) -> Result<(PostalVariant, Vec<u8>)> {
    if bars.len() != 66 && bars.len() != 78 {
        return Err(Error::undecodable("Mailmark bar count invalid"));
    }
    let kind = Kind::from_length(bars.len() / 3);

    // Bars → extender groups (invert the bit/parity mapping).
    let mut extender = vec![0u8; kind.length];
    let mut idx = 0;
    for (i, slot) in extender.iter_mut().enumerate() {
        let mut e = 0u8;
        for j in 0..3 {
            let (hi, lo) = match (bars[idx], i % 2) {
                (BarState::Full, _) => (1, 1),
                (BarState::Tracker, _) => (0, 0),
                (BarState::Ascender, 0) => (1, 0),
                (BarState::Ascender, _) => (0, 1),
                (BarState::Descender, 0) => (0, 1),
                (BarState::Descender, _) => (1, 0),
            };
            e |= hi << (5 - j);
            e |= lo << (2 - j);
            idx += 1;
        }
        *slot = e;
    }

    // Extender groups → symbols (invert the permutation).
    let ext = kind.extender();
    let mut data = vec![0u8; kind.length];
    for (i, &pos) in ext.iter().enumerate() {
        data[i] = extender[pos];
    }

    // Symbols → numbers.
    let mut nums = vec![0u8; kind.length];
    for (i, slot) in nums.iter_mut().enumerate() {
        let sym = data[i];
        let n = if i <= kind.data_step {
            SYMBOL_EVEN.iter().position(|&c| c == sym)
        } else {
            SYMBOL_ODD.iter().position(|&c| c == sym)
        };
        *slot = n.ok_or_else(|| Error::undecodable("Mailmark symbol not in table"))? as u8;
    }

    // Verify the Reed–Solomon check numbers.
    let expected = rs_check(&nums, kind);
    if nums[kind.data_top + 1..] != expected[..] {
        return Err(Error::undecodable("Mailmark Reed-Solomon mismatch"));
    }

    // Numbers → CDV.
    let mut cdv: u128 = 0;
    for &d in &nums[..=kind.data_step] {
        cdv = cdv * 30 + d as u128;
    }
    for &d in &nums[kind.data_step + 1..=kind.data_top] {
        cdv = cdv * 32 + d as u128;
    }

    // CDV → fields.
    let version_id = (cdv % 4) as u32;
    cdv /= 4;
    let format = (cdv % 5) as u32;
    cdv /= 5;
    let mail_class = (cdv % 15) as u32;
    cdv /= 15;
    let supply_chain_id = cdv % kind.item_mul;
    cdv /= kind.item_mul;
    let item_id = cdv % 100_000_000;
    cdv /= 100_000_000;
    let postcode = int_to_postcode(cdv);

    // Rebuild the canonical field string.
    let mut out = Vec::with_capacity(kind.length);
    out.push(itoc(format));
    out.push(itoc(version_id + 1));
    out.push(itoc(mail_class));
    push_fixed(&mut out, supply_chain_id, kind.chain_width);
    push_fixed(&mut out, item_id, 8);
    out.extend_from_slice(&postcode);
    Ok((PostalVariant::Mailmark, out))
}

/// Append `value` as exactly `width` ASCII decimal digits.
fn push_fixed(out: &mut Vec<u8>, value: u128, width: usize) {
    let text = value.to_string();
    for _ in text.len()..width {
        out.push(b'0');
    }
    out.extend_from_slice(text.as_bytes());
}
