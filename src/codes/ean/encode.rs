//! EAN/UPC encoding: [`Symbol`] → [`LinearPattern`].
//!
//! The [`EanEncoder`] renders an exact module pattern from a [`Symbol`] carrying an
//! [`EanMeta`] (or, heap-free, from borrowed digits via
//! [`EanEncoder::encode_into`]), and offers `build_*` helpers that validate/compute
//! check digits and assemble a reproducible [`Symbol`] from digit strings.
//!
//! [`Symbol`]: crate::Symbol
//! [`LinearPattern`]: crate::output::LinearPattern
//! [`EanMeta`]: super::EanMeta

use super::tables::{
    ADDON_LEAD, ADDON_SEP, CENTER_GUARD, EAN2_PARITY, EAN5_PARITY, EAN13_FIRST_PARITY,
    NORMAL_GUARD, UPCE_END_GUARD, check_digit, ean5_checksum, l_code, left_code, r_code,
    upce_expand, upce_parity,
};
#[cfg(feature = "alloc")]
use super::{AddOn, EanMeta};
use super::{AddOnKind, AddOnView, EanVariant};
use crate::error::{Error, Result};
use crate::output::LinearSink;
#[cfg(feature = "alloc")]
use crate::output::{Encoding, LinearPattern};
#[cfg(feature = "alloc")]
use crate::segment::{Mode, Segment};
#[cfg(feature = "alloc")]
use crate::symbol::{Symbol, SymbolMeta};
#[cfg(feature = "alloc")]
use crate::symbology::Symbology;
#[cfg(feature = "alloc")]
use crate::traits::Encode;
#[cfg(feature = "alloc")]
use alloc::{format, vec, vec::Vec};

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

    /// Number of modules [`EanEncoder::encode_into`] emits for `variant` followed by
    /// an optional add-on of kind `addon`, excluding quiet zones (exact).
    pub const fn max_modules(variant: EanVariant, addon: Option<AddOnKind>) -> usize {
        let main = match variant {
            EanVariant::Ean13 | EanVariant::UpcA => 95,
            EanVariant::Ean8 => 67,
            EanVariant::UpcE => 51,
            EanVariant::Ean2 => 20,
            EanVariant::Ean5 => 47,
        };
        match addon {
            None => main,
            Some(AddOnKind::Two) => main + ADDON_GAP + 20,
            Some(AddOnKind::Five) => main + ADDON_GAP + 47,
        }
    }

    /// Heap-free encoding: write the `variant` symbol for ASCII `digits`, followed by
    /// the optional `addon`, to `out`.
    ///
    /// `digits` is the symbol's full digit string including the check digit, exactly
    /// as stored in the numeric segment of a built [`Symbol`] (UPC-E takes its 8-digit
    /// form: number system, six payload digits, check). The check digit is verified,
    /// not computed. A standalone EAN-2/EAN-5 cannot carry an add-on. The input is
    /// validated before the first module is written. Size a
    /// [`LinearBuf`](crate::output::LinearBuf) with [`EanEncoder::max_modules`].
    ///
    /// [`Symbol`]: crate::Symbol
    pub fn encode_into<S: LinearSink>(
        &self,
        variant: EanVariant,
        digits: &[u8],
        addon: Option<AddOnView<'_>>,
        out: &mut S,
    ) -> Result<()> {
        let main = Parsed::main(variant, digits)?;
        let addon = match addon {
            Some(_) if matches!(variant, EanVariant::Ean2 | EanVariant::Ean5) => {
                return Err(Error::invalid_parameter(
                    "an add-on symbol cannot itself carry an add-on",
                ));
            }
            Some(a) => Some(Parsed::addon(a)?),
            None => None,
        };

        let quiet = match main {
            Parsed::Ean2(_) | Parsed::Ean5(_) => QUIET_ADDON,
            _ => QUIET_MAIN,
        };
        out.begin(quiet)?;
        main.render(out)?;
        if let Some(addon) = addon {
            out.push_run(false, ADDON_GAP)?;
            addon.render(out)?;
        }
        Ok(())
    }
}

