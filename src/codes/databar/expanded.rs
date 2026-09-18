//! GS1 DataBar Expanded (RSS Expanded), non-stacked linear variant.
//!
//! Expanded encodes a GS1 element string (Application Identifier data) rather than
//! a bare GTIN. The payload is compacted into a binary bit stream by the
//! *general-purpose* encodation of ISO/IEC 24724:2011 §7.2.5, split into 12-bit
//! symbol characters, and drawn with a variable number of finder patterns.
//!
//! ## Encodation methods
//! All fourteen encodation methods of §7.2.5.4 are produced and read:
//! - **Method 1** (`"1"`): data beginning with AI `(01)` + 14-digit GTIN — the
//!   GTIN is packed into a compressed data field, the remainder into the general
//!   field.
//! - **Method 2** (`"00"`): any other data — the whole element string goes into the
//!   general field.
//! - **Methods 3–14** (`"0100"`, `"0101"`, `"01100"`, `"01101"`, `"0111000"` –
//!   `"0111111"`): the variable-weight / price / date shortcuts for `(01)` GTINs with
//!   indicator digit 9 followed by `(310x)`, `(320x)`, `(392x)` or `(393x)` (and an
//!   optional `(11)`/`(13)`/`(15)`/`(17)` date). The method is chosen exactly as the
//!   standard prescribes (verified bit-for-bit against zint), so a canonical element
//!   string always maps to the same symbol.
//!
//! ## Payload representation
//! The element string is stored as a single [`Segment::byte`] in *reduced* form:
//! AI digits and data concatenated, with a GS1 separator (FNC1, byte `0x1D`)
//! between a variable-length AI's value and a following AI. This is exactly the
//! byte string the general-field compactor consumes, so decoding recovers it
//! verbatim and `encode(decode(x)) == x`.
//!
//! The compaction itself (numeric / alphanumeric / ISO/IEC 646 modes, their mode
//! latches, and FNC1 handling) mirrors ISO/IEC 24724:2011 §7.2.5.5; the reverse
//! bit-stream parse mirrors the termination logic used by independent decoders
//! (ZXing's `GeneralAppIdDecoder`), so trailing pad bits never yield spurious
//! characters.

use super::tables::{
    EXP_CHECK_WEIGHT, EXP_FINDER, EXP_FINDER_SEQUENCE, EXP_G_SUM, EXP_MODULES, EXP_T_EVEN,
    EXP_WEIGHT_ROWS, EXP_WIDEST, gtin_check_digit,
};
use super::widths::{get_value, interleave};
use super::{DataBarMeta, DataBarVariant};
use crate::error::{Error, Result};
#[cfg(all(feature = "alloc", feature = "encode"))]
use crate::output::Encoding;
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use alloc::{format, vec, vec::Vec};

/// GS1 separator (FNC1) as carried inside a reduced element string.
const FNC1: u8 = 0x1D;
/// Alphanumeric punctuation, in code-value order (`"*,-./"`).
const ALNUM_PUNCS: &[u8] = b"*,-./";
/// ISO/IEC 646 punctuation, in code-value order (contains a trailing space).
const ISOIEC_PUNCS: &[u8] = b"!\"%&'()*+,-./:;<=>?_ ";

/// General-field compaction mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GfMode {
    Numeric,
    Alphanumeric,
    IsoIec,
}

// ======== bit helpers ========

/// Append the low `n` bits of `value`, most-significant bit first.
fn push_bits(bits: &mut Vec<bool>, value: u32, n: u32) {
    for k in (0..n).rev() {
        bits.push((value >> k) & 1 == 1);
    }
}

/// Read `n` bits starting at `pos`, most-significant bit first.
fn read_bits(bits: &[bool], pos: usize, n: usize) -> u32 {
    let mut v = 0u32;
    for i in 0..n {
        v = (v << 1) | (bits[pos + i] as u32);
    }
    v
}

// ======== general field: character classification & lookahead ========

/// Classify the character at `field[i]` (FNC1 counts as numeric). `None` if the
/// character is not encodable in any general-field mode.
fn gf_type(field: &[u8], i: usize) -> Option<GfMode> {
    let c = field[i];
    if c == FNC1 || c.is_ascii_digit() {
        Some(GfMode::Numeric)
    } else if c.is_ascii_uppercase() || ALNUM_PUNCS.contains(&c) {
        Some(GfMode::Alphanumeric)
    } else if c.is_ascii_lowercase() || ISOIEC_PUNCS.contains(&c) {
        Some(GfMode::IsoIec)
    } else {
        None
    }
}

/// True if the next `num` characters (from `i`) are all of type `t1` (or `t2`).
fn gf_next(field: &[u8], i: usize, len: usize, num: usize, t1: GfMode, t2: Option<GfMode>) -> bool {
    if i + num > len {
        return false;
    }
    for j in i..(i + num) {
        match gf_type(field, j) {
            Some(t) if t == t1 || Some(t) == t2 => {}
            _ => return false,
        }
    }
    true
}

/// True if characters from `i` to the end are all type `t1`, they number at least
/// `num`, and the end occurs within `max_num` characters.
fn gf_next_terminate(
    field: &[u8],
    i: usize,
    len: usize,
    num: usize,
    max_num: usize,
    t1: GfMode,
) -> bool {
    if i + max_num < len {
        return false;
    }
    let mut remaining = num as i32;
    for j in i..len {
        if gf_type(field, j) != Some(t1) {
            return false;
        }
        remaining -= 1;
    }
    remaining <= 0
}

/// True if none of the next `num` characters (or up to the end) are type `t1`.
fn gf_next_none(field: &[u8], i: usize, len: usize, num: usize, t1: GfMode) -> bool {
    let mut n = num;
    let mut j = i;
    while j < len && n > 0 {
        if gf_type(field, j) == Some(t1) {
            return false;
        }
        j += 1;
        n -= 1;
    }
    n == 0 || j == len
}

