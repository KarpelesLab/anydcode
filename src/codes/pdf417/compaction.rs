//! PDF417 high-level data compaction (ISO/IEC 15438 §5.4 / Annex P) and its inverse.
//!
//! Each data [`Segment`] maps to one compaction mode preceded by an explicit mode
//! latch, so segment boundaries survive decoding:
//! - [`Mode::Numeric`] → Numeric compaction (latch 902),
//! - [`Mode::Byte`] → Byte compaction (latch 901, or 924 when the length is a
//!   multiple of six),
//! - [`Mode::Alphanumeric`] → Text compaction (latch 900), covering the full
//!   printable-ASCII set plus HT/LF/CR via the Alpha/Lower/Mixed/Punctuation
//!   sub-modes,
//! - [`Mode::Eci`] → an Extended Channel Interpretation escape (927 / 926 / 925 by
//!   assignment number range, §5.5), which carries no data of its own.
//!
//! Content codewords are always below 900; only mode latches, escapes and padding are
//! ≥ 900, which is what lets the decoder split the stream back into segments.
//!
//! The decoder additionally reads what other encoders emit but this one does not: a
//! leading Text run without its (default-mode) latch, the 913 single-byte shift out
//! of Text compaction, and data that carries on after an ECI escape without a new
//! latch (the compaction mode, and in Text compaction the sub-mode, stay in force).

use crate::error::{Error, Result};
use crate::segment::{Mode, Segment};
use alloc::{vec, vec::Vec};

const LATCH_TEXT: u32 = 900;
const LATCH_BYTE: u32 = 901;
const LATCH_NUMERIC: u32 = 902;
const LATCH_BYTE_6: u32 = 924;
/// Shift to Byte compaction for the single following codeword (from Text only).
const SHIFT_BYTE: u32 = 913;
/// ECI escape for assignments 0..=899: one following codeword, the number itself.
const ECI_CHARSET: u32 = 927;
/// ECI escape for assignments 900..=810 899: two following codewords `g1`, `g2`
/// with number `(g1 + 1) * 900 + g2`.
const ECI_GENERAL: u32 = 926;
/// ECI escape for assignments 810 900..=811 799: one following codeword, the number
/// less 810 900.
const ECI_USER: u32 = 925;
/// The largest ECI assignment number PDF417 can express.
const ECI_MAX: u32 = 811_799;

// Text sub-mode control values (used within Text compaction only).
const TEXT_LL: u8 = 27; // latch to Lower
const TEXT_ML: u8 = 28; // latch to Mixed (also AL = latch to Alpha, same value)
const TEXT_PS: u8 = 29; // shift to Punctuation (also, from Punct, AL)
const TEXT_AS: u8 = 27; // shift to Alpha (from Lower)
const TEXT_PL: u8 = 25; // latch to Punctuation (from Mixed)

/// Raw code table for the Text compaction Mixed sub-mode: `value -> ASCII byte`
/// (`0` marks a control value that is not a printable character).
const MIXED_RAW: [u8; 30] = [
    48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 38, 13, 9, 44, 58, 35, 45, 46, 36, 47, 43, 37, 42, 61,
    94, 0, 32, 0, 0, 0,
];

/// Raw code table for the Text compaction Punctuation sub-mode: `value -> ASCII byte`.
const PUNCT_RAW: [u8; 30] = [
    59, 60, 62, 64, 91, 92, 93, 95, 96, 126, 33, 13, 9, 44, 58, 10, 45, 46, 36, 47, 34, 124, 42,
    40, 41, 63, 123, 125, 39, 0,
];

/// Text compaction sub-mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubMode {
    Alpha,
    Lower,
    Mixed,
    Punct,
    AlphaShift,
    PunctShift,
}

fn is_alpha_upper(c: u8) -> bool {
    c == b' ' || c.is_ascii_uppercase()
}

fn is_alpha_lower(c: u8) -> bool {
    c == b' ' || c.is_ascii_lowercase()
}

fn mixed_value(c: u8) -> Option<u8> {
    (0..25)
        .chain(core::iter::once(26))
        .find(|&v| MIXED_RAW[v as usize] == c)
}

