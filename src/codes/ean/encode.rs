//! EAN/UPC encoding: [`Symbol`] → [`LinearPattern`].
//!
//! The [`EanEncoder`] renders an exact module pattern from a [`Symbol`] carrying an
//! [`EanMeta`], and offers `build_*` helpers that validate/compute check digits and
//! assemble a reproducible [`Symbol`] from digit strings.

use super::tables::{
    ADDON_LEAD, ADDON_SEP, CENTER_GUARD, EAN2_PARITY, EAN5_PARITY, EAN13_FIRST_PARITY,
    NORMAL_GUARD, UPCE_END_GUARD, check_digit, ean5_checksum, l_code, left_code, r_code,
    upce_expand, upce_parity,
};
use super::{AddOn, AddOnKind, EanMeta, EanVariant};
use crate::error::{Error, Result};
use crate::output::{Encoding, LinearPattern};
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// Light-module quiet zone recorded for a symbol containing a main pattern.
const QUIET_MAIN: usize = 9;
/// Light-module quiet zone recorded for a standalone add-on.
const QUIET_ADDON: usize = 7;
/// Light modules inserted between a main symbol and its trailing add-on.
const ADDON_GAP: usize = 9;

/// EAN/UPC encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct EanEncoder;

impl EanEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        EanEncoder
    }

    /// Build an EAN-13 symbol from 12 digits (check computed) or 13 digits (check
    /// verified).
    pub fn build_ean13(&self, digits: &str) -> Result<Symbol> {
        let d = with_check(digits, 12)?;
        Ok(main_symbol(Symbology::Ean13, EanVariant::Ean13, d))
    }

    /// Build an EAN-8 symbol from 7 digits (check computed) or 8 digits (verified).
    pub fn build_ean8(&self, digits: &str) -> Result<Symbol> {
        let d = with_check(digits, 7)?;
        Ok(main_symbol(Symbology::Ean8, EanVariant::Ean8, d))
    }

    /// Build a UPC-A symbol from 11 digits (check computed) or 12 digits (verified).
    pub fn build_upca(&self, digits: &str) -> Result<Symbol> {
        let d = with_check(digits, 11)?;
        Ok(main_symbol(Symbology::UpcA, EanVariant::UpcA, d))
    }

    /// Build a UPC-E symbol. Accepts 6 payload digits (number system assumed `0`,
    /// check computed), 7 digits (number system + payload, check computed) or 8
    /// digits (number system + payload + check, verified).
    pub fn build_upce(&self, digits: &str) -> Result<Symbol> {
        let raw = ascii_digits(digits)?;
        let (ns, payload_ascii): (u8, &[u8]) = match raw.len() {
            6 => (0, &raw[0..6]),
            7 | 8 => (raw[0] - b'0', &raw[1..7]),
            _ => return Err(Error::invalid_data("UPC-E expects 6, 7 or 8 digits")),
        };
        if ns > 1 {
            return Err(Error::invalid_data("UPC-E number system must be 0 or 1"));
        }
        let payload = val6(&digit_values(payload_ascii)?);
        let check = check_digit(&upce_expand(ns, &payload));
        if raw.len() == 8 && raw[7] - b'0' != check {
            return Err(Error::invalid_data("UPC-E check digit mismatch"));
        }
        // Store the canonical 8-digit form: number system + payload + check.
        let mut d = Vec::with_capacity(8);
        d.push(b'0' + ns);
        d.extend_from_slice(payload_ascii);
        d.push(b'0' + check);
        Ok(main_symbol(Symbology::UpcE, EanVariant::UpcE, d))
    }

    /// Build a standalone EAN-2 add-on from exactly 2 digits.
    pub fn build_ean2(&self, digits: &str) -> Result<Symbol> {
        let d = exact_digits(digits, 2)?;
        Ok(main_symbol(Symbology::Ean2, EanVariant::Ean2, d))
    }

    /// Build a standalone EAN-5 add-on from exactly 5 digits.
    pub fn build_ean5(&self, digits: &str) -> Result<Symbol> {
        let d = exact_digits(digits, 5)?;
        Ok(main_symbol(Symbology::Ean5, EanVariant::Ean5, d))
    }

    /// Attach a 2- or 5-digit add-on to a previously built main symbol.
    pub fn with_addon(&self, mut symbol: Symbol, addon: &str) -> Result<Symbol> {
        let digits = ascii_digits(addon)?;
        let kind = match digits.len() {
            2 => AddOnKind::Two,
            5 => AddOnKind::Five,
            _ => return Err(Error::invalid_data("add-on must be 2 or 5 digits")),
        };
        match &mut symbol.meta {
            SymbolMeta::Ean(meta) => meta.addon = Some(AddOn { kind, digits }),
            _ => return Err(Error::invalid_parameter("not an EAN/UPC symbol")),
        }
        Ok(symbol)
    }
}