/// Compact the general field per ISO/IEC 24724:2011 §7.2.5.5, appending to `bits`.
///
/// Returns the ending mode and, if the field ended on a lone numeric digit, that
/// digit (encoded separately by the caller per §7.2.5.5.1 c).
fn general_field_encode(field: &[u8], bits: &mut Vec<bool>) -> Result<(GfMode, Option<u8>)> {
    let len = field.len();
    let mut mode = GfMode::Numeric;
    let mut last_digit = None;
    let mut i = 0;
    while i < len {
        let t = gf_type(field, i).ok_or_else(|| {
            Error::invalid_data("DataBar Expanded: character not encodable in general field")
        })?;
        match mode {
            GfMode::Numeric => {
                if i < len - 1 {
                    if t != GfMode::Numeric || gf_type(field, i + 1) != Some(GfMode::Numeric) {
                        push_bits(bits, 0, 4); // alphanumeric latch "0000"
                        mode = GfMode::Alphanumeric;
                    } else {
                        let d1 = digit_or_fnc1(field[i]);
                        let d2 = digit_or_fnc1(field[i + 1]);
                        push_bits(bits, 11 * d1 + d2 + 8, 7);
                        i += 2;
                    }
                } else if t != GfMode::Numeric {
                    push_bits(bits, 0, 4);
                    mode = GfMode::Alphanumeric;
                } else {
                    last_digit = Some(field[i]);
                    i += 1;
                }
            }
            GfMode::Alphanumeric => {
                if field[i] == FNC1 {
                    push_bits(bits, 15, 5); // "01111" FNC1 -> numeric
                    mode = GfMode::Numeric;
                    i += 1;
                } else if t == GfMode::IsoIec {
                    push_bits(bits, 4, 5); // ISO/IEC latch "00100"
                    mode = GfMode::IsoIec;
                } else if gf_next(field, i, len, 6, GfMode::Numeric, None)
                    || gf_next_terminate(field, i, len, 4, 5, GfMode::Numeric)
                {
                    push_bits(bits, 0, 3); // numeric latch "000"
                    mode = GfMode::Numeric;
                } else if field[i].is_ascii_digit() {
                    push_bits(bits, (field[i] - 43) as u32, 5);
                    i += 1;
                } else if field[i].is_ascii_uppercase() {
                    push_bits(bits, (field[i] - 33) as u32, 6);
                    i += 1;
                } else {
                    let p = ALNUM_PUNCS.iter().position(|&x| x == field[i]).unwrap();
                    push_bits(bits, p as u32 + 58, 6);
                    i += 1;
                }
            }
            GfMode::IsoIec => {
                if field[i] == FNC1 {
                    push_bits(bits, 15, 5);
                    mode = GfMode::Numeric;
                    i += 1;
                } else {
                    let next10 = gf_next_none(field, i, len, 10, GfMode::IsoIec);
                    if next10 && gf_next(field, i, len, 4, GfMode::Numeric, None) {
                        push_bits(bits, 0, 3); // numeric latch "000"
                        mode = GfMode::Numeric;
                    } else if next10
                        && gf_next(
                            field,
                            i,
                            len,
                            5,
                            GfMode::Alphanumeric,
                            Some(GfMode::Numeric),
                        )
                    {
                        push_bits(bits, 4, 5); // alphanumeric latch "00100"
                        mode = GfMode::Alphanumeric;
                    } else if field[i].is_ascii_digit() {
                        push_bits(bits, (field[i] - 43) as u32, 5);
                        i += 1;
                    } else if field[i].is_ascii_uppercase() {
                        push_bits(bits, (field[i] - 1) as u32, 7);
                        i += 1;
                    } else if field[i].is_ascii_lowercase() {
                        push_bits(bits, (field[i] - 7) as u32, 7);
                        i += 1;
                    } else {
                        let p = ISOIEC_PUNCS.iter().position(|&x| x == field[i]).unwrap();
                        push_bits(bits, p as u32 + 232, 8);
                        i += 1;
                    }
                }
            }
        }
    }
    Ok((mode, last_digit))
}

/// Numeric code for a digit byte, or `10` for FNC1.
fn digit_or_fnc1(c: u8) -> u32 {
    if c == FNC1 { 10 } else { (c - b'0') as u32 }
}

// ======== binary string assembly (§7.2.5) ========