fn punct_value(c: u8) -> Option<u8> {
    (0u8..29).find(|&v| PUNCT_RAW[v as usize] == c)
}

fn is_mixed(c: u8) -> bool {
    mixed_value(c).is_some()
}

fn is_punct(c: u8) -> bool {
    punct_value(c).is_some()
}

/// Whether a byte can be represented by Text compaction at all.
fn is_text(c: u8) -> bool {
    c == b'\t' || c == b'\n' || c == b'\r' || (32..=126).contains(&c)
}

/// Encode all data segments into the high-level codeword stream (mode latches plus
/// content), without the symbol-length descriptor or padding.
pub fn encode_segments(segments: &[Segment]) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    for seg in segments {
        if let Mode::Eci(assignment) = seg.mode {
            if !seg.data.is_empty() {
                return Err(Error::invalid_data("PDF417 ECI segment carries data"));
            }
            eci_encode(assignment, &mut out)?;
            continue;
        }
        if seg.data.is_empty() {
            return Err(Error::invalid_data("empty PDF417 segment"));
        }
        match &seg.mode {
            Mode::Numeric => {
                if !seg.data.iter().all(u8::is_ascii_digit) {
                    return Err(Error::invalid_data("non-digit in numeric segment"));
                }
                out.push(LATCH_NUMERIC);
                numeric_encode(&seg.data, &mut out);
            }
            Mode::Byte => {
                out.push(if seg.data.len().is_multiple_of(6) {
                    LATCH_BYTE_6
                } else {
                    LATCH_BYTE
                });
                byte_encode(&seg.data, &mut out);
            }
            Mode::Alphanumeric => {
                out.push(LATCH_TEXT);
                text_encode(&seg.data, &mut out)?;
            }
            Mode::Kanji => {
                return Err(Error::Unsupported {
                    what: "PDF417 Kanji compaction",
                });
            }
            Mode::Eci(_) => unreachable!("ECI segments are handled above"),
        }
    }
    Ok(out)
}

/// Append the ECI escape sequence for `assignment` (ISO/IEC 15438 §5.5.2).
fn eci_encode(assignment: u32, out: &mut Vec<u32>) -> Result<()> {
    match assignment {
        0..=899 => out.extend_from_slice(&[ECI_CHARSET, assignment]),
        900..=810_899 => {
            out.extend_from_slice(&[ECI_GENERAL, assignment / 900 - 1, assignment % 900])
        }
        810_900..=ECI_MAX => out.extend_from_slice(&[ECI_USER, assignment - 810_900]),
        _ => {
            return Err(Error::invalid_data(
                "PDF417 ECI assignment number exceeds 811799",
            ));
        }
    }
    Ok(())
}

/// Read the ECI escape at `cws[i]`, returning the assignment number and the index
/// just past the sequence.
fn eci_decode(cws: &[u32], i: usize) -> Result<(u32, usize)> {
    let operand = |k: usize| match cws.get(i + k) {
        Some(&cw) if cw < 900 => Ok(cw),
        _ => Err(Error::undecodable("invalid PDF417 ECI escape sequence")),
    };
    Ok(match cws[i] {
        ECI_CHARSET => (operand(1)?, i + 2),
        ECI_GENERAL => ((operand(1)? + 1) * 900 + operand(2)?, i + 3),
        _ => (810_900 + operand(1)?, i + 2),
    })
}

