//! Japan Post 4-State Customer Barcode ("Kasutama" / Customer Barcode).
//!
//! The payload is the (upper-cased) address string over digits, `-` and `A-Z`.
//! It is first mapped to a fixed field of 20 *intermediate* characters over the
//! alphabet `1234567890-abcdefgh` (`KASUTSET`): digits and `-` pass through,
//! each letter expands to a control character (`a`/`b`/`c` for `A-J`/`K-T`/`U-Z`)
//! plus a digit, and the field is right-padded with `d`. A mod-19 check
//! character is appended.
//!
//! Each of the 21 characters is drawn as three bars via `JAPAN_TABLE`, framed by
//! a start pair `Full, Descender` and a stop pair `Descender, Full`, for a fixed
//! 67-bar symbol. Bar values use the convention `0 = Full`, `1 = Ascender`,
//! `2 = Descender`, `3 = Tracker`.
//!
//! Reference: verified against zint's `test_postal` Japan Post cases (Zip/Barcode
//! Manual p.6 examples `15400233-16-4-205` and `350110622-1A308`), see
//! `tests/postal_extra.rs`.

use super::{BarState, PostalVariant};
use crate::error::{Error, Result};

/// Three bar values per intermediate character, indexed by position in
/// [`KASUTSET`].
const JAPAN_TABLE: [[u8; 3]; 19] = [
    [0, 0, 3],
    [0, 2, 1],
    [2, 0, 1],
    [0, 1, 2],
    [0, 3, 0],
    [2, 1, 0],
    [1, 0, 2],
    [1, 2, 0],
    [3, 0, 0],
    [0, 3, 3],
    [3, 0, 3],
    [2, 1, 3],
    [2, 3, 1],
    [1, 2, 3],
    [3, 2, 1],
    [1, 3, 2],
    [3, 1, 2],
    [3, 3, 0],
    [0, 0, 0],
];

/// The encoding alphabet (position indexes [`JAPAN_TABLE`]).
const KASUTSET: &[u8; 19] = b"1234567890-abcdefgh";

/// The checksum alphabet (position gives each character's checksum weight).
const CHKASUTSET: &[u8; 19] = b"0123456789-abcdefgh";

/// Start pair (`Full, Descender`).
const START: [u8; 2] = [0, 2];
/// Stop pair (`Descender, Full`).
const STOP: [u8; 2] = [2, 0];

/// A bar value `0..=3` as a [`BarState`].
fn value_to_bar(v: u8) -> BarState {
    match v {
        0 => BarState::Full,
        1 => BarState::Ascender,
        2 => BarState::Descender,
        _ => BarState::Tracker,
    }
}

/// A [`BarState`] as a bar value `0..=3`.
fn bar_to_value(b: BarState) -> u8 {
    match b {
        BarState::Full => 0,
        BarState::Ascender => 1,
        BarState::Descender => 2,
        BarState::Tracker => 3,
    }
}

/// Position of a byte in [`KASUTSET`].
fn kasut_index(byte: u8) -> Option<usize> {
    KASUTSET.iter().position(|&c| c == byte)
}

/// Position of a byte in [`CHKASUTSET`].
fn chkasut_index(byte: u8) -> Option<usize> {
    CHKASUTSET.iter().position(|&c| c == byte)
}

/// Validate the source: digits, `-` and upper-case `A-Z`, non-empty, and short
/// enough that its intermediate expansion fits in 20 characters.
pub(super) fn validate(source: &[u8]) -> Result<Vec<u8>> {
    if source.is_empty() {
        return Err(Error::invalid_data("Japan Post data must not be empty"));
    }
    if source.len() > 20 {
        return Err(Error::capacity("Japan Post input too long (maximum 20)"));
    }
    for &b in source {
        if !(b.is_ascii_digit() || b == b'-' || b.is_ascii_uppercase()) {
            return Err(Error::invalid_data(
                "Japan Post data must be digits, '-' or A-Z (upper-case)",
            ));
        }
    }
    intermediate(source)
}