/// Build the fully padded binary string for a reduced element string. Its length
/// is a multiple of 12 (one 12-bit symbol character per group).
///
/// `characters_per_row` is 0 for the linear symbol; for Expanded Stacked it is the
/// number of symbol characters per row (twice the column-pair count), which triggers
/// the ISO/IEC 24724:2011 §7.2.8 rule that pads a would-be single-character last row
/// out to a full column pair.
fn build_binary(reduced: &[u8], characters_per_row: usize) -> Result<Vec<bool>> {
    let mut bits: Vec<bool> = Vec::new();
    bits.push(false); // linkage flag: standalone (non-composite)

    let method = select_method(reduced);
    // Header: the method bits, with two placeholder variable-length bits where the
    // method has them; `read_posn` is where the general field starts.
    let (var_pos, read_posn) = match method {
        Method::General => {
            push_bits(&mut bits, 0, 4); // "00" + 2 placeholder length bits
            (Some(3usize), 0usize)
        }
        Method::Gtin => {
            push_bits(&mut bits, 4, 3); // "1" + 2 placeholder length bits
            (Some(2), 16)
        }
        Method::Weight3103 => {
            push_bits(&mut bits, 0b0100, 4);
            (None, 26)
        }
        Method::Weight320x => {
            push_bits(&mut bits, 0b0101, 4);
            (None, 26)
        }
        Method::Price => {
            push_bits(&mut bits, 0b01100, 5);
            push_bits(&mut bits, 0, 2); // placeholder length bits
            (Some(6), 20)
        }
        Method::PriceCurrency => {
            push_bits(&mut bits, 0b01101, 5);
            push_bits(&mut bits, 0, 2); // placeholder length bits
            (Some(6), 23)
        }
        Method::WeightDate(m) => {
            push_bits(&mut bits, 0b0111, 4);
            push_bits(&mut bits, (m - 7) as u32, 3);
            (None, reduced.len())
        }
    };

    if method != Method::General {
        // The compressed-field source must be numeric.
        if !reduced[..16].iter().all(u8::is_ascii_digit) {
            return Err(Error::invalid_data(
                "DataBar Expanded: compressed data field requires digits",
            ));
        }
        // Validate the (01) GTIN check digit; the encoder drops it and the decoder
        // recomputes it, so an incorrect one would break the round-trip.
        let check = gtin_check_digit(&reduced[2..15]);
        if reduced[15] != check {
            return Err(Error::invalid_data(format!(
                "DataBar Expanded: (01) GTIN check digit mismatch: expected '{}', got '{}'",
                check as char, reduced[15] as char
            )));
        }
        if method == Method::Gtin {
            push_bits(&mut bits, (reduced[2] - b'0') as u32, 4); // indicator digit
        }
        // Methods 3–14 imply indicator digit 9; all pack the 12 body digits.
        let mut k = 3;
        while k < 15 {
            push_bits(&mut bits, parse3(&reduced[k..k + 3]), 10);
            k += 3;
        }
    }

    match method {
        Method::Weight3103 => push_bits(&mut bits, parse_digits(&reduced[20..26]), 15),
        Method::Weight320x => {
            // (3202) weights 0..=9999 as is; (3203) weights 0..=22767 offset by 10000.
            let mut weight = parse_digits(&reduced[20..26]);
            if reduced[19] == b'3' {
                weight += 10_000;
            }
            push_bits(&mut bits, weight, 15);
        }
        Method::Price => push_bits(&mut bits, (reduced[19] - b'0') as u32, 2),
        Method::PriceCurrency => {
            push_bits(&mut bits, (reduced[19] - b'0') as u32, 2);
            push_bits(&mut bits, parse3(&reduced[20..23]), 10);
        }
        Method::WeightDate(_) => {
            // 20 bits: the AI's last digit (decimal point position) times 100 000
            // plus the five-digit weight; 16 bits: the date, or 38400 for none.
            let weight = (reduced[19] - b'0') as u32 * 100_000 + parse_digits(&reduced[21..26]);
            push_bits(&mut bits, weight, 20);
            let date = if reduced.len() == 34 {
                let yy = parse_digits(&reduced[28..30]);
                let mm = parse_digits(&reduced[30..32]);
                let dd = parse_digits(&reduced[32..34]);
                yy * 384 + (mm - 1) * 32 + dd
            } else {
                NO_DATE
            };
            push_bits(&mut bits, date, 16);
        }
        Method::General | Method::Gtin => {}
    }

    let (mode, last_digit) = if read_posn < reduced.len() {
        general_field_encode(&reduced[read_posn..], &mut bits)?
    } else {
        (GfMode::Numeric, None)
    };

    let mut symbol_characters = finalize_sym_chars(bits.len(), characters_per_row);
    let mut remainder = 12 * (symbol_characters - 1) - bits.len();

    if let Some(ld) = last_digit {
        // §7.2.5.5.1 c: encode the trailing lone digit.
        if (4..=6).contains(&remainder) {
            push_bits(&mut bits, (ld - b'0') as u32 + 1, 4);
        } else {
            push_bits(&mut bits, (ld - b'0') as u32 * 11 + 10 + 8, 7);
        }
        symbol_characters = finalize_sym_chars(bits.len(), characters_per_row);
        remainder = 12 * (symbol_characters - 1) - bits.len();
    }

    if bits.len() > 252 {
        return Err(Error::capacity(format!(
            "DataBar Expanded input too long: {} data characters (maximum 21)",
            bits.len().div_ceil(12)
        )));
    }

    // Patch the variable-length bit field (§7.2.5.5).
    if let Some(var_pos) = var_pos {
        bits[var_pos] = symbol_characters & 1 != 0;
        bits[var_pos + 1] = symbol_characters > 14;
    }

    // Padding (§7.2.5.5.4): a numeric "0000" flag if still numeric, then "00100"s.
    let target = 12 * (symbol_characters - 1);
    let mut i = remainder as i64;
    if mode == GfMode::Numeric {
        push_bits(&mut bits, 0, 4);
        i -= 4;
    }
    while i > 0 {
        push_bits(&mut bits, 4, 5); // "00100"
        i -= 5;
    }
    bits.truncate(target);
    Ok(bits)
}

/// Number of symbol characters (data + check) holding `bp` bits after padding to a
/// 12-bit boundary, before the minimum-of-4 clamp.
fn sym_chars_for(bp: usize) -> usize {
    let remainder = (12 - bp % 12) % 12;
    (bp + remainder) / 12 + 1
}

/// The final symbol-character count for `bp` bits: the raw count, then the Expanded
/// Stacked column-pair padding (ISO/IEC 24724:2011 §7.2.8) when `characters_per_row`
/// is non-zero, then the minimum of 4 (§7.2.5.5).
fn finalize_sym_chars(bp: usize, characters_per_row: usize) -> usize {
    let mut sc = sym_chars_for(bp);
    if characters_per_row != 0 && sc % characters_per_row == 1 {
        sc += 1;
    }
    sc.max(4)
}