/// Parse corrected data codewords (with the symbol-length descriptor at index 0)
/// back into segments, dropping trailing pad codewords.
pub fn decode_segments(data: &[u32]) -> Result<Vec<Segment>> {
    if data.is_empty() {
        return Err(Error::undecodable("empty PDF417 data"));
    }
    let n = data[0] as usize;
    if n == 0 || n > data.len() {
        return Err(Error::undecodable("bad PDF417 symbol length descriptor"));
    }
    let cws = &data[1..n];

    let mut segments = Vec::new();
    // The compaction mode in force (its latch codeword) and the Text sub-mode state.
    // Text/Alpha is the default; both persist across an ECI escape, so content that
    // follows one without a new latch continues where the run before it stopped.
    let mut mode = LATCH_TEXT;
    let mut text = TextState::default();
    let mut i = 0;
    while i < cws.len() {
        let cw = cws[i];
        match cw {
            LATCH_TEXT => {
                // An empty text run is padding and yields no segment.
                mode = cw;
                text = TextState::default();
                i = text_run_decode(cws, i + 1, &mut segments, &mut text)?;
            }
            LATCH_NUMERIC | LATCH_BYTE | LATCH_BYTE_6 => {
                mode = cw;
                i = plain_run_decode(cws, i + 1, mode, &mut segments)?;
            }
            ECI_CHARSET | ECI_GENERAL | ECI_USER => {
                let (assignment, next) = eci_decode(cws, i)?;
                segments.push(Segment::eci(assignment));
                i = next;
            }
            // Content with no latch of its own: the mode in force takes it.
            _ if mode == LATCH_TEXT && (cw < 900 || cw == SHIFT_BYTE) => {
                i = text_run_decode(cws, i, &mut segments, &mut text)?;
            }
            _ if cw < 900 => {
                i = plain_run_decode(cws, i, mode, &mut segments)?;
            }
            _ => {
                // A ≥900 control codeword we do not handle.
                return Err(Error::undecodable("unsupported PDF417 mode codeword"));
            }
        }
    }
    Ok(segments)
}

/// Decode one Numeric or Byte compaction run (per `mode`, its latch codeword)
/// starting at `cws[i]`, pushing its segment and returning the index of the
/// codeword that ends it.
fn plain_run_decode(
    cws: &[u32],
    mut i: usize,
    mode: u32,
    segments: &mut Vec<Segment>,
) -> Result<usize> {
    let start = i;
    while i < cws.len() && cws[i] < 900 {
        i += 1;
    }
    let run = &cws[start..i];
    if mode == LATCH_NUMERIC {
        let digits = numeric_decode(run)?;
        if !digits.is_empty() {
            segments.push(Segment::numeric(digits));
        }
    } else {
        let bytes = byte_decode(run, mode)?;
        if !bytes.is_empty() {
            segments.push(Segment::byte(bytes));
        }
    }
    Ok(i)
}

/// Text compaction sub-mode state carried from one run to the next.
#[derive(Debug, Clone, Copy)]
struct TextState {
    submode: SubMode,
    /// The sub-mode a pending shift returns to.
    prior: SubMode,
}

impl Default for TextState {
    fn default() -> Self {
        TextState {
            submode: SubMode::Alpha,
            prior: SubMode::Alpha,
        }
    }
}

// ---------------------------------------------------------------------------
// Text compaction
// ---------------------------------------------------------------------------

/// Convert text bytes into the sequence of 0..29 sub-mode values.
fn text_values(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut submode = SubMode::Alpha;
    let mut i = 0;
    while i < data.len() {
        let ch = data[i];
        if !is_text(ch) {
            return Err(Error::invalid_data(
                "character not representable in PDF417 text",
            ));
        }
        let mut advance = true;
        match submode {
            SubMode::Alpha => {
                if is_alpha_upper(ch) {
                    out.push(if ch == b' ' { 26 } else { ch - b'A' });
                } else if is_alpha_lower(ch) {
                    submode = SubMode::Lower;
                    out.push(TEXT_LL);
                    advance = false;
                } else if is_mixed(ch) {
                    submode = SubMode::Mixed;
                    out.push(TEXT_ML);
                    advance = false;
                } else {
                    out.push(TEXT_PS);
                    out.push(punct_value(ch).unwrap());
                }
            }
            SubMode::Lower => {
                if is_alpha_lower(ch) {
                    out.push(if ch == b' ' { 26 } else { ch - b'a' });
                } else if is_alpha_upper(ch) {
                    out.push(TEXT_AS);
                    out.push(ch - b'A');
                } else if is_mixed(ch) {
                    submode = SubMode::Mixed;
                    out.push(TEXT_ML);
                    advance = false;
                } else {
                    out.push(TEXT_PS);
                    out.push(punct_value(ch).unwrap());
                }
            }
            SubMode::Mixed => {
                if let Some(v) = mixed_value(ch) {
                    out.push(v);
                } else if is_alpha_upper(ch) {
                    submode = SubMode::Alpha;
                    out.push(TEXT_ML); // 28 = AL when in Mixed
                    advance = false;
                } else if is_alpha_lower(ch) {
                    submode = SubMode::Lower;
                    out.push(TEXT_LL);
                    advance = false;
                } else if i + 1 < data.len() && is_punct(data[i + 1]) {
                    submode = SubMode::Punct;
                    out.push(TEXT_PL);
                    advance = false;
                } else {
                    out.push(TEXT_PS);
                    out.push(punct_value(ch).unwrap());
                }
            }
            SubMode::Punct => {
                if let Some(v) = punct_value(ch) {
                    out.push(v);
                } else {
                    submode = SubMode::Alpha;
                    out.push(TEXT_PS); // 29 = AL when in Punct
                    advance = false;
                }
            }
            SubMode::AlphaShift | SubMode::PunctShift => {
                unreachable!("shift states are decode-only")
            }
        }
        if advance {
            i += 1;
        }
    }
    Ok(out)
}

