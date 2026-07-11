//! MSI Plessey ([`Symbology::MsiPlessey`]) and the original Plessey code
//! ([`Symbology::Plessey`]).
//!
//! # MSI Plessey
//!
//! A numeric symbology. Each digit is four BCD bits (MSB first); each bit renders as
//! three modules — `1` → `110`, `0` → `100`. The symbol is framed by start `110` and
//! stop `1001`. An optional check scheme is appended before encoding and recorded in
//! [`MsiMeta`]:
//!
//! - [`MsiCheck::Mod10`]  — Luhn mod-10 (one digit).
//! - [`MsiCheck::Mod11`]  — IBM mod-11, weights 2..7 (one digit).
//! - [`MsiCheck::Mod1010`] — mod-10 twice (two digits).
//! - [`MsiCheck::Mod1110`] — mod-11 then mod-10 (two digits).
//!
//! Because the scheme is not encoded in the bars, the decoder is told which to expect
//! via [`MsiDecoder::with_check`].
//!
//! # Plessey
//!
//! A hexadecimal symbology (`0-9 A-F`). Each digit is four bits, LSB first; each bit
//! renders as a bar/space pair (`0` → narrow bar + wide space, `1` → wide bar + narrow
//! space). An 8-bit CRC (generator `x^8+x^6+x^5+x^4+x^2+1`) is appended. Select the
//! Plessey path with [`MsiDecoder::plessey`].
//!
//! # Validation
//!
//! MSI mod-10 and mod-11 are hand-verified against the Wikipedia MSI example
//! `1234567` → check `4` for both schemes. The Plessey encoding (bit order, bar
//! mapping, CRC grid `111101001` and start/stop) matches the reference implementation
//! [zint](https://github.com/zint/zint) (`plessey.c`).

use crate::error::{Error, Result};
use crate::output::{Encoding, LinearPattern};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::{Decode, Encode};

/// Quiet-zone width in narrow modules on each side.
const QUIET_ZONE: usize = 10;

/// MSI Plessey check-digit scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MsiCheck {
    /// No check digit.
    #[default]
    None,
    /// Luhn mod-10 (one digit).
    Mod10,
    /// IBM mod-11, weights 2..7 (one digit).
    Mod11,
    /// mod-10 applied twice (two digits).
    Mod1010,
    /// mod-11 then mod-10 (two digits).
    Mod1110,
}

/// Parameters required to re-encode an MSI Plessey / Plessey symbol identically.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MsiMeta {
    /// Check scheme (MSI only; ignored for Plessey, which always carries its CRC).
    pub check: MsiCheck,
}

// ---------- MSI check arithmetic ----------