/// Parse 3 ASCII digits into a value 0..=999.
fn parse3(d: &[u8]) -> u32 {
    (d[0] - b'0') as u32 * 100 + (d[1] - b'0') as u32 * 10 + (d[2] - b'0') as u32
}

/// Parse a run of ASCII digits (at most nine) into its value.
fn parse_digits(d: &[u8]) -> u32 {
    d.iter().fold(0, |acc, &c| acc * 10 + (c - b'0') as u32)
}

/// The 16-bit date field value meaning "no date" in methods 7–14.
const NO_DATE: u32 = 38400;

/// The encodation method of ISO/IEC 24724:2011 §7.2.5.4 for a reduced element string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    /// Method 2 (`"00"`): everything in the general field.
    General,
    /// Method 1 (`"1"`): compressed `(01)` GTIN, remainder in the general field.
    Gtin,
    /// Method 3 (`"0100"`): `(01)` indicator 9 + `(3103)` weight ≤ 32.767 kg.
    Weight3103,
    /// Method 4 (`"0101"`): `(01)` indicator 9 + `(3202)` ≤ 99.99 lb or `(3203)` ≤ 22.767 lb.
    Weight320x,
    /// Method 5 (`"01100"`): `(01)` indicator 9 + `(392x)` price, remainder general.
    Price,
    /// Method 6 (`"01101"`): `(01)` indicator 9 + `(393x)` currency + price, remainder general.
    PriceCurrency,
    /// Methods 7–14 (`"0111xxx"`): `(01)` indicator 9 + `(310x)`/`(320x)` weight and an
    /// optional `(11)`/`(13)`/`(15)`/`(17)` date, nothing else.
    WeightDate(u8),
}

/// Choose the encodation method for `reduced` exactly as §7.2.5.4 prescribes: the
/// most specific compressed form whose field constraints the data satisfies, else
/// the GTIN method, else the general method.
fn select_method(reduced: &[u8]) -> Method {
    let len = reduced.len();
    if len < 16 || &reduced[..2] != b"01" {
        return Method::General;
    }
    // Methods 3–14 require indicator digit 9 and a following (3xxx) AI. (A non-digit
    // in the GTIN is rejected by the caller whichever GTIN method is picked.)
    if len < 20 || reduced[2] != b'9' || reduced[16] != b'3' {
        return Method::Gtin;
    }
    let ai = &reduced[17..20];
    match ai {
        // (310x) / (320x): six-digit weight, at most five significant digits.
        [b'1' | b'2', b'0', x] if x.is_ascii_digit() => {
            let weight_ok =
                len >= 26 && reduced[20] == b'0' && reduced[20..26].iter().all(u8::is_ascii_digit);
            if !weight_ok {
                return Method::Gtin;
            }
            let weight = parse_digits(&reduced[20..26]);
            if len == 26 {
                if ai == b"103" && weight <= 32_767 {
                    return Method::Weight3103;
                }
                if (ai == b"202" && weight <= 9_999) || (ai == b"203" && weight <= 22_767) {
                    return Method::Weight320x;
                }
            }
            let base = if ai[0] == b'1' { 7 } else { 8 };
            if len == 26 {
                return Method::WeightDate(base);
            }
            if len == 34 && reduced[26] == b'1' && reduced[28..34].iter().all(u8::is_ascii_digit) {
                let mm = parse_digits(&reduced[30..32]);
                let dd = parse_digits(&reduced[32..34]);
                let date_ok = (1..=12).contains(&mm) && dd <= 31;
                let offset = match reduced[27] {
                    b'1' => Some(0),
                    b'3' => Some(2),
                    b'5' => Some(4),
                    b'7' => Some(6),
                    _ => None,
                };
                if let (true, Some(offset)) = (date_ok, offset) {
                    return Method::WeightDate(base + offset);
                }
            }
            Method::Gtin
        }
        // (392x) price: any general-field content may follow.
        [b'9', b'2', b'0'..=b'3'] => Method::Price,
        // (393x) three-digit ISO 4217 currency code, then the price.
        [b'9', b'3', b'0'..=b'3']
            if len >= 23 && reduced[20..23].iter().all(u8::is_ascii_digit) =>
        {
            Method::PriceCurrency
        }
        _ => Method::Gtin,
    }
}

// ======== symbol-character widths, checksum & layout ========

/// The Expanded group (0..=4) for a 12-bit symbol-character value.
fn exp_group(val: i32) -> usize {
    for (i, _) in EXP_G_SUM.iter().enumerate().take(4) {
        if val < EXP_G_SUM[i + 1] {
            return i;
        }
    }
    4
}

/// The 8 element widths of the symbol character encoding value `val`.
fn char_widths(val: i32) -> [i32; 8] {
    let group = exp_group(val);
    let odd = (val - EXP_G_SUM[group]) / EXP_T_EVEN[group];
    let even = (val - EXP_G_SUM[group]) % EXP_T_EVEN[group];
    let w = interleave(
        odd as i64,
        even as i64,
        EXP_MODULES[group],
        17 - EXP_MODULES[group],
        4,
        EXP_WIDEST[group],
        true,
    );
    w.try_into().expect("interleave yields 8 widths")
}

/// Recover the 12-bit value encoded by a symbol character's 8 element widths.
fn char_value(w: &[i32; 8]) -> Result<i32> {
    let odd = [w[0], w[2], w[4], w[6]];
    let even = [w[1], w[3], w[5], w[7]];
    let n_odd: i32 = odd.iter().sum();
    let group = EXP_MODULES
        .iter()
        .position(|&m| m == n_odd)
        .ok_or_else(|| Error::undecodable("DataBar Expanded: bad symbol-character module sum"))?;
    let odd_val = get_value(&odd, EXP_WIDEST[group], true);
    let even_val = get_value(&even, 9 - EXP_WIDEST[group], false);
    Ok(EXP_G_SUM[group] + (odd_val * EXP_T_EVEN[group] as i64 + even_val) as i32)
}

