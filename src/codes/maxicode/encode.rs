//! MaxiCode encoding: [`Symbol`] → [`BitMatrix`].
//!
//! [`MaxiCodeEncoder::build`] turns raw bytes into a reproducible [`Symbol`] by
//! running a code-set state machine (a simple latch-based encoder), storing the
//! resulting data-codeword body in [`MaxiCodeMeta`]. [`Encode::encode`] then places
//! that body verbatim, formats the Structured Carrier primary for modes 2/3, and
//! appends freshly computed Reed–Solomon — guaranteeing a lossless round-trip.

use super::tables::value_in_set;
use super::{Carrier, MaxiCodeMeta, TOTAL_CW, add_error_correction, body_len, render_matrix};
use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

// Code set indices.
const SET_A: usize = 0;
const SET_B: usize = 1;
const SET_C: usize = 2;
const SET_D: usize = 3;
const SET_E: usize = 4;

/// Preferred order in which to reach for a code set when the current one cannot hold
/// a character (A, B, E first — the sets that have a PAD symbol).
const SET_PREFERENCE: [usize; 5] = [SET_A, SET_B, SET_E, SET_C, SET_D];

/// MaxiCode encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MaxiCodeEncoder;

impl MaxiCodeEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        MaxiCodeEncoder
    }

    /// Build a reproducible Mode 4 [`Symbol`] from raw `data`.
    pub fn build(&self, data: &[u8]) -> Result<Symbol> {
        self.build_mode(data, 4)
    }

    /// Convenience: build a Mode 4 symbol from UTF-8 `text`.
    pub fn build_text(&self, text: &str) -> Result<Symbol> {
        self.build(text.as_bytes())
    }

    /// Build a data symbol in `mode` 4 (standard EC), 5 (enhanced EC) or 6 (reader
    /// programming).
    pub fn build_mode(&self, data: &[u8], mode: u8) -> Result<Symbol> {
        if !matches!(mode, 4..=6) {
            return Err(Error::invalid_parameter(
                "MaxiCodeEncoder::build_mode expects mode 4, 5 or 6",
            ));
        }
        let body = encode_body(data, mode)?;
        let segments = super::decode::decode_body(&body);
        Ok(Symbol::new(
            Symbology::MaxiCode,
            segments,
            SymbolMeta::MaxiCode(MaxiCodeMeta {
                mode,
                carrier: None,
                body,
            }),
        ))
    }

    /// Build a Structured Carrier symbol (mode 2 = numeric postcode, mode 3 =
    /// alphanumeric postcode), with a postal code, `country` code (0..=999) and
    /// `service` class (0..=999) in the primary message and `data` in the secondary.
    pub fn build_structured(
        &self,
        mode: u8,
        postcode: &str,
        country: u16,
        service: u16,
        data: &[u8],
    ) -> Result<Symbol> {
        let carrier = normalize_carrier(mode, postcode, country, service)?;
        let body = encode_body(data, mode)?;
        let segments = super::decode::decode_body(&body);
        Ok(Symbol::new(
            Symbology::MaxiCode,
            segments,
            SymbolMeta::MaxiCode(MaxiCodeMeta {
                mode,
                carrier: Some(carrier),
                body,
            }),
        ))
    }
}

impl Encode for MaxiCodeEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::MaxiCode {
            return Err(Error::invalid_parameter(
                "MaxiCodeEncoder given a non-MaxiCode symbol",
            ));
        }
        let SymbolMeta::MaxiCode(meta) = &symbol.meta else {
            return Err(Error::invalid_parameter(
                "MaxiCode symbol missing MaxiCodeMeta",
            ));
        };
        let mode = meta.mode;
        if !(2..=6).contains(&mode) {
            return Err(Error::invalid_parameter(
                "MaxiCode mode out of range (2..=6)",
            ));
        }
        if meta.body.len() != body_len(mode) {
            return Err(Error::invalid_parameter(
                "MaxiCode body length does not match the mode",
            ));
        }

        let mut cw = [0u8; TOTAL_CW];
        if mode <= 3 {
            let carrier = meta.carrier.as_ref().ok_or_else(|| {
                Error::invalid_parameter("MaxiCode mode 2/3 requires a Structured Carrier")
            })?;
            format_primary(&mut cw, mode, carrier)?;
            // The entire body is the secondary message.
            cw[20..20 + meta.body.len()].copy_from_slice(&meta.body);
        } else {
            cw[0] = mode;
            cw[1..10].copy_from_slice(&meta.body[0..9]);
            cw[20..20 + (meta.body.len() - 9)].copy_from_slice(&meta.body[9..]);
        }

        add_error_correction(&mut cw, mode);
        Ok(Encoding::Matrix(render_matrix(&cw)))
    }
}