/// Luhn mod-10 check digit for ASCII `digits` (rightmost digit doubled).
fn msi_mod10(digits: &[u8]) -> u8 {
    let mut sum = 0u32;
    let mut double = true;
    for &c in digits.iter().rev() {
        let mut v = (c - b'0') as u32;
        if double {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
        double = !double;
    }
    ((10 - sum % 10) % 10) as u8
}

/// IBM mod-11 check digit (weights 2..7 cycling from the right), or `None` if the
/// result is 10 (not representable as a single digit).
fn msi_mod11(digits: &[u8]) -> Option<u8> {
    let mut sum = 0u32;
    let mut weight = 2u32;
    for &c in digits.iter().rev() {
        sum += (c - b'0') as u32 * weight;
        weight = if weight == 7 { 2 } else { weight + 1 };
    }
    let c = (11 - sum % 11) % 11;
    (c < 10).then_some(c as u8)
}

/// The full digit string (data + check digits) for a scheme.
fn apply_check(data: &[u8], scheme: MsiCheck) -> Result<Vec<u8>> {
    let mut out = data.to_vec();
    match scheme {
        MsiCheck::None => {}
        MsiCheck::Mod10 => out.push(b'0' + msi_mod10(data)),
        MsiCheck::Mod11 => {
            let c = msi_mod11(data)
                .ok_or_else(|| Error::invalid_data("MSI mod-11 check digit is 10"))?;
            out.push(b'0' + c);
        }
        MsiCheck::Mod1010 => {
            let c1 = msi_mod10(&out);
            out.push(b'0' + c1);
            let c2 = msi_mod10(&out);
            out.push(b'0' + c2);
        }
        MsiCheck::Mod1110 => {
            let c1 = msi_mod11(&out)
                .ok_or_else(|| Error::invalid_data("MSI mod-11 check digit is 10"))?;
            out.push(b'0' + c1);
            let c2 = msi_mod10(&out);
            out.push(b'0' + c2);
        }
    }
    Ok(out)
}

/// Number of check digits a scheme appends.
fn check_len(scheme: MsiCheck) -> usize {
    match scheme {
        MsiCheck::None => 0,
        MsiCheck::Mod10 | MsiCheck::Mod11 => 1,
        MsiCheck::Mod1010 | MsiCheck::Mod1110 => 2,
    }
}

fn ensure_digits(digits: &[u8]) -> Result<()> {
    if digits.is_empty() {
        return Err(Error::invalid_data("MSI payload is empty"));
    }
    if digits.iter().all(u8::is_ascii_digit) {
        Ok(())
    } else {
        Err(Error::invalid_data("MSI payload must be ASCII digits"))
    }
}

// ---------- shared helpers ----------

/// Push module booleans from a `"10"` string (`'1'` = bar).
fn push_bits(modules: &mut Vec<bool>, bits: &str) {
    modules.extend(bits.bytes().map(|b| b == b'1'));
}

/// Run-length encode `modules` into element widths, starting with a bar.
fn rle(modules: &[bool]) -> Result<Vec<u32>> {
    if modules.is_empty() || !modules[0] {
        return Err(Error::undecodable("linear pattern must start with a bar"));
    }
    let mut runs = Vec::new();
    let mut cur = modules[0];
    let mut len = 0u32;
    for &m in modules {
        if m == cur {
            len += 1;
        } else {
            runs.push(len);
            cur = m;
            len = 1;
        }
    }
    runs.push(len);
    Ok(runs)
}

// ---------- MSI encoding ----------

fn msi_encode(digits: &[u8]) -> Vec<bool> {
    let mut modules = Vec::new();
    push_bits(&mut modules, "110"); // start
    for &d in digits {
        let v = d - b'0';
        for bit in (0..4).rev() {
            if (v >> bit) & 1 == 1 {
                push_bits(&mut modules, "110");
            } else {
                push_bits(&mut modules, "100");
            }
        }
    }
    push_bits(&mut modules, "1001"); // stop
    modules
}

fn msi_decode(modules: &[bool], scheme: MsiCheck) -> Result<Vec<u8>> {
    // start(3) + n*12 + stop(4)
    if modules.len() < 3 + 12 + 4 || !(modules.len() - 7).is_multiple_of(12) {
        return Err(Error::undecodable("malformed MSI structure"));
    }
    let bit = |i: usize| modules[i];
    if !(bit(0) && bit(1) && !bit(2)) {
        return Err(Error::undecodable("bad MSI start"));
    }
    let stop_at = modules.len() - 4;
    if !bit(stop_at) || bit(stop_at + 1) || bit(stop_at + 2) || !bit(stop_at + 3) {
        return Err(Error::undecodable("bad MSI stop"));
    }

    let n = (modules.len() - 7) / 12;
    let mut digits = Vec::with_capacity(n);
    for d in 0..n {
        let mut value = 0u8;
        for b in 0..4 {
            let base = 3 + d * 12 + b * 3;
            let triple = (modules[base], modules[base + 1], modules[base + 2]);
            let one = match triple {
                (true, true, false) => 1,
                (true, false, false) => 0,
                _ => return Err(Error::undecodable("invalid MSI bit group")),
            };
            value = (value << 1) | one;
        }
        digits.push(b'0' + value);
    }

    // Verify and strip check digits.
    let clen = check_len(scheme);
    if digits.len() < clen {
        return Err(Error::undecodable("MSI too short for check scheme"));
    }
    let data_len = digits.len() - clen;
    let expected = apply_check(&digits[..data_len], scheme)?;
    if expected != digits {
        return Err(Error::undecodable("MSI check digit mismatch"));
    }
    digits.truncate(data_len);
    Ok(digits)
}

// ---------- Plessey encoding ----------

/// CRC feedback pattern (generator `111101001`), from zint `plessey.c`.
const PLESSEY_GRID: [u8; 9] = [1, 1, 1, 1, 0, 1, 0, 0, 1];
/// Plessey start element widths (bit pattern `1101`), from zint.
const PLESSEY_START: [u32; 8] = [3, 1, 3, 1, 1, 3, 3, 1];
/// Plessey stop / termination element widths, from zint.
const PLESSEY_STOP: [u32; 9] = [3, 3, 1, 3, 1, 1, 3, 1, 3];

/// Value of a Plessey hex digit.
fn hex_value(c: u8) -> Result<u8> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(Error::invalid_data("Plessey payload must be 0-9 or A-F")),
    }
}