/// Weighted checksum over the data characters (§7.2.6).
fn checksum(char_ws: &[[i32; 8]]) -> i32 {
    let data_chars = char_ws.len();
    let idx = (data_chars - 2) / 2;
    let mut sum = 0i32;
    for (i, w) in char_ws.iter().enumerate() {
        let row = EXP_WEIGHT_ROWS[idx][i] as usize;
        for j in 0..8 {
            sum += w[j] * EXP_CHECK_WEIGHT[row][j];
        }
    }
    sum
}

/// The core element-width sequence of a reduced element string, together with the
/// derived symbol dimensions. The four guard elements (the first two and last two
/// slots of the `pattern_width` array) are left `0`; [`element_widths`] sets them for
/// the linear symbol, while the stacked builder places its own per-row guards.
pub(super) struct ExpElements {
    /// The `pattern_width` element widths (guard slots zeroed).
    pub elements: Vec<i32>,
    /// Number of data characters (excludes the check character).
    pub data_chars: usize,
    /// Number of codeblocks (`codeblocks = ceil(symbol_chars / 2)`).
    pub codeblocks: usize,
    /// Total element count of the linear layout.
    pub pattern_width: usize,
}

/// Assemble the core element array (guards zeroed) for a reduced element string,
/// applying the Expanded Stacked column-pair padding when `characters_per_row != 0`.
pub(super) fn expanded_elements(reduced: &[u8], characters_per_row: usize) -> Result<ExpElements> {
    if reduced.is_empty() {
        return Err(Error::invalid_data("DataBar Expanded payload is empty"));
    }
    let bits = build_binary(reduced, characters_per_row)?;
    let data_chars = bits.len() / 12;

    let mut char_ws: Vec<[i32; 8]> = Vec::with_capacity(data_chars);
    for i in 0..data_chars {
        let mut val = 0i32;
        for j in 0..12 {
            if bits[i * 12 + j] {
                val |= 0x800 >> j;
            }
        }
        char_ws.push(char_widths(val));
    }

    let symbol_chars = data_chars + 1;
    let check_char = 211 * (symbol_chars as i32 - 4) + checksum(&char_ws) % 211;
    let check_ws = char_widths(check_char);

    let codeblocks = symbol_chars.div_ceil(2);
    let pattern_width = codeblocks * 5 + symbol_chars * 8 + 4;
    let mut el = vec![0i32; pattern_width];

    // Finder patterns.
    let p = (symbol_chars - 1) / 2 - 1;
    for (i, cell) in EXP_FINDER_SEQUENCE[p].iter().enumerate().take(codeblocks) {
        let k = *cell as usize - 1;
        for j in 0..5 {
            el[21 * i + j + 10] = EXP_FINDER[k][j] as i32;
        }
    }
    // Check character.
    el[2..10].copy_from_slice(&check_ws);
    // Forward (odd-index) data characters.
    let mut i = 1;
    while i < data_chars {
        let k = ((i - 1) / 2) * 21 + 23;
        el[k..k + 8].copy_from_slice(&char_ws[i]);
        i += 2;
    }
    // Reversed (even-index) data characters.
    let mut i = 0;
    while i < data_chars {
        let k = (i / 2) * 21 + 15;
        for j in 0..8 {
            el[k + j] = char_ws[i][7 - j];
        }
        i += 2;
    }
    Ok(ExpElements {
        elements: el,
        data_chars,
        codeblocks,
        pattern_width,
    })
}

/// Assemble the full linear element-width sequence for a reduced element string.
pub(super) fn element_widths(reduced: &[u8]) -> Result<Vec<i32>> {
    let ExpElements {
        mut elements,
        pattern_width,
        ..
    } = expanded_elements(reduced, 0)?;
    // Guards.
    elements[0] = 1;
    elements[1] = 1;
    elements[pattern_width - 2] = 1;
    elements[pattern_width - 1] = 1;
    Ok(elements)
}

/// Encode a DataBar Expanded symbol into its linear module pattern.
#[cfg(all(feature = "alloc", feature = "encode"))]
pub(super) fn encode(symbol: &Symbol) -> Result<Encoding> {
    let reduced = extract_reduced(&symbol.segments)?;
    let widths = element_widths(&reduced)?;
    Ok(super::encode::linear(&widths))
}

/// Concatenate the reduced element string from a symbol's data segments.
pub(super) fn extract_reduced(segments: &[Segment]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for seg in segments {
        match seg.mode {
            Mode::Numeric | Mode::Alphanumeric | Mode::Byte => out.extend_from_slice(&seg.data),
            _ => {
                return Err(Error::invalid_data(
                    "DataBar Expanded payload must be data segments",
                ));
            }
        }
    }
    if out.is_empty() {
        return Err(Error::invalid_data("DataBar Expanded payload is empty"));
    }
    Ok(out)
}

// ======== decoding ========

/// Which symbology owns a linear pattern of `pattern_width` elements, if it could
/// be a DataBar Expanded symbol; returns the symbol-character count.
fn symbol_chars_for_width(pattern_width: usize) -> Option<usize> {
    // Up to 21 data characters plus 1 check character = 22 symbol characters.
    (4usize..=22).find(|&sc| {
        let codeblocks = sc.div_ceil(2);
        codeblocks * 5 + sc * 8 + 4 == pattern_width
    })
}

/// True if a linear element-width sequence has a valid DataBar Expanded length.
pub(super) fn is_expanded_width(pattern_width: usize) -> bool {
    symbol_chars_for_width(pattern_width).is_some()
}