impl Encode for EanEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        let meta = match &symbol.meta {
            SymbolMeta::Ean(m) => m,
            _ => return Err(Error::invalid_parameter("EAN symbol missing EanMeta")),
        };
        let digits = segment_digits(symbol)?;

        let (mut modules, quiet) = render_variant(meta.variant, &digits)?;
        if let Some(addon) = &meta.addon {
            if matches!(meta.variant, EanVariant::Ean2 | EanVariant::Ean5) {
                return Err(Error::invalid_parameter(
                    "an add-on symbol cannot itself carry an add-on",
                ));
            }
            modules.extend(std::iter::repeat_n(false, ADDON_GAP));
            modules.extend(render_addon(addon)?);
        }
        Ok(Encoding::Linear(LinearPattern {
            modules,
            quiet_zone: quiet,
        }))
    }
}

/// Render the modules for a variant's digits, returning `(modules, quiet_zone)`.
fn render_variant(variant: EanVariant, digits: &[u8]) -> Result<(Vec<bool>, usize)> {
    let vals = digit_values(digits)?;
    let modules = match variant {
        EanVariant::Ean13 => {
            expect_len(&vals, 13, "EAN-13")?;
            verify_check(&vals, 12)?;
            render_ean13(&vals)
        }
        EanVariant::UpcA => {
            expect_len(&vals, 12, "UPC-A")?;
            verify_check(&vals, 11)?;
            // UPC-A is EAN-13 with an implicit leading zero.
            let mut ean = Vec::with_capacity(13);
            ean.push(0);
            ean.extend_from_slice(&vals);
            render_ean13(&ean)
        }
        EanVariant::Ean8 => {
            expect_len(&vals, 8, "EAN-8")?;
            verify_check(&vals, 7)?;
            render_ean8(&vals)
        }
        EanVariant::UpcE => {
            expect_len(&vals, 8, "UPC-E")?;
            render_upce(&vals)?
        }
        EanVariant::Ean2 => {
            expect_len(&vals, 2, "EAN-2")?;
            return Ok((render_addon2(&vals), QUIET_ADDON));
        }
        EanVariant::Ean5 => {
            expect_len(&vals, 5, "EAN-5")?;
            return Ok((render_addon5(&vals), QUIET_ADDON));
        }
    };
    Ok((modules, QUIET_MAIN))
}

fn render_ean13(vals: &[u8]) -> Vec<bool> {
    let first = vals[0] as usize;
    let parity = EAN13_FIRST_PARITY[first];
    let mut m = Vec::with_capacity(95);
    m.extend_from_slice(&NORMAL_GUARD);
    for i in 0..6 {
        m.extend_from_slice(&left_code(vals[1 + i], parity[i]));
    }
    m.extend_from_slice(&CENTER_GUARD);
    for i in 0..6 {
        m.extend_from_slice(&r_code(vals[7 + i]));
    }
    m.extend_from_slice(&NORMAL_GUARD);
    m
}

fn render_ean8(vals: &[u8]) -> Vec<bool> {
    let mut m = Vec::with_capacity(67);
    m.extend_from_slice(&NORMAL_GUARD);
    for &v in &vals[0..4] {
        m.extend_from_slice(&l_code(v));
    }
    m.extend_from_slice(&CENTER_GUARD);
    for &v in &vals[4..8] {
        m.extend_from_slice(&r_code(v));
    }
    m.extend_from_slice(&NORMAL_GUARD);
    m
}

fn render_upce(vals: &[u8]) -> Result<Vec<bool>> {
    let ns = vals[0];
    if ns > 1 {
        return Err(Error::invalid_data("UPC-E number system must be 0 or 1"));
    }
    let payload: [u8; 6] = val6(&vals[1..7]);
    let expanded = upce_expand(ns, &payload);
    let check = check_digit(&expanded);
    if vals[7] != check {
        return Err(Error::invalid_data("UPC-E check digit mismatch"));
    }
    let parity = upce_parity(ns, check);
    let mut m = Vec::with_capacity(51);
    m.extend_from_slice(&NORMAL_GUARD);
    for i in 0..6 {
        m.extend_from_slice(&left_code(payload[i], parity[i]));
    }
    m.extend_from_slice(&UPCE_END_GUARD);
    Ok(m)
}