fn text_encode(data: &[u8], out: &mut Vec<u32>) -> Result<()> {
    let values = text_values(data)?;
    let mut h = 0u32;
    for (idx, &v) in values.iter().enumerate() {
        if idx % 2 == 1 {
            out.push(h * 30 + v as u32);
        } else {
            h = v as u32;
        }
    }
    if values.len() % 2 == 1 {
        out.push(h * 30 + TEXT_PS as u32);
    }
    Ok(())
}

/// Decode one Text compaction run starting at `cws[i]`, pushing its segments and
/// returning the index of the codeword that ends it (a mode latch, an escape, or the
/// end). `state` is the sub-mode to start in and receives the one the run ends in.
///
/// A 913 "shift to Byte" inside the run yields a one-byte [`Mode::Byte`] segment
/// between the surrounding text; the Text sub-mode carries across it. A sub-mode
/// shift left pending where the run is interrupted or ends (the odd-length pad) is
/// dropped.
fn text_run_decode(
    cws: &[u32],
    mut i: usize,
    segments: &mut Vec<Segment>,
    state: &mut TextState,
) -> Result<usize> {
    loop {
        let start = i;
        while i < cws.len() && cws[i] < 900 {
            i += 1;
        }
        let bytes = text_decode(&cws[start..i], &mut state.submode, &mut state.prior);
        if !bytes.is_empty() {
            segments.push(Segment::alphanumeric(bytes));
        }
        if matches!(state.submode, SubMode::AlphaShift | SubMode::PunctShift) {
            state.submode = state.prior;
        }
        if i >= cws.len() || cws[i] != SHIFT_BYTE {
            return Ok(i);
        }
        match cws.get(i + 1) {
            Some(&b) if b < 256 => segments.push(Segment::byte(vec![b as u8])),
            _ => return Err(Error::undecodable("invalid PDF417 byte shift")),
        }
        i += 2;
    }
}