/// Build the 20-character intermediate field from the source, erroring if it
/// does not fit (matching zint's overflow check).
fn intermediate(source: &[u8]) -> Result<Vec<u8>> {
    let mut inter = vec![b'd'; 20];
    let mut pos = 0usize;
    for &c in source {
        if pos >= 20 {
            return Err(Error::capacity(
                "Japan Post input requires too many symbol characters (maximum 20)",
            ));
        }
        if c.is_ascii_digit() || c == b'-' {
            inter[pos] = c;
            pos += 1;
        } else {
            if pos + 1 >= 20 {
                return Err(Error::capacity(
                    "Japan Post input requires too many symbol characters (maximum 20)",
                ));
            }
            let (ctrl, digit) = if c <= b'J' {
                (b'a', c - b'A')
            } else if c <= b'T' {
                (b'b', c - b'K')
            } else {
                (b'c', c - b'U')
            };
            inter[pos] = ctrl;
            inter[pos + 1] = b'0' + digit;
            pos += 2;
        }
    }
    Ok(inter)
}

/// The mod-19 check character for a 20-character intermediate field.
fn check_char(inter: &[u8]) -> u8 {
    let sum: usize = inter.iter().map(|&c| chkasut_index(c).unwrap()).sum();
    let mut check = 19 - (sum % 19);
    if check == 19 {
        check = 0;
    }
    if check <= 9 {
        b'0' + check as u8
    } else if check == 10 {
        b'-'
    } else {
        b'a' + (check - 11) as u8
    }
}

/// Encode a validated source string into the 67-bar sequence.
pub(super) fn encode(source: &[u8]) -> Result<Vec<BarState>> {
    let inter = validate(source)?;
    let mut dest: Vec<u8> = Vec::with_capacity(67);
    dest.extend_from_slice(&START);
    for &c in &inter {
        dest.extend_from_slice(&JAPAN_TABLE[kasut_index(c).unwrap()]);
    }
    let check = check_char(&inter);
    dest.extend_from_slice(&JAPAN_TABLE[kasut_index(check).unwrap()]);
    dest.extend_from_slice(&STOP);
    Ok(dest.iter().map(|&v| value_to_bar(v)).collect())
}

/// Decode a 67-bar Japan Post sequence back to the source string.
pub(super) fn decode(bars: &[BarState]) -> Result<(PostalVariant, Vec<u8>)> {
    if bars.len() != 67 {
        return Err(Error::undecodable("Japan Post must have 67 bars"));
    }
    let v: Vec<u8> = bars.iter().map(|&b| bar_to_value(b)).collect();
    if v[..2] != START || v[65..] != STOP {
        return Err(Error::undecodable("Japan Post start/stop bars invalid"));
    }
    let body = &v[2..65]; // 21 characters × 3 bars
    let mut chars = Vec::with_capacity(21);
    for tri in body.chunks_exact(3) {
        let idx = JAPAN_TABLE
            .iter()
            .position(|&e| e == [tri[0], tri[1], tri[2]])
            .ok_or_else(|| Error::undecodable("Japan Post character invalid"))?;
        chars.push(KASUTSET[idx]);
    }
    let (&check, inter) = chars
        .split_last()
        .ok_or_else(|| Error::undecodable("Japan Post missing check character"))?;
    if check_char(inter) != check {
        return Err(Error::undecodable("Japan Post checksum mismatch"));
    }

    // Reverse the intermediate field back to the source string.
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < inter.len() {
        let c = inter[i];
        if c == b'd' {
            break; // padding: the rest of the field is padding
        } else if c == b'a' || c == b'b' || c == b'c' {
            let digit = inter
                .get(i + 1)
                .filter(|d| d.is_ascii_digit())
                .map(|d| d - b'0')
                .ok_or_else(|| Error::undecodable("Japan Post control sequence invalid"))?;
            let base = match c {
                b'a' => b'A',
                b'b' => b'K',
                _ => b'U',
            };
            out.push(base + digit);
            i += 2;
        } else {
            out.push(c); // digit or '-'
            i += 1;
        }
    }
    Ok((PostalVariant::JapanPost, out))
}