fn render_addon(addon: &AddOn) -> Result<Vec<bool>> {
    let vals = digit_values(&addon.digits)?;
    Ok(match addon.kind {
        AddOnKind::Two => {
            expect_len(&vals, 2, "EAN-2 add-on")?;
            render_addon2(&vals)
        }
        AddOnKind::Five => {
            expect_len(&vals, 5, "EAN-5 add-on")?;
            render_addon5(&vals)
        }
    })
}

fn render_addon2(vals: &[u8]) -> Vec<bool> {
    let value = vals[0] * 10 + vals[1];
    let parity = EAN2_PARITY[(value % 4) as usize];
    let mut m = Vec::with_capacity(20);
    m.extend_from_slice(&ADDON_LEAD);
    m.extend_from_slice(&left_code(vals[0], parity[0]));
    m.extend_from_slice(&ADDON_SEP);
    m.extend_from_slice(&left_code(vals[1], parity[1]));
    m
}

fn render_addon5(vals: &[u8]) -> Vec<bool> {
    let checksum = ean5_checksum(vals);
    let parity = EAN5_PARITY[checksum as usize];
    let mut m = Vec::with_capacity(47);
    m.extend_from_slice(&ADDON_LEAD);
    for i in 0..5 {
        if i > 0 {
            m.extend_from_slice(&ADDON_SEP);
        }
        m.extend_from_slice(&left_code(vals[i], parity[i]));
    }
    m
}

// ---- Symbol / digit helpers ------------------------------------------------

/// Assemble a main symbol carrying `digits` (ASCII, incl. check) for `variant`.
fn main_symbol(symbology: Symbology, variant: EanVariant, digits: Vec<u8>) -> Symbol {
    Symbol::new(
        symbology,
        vec![Segment::numeric(digits)],
        SymbolMeta::Ean(EanMeta::new(variant)),
    )
}

/// The single numeric segment's digits, validated as ASCII digits.
fn segment_digits(symbol: &Symbol) -> Result<Vec<u8>> {
    let seg = symbol
        .segments
        .iter()
        .find(|s| matches!(s.mode, Mode::Numeric))
        .ok_or_else(|| Error::invalid_data("EAN symbol has no numeric segment"))?;
    validate_ascii(&seg.data)?;
    Ok(seg.data.clone())
}

/// Parse a digit string into ASCII digit bytes, rejecting anything else.
fn ascii_digits(s: &str) -> Result<Vec<u8>> {
    let bytes = s.as_bytes().to_vec();
    validate_ascii(&bytes)?;
    Ok(bytes)
}

fn validate_ascii(bytes: &[u8]) -> Result<()> {
    if bytes.iter().all(u8::is_ascii_digit) {
        Ok(())
    } else {
        Err(Error::invalid_data("EAN/UPC data must be ASCII digits"))
    }
}

/// Parse an exact-length digit string.
fn exact_digits(s: &str, len: usize) -> Result<Vec<u8>> {
    let d = ascii_digits(s)?;
    if d.len() != len {
        return Err(Error::invalid_data(format!("expected {len} digits")));
    }
    Ok(d)
}

/// Accept `data_len` digits (append computed check) or `data_len + 1` (verify).
fn with_check(s: &str, data_len: usize) -> Result<Vec<u8>> {
    let mut d = ascii_digits(s)?;
    if d.len() == data_len {
        let vals = digit_values(&d)?;
        d.push(b'0' + check_digit(&vals));
    } else if d.len() == data_len + 1 {
        let vals = digit_values(&d)?;
        verify_check(&vals, data_len)?;
    } else {
        return Err(Error::invalid_data(format!(
            "expected {data_len} or {} digits",
            data_len + 1
        )));
    }
    Ok(d)
}

/// Convert ASCII digit bytes to 0..=9 values.
fn digit_values(digits: &[u8]) -> Result<Vec<u8>> {
    validate_ascii(digits)?;
    Ok(digits.iter().map(|&b| b - b'0').collect())
}

fn val6(vals: &[u8]) -> [u8; 6] {
    let mut out = [0u8; 6];
    out.copy_from_slice(&vals[0..6]);
    out
}

fn expect_len(vals: &[u8], len: usize, what: &str) -> Result<()> {
    if vals.len() == len {
        Ok(())
    } else {
        Err(Error::invalid_data(format!(
            "{what} expects {len} digits, got {}",
            vals.len()
        )))
    }
}

/// Verify the check digit at `vals[data_len]` against `vals[..data_len]`.
fn verify_check(vals: &[u8], data_len: usize) -> Result<()> {
    if check_digit(&vals[..data_len]) == vals[data_len] {
        Ok(())
    } else {
        Err(Error::invalid_data("check digit mismatch"))
    }
}
