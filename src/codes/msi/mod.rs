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

// With neither `encode` nor `decode` only the metadata types remain; their shared
// helpers are then unused.
#![cfg_attr(
    not(any(feature = "encode", feature = "decode")),
    allow(dead_code, unused_imports)
)]

use crate::error::{Error, Result};
#[cfg(feature = "alloc")]
use crate::output::Encoding;
#[cfg(all(feature = "alloc", feature = "encode"))]
use crate::output::LinearPattern;
#[cfg(feature = "encode")]
use crate::output::LinearSink;
#[cfg(feature = "alloc")]
use crate::segment::Segment;
#[cfg(feature = "alloc")]
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
#[cfg(feature = "decode")]
use crate::traits::Decode;
#[cfg(all(feature = "alloc", feature = "encode"))]
use crate::traits::Encode;
#[cfg(feature = "alloc")]
use alloc::{vec, vec::Vec};

/// Quiet-zone width in narrow modules on each side.
#[cfg(feature = "encode")]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MsiMeta {
    /// Check scheme (MSI only; ignored for Plessey, which always carries its CRC).
    pub check: MsiCheck,
}

// ---------- MSI check arithmetic ----------

/// Luhn mod-10 check digit for ASCII `digits` (rightmost digit doubled).
#[cfg(test)]
fn msi_mod10(digits: &[u8]) -> u8 {
    mod10_split(digits, &[])
}

/// Luhn mod-10 check digit for the ASCII digit string `head` followed by `tail`.
fn mod10_split(head: &[u8], tail: &[u8]) -> u8 {
    let mut sum = 0u32;
    let mut double = true;
    for &c in head.iter().chain(tail).rev() {
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

/// The ASCII check digits a scheme appends to `data`; only the first
/// [`check_len`] bytes of the returned array are meaningful.
fn check_digits(data: &[u8], scheme: MsiCheck) -> Result<[u8; 2]> {
    let mod11 =
        |d: &[u8]| msi_mod11(d).ok_or_else(|| Error::invalid_data("MSI mod-11 check digit is 10"));
    let mut out = [0u8; 2];
    match scheme {
        MsiCheck::None => {}
        MsiCheck::Mod10 => out[0] = b'0' + mod10_split(data, &[]),
        MsiCheck::Mod11 => out[0] = b'0' + mod11(data)?,
        MsiCheck::Mod1010 => {
            out[0] = b'0' + mod10_split(data, &[]);
            out[1] = b'0' + mod10_split(data, &out[..1]);
        }
        MsiCheck::Mod1110 => {
            out[0] = b'0' + mod11(data)?;
            out[1] = b'0' + mod10_split(data, &out[..1]);
        }
    }
    Ok(out)
}

/// The full digit string (data + check digits) for a scheme.
#[cfg(feature = "alloc")]
fn apply_check(data: &[u8], scheme: MsiCheck) -> Result<Vec<u8>> {
    let check = check_digits(data, scheme)?;
    let mut out = data.to_vec();
    out.extend_from_slice(&check[..check_len(scheme)]);
    Ok(out)
}

/// Number of check digits a scheme appends.
const fn check_len(scheme: MsiCheck) -> usize {
    match scheme {
        MsiCheck::None => 0,
        MsiCheck::Mod10 | MsiCheck::Mod11 => 1,
        MsiCheck::Mod1010 | MsiCheck::Mod1110 => 2,
    }
}

#[cfg(feature = "encode")]
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
#[cfg(feature = "encode")]
fn push_bits(out: &mut impl LinearSink, bits: &str) -> Result<()> {
    bits.bytes().try_for_each(|b| out.push(b == b'1'))
}

/// Run-length encode `modules` into element widths, starting with a bar.
#[cfg(feature = "decode")]
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

/// Emit an MSI symbol for validated ASCII `digits` followed by the `check` digits.
#[cfg(feature = "encode")]
fn msi_encode(out: &mut impl LinearSink, digits: &[u8], check: &[u8]) -> Result<()> {
    push_bits(out, "110")?; // start
    for &d in digits.iter().chain(check) {
        let v = d - b'0';
        for bit in (0..4).rev() {
            push_bits(out, if (v >> bit) & 1 == 1 { "110" } else { "100" })?;
        }
    }
    push_bits(out, "1001") // stop
}

#[cfg(feature = "decode")]
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
/// Modules in the Plessey start (sum of [`PLESSEY_START`]).
#[cfg(feature = "encode")]
const PLESSEY_START_MODULES: usize = 16;
/// Modules in the Plessey stop (sum of [`PLESSEY_STOP`]).
#[cfg(feature = "encode")]
const PLESSEY_STOP_MODULES: usize = 19;

/// Value of a Plessey hex digit.
#[cfg(feature = "encode")]
fn hex_value(c: u8) -> Result<u8> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(Error::invalid_data("Plessey payload must be 0-9 or A-F")),
    }
}