/// Validate and normalize the Structured Carrier fields into their canonical stored
/// form (mode 3 postcode upper-cased and space-padded to 6 characters).
fn normalize_carrier(mode: u8, postcode: &str, country: u16, service: u16) -> Result<Carrier> {
    if !matches!(mode, 2 | 3) {
        return Err(Error::invalid_parameter(
            "MaxiCodeEncoder::build_structured expects mode 2 or 3",
        ));
    }
    if country > 999 || service > 999 {
        return Err(Error::invalid_parameter(
            "MaxiCode country code and service class must be 0..=999",
        ));
    }
    let postcode = if mode == 2 {
        if postcode.is_empty()
            || postcode.len() > 9
            || !postcode.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(Error::invalid_data(
                "MaxiCode mode 2 postcode must be 1..=9 digits",
            ));
        }
        postcode.to_string()
    } else {
        if postcode.len() > 6 {
            return Err(Error::invalid_data(
                "MaxiCode mode 3 postcode must be at most 6 characters",
            ));
        }
        let mut s = postcode.to_ascii_uppercase();
        // Only Code Set A characters are allowed.
        for b in s.bytes() {
            if value_in_set(SET_A, b).is_none() {
                return Err(Error::invalid_data(
                    "MaxiCode mode 3 postcode must use Code Set A characters",
                ));
            }
        }
        while s.len() < 6 {
            s.push(' ');
        }
        s
    };
    Ok(Carrier {
        postcode,
        country,
        service,
    })
}

/// Format the Structured Carrier primary message (`cw[0..10]`) for mode 2 or 3.
fn format_primary(cw: &mut [u8; TOTAL_CW], mode: u8, carrier: &Carrier) -> Result<()> {
    let country = carrier.country as u32;
    let service = carrier.service as u32;
    if mode == 2 {
        let pcn: u32 = carrier
            .postcode
            .parse()
            .map_err(|_| Error::invalid_data("MaxiCode mode 2 postcode must be numeric"))?;
        let len = carrier.postcode.len() as u32;
        cw[0] = (((pcn & 0x03) << 4) | 2) as u8;
        cw[1] = ((pcn & 0xFC) >> 2) as u8;
        cw[2] = ((pcn & 0x3F00) >> 8) as u8;
        cw[3] = ((pcn & 0xFC000) >> 14) as u8;
        cw[4] = ((pcn & 0x3F0_0000) >> 20) as u8;
        cw[5] = (((pcn & 0x3C00_0000) >> 26) | ((len & 0x03) << 4)) as u8;
        cw[6] = (((len & 0x3C) >> 2) | ((country & 0x03) << 4)) as u8;
        cw[7] = ((country & 0xFC) >> 2) as u8;
        cw[8] = (((country & 0x300) >> 8) | ((service & 0x0F) << 2)) as u8;
        cw[9] = ((service & 0x3F0) >> 4) as u8;
    } else {
        let mut p = [0u8; 6];
        for (k, b) in carrier.postcode.bytes().enumerate() {
            p[k] = value_in_set(SET_A, b)
                .ok_or_else(|| Error::invalid_data("MaxiCode mode 3 postcode character"))?;
        }
        let p = p.map(|v| v as u32);
        cw[0] = (((p[5] & 0x03) << 4) | 3) as u8;
        cw[1] = (((p[4] & 0x03) << 4) | ((p[5] & 0x3C) >> 2)) as u8;
        cw[2] = (((p[3] & 0x03) << 4) | ((p[4] & 0x3C) >> 2)) as u8;
        cw[3] = (((p[2] & 0x03) << 4) | ((p[3] & 0x3C) >> 2)) as u8;
        cw[4] = (((p[1] & 0x03) << 4) | ((p[2] & 0x3C) >> 2)) as u8;
        cw[5] = (((p[0] & 0x03) << 4) | ((p[1] & 0x3C) >> 2)) as u8;
        cw[6] = (((p[0] & 0x3C) >> 2) | ((country & 0x03) << 4)) as u8;
        cw[7] = ((country & 0xFC) >> 2) as u8;
        cw[8] = (((country & 0x300) >> 8) | ((service & 0x0F) << 2)) as u8;
        cw[9] = ((service & 0x3F0) >> 4) as u8;
    }
    Ok(())
}