/// Decode Text compaction codewords, continuing from (and updating) the given
/// sub-mode state.
fn text_decode(cws: &[u32], submode_state: &mut SubMode, prior_state: &mut SubMode) -> Vec<u8> {
    // Unpack each codeword into two sub-mode values.
    let mut values = Vec::with_capacity(cws.len() * 2);
    for &cw in cws {
        values.push((cw / 30) as u8);
        values.push((cw % 30) as u8);
    }

    let mut out = Vec::new();
    let mut submode = *submode_state;
    let mut prior = *prior_state;
    for &v in &values {
        match submode {
            SubMode::Alpha => {
                if v < 26 {
                    out.push(b'A' + v);
                } else {
                    match v {
                        26 => out.push(b' '),
                        27 => submode = SubMode::Lower,
                        28 => submode = SubMode::Mixed,
                        29 => {
                            prior = SubMode::Alpha;
                            submode = SubMode::PunctShift;
                        }
                        _ => {}
                    }
                }
            }
            SubMode::Lower => {
                if v < 26 {
                    out.push(b'a' + v);
                } else {
                    match v {
                        26 => out.push(b' '),
                        27 => {
                            prior = SubMode::Lower;
                            submode = SubMode::AlphaShift;
                        }
                        28 => submode = SubMode::Mixed,
                        29 => {
                            prior = SubMode::Lower;
                            submode = SubMode::PunctShift;
                        }
                        _ => {}
                    }
                }
            }
            SubMode::Mixed => {
                if v < 25 {
                    out.push(MIXED_RAW[v as usize]);
                } else {
                    match v {
                        25 => submode = SubMode::Punct,
                        26 => out.push(b' '),
                        27 => submode = SubMode::Lower,
                        28 => submode = SubMode::Alpha,
                        29 => {
                            prior = SubMode::Mixed;
                            submode = SubMode::PunctShift;
                        }
                        _ => {}
                    }
                }
            }
            SubMode::Punct => {
                if v < 29 {
                    out.push(PUNCT_RAW[v as usize]);
                } else {
                    submode = SubMode::Alpha; // v == 29 (AL)
                }
            }
            SubMode::AlphaShift => {
                submode = prior;
                if v < 26 {
                    out.push(b'A' + v);
                } else if v == 26 {
                    out.push(b' ');
                }
            }
            SubMode::PunctShift => {
                submode = prior;
                if v < 29 {
                    out.push(PUNCT_RAW[v as usize]);
                }
                // v == 29 here is padding; emit nothing.
            }
        }
    }
    *submode_state = submode;
    *prior_state = prior;
    out
}

// ---------------------------------------------------------------------------
// Byte compaction
// ---------------------------------------------------------------------------

fn byte_encode(bytes: &[u8], out: &mut Vec<u32>) {
    let full = bytes.len() / 6;
    for g in 0..full {
        let chunk = &bytes[g * 6..g * 6 + 6];
        let mut t: u64 = 0;
        for &b in chunk {
            t = (t << 8) | b as u64;
        }
        let mut digits = [0u32; 5];
        for slot in digits.iter_mut() {
            *slot = (t % 900) as u32;
            t /= 900;
        }
        for &d in digits.iter().rev() {
            out.push(d);
        }
    }
    for &b in &bytes[full * 6..] {
        out.push(b as u32);
    }
}

