//! EAN/UPC decoding: [`LinearPattern`] → [`Symbol`].
//!
//! The structural decoder consumes a clean module row (as produced by the encoder or
//! an image-sampling front-end) and recovers the exact digits, variant and add-on so
//! the result re-encodes identically. Every candidate interpretation is confirmed by
//! its guards, valid L/G/R element patterns, parity/number-system lookup, and check
//! digit, which makes the family members mutually unambiguous.

use super::tables::{
    ADDON_LEAD, ADDON_SEP, CENTER_GUARD, EAN2_PARITY, EAN5_PARITY, EAN13_FIRST_PARITY,
    NORMAL_GUARD, UPCE_END_GUARD, check_digit, ean5_checksum, match_left, match_right, upce_expand,
    upce_from_parity,
};
use super::{AddOn, AddOnKind, EanMeta, EanVariant};
use crate::error::{Error, Result};
use crate::output::{Encoding, LinearPattern};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// EAN/UPC structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct EanDecoder;

impl EanDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        EanDecoder
    }
}

impl Decode for EanDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Linear(lp) => decode_linear(lp),
            Encoding::Matrix(_) => Err(Error::Unsupported {
                what: "EAN/UPC decode of a matrix",
            }),
        }
    }
}

fn decode_linear(lp: &LinearPattern) -> Result<Symbol> {
    let m = trim(&lp.modules);
    if m.len() < 3 || m[0..3] != NORMAL_GUARD {
        return Err(Error::undecodable("no EAN/UPC start guard"));
    }

    // A standalone add-on starts `1011`; a main symbol continues `101` + a left digit
    // (which always begins with a space).
    if m.len() >= 4 && m[3] {
        let (addon, consumed) =
            parse_addon(m).ok_or_else(|| Error::undecodable("malformed add-on"))?;
        if consumed != m.len() {
            return Err(Error::undecodable("trailing data after add-on"));
        }
        let (symbology, variant) = match addon.kind {
            AddOnKind::Two => (Symbology::Ean2, EanVariant::Ean2),
            AddOnKind::Five => (Symbology::Ean5, EanVariant::Ean5),
        };
        return Ok(Symbol::new(
            symbology,
            vec![Segment::numeric(addon.digits)],
            SymbolMeta::Ean(EanMeta::new(variant)),
        ));
    }

    // Try each main variant in turn; accept the first whose guards, digits and check
    // validate and whose trailing modules form nothing or a valid add-on.
    for parser in [try_ean13, try_ean8, try_upce] {
        if let Some((variant, digits, consumed)) = parser(m)
            && let Some(addon) = parse_trailing_addon(&m[consumed..])
        {
            let symbology = symbology_of(variant);
            return Ok(Symbol::new(
                symbology,
                vec![Segment::numeric(digits)],
                SymbolMeta::Ean(EanMeta { variant, addon }),
            ));
        }
    }
    Err(Error::undecodable("unrecognized EAN/UPC symbol"))
}

/// Strip leading and trailing light modules.
fn trim(modules: &[bool]) -> &[bool] {
    let start = modules.iter().position(|&b| b).unwrap_or(modules.len());
    let end = modules.iter().rposition(|&b| b).map_or(start, |p| p + 1);
    &modules[start..end]
}

fn symbology_of(variant: EanVariant) -> Symbology {
    match variant {
        EanVariant::Ean13 => Symbology::Ean13,
        EanVariant::Ean8 => Symbology::Ean8,
        EanVariant::UpcA => Symbology::UpcA,
        EanVariant::UpcE => Symbology::UpcE,
        EanVariant::Ean2 => Symbology::Ean2,
        EanVariant::Ean5 => Symbology::Ean5,
    }
}

fn to_ascii(vals: &[u8]) -> Vec<u8> {
    vals.iter().map(|&v| b'0' + v).collect()
}

// ---- Main-variant parsers --------------------------------------------------

/// Parse EAN-13 / UPC-A (95 modules). Returns `(variant, ascii digits, consumed)`.
fn try_ean13(m: &[bool]) -> Option<(EanVariant, Vec<u8>, usize)> {
    if m.len() < 95
        || m[0..3] != NORMAL_GUARD
        || m[45..50] != CENTER_GUARD
        || m[92..95] != NORMAL_GUARD
    {
        return None;
    }
    let mut left = [0u8; 6];
    let mut parity = [false; 6];
    for i in 0..6 {
        let (d, even) = match_left(&m[3 + i * 7..10 + i * 7])?;
        left[i] = d;
        parity[i] = even;
    }
    let mut right = [0u8; 6];
    for i in 0..6 {
        right[i] = match_right(&m[50 + i * 7..57 + i * 7])?;
    }
    let first = (0..10u8).find(|&f| EAN13_FIRST_PARITY[f as usize] == parity)?;

    let mut vals = Vec::with_capacity(13);
    vals.push(first);
    vals.extend_from_slice(&left);
    vals.extend_from_slice(&right);
    if check_digit(&vals[..12]) != vals[12] {
        return None;
    }
    // A leading zero means the symbol is a UPC-A (dropping the implicit zero).
    if first == 0 {
        Some((EanVariant::UpcA, to_ascii(&vals[1..]), 95))
    } else {
        Some((EanVariant::Ean13, to_ascii(&vals), 95))
    }
}