/// Decode a DataBar Expanded element-width sequence into a symbol.
pub(super) fn decode(el: &[i32]) -> Result<Symbol> {
    let symbol_chars = symbol_chars_for_width(el.len())
        .ok_or_else(|| Error::undecodable("DataBar Expanded: invalid pattern width"))?;
    let data_chars = symbol_chars - 1;
    let codeblocks = symbol_chars.div_ceil(2);

    // Extract check character and data characters (even indices are reversed).
    let check_ws: [i32; 8] = el[2..10].try_into().unwrap();
    let mut char_ws: Vec<[i32; 8]> = vec![[0i32; 8]; data_chars];
    for (d, w) in char_ws.iter_mut().enumerate() {
        if d % 2 == 0 {
            let base = (d / 2) * 21 + 15;
            for j in 0..8 {
                w[j] = el[base + 7 - j];
            }
        } else {
            let base = ((d - 1) / 2) * 21 + 23;
            w.copy_from_slice(&el[base..base + 8]);
        }
    }

    // Recover values and verify the check character.
    let values: Vec<i32> = char_ws.iter().map(char_value).collect::<Result<_>>()?;
    let expected_check = 211 * (symbol_chars as i32 - 4) + checksum(&char_ws) % 211;
    if char_value(&check_ws)? != expected_check {
        return Err(Error::undecodable("DataBar Expanded: checksum mismatch"));
    }

    // Verify the finder-pattern sequence.
    let p = (symbol_chars - 1) / 2 - 1;
    for i in 0..codeblocks {
        let k = EXP_FINDER_SEQUENCE[p][i] as usize - 1;
        let got = &el[21 * i + 10..21 * i + 15];
        if !EXP_FINDER[k].iter().zip(got).all(|(&a, &b)| a as i32 == b) {
            return Err(Error::undecodable(
                "DataBar Expanded: finder pattern does not match sequence",
            ));
        }
    }

    // Reassemble the binary string (12 bits per data character, MSB first).
    let mut bits = Vec::with_capacity(12 * data_chars);
    for &v in &values {
        for j in 0..12 {
            bits.push((v >> (11 - j)) & 1 == 1);
        }
    }

    let reduced = decode_binary(&bits)?;
    Ok(Symbol::new(
        Symbology::DataBarExpanded,
        vec![Segment::byte(reduced)],
        SymbolMeta::DataBar(DataBarMeta::new(DataBarVariant::Expanded)),
    ))
}

/// Parse the binary string back into a reduced element string.
fn decode_binary(bits: &[bool]) -> Result<Vec<u8>> {
    let size = bits.len();
    if size < 5 {
        return Err(Error::undecodable(
            "DataBar Expanded: binary string too short",
        ));
    }
    if bits[0] {
        return Err(Error::Unsupported {
            what: "GS1 DataBar Expanded composite (linkage flag set)",
        });
    }

    let mut out = Vec::new();
    // Header (after the linkage flag): "1" method 1; "00" method 2; "0100"/"0101"
    // methods 3/4; "01100"/"01101" (+2 length bits) methods 5/6; "0111xxx" 7–14.
    let (method, header_len) = if bits[1] {
        (Method::Gtin, 4)
    } else if !bits[2] {
        (Method::General, 5)
    } else if size < 8 {
        return Err(Error::undecodable("DataBar Expanded: truncated header"));
    } else if !bits[3] {
        (
            if bits[4] {
                Method::Weight320x
            } else {
                Method::Weight3103
            },
            5,
        )
    } else if !bits[4] {
        (
            if bits[5] {
                Method::PriceCurrency
            } else {
                Method::Price
            },
            8,
        )
    } else {
        (Method::WeightDate(7 + read_bits(bits, 5, 3) as u8), 8)
    };
    // Fixed-field lengths after the header: the compressed GTIN body (40 bits, plus a
    // 4-bit indicator digit for method 1) and each method's own fields.
    let fixed = match method {
        Method::General => 0,
        Method::Gtin => 44,
        Method::Weight3103 | Method::Weight320x => 55,
        Method::Price => 42,
        Method::PriceCurrency => 52,
        Method::WeightDate(_) => 76,
    };
    if size < header_len + fixed {
        return Err(Error::undecodable(
            "DataBar Expanded: truncated compressed data field",
        ));
    }
    let mut pos = header_len;

    if method != Method::General {
        let indicator = if method == Method::Gtin {
            let v = read_bits(bits, pos, 4);
            pos += 4;
            v
        } else {
            9
        };
        if indicator > 9 {
            return Err(Error::undecodable("DataBar Expanded: bad indicator digit"));
        }
        let mut body = Vec::with_capacity(13);
        body.push(b'0' + indicator as u8);
        for _ in 0..4 {
            let v = read_bits(bits, pos, 10);
            if v > 999 {
                return Err(Error::undecodable(
                    "DataBar Expanded: bad compressed digits",
                ));
            }
            push_decimal(&mut body, v, 3);
            pos += 10;
        }
        let check = gtin_check_digit(&body);
        out.extend_from_slice(b"01");
        out.extend_from_slice(&body);
        out.push(check);
    }

    match method {
        Method::Weight3103 => {
            let weight = read_bits(bits, pos, 15);
            pos += 15;
            out.extend_from_slice(b"3103");
            push_decimal(&mut out, weight, 6);
        }
        Method::Weight320x => {
            let weight = read_bits(bits, pos, 15);
            pos += 15;
            if weight < 10_000 {
                out.extend_from_slice(b"3202");
                push_decimal(&mut out, weight, 6);
            } else {
                out.extend_from_slice(b"3203");
                push_decimal(&mut out, weight - 10_000, 6);
            }
        }
        Method::Price | Method::PriceCurrency => {
            out.extend_from_slice(if method == Method::Price {
                b"392"
            } else {
                b"393"
            });
            out.push(b'0' + read_bits(bits, pos, 2) as u8);
            pos += 2;
            if method == Method::PriceCurrency {
                let currency = read_bits(bits, pos, 10);
                pos += 10;
                if currency > 999 {
                    return Err(Error::undecodable("DataBar Expanded: bad currency code"));
                }
                push_decimal(&mut out, currency, 3);
            }
        }
        Method::WeightDate(m) => {
            let weight = read_bits(bits, pos, 20);
            pos += 20;
            let date = read_bits(bits, pos, 16);
            pos += 16;
            if weight >= 1_000_000 || date > NO_DATE {
                return Err(Error::undecodable(
                    "DataBar Expanded: bad weight or date field",
                ));
            }
            out.extend_from_slice(if m % 2 == 1 { b"310" } else { b"320" });
            out.push(b'0' + (weight / 100_000) as u8);
            push_decimal(&mut out, weight % 100_000, 6);
            if date != NO_DATE {
                let month = date % 384 / 32;
                if month > 11 {
                    return Err(Error::undecodable("DataBar Expanded: bad date field"));
                }
                out.extend_from_slice(match m {
                    7 | 8 => b"11",
                    9 | 10 => b"13",
                    11 | 12 => b"15",
                    _ => b"17",
                });
                push_decimal(&mut out, date / 384, 2);
                push_decimal(&mut out, month + 1, 2);
                push_decimal(&mut out, date % 32, 2);
            }
        }
        Method::General | Method::Gtin => {}
    }

    parse_general_field(bits, pos, &mut out)?;
    Ok(out)
}