#[cfg(feature = "alloc")]
impl EanEncoder {
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

#[cfg(feature = "alloc")]
impl Encode for EanEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        let meta = match &symbol.meta {
            SymbolMeta::Ean(m) => m,
            _ => return Err(Error::invalid_parameter("EAN symbol missing EanMeta")),
        };
        let digits = segment_digits(symbol)?;
        let mut pattern = LinearPattern::new();
        self.encode_into(
            meta.variant,
            digits,
            meta.addon.as_ref().map(AddOn::view),
            &mut pattern,
        )?;
        Ok(Encoding::Linear(pattern))
    }
}

/// A validated main symbol or add-on, as digit values ready to render.
enum Parsed {
    /// EAN-13 digit values (UPC-A gets its implicit leading zero).
    Ean13([u8; 13]),
    /// EAN-8 digit values.
    Ean8([u8; 8]),
    /// UPC-E payload digit values and their parity pattern.
    UpcE([u8; 6], [bool; 6]),
    /// EAN-2 digit values.
    Ean2([u8; 2]),
    /// EAN-5 digit values.
    Ean5([u8; 5]),
}

impl Parsed {
    /// Validate a main (or standalone add-on) symbol's ASCII digits for `variant`.
    fn main(variant: EanVariant, digits: &[u8]) -> Result<Self> {
        validate_ascii(digits)?;
        Ok(match variant {
            EanVariant::Ean13 => {
                let vals: [u8; 13] = values(digits, "EAN-13 expects 13 digits")?;
                verify_check(&vals, 12)?;
                Parsed::Ean13(vals)
            }
            EanVariant::UpcA => {
                let upc: [u8; 12] = values(digits, "UPC-A expects 12 digits")?;
                verify_check(&upc, 11)?;
                // UPC-A is EAN-13 with an implicit leading zero.
                let mut vals = [0u8; 13];
                vals[1..].copy_from_slice(&upc);
                Parsed::Ean13(vals)
            }
            EanVariant::Ean8 => {
                let vals: [u8; 8] = values(digits, "EAN-8 expects 8 digits")?;
                verify_check(&vals, 7)?;
                Parsed::Ean8(vals)
            }
            EanVariant::UpcE => {
                let vals: [u8; 8] = values(digits, "UPC-E expects 8 digits")?;
                let ns = vals[0];
                if ns > 1 {
                    return Err(Error::invalid_data("UPC-E number system must be 0 or 1"));
                }
                let payload = val6(&vals[1..7]);
                let check = check_digit(&upce_expand(ns, &payload));
                if vals[7] != check {
                    return Err(Error::invalid_data("UPC-E check digit mismatch"));
                }
                Parsed::UpcE(payload, upce_parity(ns, check))
            }
            EanVariant::Ean2 => Parsed::Ean2(values(digits, "EAN-2 expects 2 digits")?),
            EanVariant::Ean5 => Parsed::Ean5(values(digits, "EAN-5 expects 5 digits")?),
        })
    }

    /// Validate a trailing add-on.
    fn addon(addon: AddOnView<'_>) -> Result<Self> {
        validate_ascii(addon.digits)?;
        Ok(match addon.kind {
            AddOnKind::Two => Parsed::Ean2(values(addon.digits, "EAN-2 add-on expects 2 digits")?),
            AddOnKind::Five => Parsed::Ean5(values(addon.digits, "EAN-5 add-on expects 5 digits")?),
        })
    }