fn hex_char(v: u8) -> u8 {
    if v < 10 { b'0' + v } else { b'A' + (v - 10) }
}

/// Compute the 8 CRC bits for the data bit vector (LSB-first per digit).
fn plessey_crc(data_bits: &[u8]) -> [u8; 8] {
    let mut work = data_bits.to_vec();
    work.extend([0u8; 8]);
    for i in 0..data_bits.len() {
        if work[i] == 1 {
            for (j, &g) in PLESSEY_GRID.iter().enumerate() {
                work[i + j] ^= g;
            }
        }
    }
    let mut crc = [0u8; 8];
    crc.copy_from_slice(&work[data_bits.len()..data_bits.len() + 8]);
    crc
}

/// Render an alternating (bar, space, ...) width sequence.
fn render_widths(modules: &mut Vec<bool>, widths: &[u32]) {
    for (i, &w) in widths.iter().enumerate() {
        modules.extend(std::iter::repeat_n(i % 2 == 0, w as usize));
    }
}

/// One data/CRC bit → its bar/space widths (`0` → 1,3; `1` → 3,1).
fn bit_widths(bit: u8) -> [u32; 2] {
    if bit == 1 { [3, 1] } else { [1, 3] }
}

fn plessey_encode(digits: &[u8]) -> Result<Vec<bool>> {
    // Data bits, 4 per digit, LSB first.
    let mut data_bits = Vec::with_capacity(digits.len() * 4);
    for &c in digits {
        let v = hex_value(c)?;
        for bit in 0..4 {
            data_bits.push((v >> bit) & 1);
        }
    }
    let crc = plessey_crc(&data_bits);

    let mut modules = Vec::new();
    render_widths(&mut modules, &PLESSEY_START);
    for &b in &data_bits {
        render_widths(&mut modules, &bit_widths(b));
    }
    for &b in &crc {
        render_widths(&mut modules, &bit_widths(b));
    }
    render_widths(&mut modules, &PLESSEY_STOP);
    Ok(modules)
}

/// Match a run slice against a width pattern by wide/narrow class.
fn matches_widths(runs: &[u32], pattern: &[u32]) -> bool {
    runs.len() == pattern.len() && runs.iter().zip(pattern).all(|(&r, &p)| (r > 1) == (p > 1))
}

fn plessey_decode(modules: &[bool]) -> Result<Vec<u8>> {
    let runs = rle(modules)?;
    let (sl, tl) = (PLESSEY_START.len(), PLESSEY_STOP.len());
    if runs.len() < sl + tl + 16 {
        return Err(Error::undecodable("Plessey too short"));
    }
    if !matches_widths(&runs[..sl], &PLESSEY_START) {
        return Err(Error::undecodable("bad Plessey start"));
    }
    if !matches_widths(&runs[runs.len() - tl..], &PLESSEY_STOP) {
        return Err(Error::undecodable("bad Plessey stop"));
    }
    let body = &runs[sl..runs.len() - tl];
    if body.len() % 2 != 0 {
        return Err(Error::undecodable("malformed Plessey body"));
    }
    // Each bit is a (bar, space) pair.
    let mut bits = Vec::with_capacity(body.len() / 2);
    for pair in body.chunks_exact(2) {
        let bit = match (pair[0] > 1, pair[1] > 1) {
            (false, true) => 0u8, // narrow bar, wide space
            (true, false) => 1u8, // wide bar, narrow space
            _ => return Err(Error::undecodable("invalid Plessey bit pair")),
        };
        bits.push(bit);
    }
    if bits.len() < 8 || (bits.len() - 8) % 4 != 0 {
        return Err(Error::undecodable("Plessey bit count invalid"));
    }
    let data_bits = &bits[..bits.len() - 8];
    let crc = &bits[bits.len() - 8..];
    if plessey_crc(data_bits) != crc {
        return Err(Error::undecodable("Plessey CRC mismatch"));
    }
    let mut digits = Vec::with_capacity(data_bits.len() / 4);
    for chunk in data_bits.chunks_exact(4) {
        let mut v = 0u8;
        for (bit, &b) in chunk.iter().enumerate() {
            v |= b << bit;
        }
        digits.push(hex_char(v));
    }
    Ok(digits)
}

// ---------- public API ----------

/// MSI Plessey / Plessey encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MsiEncoder;