/// Append `value` as exactly `digits` ASCII digits, zero padded.
fn push_decimal(out: &mut Vec<u8>, value: u32, digits: usize) {
    let start = out.len();
    out.resize(start + digits, b'0');
    let mut v = value;
    for slot in out[start..].iter_mut().rev() {
        *slot = b'0' + (v % 10) as u8;
        v /= 10;
    }
}

/// Decode the general field from `start`, appending characters to `out`.
///
/// Mirrors the ISO/IEC 24724 general-field decoding and the size-aware termination
/// of independent decoders: trailing pad bits fail the "still <mode>" tests and are
/// consumed as no-op latches, so no spurious characters are emitted.
fn parse_general_field(bits: &[bool], start: usize, out: &mut Vec<u8>) -> Result<()> {
    let size = bits.len();
    let mut pos = start;
    let mut mode = GfMode::Numeric;
    // A numeric pair ending in FNC1 is emitted lazily: it is a real separator only
    // if more data follows (a trailing one is the lone-digit terminator).
    let mut pending_fnc1 = false;

    macro_rules! emit {
        ($b:expr) => {{
            if pending_fnc1 {
                out.push(FNC1);
                pending_fnc1 = false;
            }
            out.push($b);
        }};
    }
    macro_rules! emit_fnc1 {
        () => {{
            if pending_fnc1 {
                out.push(FNC1);
            }
            out.push(FNC1);
            pending_fnc1 = false;
        }};
    }

    loop {
        if pos >= size {
            break;
        }
        let before = pos;
        match mode {
            GfMode::Numeric => {
                if is_still_numeric(bits, pos, size) {
                    if pos + 7 > size {
                        let v = read_bits(bits, pos, 4);
                        pos = size;
                        if v != 0 {
                            emit!(b'0' + (v - 1) as u8); // trailing lone digit
                        }
                    } else {
                        let v = read_bits(bits, pos, 7);
                        pos += 7;
                        if v < 8 {
                            return Err(Error::undecodable("DataBar Expanded: bad numeric code"));
                        }
                        let (d1, d2) = ((v - 8) / 11, (v - 8) % 11);
                        if d1 == 10 {
                            emit_fnc1!();
                        } else {
                            emit!(b'0' + d1 as u8);
                        }
                        if d2 == 10 {
                            pending_fnc1 = true;
                        } else {
                            emit!(b'0' + d2 as u8);
                        }
                    }
                } else if is_zero_run(bits, pos, size, 4) {
                    pos += 4; // "0000" numeric -> alphanumeric latch
                    mode = GfMode::Alphanumeric;
                } else {
                    break;
                }
            }
            GfMode::Alphanumeric => {
                if is_still_alpha(bits, pos, size) {
                    let v5 = read_bits(bits, pos, 5);
                    if v5 == 15 {
                        emit_fnc1!();
                        pos += 5;
                        mode = GfMode::Numeric;
                    } else if (5..15).contains(&v5) {
                        emit!(b'0' + (v5 - 5) as u8);
                        pos += 5;
                    } else {
                        let v6 = read_bits(bits, pos, 6);
                        pos += 6;
                        let c = if (32..58).contains(&v6) {
                            (v6 + 33) as u8
                        } else {
                            ALNUM_PUNCS[(v6 - 58) as usize]
                        };
                        emit!(c);
                    }
                } else if is_zero_run(bits, pos, size, 3) {
                    pos += 3; // "000" numeric latch
                    mode = GfMode::Numeric;
                } else if is_switch_latch(bits, pos, size) {
                    pos = (pos + 5).min(size); // "00100" -> ISO/IEC
                    mode = GfMode::IsoIec;
                } else {
                    break;
                }
            }
            GfMode::IsoIec => {
                if is_still_iso(bits, pos, size) {
                    let v5 = read_bits(bits, pos, 5);
                    if v5 == 15 {
                        emit_fnc1!();
                        pos += 5;
                        mode = GfMode::Numeric;
                    } else if (5..15).contains(&v5) {
                        emit!(b'0' + (v5 - 5) as u8);
                        pos += 5;
                    } else {
                        let v7 = read_bits(bits, pos, 7);
                        let c = if (64..90).contains(&v7) {
                            pos += 7;
                            (v7 + 1) as u8
                        } else if (90..116).contains(&v7) {
                            pos += 7;
                            (v7 + 7) as u8
                        } else {
                            let v8 = read_bits(bits, pos, 8);
                            pos += 8;
                            if (232..=252).contains(&v8) {
                                ISOIEC_PUNCS[(v8 - 232) as usize]
                            } else {
                                return Err(Error::undecodable(
                                    "DataBar Expanded: bad ISO/IEC code",
                                ));
                            }
                        };
                        emit!(c);
                    }
                } else if is_zero_run(bits, pos, size, 3) {
                    pos += 3; // "000" numeric latch
                    mode = GfMode::Numeric;
                } else if is_switch_latch(bits, pos, size) {
                    pos = (pos + 5).min(size); // "00100" -> alphanumeric
                    mode = GfMode::Alphanumeric;
                } else {
                    break;
                }
            }
        }
        if pos == before {
            break;
        }
    }
    Ok(())
}