/// Encode `data` into a full data-codeword body of `body_len(mode)` symbols, using a
/// latch-based code-set state machine and padding to length.
fn encode_body(data: &[u8], mode: u8) -> Result<Vec<u8>> {
    let cap = body_len(mode);
    let mut out: Vec<u8> = Vec::new();
    let mut state = SET_A;

    for &b in data {
        if let Some(v) = value_in_set(state, b) {
            out.push(v);
            continue;
        }
        let target = choose_set(b).ok_or_else(|| {
            Error::invalid_data(format!("byte {b:#04x} has no MaxiCode code set"))
        })?;
        emit_latch(&mut out, state, target);
        let v = value_in_set(target, b).expect("chosen set encodes the byte");
        out.push(v);
        state = target;
    }

    // Padding must happen in a set whose pad symbol is a no-op (A, B or E).
    if state == SET_C || state == SET_D {
        out.push(58); // Latch A (from C/D)
        state = SET_A;
    }
    if out.len() > cap {
        return Err(Error::capacity(format!(
            "data needs {} codewords but mode {mode} holds {cap}",
            out.len()
        )));
    }
    let pad = if state == SET_E { 28 } else { 33 };
    out.resize(cap, pad);
    Ok(out)
}

/// Pick a code set that can encode `byte`, honoring [`SET_PREFERENCE`].
fn choose_set(byte: u8) -> Option<usize> {
    SET_PREFERENCE
        .into_iter()
        .find(|&s| value_in_set(s, byte).is_some())
}

/// Emit the latch codeword(s) to move from code set `from` to `to`. Latching into C,
/// D or E uses a shift immediately followed by the target set's LOCK symbol.
fn emit_latch(out: &mut Vec<u8>, from: usize, to: usize) {
    match to {
        SET_A => out.push(if from == SET_B { 63 } else { 58 }),
        SET_B => out.push(63),
        SET_C => out.extend_from_slice(&[60, 60]),
        SET_D => out.extend_from_slice(&[61, 61]),
        SET_E => out.extend_from_slice(&[62, 62]),
        _ => unreachable!("invalid target code set {to}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_bytes_have_a_code_set() {
        for b in 0u16..=255 {
            assert!(
                choose_set(b as u8).is_some(),
                "byte {b:#04x} has no code set"
            );
        }
    }

    #[test]
    fn pure_set_a_needs_no_latches() {
        let body = encode_body(b"ABC123", 4).unwrap();
        assert_eq!(&body[0..6], &[1, 2, 3, 49, 50, 51]);
        assert_eq!(body.len(), body_len(4));
        // Trailing padding is the Code Set A PAD symbol.
        assert!(body[6..].iter().all(|&c| c == 33));
    }

    #[test]
    fn mode_marks_segment_mode_byte() {
        use crate::segment::Mode;
        let sym = MaxiCodeEncoder::new().build_text("HELLO").unwrap();
        assert!(sym.segments.iter().all(|s| s.mode == Mode::Byte));
    }
}