#[cfg(feature = "decode")]
fn hex_char(v: u8) -> u8 {
    if v < 10 { b'0' + v } else { b'A' + (v - 10) }
}

/// Compute the 8 CRC bits for a data bit stream (LSB-first per digit).
///
/// Polynomial long division by [`PLESSEY_GRID`], run as a shift register over the
/// data followed by eight zero bits, so no working copy of the stream is needed.
fn plessey_crc(data_bits: impl Iterator<Item = u8>) -> [u8; 8] {
    // Register bit k holds the working bit k + 1 positions past the current one;
    // `taps` is PLESSEY_GRID[1..] (GRID[0] only clears the leading bit).
    let taps = PLESSEY_GRID[1..]
        .iter()
        .enumerate()
        .fold(0u8, |acc, (k, &g)| acc | (g << k));
    let mut reg = 0u8;
    for bit in data_bits.chain([0u8; 8]) {
        let top = reg & 1;
        reg = (reg >> 1) | (bit << 7);
        if top == 1 {
            reg ^= taps;
        }
    }
    core::array::from_fn(|k| (reg >> k) & 1)
}

/// The 4 data bits of each (validated) hex digit in `digits`, LSB first.
#[cfg(feature = "encode")]
fn plessey_bits(digits: &[u8]) -> impl Iterator<Item = u8> + '_ {
    digits.iter().flat_map(|&c| {
        let v = hex_value(c).expect("validated Plessey digit");
        (0..4).map(move |bit| (v >> bit) & 1)
    })
}

/// Render an alternating (bar, space, ...) width sequence.
#[cfg(feature = "encode")]
fn render_widths(out: &mut impl LinearSink, widths: &[u32]) -> Result<()> {
    for (i, &w) in widths.iter().enumerate() {
        out.push_run(i % 2 == 0, w as usize)?;
    }
    Ok(())
}

/// One data/CRC bit → its bar/space widths (`0` → 1,3; `1` → 3,1).
#[cfg(feature = "encode")]
fn bit_widths(bit: u8) -> [u32; 2] {
    if bit == 1 { [3, 1] } else { [1, 3] }
}

/// Emit a Plessey symbol for validated hex `digits`.
#[cfg(feature = "encode")]
fn plessey_encode(out: &mut impl LinearSink, digits: &[u8]) -> Result<()> {
    let crc = plessey_crc(plessey_bits(digits));
    render_widths(out, &PLESSEY_START)?;
    for b in plessey_bits(digits).chain(crc) {
        render_widths(out, &bit_widths(b))?;
    }
    render_widths(out, &PLESSEY_STOP)
}

/// Match a run slice against a width pattern by wide/narrow class.
#[cfg(feature = "decode")]
fn matches_widths(runs: &[u32], pattern: &[u32]) -> bool {
    runs.len() == pattern.len() && runs.iter().zip(pattern).all(|(&r, &p)| (r > 1) == (p > 1))
}

