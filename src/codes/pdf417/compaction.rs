//! PDF417 high-level data compaction (ISO/IEC 15438 §5.4 / Annex P) and its inverse.
//!
//! Each data [`Segment`] maps to one compaction mode preceded by an explicit mode
//! latch, so segment boundaries survive decoding:
//! - [`Mode::Numeric`] → Numeric compaction (latch 902),
//! - [`Mode::Byte`] → Byte compaction (latch 901, or 924 when the length is a
//!   multiple of six),
//! - [`Mode::Alphanumeric`] → Text compaction (latch 900), covering the full
//!   printable-ASCII set plus HT/LF/CR via the Alpha/Lower/Mixed/Punctuation
//!   sub-modes.
//!
//! Content codewords are always below 900; only mode latches and padding are ≥ 900,
//! which is what lets the decoder split the stream back into segments.

use crate::error::{Error, Result};
use crate::segment::{Mode, Segment};

const LATCH_TEXT: u32 = 900;
const LATCH_BYTE: u32 = 901;
const LATCH_NUMERIC: u32 = 902;
const LATCH_BYTE_6: u32 = 924;

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
        .chain(std::iter::once(26))
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
            Mode::Eci(_) => {
                return Err(Error::Unsupported {
                    what: "PDF417 ECI segments",
                });
            }
        }
    }
    Ok(out)
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
    let mut i = 0;
    while i < cws.len() {
        let latch = cws[i];
        match latch {
            LATCH_TEXT => {
                i += 1;
                let start = i;
                while i < cws.len() && cws[i] < 900 {
                    i += 1;
                }
                let bytes = text_decode(&cws[start..i]);
                if !bytes.is_empty() {
                    segments.push(Segment::alphanumeric(bytes));
                }
                // else: an empty text run is padding — skip it.
            }
            LATCH_NUMERIC => {
                i += 1;
                let start = i;
                while i < cws.len() && cws[i] < 900 {
                    i += 1;
                }
                let digits = numeric_decode(&cws[start..i])?;
                if !digits.is_empty() {
                    segments.push(Segment::numeric(digits));
                }
            }
            LATCH_BYTE | LATCH_BYTE_6 => {
                i += 1;
                let start = i;
                while i < cws.len() && cws[i] < 900 {
                    i += 1;
                }
                let bytes = byte_decode(&cws[start..i], latch);
                if !bytes.is_empty() {
                    segments.push(Segment::byte(bytes));
                }
            }
            _ => {
                // A content codeword with no active latch (or an unsupported control
                // codeword). The default PDF417 mode is Text; treat the run as text.
                let start = i;
                while i < cws.len() && cws[i] < 900 {
                    i += 1;
                }
                if i == start {
                    // A ≥900 codeword we do not handle; stop to avoid looping.
                    return Err(Error::undecodable("unsupported PDF417 mode codeword"));
                }
                let bytes = text_decode(&cws[start..i]);
                if !bytes.is_empty() {
                    segments.push(Segment::alphanumeric(bytes));
                }
            }
        }
    }
    Ok(segments)
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

fn text_decode(cws: &[u32]) -> Vec<u8> {
    // Unpack each codeword into two sub-mode values.
    let mut values = Vec::with_capacity(cws.len() * 2);
    for &cw in cws {
        values.push((cw / 30) as u8);
        values.push((cw % 30) as u8);
    }

    let mut out = Vec::new();
    let mut submode = SubMode::Alpha;
    let mut prior = SubMode::Alpha;
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

fn byte_decode(slice: &[u32], mode: u32) -> Vec<u8> {
    let len = slice.len();
    if len == 0 {
        return Vec::new();
    }
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
        for i in 0..6 {
            out.push((v >> (8 * (5 - i))) as u8);
        }
    }
    for &c in &slice[groups * 5..] {
        out.push(c as u8);
    }
    out
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

#[cfg(test)]
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

    fn roundtrip(seg: Segment) {
        let stream = encode_segments(std::slice::from_ref(&seg)).unwrap();
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