/// Invert Byte compaction. A five-codeword group is a base-900 number below 256^6
/// and a lone codeword is one byte; anything larger is not Byte compaction output
/// and is rejected rather than truncated to fit.
fn byte_decode(slice: &[u32], mode: u32) -> Result<Vec<u8>> {
    let len = slice.len();
    if len == 0 {
        return Ok(Vec::new());
    }
    let bad = || Error::undecodable("invalid PDF417 byte compaction codeword");
    let groups = if mode == LATCH_BYTE_6 {
        len / 5
    } else {
        (len - 1) / 5
    };
    let mut out = Vec::new();
    for g in 0..groups {
        let cws = &slice[g * 5..g * 5 + 5];
        let mut v: u64 = 0;
        for &c in cws {
            v = v * 900 + c as u64;
        }
        if v >> 48 != 0 {
            return Err(bad());
        }
        for i in 0..6 {
            out.push((v >> (8 * (5 - i))) as u8);
        }
    }
    for &c in &slice[groups * 5..] {
        out.push(u8::try_from(c).map_err(|_| bad())?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Numeric compaction
// ---------------------------------------------------------------------------

const MAX_NUMERIC_CODEWORDS: usize = 15;
const MAX_NUMERIC_DIGITS: usize = 44;

fn numeric_encode(digits: &[u8], out: &mut Vec<u32>) {
    let mut idx = 0;
    while idx < digits.len() {
        let len = (digits.len() - idx).min(MAX_NUMERIC_DIGITS);
        // Prepend a leading "1" to preserve leading zeros, then convert to base 900.
        let mut dec = Vec::with_capacity(len + 1);
        dec.push(1u32);
        for &d in &digits[idx..idx + len] {
            dec.push((d - b'0') as u32);
        }
        for cw in decimal_to_base900(dec) {
            out.push(cw as u32);
        }
        idx += len;
    }
}

fn numeric_decode(slice: &[u32]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for chunk in slice.chunks(MAX_NUMERIC_CODEWORDS) {
        let dec = base900_to_decimal(chunk);
        // The reconstructed number must begin with the sentinel '1'.
        if dec.first() != Some(&1) {
            return Err(Error::undecodable("invalid numeric compaction group"));
        }
        for &d in &dec[1..] {
            out.push(b'0' + d as u8);
        }
    }
    Ok(out)
}

/// Convert a base-10 number (digits most-significant first, each `0..=9`) into
/// base-900 codewords, most-significant first.
fn decimal_to_base900(dec: Vec<u32>) -> Vec<u16> {
    let mut cur = dec;
    let mut base900_lsf = Vec::new();
    loop {
        let mut rem = 0u32;
        let mut quotient = Vec::with_capacity(cur.len());
        for &d in &cur {
            let acc = rem * 10 + d;
            quotient.push(acc / 900);
            rem = acc % 900;
        }
        base900_lsf.push(rem as u16);
        // Strip leading zeros from the quotient.
        let first = quotient
            .iter()
            .position(|&x| x != 0)
            .unwrap_or(quotient.len());
        cur = quotient[first..].to_vec();
        if cur.is_empty() {
            break;
        }
    }
    base900_lsf.reverse();
    base900_lsf
}

/// Convert base-900 codewords (most-significant first) into base-10 digits
/// (most-significant first, each `0..=9`).
fn base900_to_decimal(cws: &[u32]) -> Vec<u32> {
    let mut dec = vec![0u32];
    for &c in cws {
        mul_add(&mut dec, 900, c);
    }
    dec
}

/// `dec ← dec * mul + add`, where `dec` is base-10 digits most-significant first.
fn mul_add(dec: &mut Vec<u32>, mul: u32, add: u32) {
    let mut carry = add;
    for d in dec.iter_mut().rev() {
        let acc = *d * mul + carry;
        *d = acc % 10;
        carry = acc / 10;
    }
    while carry > 0 {
        dec.insert(0, carry % 10);
        carry /= 10;
    }
    while dec.len() > 1 && dec[0] == 0 {
        dec.remove(0);
    }
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    /// Independent reference vectors from ZXing's `PDF417EncoderTestCase`
    /// (which follows ISO/IEC 15438 Annex P):
    /// text "ABCD" → 1, 63; numeric "1234" → 902, 12, 434; byte "abcd" →
    /// 901, 97, 98, 99, 100.
    #[test]
    fn reference_vectors() {
        assert_eq!(
            encode_segments(&[Segment::alphanumeric(b"ABCD".to_vec())]).unwrap(),
            vec![900, 1, 63]
        );
        assert_eq!(
            encode_segments(&[Segment::numeric(b"1234".to_vec())]).unwrap(),
            vec![902, 12, 434]
        );
        assert_eq!(
            encode_segments(&[Segment::byte(b"abcd".to_vec())]).unwrap(),
            vec![901, 97, 98, 99, 100]
        );
    }

    /// zint 2.16.0's data codewords for "abc\xE9def": the 913 byte shift interrupts
    /// the text run without resetting the Lower sub-mode.
    #[test]
    fn byte_shift_keeps_text_submode() {
        let segs = decode_segments(&[7, 810, 32, 913, 233, 94, 179]).unwrap();
        assert_eq!(
            segs,
            vec![
                Segment::alphanumeric(b"abc".to_vec()),
                Segment::byte(vec![0xe9]),
                Segment::alphanumeric(b"def".to_vec()),
            ]
        );
        // A dangling or out-of-range byte shift is malformed.
        assert!(decode_segments(&[3, 810, 913]).is_err());
        assert!(decode_segments(&[4, 810, 913, 256]).is_err());
    }

    /// Byte compaction carries six bytes in five codewords (a base-900 number below
    /// 256^6) and leftover bytes one per codeword (0..=255). Codewords outside those
    /// ranges are not bytes; they must be refused, not truncated into some.
    #[test]
    fn byte_compaction_rejects_out_of_range_codewords() {
        // A lone "byte" codeword of 300 (was read as 300 mod 256 = 44).
        assert!(decode_segments(&[3, 901, 300]).is_err());
        assert!(decode_segments(&[4, 901, 65, 899]).is_err());
        // Five codewords whose base-900 value needs more than 48 bits (was read
        // with the high bits dropped), under either latch.
        assert!(decode_segments(&[7, 924, 899, 899, 899, 899, 899]).is_err());
        assert!(decode_segments(&[8, 901, 899, 899, 899, 899, 899, 65]).is_err());
        // The largest valid group, 256^6 - 1, still decodes.
        assert_eq!(
            decode_segments(&[7, 924, 429, 11, 71, 222, 855]).unwrap(),
            vec![Segment::byte(vec![0xff; 6])]
        );
        assert_eq!(
            encode_segments(&[Segment::byte(vec![0xff; 6])]).unwrap(),
            vec![924, 429, 11, 71, 222, 855]
        );
        // One past it does not.
        assert!(decode_segments(&[7, 924, 429, 11, 71, 222, 856]).is_err());
    }

    /// ECI escapes: 927 for assignments up to 899, 926 for up to 810 899 (two
    /// codewords), 925 above. zint 2.16.0 reference streams
    /// (`zint -b PDF417 --eci=26 -d é`, and `--eci=811799 -d 123456789012345
    /// --seg1=810900,98765432109876`).
    #[test]
    fn eci_escapes_match_zint() {
        let segs = decode_segments(&[6, 927, 26, 901, 195, 169]).unwrap();
        assert_eq!(
            segs,
            vec![Segment::eci(26), Segment::byte(vec![0xc3, 0xa9])]
        );
        assert_eq!(encode_segments(&segs).unwrap(), [927, 26, 901, 195, 169]);

        let stream = [
            925, 899, 902, 1, 812, 292, 890, 124, 745, 925, 0, 902, 302, 854, 822, 322, 76,
        ];
        let segs = vec![
            Segment::eci(811_799),
            Segment::numeric(b"123456789012345".to_vec()),
            Segment::eci(810_900),
            Segment::numeric(b"98765432109876".to_vec()),
        ];
        assert_eq!(encode_segments(&segs).unwrap(), stream);
        let mut data = vec![stream.len() as u32 + 1];
        data.extend_from_slice(&stream);
        assert_eq!(decode_segments(&data).unwrap(), segs);

        // Every range boundary survives, and nothing past the last one encodes.
        for n in [0, 3, 899, 900, 901, 1799, 1800, 810_899, 810_900, 811_799] {
            let stream = encode_segments(&[Segment::eci(n)]).unwrap();
            let mut data = vec![stream.len() as u32 + 1];
            data.extend_from_slice(&stream);
            assert_eq!(
                decode_segments(&data).unwrap(),
                vec![Segment::eci(n)],
                "{n}"
            );
        }
        assert_eq!(encode_segments(&[Segment::eci(900)]).unwrap(), [926, 0, 0]);
        assert_eq!(
            encode_segments(&[Segment::eci(810_899)]).unwrap(),
            [926, 899, 899]
        );
        assert!(encode_segments(&[Segment::eci(811_800)]).is_err());
        assert!(encode_segments(&[Segment::eci(u32::MAX)]).is_err());
        // A truncated or out-of-range escape is malformed.
        assert!(decode_segments(&[2, 927]).is_err());
        assert!(decode_segments(&[3, 927, 900]).is_err());
        assert!(decode_segments(&[3, 926, 5]).is_err());
        assert!(decode_segments(&[2, 925]).is_err());
    }

    /// Data that carries on after an ECI escape without a new latch stays in the
    /// compaction mode — and Text sub-mode — that was in force. zint 2.16.0,
    /// `zint -b PDF417 --eci=3 -d abc --seg1=899,de7 --seg2=900,XYZ`: `de` follows
    /// ECI 899 still in the Lower sub-mode, `XYZ` follows ECI 900 from Mixed.
    #[test]
    fn mode_and_submode_persist_across_eci() {
        let segs = decode_segments(&[14, 927, 3, 810, 32, 927, 899, 94, 847, 926, 0, 0, 863, 745])
            .unwrap();
        assert_eq!(
            segs,
            vec![
                Segment::eci(3),
                Segment::alphanumeric(b"abc".to_vec()),
                Segment::eci(899),
                Segment::alphanumeric(b"de7".to_vec()),
                Segment::eci(900),
                Segment::alphanumeric(b"XYZ".to_vec()),
            ]
        );
        // The same for Byte and Numeric compaction: no re-latch needed.
        let segs = decode_segments(&[8, 901, 65, 66, 927, 4, 67, 68]).unwrap();
        assert_eq!(
            segs,
            vec![
                Segment::byte(b"AB".to_vec()),
                Segment::eci(4),
                Segment::byte(b"CD".to_vec()),
            ]
        );
        let segs = decode_segments(&[8, 902, 12, 434, 927, 4, 12, 434]).unwrap();
        assert_eq!(
            segs,
            vec![
                Segment::numeric(b"1234".to_vec()),
                Segment::eci(4),
                Segment::numeric(b"1234".to_vec()),
            ]
        );
        // A latch after the escape still resets Text compaction to Alpha.
        let segs = decode_segments(&[8, 810, 32, 927, 4, 900, 1, 63]).unwrap();
        assert_eq!(
            segs,
            vec![
                Segment::alphanumeric(b"abc".to_vec()),
                Segment::eci(4),
                Segment::alphanumeric(b"ABCD".to_vec()),
            ]
        );
    }

    /// Garbage codeword streams must decode or fail cleanly, never panic.
    #[test]
    fn garbage_streams_never_panic() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let len = 1 + (next() % 60) as usize;
            let mut data: Vec<u32> = (0..len)
                .map(|_| {
                    // Bias towards the mode latches so every branch is reached.
                    match next() % 8 {
                        0 => 900 + (next() % 29) as u32,
                        1 => [900, 901, 902, 913, 924, 925, 926, 927][(next() % 8) as usize],
                        _ => (next() % 900) as u32,
                    }
                })
                .collect();
            if next() % 4 != 0 {
                data[0] = len as u32;
            }
            let _ = decode_segments(&data);
        }
    }

    fn roundtrip(seg: Segment) {
        let stream = encode_segments(core::slice::from_ref(&seg)).unwrap();
        // Simulate a symbol-length descriptor for decode_segments.
        let mut data = vec![(stream.len() + 1) as u32];
        data.extend_from_slice(&stream);
        let segs = decode_segments(&data).unwrap();
        assert_eq!(segs, vec![seg]);
    }

    #[test]
    fn text_roundtrips() {
        roundtrip(Segment::alphanumeric(b"HELLO world 123".to_vec()));
        roundtrip(Segment::alphanumeric(b"Mix3d: $tuff! (a) [b] {c}".to_vec()));
        roundtrip(Segment::alphanumeric(b"ABC".to_vec()));
        roundtrip(Segment::alphanumeric(b"a".to_vec()));
    }

    /// Every ordered pair of text bytes, entered from each sub-mode, survives: this
    /// walks all latch / shift transitions and both pad parities.
    #[test]
    fn text_roundtrips_every_transition() {
        let text: Vec<u8> = (0u8..=127).filter(|&c| is_text(c)).collect();
        for prefix in [&b""[..], b"a", b"1", b"1;;"] {
            for &a in &text {
                for &b in &text {
                    let mut data = prefix.to_vec();
                    data.extend_from_slice(&[a, b]);
                    roundtrip(Segment::alphanumeric(data.clone()));
                    data.push(b'Z');
                    roundtrip(Segment::alphanumeric(data));
                }
            }
        }
    }

    #[test]
    fn byte_roundtrips_all_remainders() {
        for len in 1..=20usize {
            let data: Vec<u8> = (0..len)
                .map(|i| (i as u8).wrapping_mul(37).wrapping_add(3))
                .collect();
            roundtrip(Segment::byte(data));
        }
    }

    #[test]
    fn numeric_roundtrips_long() {
        roundtrip(Segment::numeric(b"1234".to_vec()));
        roundtrip(Segment::numeric(b"000213298174000".to_vec()));
        let long: Vec<u8> = (0..130).map(|i| b'0' + (i % 10) as u8).collect();
        roundtrip(Segment::numeric(long));
    }
}