impl MsiEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

    /// Build an MSI Plessey symbol from `digits` with the given check `scheme`.
    pub fn build_msi(&self, digits: &[u8], scheme: MsiCheck) -> Result<Symbol> {
        ensure_digits(digits)?;
        apply_check(digits, scheme)?; // validate (e.g. mod-11 == 10)
        Ok(Symbol::new(
            Symbology::MsiPlessey,
            vec![Segment::numeric(digits.to_vec())],
            SymbolMeta::Msi(MsiMeta { check: scheme }),
        ))
    }

    /// Build a Plessey symbol from hex `digits` (`0-9 A-F`).
    pub fn build_plessey(&self, digits: &[u8]) -> Result<Symbol> {
        for &c in digits {
            hex_value(c)?;
        }
        if digits.is_empty() {
            return Err(Error::invalid_data("Plessey payload is empty"));
        }
        Ok(Symbol::new(
            Symbology::Plessey,
            vec![Segment::byte(digits.to_vec())],
            SymbolMeta::Msi(MsiMeta::default()),
        ))
    }
}

impl Encode for MsiEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        let meta = match &symbol.meta {
            SymbolMeta::Msi(m) => m,
            _ => return Err(Error::invalid_parameter("symbol missing MsiMeta")),
        };
        let modules = match symbol.symbology {
            Symbology::MsiPlessey => {
                let digits = symbol.payload_bytes();
                ensure_digits(&digits)?;
                msi_encode(&apply_check(&digits, meta.check)?)
            }
            Symbology::Plessey => plessey_encode(&symbol.payload_bytes())?,
            _ => {
                return Err(Error::invalid_parameter(
                    "MsiEncoder given an unsupported symbology",
                ));
            }
        };
        Ok(Encoding::Linear(LinearPattern {
            modules,
            quiet_zone: QUIET_ZONE,
        }))
    }
}

/// MSI Plessey / Plessey decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MsiDecoder {
    plessey: bool,
    check: MsiCheck,
}

impl MsiDecoder {
    /// A new MSI Plessey decoder that expects no check digit.
    pub fn new() -> Self {
        Self {
            plessey: false,
            check: MsiCheck::None,
        }
    }

    /// An MSI Plessey decoder expecting the given check `scheme`.
    pub fn with_check(scheme: MsiCheck) -> Self {
        Self {
            plessey: false,
            check: scheme,
        }
    }

    /// A decoder for the original Plessey code (CRC verified).
    pub fn plessey() -> Self {
        Self {
            plessey: true,
            check: MsiCheck::None,
        }
    }
}

impl Decode for MsiDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        let pattern = match encoding {
            Encoding::Linear(p) => p,
            Encoding::Matrix(_) => {
                return Err(Error::invalid_parameter("MSI expects a linear pattern"));
            }
        };
        if self.plessey {
            let digits = plessey_decode(&pattern.modules)?;
            Ok(Symbol::new(
                Symbology::Plessey,
                vec![Segment::byte(digits)],
                SymbolMeta::Msi(MsiMeta::default()),
            ))
        } else {
            let digits = msi_decode(&pattern.modules, self.check)?;
            Ok(Symbol::new(
                Symbology::MsiPlessey,
                vec![Segment::numeric(digits)],
                SymbolMeta::Msi(MsiMeta { check: self.check }),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msi_check_reference() {
        // Wikipedia MSI example: 1234567 has mod-10 and mod-11 check digit 4.
        assert_eq!(msi_mod10(b"1234567"), 4);
        assert_eq!(msi_mod11(b"1234567"), Some(4));
    }

    #[test]
    fn msi_roundtrip_all_schemes() {
        let enc = MsiEncoder::new();
        for scheme in [
            MsiCheck::None,
            MsiCheck::Mod10,
            MsiCheck::Mod11,
            MsiCheck::Mod1010,
            MsiCheck::Mod1110,
        ] {
            let sym = enc.build_msi(b"1234567", scheme).unwrap();
            let encoding = enc.encode(&sym).unwrap();
            let back = MsiDecoder::with_check(scheme).decode(&encoding).unwrap();
            assert_eq!(back.segments, sym.segments);
            assert_eq!(back.meta, sym.meta);
            assert_eq!(enc.encode(&back).unwrap(), encoding);
        }
    }

    #[test]
    fn plessey_roundtrip() {
        let enc = MsiEncoder::new();
        for data in [&b"1234"[..], b"ABCDEF", b"0", b"DEADBEEF"] {
            let sym = enc.build_plessey(data).unwrap();
            let encoding = enc.encode(&sym).unwrap();
            let back = MsiDecoder::plessey().decode(&encoding).unwrap();
            assert_eq!(back.segments, sym.segments);
            assert_eq!(enc.encode(&back).unwrap(), encoding);
        }
    }
}