    /// Write the modules (without quiet zones) to `out`.
    fn render(&self, out: &mut impl LinearSink) -> Result<()> {
        match self {
            Parsed::Ean13(vals) => {
                let parity = EAN13_FIRST_PARITY[vals[0] as usize];
                push_bits(out, &NORMAL_GUARD)?;
                for i in 0..6 {
                    push_bits(out, &left_code(vals[1 + i], parity[i]))?;
                }
                push_bits(out, &CENTER_GUARD)?;
                for &v in &vals[7..13] {
                    push_bits(out, &r_code(v))?;
                }
                push_bits(out, &NORMAL_GUARD)
            }
            Parsed::Ean8(vals) => {
                push_bits(out, &NORMAL_GUARD)?;
                for &v in &vals[0..4] {
                    push_bits(out, &l_code(v))?;
                }
                push_bits(out, &CENTER_GUARD)?;
                for &v in &vals[4..8] {
                    push_bits(out, &r_code(v))?;
                }
                push_bits(out, &NORMAL_GUARD)
            }
            Parsed::UpcE(payload, parity) => {
                push_bits(out, &NORMAL_GUARD)?;
                for i in 0..6 {
                    push_bits(out, &left_code(payload[i], parity[i]))?;
                }
                push_bits(out, &UPCE_END_GUARD)
            }
            Parsed::Ean2(vals) => {
                let value = vals[0] * 10 + vals[1];
                let parity = EAN2_PARITY[(value % 4) as usize];
                push_bits(out, &ADDON_LEAD)?;
                push_bits(out, &left_code(vals[0], parity[0]))?;
                push_bits(out, &ADDON_SEP)?;
                push_bits(out, &left_code(vals[1], parity[1]))
            }
            Parsed::Ean5(vals) => {
                let parity = EAN5_PARITY[ean5_checksum(vals) as usize];
                push_bits(out, &ADDON_LEAD)?;
                for i in 0..5 {
                    if i > 0 {
                        push_bits(out, &ADDON_SEP)?;
                    }
                    push_bits(out, &left_code(vals[i], parity[i]))?;
                }
                Ok(())
            }
        }
    }
}

/// Append a module sequence to `out`.
fn push_bits(out: &mut impl LinearSink, bits: &[bool]) -> Result<()> {
    bits.iter().try_for_each(|&b| out.push(b))
}

/// Convert exactly `N` (already validated) ASCII digits to 0..=9 values, or fail
/// with `msg` on a length mismatch.
fn values<const N: usize>(digits: &[u8], msg: &'static str) -> Result<[u8; N]> {
    if digits.len() != N {
        return Err(Error::invalid_data(msg));
    }
    let mut vals = [0u8; N];
    for (v, &d) in vals.iter_mut().zip(digits) {
        *v = d - b'0';
    }
    Ok(vals)
}

// ---- Symbol / digit helpers ------------------------------------------------

/// Assemble a main symbol carrying `digits` (ASCII, incl. check) for `variant`.
#[cfg(feature = "alloc")]
fn main_symbol(symbology: Symbology, variant: EanVariant, digits: Vec<u8>) -> Symbol {
    Symbol::new(
        symbology,
        vec![Segment::numeric(digits)],
        SymbolMeta::Ean(EanMeta::new(variant)),
    )
}

/// The single numeric segment's digits, validated as ASCII digits.
#[cfg(feature = "alloc")]
fn segment_digits(symbol: &Symbol) -> Result<&[u8]> {
    let seg = symbol
        .segments
        .iter()
        .find(|s| matches!(s.mode, Mode::Numeric))
        .ok_or_else(|| Error::invalid_data("EAN symbol has no numeric segment"))?;
    validate_ascii(&seg.data)?;
    Ok(&seg.data)
}

/// Parse a digit string into ASCII digit bytes, rejecting anything else.
#[cfg(feature = "alloc")]
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
#[cfg(feature = "alloc")]
fn exact_digits(s: &str, len: usize) -> Result<Vec<u8>> {
    let d = ascii_digits(s)?;
    if d.len() != len {
        return Err(Error::invalid_data(format!("expected {len} digits")));
    }
    Ok(d)
}

/// Accept `data_len` digits (append computed check) or `data_len + 1` (verify).
#[cfg(feature = "alloc")]
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
#[cfg(feature = "alloc")]
fn digit_values(digits: &[u8]) -> Result<Vec<u8>> {
    validate_ascii(digits)?;
    Ok(digits.iter().map(|&b| b - b'0').collect())
}

fn val6(vals: &[u8]) -> [u8; 6] {
    let mut out = [0u8; 6];
    out.copy_from_slice(&vals[0..6]);
    out
}

/// Verify the check digit at `vals[data_len]` against `vals[..data_len]`.
fn verify_check(vals: &[u8], data_len: usize) -> Result<()> {
    if check_digit(&vals[..data_len]) == vals[data_len] {
        Ok(())
    } else {
        Err(Error::invalid_data("check digit mismatch"))
    }
}
