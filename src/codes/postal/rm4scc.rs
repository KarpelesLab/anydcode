//! Royal Mail RM4SCC and Dutch KIX: 4-state, four bars per character.
//!
//! Each of the 36 characters `0-9 A-Z` is encoded as four bars. The character is
//! located in a 6×6 grid whose row (top / ascender half) and column (bottom /
//! descender half) each select one of six "2-of-4" bar patterns:
//!
//! | value | pattern (bars 1-4) |
//! |-------|--------------------|
//! | 1 | `0011` | 2 | `0101` | 3 | `0110` |
//! | 4 | `1001` | 5 | `1010` | 6 | `1100` |
//!
//! A `1` in the top pattern raises an ascender on that bar; a `1` in the bottom
//! pattern drops a descender. Character index `= (top-1)*6 + (bottom-1)` over the
//! alphabet `0-9 A-Z`.
//!
//! - **RM4SCC** frames the data with a start bar (ascender) and stop bar (full),
//!   and appends a checksum character before the stop bar. The checksum's top and
//!   bottom values are the sums of the data characters' top and bottom values,
//!   each reduced mod 6 (a zero result maps to 6).
//! - **KIX** has no start bar, stop bar or checksum.
//!
//! References: RM4SCC (Royal Mail) specification; the character grid and checksum
//! match the worked example on Wikipedia / HandWiki "RM4SCC".

use super::{BarState, PostalVariant};
use crate::error::{Error, Result};

/// The 36-character alphabet, in grid order.
const ALPHABET: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// The six "2-of-4" bar patterns indexed by value `1..=6` (offset by one). Each
/// entry lists, left to right, whether that bar carries the half's extension.
const PATTERNS: [[bool; 4]; 6] = [
    [false, false, true, true], // 1 = 0011
    [false, true, false, true], // 2 = 0101
    [false, true, true, false], // 3 = 0110
    [true, false, false, true], // 4 = 1001
    [true, false, true, false], // 5 = 1010
    [true, true, false, false], // 6 = 1100
];

/// The `(top, bottom)` grid values (each `1..=6`) for a character index `0..=35`.
fn grid_values(index: u8) -> (u8, u8) {
    (index / 6 + 1, index % 6 + 1)
}

/// The character index `0..=35` for a `0-9 A-Z` byte.
fn char_index(byte: u8) -> Option<u8> {
    ALPHABET.iter().position(|&c| c == byte).map(|p| p as u8)
}

/// The four bars for a character index.
fn char_to_bars(index: u8) -> [BarState; 4] {
    let (top, bottom) = grid_values(index);
    let asc = PATTERNS[(top - 1) as usize];
    let desc = PATTERNS[(bottom - 1) as usize];
    core::array::from_fn(|i| BarState::from_parts(asc[i], desc[i]))
}

/// The value `1..=6` of a "2-of-4" pattern, or `None` if it is not one.
fn pattern_value(pattern: [bool; 4]) -> Option<u8> {
    PATTERNS
        .iter()
        .position(|&p| p == pattern)
        .map(|p| p as u8 + 1)
}

/// Decode four bars back to a character index, validating both halves.
fn bars_to_char(chunk: &[BarState]) -> Option<u8> {
    let asc: [bool; 4] = core::array::from_fn(|i| chunk[i].has_ascender());
    let desc: [bool; 4] = core::array::from_fn(|i| chunk[i].has_descender());
    let top = pattern_value(asc)?;
    let bottom = pattern_value(desc)?;
    Some((top - 1) * 6 + (bottom - 1))
}

/// Reduce a grid-value sum mod 6, mapping a zero result to 6.
fn checksum_value(sum: u32) -> u8 {
    let v = (sum % 6) as u8;
    if v == 0 { 6 } else { v }
}

/// The checksum character index for a sequence of data character indices.
fn checksum_index(indices: &[u8]) -> u8 {
    let mut top_sum = 0u32;
    let mut bottom_sum = 0u32;
    for &i in indices {
        let (t, b) = grid_values(i);
        top_sum += t as u32;
        bottom_sum += b as u32;
    }
    (checksum_value(top_sum) - 1) * 6 + (checksum_value(bottom_sum) - 1)
}

/// Validate that `data` is a non-empty `0-9 A-Z` string.
pub(super) fn validate(data: &[u8]) -> Result<()> {
    if data.is_empty() {
        return Err(Error::invalid_data("RM4SCC/KIX data must not be empty"));
    }
    if data.iter().any(|&b| char_index(b).is_none()) {
        return Err(Error::invalid_data(
            "RM4SCC/KIX data must be 0-9 or A-Z (uppercase)",
        ));
    }
    Ok(())
}

/// Encode a `0-9 A-Z` string into bars. `kix` omits framing bars and checksum.
pub(super) fn encode(kix: bool, data: &[u8]) -> Result<Vec<BarState>> {
    validate(data)?;
    let indices: Vec<u8> = data.iter().map(|&b| char_index(b).unwrap()).collect();

    let mut bars = Vec::new();
    if !kix {
        bars.push(BarState::Ascender); // start bar
    }
    for &i in &indices {
        bars.extend_from_slice(&char_to_bars(i));
    }
    if !kix {
        bars.extend_from_slice(&char_to_bars(checksum_index(&indices)));
        bars.push(BarState::Full); // stop bar
    }
    Ok(bars)
}

/// Decode an RM4SCC bar sequence into `(RoyalMail, data chars)` (checksum
/// stripped and verified).
pub(super) fn decode_rm4scc(bars: &[BarState]) -> Result<(PostalVariant, Vec<u8>)> {
    let n = bars.len();
    if n < 6 || !(n - 2).is_multiple_of(4) {
        return Err(Error::undecodable("RM4SCC bar count invalid"));
    }
    if bars[0] != BarState::Ascender {
        return Err(Error::undecodable("RM4SCC start bar must be an ascender"));
    }
    if bars[n - 1] != BarState::Full {
        return Err(Error::undecodable("RM4SCC stop bar must be a full bar"));
    }

    let body = &bars[1..n - 1];
    let indices = decode_groups(body)?;
    let (&check, data) = indices
        .split_last()
        .ok_or_else(|| Error::undecodable("RM4SCC has no checksum character"))?;
    if checksum_index(data) != check {
        return Err(Error::undecodable("RM4SCC checksum mismatch"));
    }
    Ok((PostalVariant::RoyalMail, indices_to_ascii(data)))
}

/// Decode a KIX bar sequence into `(KixCode, data chars)` (no checksum).
pub(super) fn decode_kix(bars: &[BarState]) -> Result<(PostalVariant, Vec<u8>)> {
    if bars.is_empty() || !bars.len().is_multiple_of(4) {
        return Err(Error::undecodable("KIX bar count invalid"));
    }
    let indices = decode_groups(bars)?;
    Ok((PostalVariant::KixCode, indices_to_ascii(&indices)))
}

/// Decode a run of 4-bar groups into character indices.
fn decode_groups(body: &[BarState]) -> Result<Vec<u8>> {
    body.chunks_exact(4)
        .map(|chunk| {
            bars_to_char(chunk).ok_or_else(|| Error::undecodable("invalid RM4SCC character"))
        })
        .collect()
}

/// Map character indices back to their ASCII bytes.
fn indices_to_ascii(indices: &[u8]) -> Vec<u8> {
    indices.iter().map(|&i| ALPHABET[i as usize]).collect()
}