#[cfg(feature = "decode")]
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
    for pair in body.as_chunks::<2>().0 {
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
    if plessey_crc(data_bits.iter().copied()) != crc {
        return Err(Error::undecodable("Plessey CRC mismatch"));
    }
    let mut digits = Vec::with_capacity(data_bits.len() / 4);
    for chunk in data_bits.as_chunks::<4>().0 {
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
#[cfg(feature = "encode")]
#[derive(Debug, Default, Clone, Copy)]
pub struct MsiEncoder;

#[cfg(feature = "encode")]
impl MsiEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

    /// Number of modules [`MsiEncoder::encode_into`] emits for `data_len` payload
    /// digits, excluding quiet zones (exact). `meta.check` only matters for
    /// [`Symbology::MsiPlessey`]; any other symbology is sized as Plessey.
    pub const fn max_modules(symbology: Symbology, data_len: usize, meta: &MsiMeta) -> usize {
        match symbology {
            Symbology::MsiPlessey => 3 + 12 * (data_len + check_len(meta.check)) + 4,
            _ => PLESSEY_START_MODULES + 16 * data_len + 4 * 8 + PLESSEY_STOP_MODULES,
        }
    }

    /// Heap-free encoding: write `data` as `symbology` ([`Symbology::MsiPlessey`] or
    /// [`Symbology::Plessey`]) to `out`.
    ///
    /// MSI Plessey takes ASCII digits and appends the check digits of `meta.check`;
    /// Plessey takes hex digits `0-9 A-F` and ignores `meta`. The payload must not be
    /// empty. The input is validated before the first module is written. Size a
    /// [`LinearBuf`](crate::output::LinearBuf) with [`MsiEncoder::max_modules`].
    pub fn encode_into<S: LinearSink>(
        &self,
        symbology: Symbology,
        data: &[u8],
        meta: &MsiMeta,
        out: &mut S,
    ) -> Result<()> {
        match symbology {
            Symbology::MsiPlessey => {
                ensure_digits(data)?;
                let check = check_digits(data, meta.check)?;
                out.begin(QUIET_ZONE)?;
                msi_encode(out, data, &check[..check_len(meta.check)])
            }
            Symbology::Plessey => {
                for &c in data {
                    hex_value(c)?;
                }
                if data.is_empty() {
                    return Err(Error::invalid_data("Plessey payload is empty"));
                }
                out.begin(QUIET_ZONE)?;
                plessey_encode(out, data)
            }
            _ => Err(Error::invalid_parameter(
                "MsiEncoder given an unsupported symbology",
            )),
        }
    }
}

#[cfg(all(feature = "alloc", feature = "encode"))]
impl MsiEncoder {
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

#[cfg(all(feature = "alloc", feature = "encode"))]
impl Encode for MsiEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        let meta = match &symbol.meta {
            SymbolMeta::Msi(m) => m,
            _ => return Err(Error::invalid_parameter("symbol missing MsiMeta")),
        };
        let mut pattern = LinearPattern::new();
        self.encode_into(
            symbol.symbology,
            &symbol.payload_bytes(),
            meta,
            &mut pattern,
        )?;
        Ok(Encoding::Linear(pattern))
    }
}

/// MSI Plessey / Plessey decoder.
#[cfg(feature = "decode")]
#[derive(Debug, Default, Clone, Copy)]
pub struct MsiDecoder {
    plessey: bool,
    check: MsiCheck,
}

#[cfg(feature = "decode")]
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

#[cfg(feature = "decode")]
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

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    #[test]
    fn msi_check_reference() {
        // Wikipedia MSI example: 1234567 has mod-10 and mod-11 check digit 4.
        assert_eq!(msi_mod10(b"1234567"), 4);
        assert_eq!(msi_mod11(b"1234567"), Some(4));
    }

    #[test]
    fn plessey_crc_matches_long_division() {
        // Reference: explicit polynomial long division over a working copy.
        fn reference(data_bits: &[u8]) -> [u8; 8] {
            let mut work = data_bits.to_vec();
            work.extend([0u8; 8]);
            for i in 0..data_bits.len() {
                if work[i] == 1 {
                    for (j, &g) in PLESSEY_GRID.iter().enumerate() {
                        work[i + j] ^= g;
                    }
                }
            }
            work[data_bits.len()..].try_into().unwrap()
        }
        for digits in [&b"0"[..], b"1234", b"ABCDEF", b"DEADBEEF", b"F0F0F0F0F1"] {
            let bits: Vec<u8> = plessey_bits(digits).collect();
            assert_eq!(plessey_crc(bits.iter().copied()), reference(&bits));
        }
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
