//! USPS POSTNET and PLANET: 2-state height-modulated codes.
//!
//! Both encode decimal digits as groups of five bars using the "2-of-5" height
//! weighting `7-4-2-1-0`, framed by a tall bar at each end and terminated by a
//! mod-10 check digit group. POSTNET marks each group with **two tall** bars;
//! PLANET uses the bitwise-complementary patterns with **three tall** bars.
//!
//! References: USPS POSTNET/PLANET specifications; the digit weighting and check
//! digit are the widely documented `7-4-2-1-0` scheme (see e.g. the USPS DMM and
//! the Wikipedia "POSTNET"/"PLANET" articles).

use super::{BarState, PostalVariant};
use crate::error::{Error, Result};

/// POSTNET digit patterns: for each digit `0..=9`, the five bars (`true` = tall)
/// under the `7-4-2-1-0` weighting. Each has exactly two tall bars.
const POSTNET: [[bool; 5]; 10] = [
    [true, true, false, false, false], // 0 = 7+4
    [false, false, false, true, true], // 1 = 1+0
    [false, false, true, false, true], // 2 = 2+0
    [false, false, true, true, false], // 3 = 2+1
    [false, true, false, false, true], // 4 = 4+0
    [false, true, false, true, false], // 5 = 4+1
    [false, true, true, false, false], // 6 = 4+2
    [true, false, false, false, true], // 7 = 7+0
    [true, false, false, true, false], // 8 = 7+1
    [true, false, true, false, false], // 9 = 7+2
];

/// A tall bar renders as an ascender, a short bar as a bare tracker.
fn bar(tall: bool) -> BarState {
    if tall {
        BarState::Ascender
    } else {
        BarState::Tracker
    }
}

/// The five-bar (`tall`) pattern for one digit under the requested variant.
/// PLANET patterns are the bitwise complement of POSTNET's.
fn pattern(digit: u8, planet: bool) -> [bool; 5] {
    let mut p = POSTNET[digit as usize];
    if planet {
        for b in &mut p {
            *b = !*b;
        }
    }
    p
}

/// Validate that `digits` are all ASCII decimal digits and non-empty.
pub(super) fn validate_digits(digits: &[u8]) -> Result<()> {
    if digits.is_empty() {
        return Err(Error::invalid_data("POSTNET/PLANET data must not be empty"));
    }
    if !digits.iter().all(u8::is_ascii_digit) {
        return Err(Error::invalid_data(
            "POSTNET/PLANET data must be ASCII digits",
        ));
    }
    Ok(())
}

/// The mod-10 check digit: the value that makes the total digit sum a multiple
/// of ten.
fn check_digit(values: &[u8]) -> u8 {
    let sum: u32 = values.iter().map(|&v| v as u32).sum();
    ((10 - (sum % 10)) % 10) as u8
}

/// Encode ZIP/data digits (ASCII, no check digit) into the bar sequence.
pub(super) fn encode(planet: bool, digits: &[u8]) -> Result<Vec<BarState>> {
    validate_digits(digits)?;
    let values: Vec<u8> = digits.iter().map(|&b| b - b'0').collect();
    let check = check_digit(&values);

    // frame + one 5-bar group per data digit + check group + frame.
    let mut bars = Vec::with_capacity(2 + 5 * (values.len() + 1));
    bars.push(bar(true)); // leading frame bar (always tall)
    for &v in values.iter().chain(std::iter::once(&check)) {
        for &tall in &pattern(v, planet) {
            bars.push(bar(tall));
        }
    }
    bars.push(bar(true)); // trailing frame bar
    Ok(bars)
}

/// Decode a POSTNET/PLANET bar sequence into `(variant, data digits)` (check
/// digit stripped). Framing bars must be tall and every bar 2-state.
pub(super) fn decode(bars: &[BarState]) -> Result<(PostalVariant, Vec<u8>)> {
    let n = bars.len();
    if n < 7 || !(n - 2).is_multiple_of(5) {
        return Err(Error::undecodable("POSTNET/PLANET bar count invalid"));
    }
    if bars.iter().any(|b| b.has_descender()) {
        return Err(Error::undecodable("POSTNET/PLANET bars must be 2-state"));
    }
    if bars[0] != BarState::Ascender || bars[n - 1] != BarState::Ascender {
        return Err(Error::undecodable(
            "POSTNET/PLANET framing bars must be tall",
        ));
    }

    let groups = &bars[1..n - 1];
    // Try POSTNET first, then PLANET; the two-tall vs three-tall patterns are
    // disjoint, so at most one interpretation validates.
    for planet in [false, true] {
        if let Some(values) = decode_groups(groups, planet)
            && check_digit(&values[..values.len() - 1]) == values[values.len() - 1]
        {
            let variant = if planet {
                PostalVariant::Planet
            } else {
                PostalVariant::Postnet
            };
            let digits = values[..values.len() - 1]
                .iter()
                .map(|&v| v + b'0')
                .collect();
            return Ok((variant, digits));
        }
    }
    Err(Error::undecodable("POSTNET/PLANET pattern not recognized"))
}

/// Decode each 5-bar group to a digit value under one variant, or `None` if any
/// group is not a valid pattern.
fn decode_groups(groups: &[BarState], planet: bool) -> Option<Vec<u8>> {
    groups
        .chunks_exact(5)
        .map(|chunk| {
            let tall = [
                chunk[0].has_ascender(),
                chunk[1].has_ascender(),
                chunk[2].has_ascender(),
                chunk[3].has_ascender(),
                chunk[4].has_ascender(),
            ];
            (0u8..10).find(|&d| pattern(d, planet) == tall)
        })
        .collect()
}