/// Still a numeric pair/terminator here: a full 7-bit group with a non-zero leading
/// nibble, or the 4–6 residual bits of a trailing digit / padding.
fn is_still_numeric(bits: &[bool], pos: usize, size: usize) -> bool {
    if pos + 7 > size {
        return pos + 4 <= size;
    }
    bits[pos] || bits[pos + 1] || bits[pos + 2] || bits[pos + 3]
}

/// Still an alphanumeric symbol: a 5-bit digit/FNC1 or a 6-bit letter/punctuation.
fn is_still_alpha(bits: &[bool], pos: usize, size: usize) -> bool {
    if pos + 5 > size {
        return false;
    }
    let v5 = read_bits(bits, pos, 5);
    if (5..16).contains(&v5) {
        return true;
    }
    if pos + 6 > size {
        return false;
    }
    let v6 = read_bits(bits, pos, 6);
    (16..63).contains(&v6)
}

/// Still an ISO/IEC 646 symbol: a 5-bit digit/FNC1, 7-bit letter, or 8-bit punctuation.
fn is_still_iso(bits: &[bool], pos: usize, size: usize) -> bool {
    if pos + 5 > size {
        return false;
    }
    let v5 = read_bits(bits, pos, 5);
    if (5..16).contains(&v5) {
        return true;
    }
    if pos + 7 > size {
        return false;
    }
    let v7 = read_bits(bits, pos, 7);
    if (64..116).contains(&v7) {
        return true;
    }
    if pos + 8 > size {
        return false;
    }
    let v8 = read_bits(bits, pos, 8);
    (232..253).contains(&v8)
}

/// True if the next up-to-`n` bits (bounded by `size`) are all zero — a numeric or
/// alphanumeric latch (`"000"` / `"0000"`).
fn is_zero_run(bits: &[bool], pos: usize, size: usize, n: usize) -> bool {
    if pos >= size {
        return false;
    }
    for i in 0..n {
        if pos + i < size && bits[pos + i] {
            return false;
        }
    }
    true
}

/// True if the next bits form the `"00100"` mode-switch latch (bit 2 set, rest
/// clear), tolerating truncation at the end of the symbol.
fn is_switch_latch(bits: &[bool], pos: usize, size: usize) -> bool {
    if pos >= size {
        return false;
    }
    for i in 0..5 {
        if pos + i >= size {
            break;
        }
        let expect = i == 2;
        if bits[pos + i] != expect {
            return false;
        }
    }
    true
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    /// Lay out arbitrary 12-bit data-character values as a linear element sequence
    /// with a valid check character (the layout half of [`expanded_elements`]).
    fn elements_for(values: &[i32]) -> Vec<i32> {
        let char_ws: Vec<[i32; 8]> = values.iter().map(|&v| char_widths(v)).collect();
        let symbol_chars = values.len() + 1;
        let check = 211 * (symbol_chars as i32 - 4) + checksum(&char_ws) % 211;
        let codeblocks = symbol_chars.div_ceil(2);
        let mut el = vec![0i32; codeblocks * 5 + symbol_chars * 8 + 4];
        let p = (symbol_chars - 1) / 2 - 1;
        for i in 0..codeblocks {
            let k = EXP_FINDER_SEQUENCE[p][i] as usize - 1;
            for j in 0..5 {
                el[21 * i + j + 10] = EXP_FINDER[k][j] as i32;
            }
        }
        el[2..10].copy_from_slice(&char_widths(check));
        for (i, w) in char_ws.iter().enumerate() {
            if i % 2 == 1 {
                let k = ((i - 1) / 2) * 21 + 23;
                el[k..k + 8].copy_from_slice(w);
            } else {
                let k = (i / 2) * 21 + 15;
                for j in 0..8 {
                    el[k + j] = w[7 - j];
                }
            }
        }
        let n = el.len();
        (el[0], el[1], el[n - 2], el[n - 1]) = (1, 1, 1, 1);
        el
    }

    /// A minimal (three data character, 36-bit) symbol whose header selects method 1
    /// is truncated: the compressed GTIN field alone needs 48 bits. It must be
    /// rejected, not read out of bounds.
    #[test]
    fn truncated_method1_header_is_rejected() {
        let el = elements_for(&[0x400, 0, 0]);
        assert!(decode(&el).is_err());
    }

    /// Structurally valid symbols (correct finders and check character) carrying
    /// arbitrary data-character values must decode or fail cleanly, never panic.
    #[test]
    fn arbitrary_data_characters_never_panic() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..4000 {
            let n = 3 + (next() % 19) as usize;
            let values: Vec<i32> = (0..n).map(|_| (next() % 4096) as i32).collect();
            let _ = decode(&elements_for(&values));
        }
    }
}