/// Parse EAN-8 (67 modules).
fn try_ean8(m: &[bool]) -> Option<(EanVariant, Vec<u8>, usize)> {
    if m.len() < 67
        || m[0..3] != NORMAL_GUARD
        || m[31..36] != CENTER_GUARD
        || m[64..67] != NORMAL_GUARD
    {
        return None;
    }
    let mut vals = [0u8; 8];
    for i in 0..4 {
        let (d, even) = match_left(&m[3 + i * 7..10 + i * 7])?;
        if even {
            return None; // EAN-8 left digits are all odd (L).
        }
        vals[i] = d;
    }
    for i in 0..4 {
        vals[4 + i] = match_right(&m[36 + i * 7..43 + i * 7])?;
    }
    if check_digit(&vals[..7]) != vals[7] {
        return None;
    }
    Some((EanVariant::Ean8, to_ascii(&vals), 67))
}

/// Parse UPC-E (51 modules).
fn try_upce(m: &[bool]) -> Option<(EanVariant, Vec<u8>, usize)> {
    if m.len() < 51 || m[0..3] != NORMAL_GUARD || m[45..51] != UPCE_END_GUARD {
        return None;
    }
    let mut payload = [0u8; 6];
    let mut parity = [false; 6];
    for i in 0..6 {
        let (d, even) = match_left(&m[3 + i * 7..10 + i * 7])?;
        payload[i] = d;
        parity[i] = even;
    }
    let (ns, check) = upce_from_parity(&parity)?;
    // Confirm against the expanded UPC-A check digit.
    if check_digit(&upce_expand(ns, &payload)) != check {
        return None;
    }
    let mut vals = Vec::with_capacity(8);
    vals.push(ns);
    vals.extend_from_slice(&payload);
    vals.push(check);
    Some((EanVariant::UpcE, to_ascii(&vals), 51))
}

// ---- Add-on parsers --------------------------------------------------------

/// Interpret the modules following a main symbol: `None` on invalid trailing data,
/// `Some(None)` for no add-on, `Some(Some(addon))` for a valid add-on.
fn parse_trailing_addon(rest: &[bool]) -> Option<Option<AddOn>> {
    // `rest` is already trailing-trimmed by the caller; nothing left means no add-on.
    if rest.is_empty() {
        return Some(None);
    }
    let start = rest.iter().position(|&b| b)?;
    if start == 0 {
        // No inter-symbol gap: not a separated add-on.
        return None;
    }
    let tail = &rest[start..];
    let (addon, consumed) = parse_addon(tail)?;
    if consumed == tail.len() {
        Some(Some(addon))
    } else {
        None
    }
}

/// Parse an add-on beginning at `m[0]` (the add-on lead). EAN-5 is tried first.
fn parse_addon(m: &[bool]) -> Option<(AddOn, usize)> {
    try_addon5(m).or_else(|| try_addon2(m))
}

fn try_addon2(m: &[bool]) -> Option<(AddOn, usize)> {
    if m.len() < 20 || m[0..4] != ADDON_LEAD || m[11..13] != ADDON_SEP {
        return None;
    }
    let (d0, p0) = match_left(&m[4..11])?;
    let (d1, p1) = match_left(&m[13..20])?;
    let value = d0 * 10 + d1;
    if EAN2_PARITY[(value % 4) as usize] != [p0, p1] {
        return None;
    }
    Some((
        AddOn {
            kind: AddOnKind::Two,
            digits: to_ascii(&[d0, d1]),
        },
        20,
    ))
}

fn try_addon5(m: &[bool]) -> Option<(AddOn, usize)> {
    if m.len() < 47 || m[0..4] != ADDON_LEAD {
        return None;
    }
    let mut vals = [0u8; 5];
    let mut parity = [false; 5];
    // Digits at 4, 13, 22, 31, 40; two-module separators precede digits 2..=5.
    for i in 0..5 {
        let pos = 4 + i * 9;
        if i > 0 && m[pos - 2..pos] != ADDON_SEP {
            return None;
        }
        let (d, even) = match_left(&m[pos..pos + 7])?;
        vals[i] = d;
        parity[i] = even;
    }
    if EAN5_PARITY[ean5_checksum(&vals) as usize] != parity {
        return None;
    }
    Some((
        AddOn {
            kind: AddOnKind::Five,
            digits: to_ascii(&vals),
        },
        47,
    ))
}
