//! GS1 DataBar Expanded (RSS Expanded), non-stacked linear variant.
//!
//! Expanded encodes a GS1 element string (Application Identifier data) rather than
//! a bare GTIN. The payload is compacted into a binary bit stream by the
//! *general-purpose* encodation of ISO/IEC 24724:2011 §7.2.5, split into 12-bit
//! symbol characters, and drawn with a variable number of finder patterns.
//!
//! ## Scope
//! Only the two general-purpose encodation methods are produced:
//! - **Method 1** (`"1"`): data beginning with AI `(01)` + 14-digit GTIN — the
//!   GTIN is packed into a compressed data field, the remainder into the general
//!   field.
//! - **Method 2** (`"00"`): any other data — the whole element string goes into the
//!   general field.
//!
//! The optional application-specific compaction methods 3–14 (variable weight /
//! price / date shortcuts, §7.2.5.4) are *not* emitted. They are pure size
//! optimisations for specific AI combinations; omitting them yields a slightly
//! longer but fully spec-valid symbol, and keeps the encoder ↔ decoder pair exactly
//! invertible. The stacked variant (`DataBarExpandedStacked`) is out of scope.
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
use crate::output::Encoding;
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;

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
fn build_binary(reduced: &[u8]) -> Result<Vec<bool>> {
    let mut bits: Vec<bool> = Vec::new();
    bits.push(false); // linkage flag: standalone (non-composite)

    // Method 1 applies to "(01) + GTIN14 [+ more]"; everything else uses method 2.
    let method1 = reduced.len() >= 16 && reduced[0] == b'0' && reduced[1] == b'1';
    let (var_pos, read_posn) = if method1 {
        push_bits(&mut bits, 4, 3); // "1" + 2 placeholder length bits
        (2usize, 16usize)
    } else {
        push_bits(&mut bits, 0, 4); // "00" + 2 placeholder length bits
        (3usize, 0usize)
    };

    // The compressed-field source must be numeric.
    for &b in &reduced[..read_posn] {
        if !b.is_ascii_digit() {
            return Err(Error::invalid_data(
                "DataBar Expanded: compressed data field requires digits",
            ));
        }
    }

    if method1 {
        // Validate the (01) GTIN check digit; the encoder drops it and the decoder
        // recomputes it, so an incorrect one would break the round-trip.
        let check = gtin_check_digit(&reduced[2..15]);
        if reduced[15] != check {
            return Err(Error::invalid_data(format!(
                "DataBar Expanded: (01) GTIN check digit mismatch: expected '{}', got '{}'",
                check as char, reduced[15] as char
            )));
        }
        push_bits(&mut bits, (reduced[2] - b'0') as u32, 4); // indicator digit
        let mut k = 3;
        while k < 15 {
            push_bits(&mut bits, parse3(&reduced[k..k + 3]), 10);
            k += 3;
        }
    }

    let (mode, last_digit) = if read_posn < reduced.len() {
        general_field_encode(&reduced[read_posn..], &mut bits)?
    } else {
        (GfMode::Numeric, None)
    };

    let mut symbol_characters = sym_chars_for(bits.len());
    let mut remainder = 12 * (symbol_characters - 1) - bits.len();

    if let Some(ld) = last_digit {
        // §7.2.5.5.1 c: encode the trailing lone digit.
        if (4..=6).contains(&remainder) {
            push_bits(&mut bits, (ld - b'0') as u32 + 1, 4);
        } else {
            push_bits(&mut bits, (ld - b'0') as u32 * 11 + 10 + 8, 7);
        }
        symbol_characters = sym_chars_for(bits.len());
        remainder = 12 * (symbol_characters - 1) - bits.len();
    }

    if bits.len() > 252 {
        return Err(Error::capacity(format!(
            "DataBar Expanded input too long: {} data characters (maximum 21)",
            bits.len().div_ceil(12)
        )));
    }

    // Patch the variable-length bit field (§7.2.5.5).
    bits[var_pos] = symbol_characters & 1 != 0;
    bits[var_pos + 1] = symbol_characters > 14;

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
/// 12-bit boundary, clamped to the minimum of 4.
fn sym_chars_for(bp: usize) -> usize {
    let remainder = (12 - bp % 12) % 12;
    ((bp + remainder) / 12 + 1).max(4)
}

/// Parse 3 ASCII digits into a value 0..=999.
fn parse3(d: &[u8]) -> u32 {
    (d[0] - b'0') as u32 * 100 + (d[1] - b'0') as u32 * 10 + (d[2] - b'0') as u32
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

/// Assemble the full element-width sequence for a reduced element string.
pub(super) fn element_widths(reduced: &[u8]) -> Result<Vec<i32>> {
    if reduced.is_empty() {
        return Err(Error::invalid_data("DataBar Expanded payload is empty"));
    }
    let bits = build_binary(reduced)?;
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
    // Guards.
    el[0] = 1;
    el[1] = 1;
    el[pattern_width - 2] = 1;
    el[pattern_width - 1] = 1;
    Ok(el)
}

/// Encode a DataBar Expanded symbol into its linear module pattern.
pub(super) fn encode(symbol: &Symbol) -> Result<Encoding> {
    let reduced = extract_reduced(&symbol.segments)?;
    let widths = element_widths(&reduced)?;
    Ok(super::encode::linear(&widths))
}

/// Concatenate the reduced element string from a symbol's data segments.
fn extract_reduced(segments: &[Segment]) -> Result<Vec<u8>> {
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
    let start = if bits[1] {
        // Method 1: "1" header, then compressed (01)+GTIN14 field.
        let indicator = read_bits(bits, 4, 4);
        if indicator > 9 {
            return Err(Error::undecodable("DataBar Expanded: bad indicator digit"));
        }
        let mut body = Vec::with_capacity(13);
        body.push(b'0' + indicator as u8);
        let mut pos = 8;
        for _ in 0..4 {
            let v = read_bits(bits, pos, 10);
            if v > 999 {
                return Err(Error::undecodable(
                    "DataBar Expanded: bad compressed digits",
                ));
            }
            body.push(b'0' + (v / 100) as u8);
            body.push(b'0' + (v / 10 % 10) as u8);
            body.push(b'0' + (v % 10) as u8);
            pos += 10;
        }
        let check = gtin_check_digit(&body);
        out.extend_from_slice(b"01");
        out.extend_from_slice(&body);
        out.push(check);
        48
    } else if !bits[2] {
        5 // Method 2: "00" header, no compressed field.
    } else {
        return Err(Error::Unsupported {
            what: "GS1 DataBar Expanded encoding methods 3-14",
        });
    };

    parse_general_field(bits, start, &mut out)?;
    Ok(out)
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
