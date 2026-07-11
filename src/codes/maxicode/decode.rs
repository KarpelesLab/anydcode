//! MaxiCode decoding: [`BitMatrix`] → [`Symbol`].
//!
//! This is the *structural* decoder: it consumes a clean, already-sampled 30×33
//! module grid, corrects errors with Reed–Solomon over GF(64), determines the mode,
//! reconstructs the Structured Carrier primary (modes 2/3), and re-runs the code-set
//! state machine to recover the segments and the exact [`MaxiCodeMeta`] needed to
//! re-encode identically.

use super::tables::{self, Cw};
use super::{
    Carrier, MaxiCodeMeta, TOTAL_CW, body_len, correct_primary, correct_secondary, read_codewords,
    secondary_lengths,
};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// MaxiCode structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MaxiCodeDecoder;

impl MaxiCodeDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        MaxiCodeDecoder
    }

    /// Decode a sampled MaxiCode module grid into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        if matrix.width() != tables::WIDTH || matrix.height() != tables::HEIGHT {
            return Err(Error::undecodable("MaxiCode grid must be 30×33 modules"));
        }
        let mut cw = read_codewords(matrix);

        // Correct the primary first so the mode codeword is trustworthy.
        correct_primary(&mut cw)?;
        let mode = cw[0] & 0x0F;
        if !(2..=6).contains(&mode) {
            return Err(Error::undecodable("MaxiCode mode out of range (2..=6)"));
        }
        correct_secondary(&mut cw, mode)?;

        let (carrier, body) = if mode <= 3 {
            let carrier = if mode == 2 {
                decode_primary_2(&cw)?
            } else {
                decode_primary_3(&cw)
            };
            (Some(carrier), cw[20..104].to_vec())
        } else {
            let (sec, _) = secondary_lengths(mode);
            let mut body = cw[1..10].to_vec();
            body.extend_from_slice(&cw[20..20 + sec]);
            (None, body)
        };
        debug_assert_eq!(body.len(), body_len(mode));

        let segments = decode_body(&body);
        Ok(Symbol::new(
            Symbology::MaxiCode,
            segments,
            SymbolMeta::MaxiCode(MaxiCodeMeta {
                mode,
                carrier,
                body,
            }),
        ))
    }
}

impl Decode for MaxiCodeDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "MaxiCode decode of a linear pattern",
            }),
        }
    }
}

/// Reconstruct the numeric Structured Carrier (Mode 2) primary fields.
fn decode_primary_2(cw: &[u8; TOTAL_CW]) -> Result<Carrier> {
    let g = |i: usize| cw[i] as u32;
    let pcn = ((g(0) >> 4) & 0x03)
        | (g(1) << 2)
        | (g(2) << 8)
        | (g(3) << 14)
        | (g(4) << 20)
        | ((g(5) & 0x0F) << 26);
    let len = (((g(5) >> 4) & 0x03) | ((g(6) & 0x0F) << 2)) as usize;
    if len == 0 || len > 9 {
        return Err(Error::undecodable(
            "MaxiCode mode 2 postcode length invalid",
        ));
    }
    let country = ((g(6) >> 4) & 0x03) | (g(7) << 2) | ((g(8) & 0x03) << 8);
    let service = ((g(8) >> 2) & 0x0F) | (g(9) << 4);
    Ok(Carrier {
        postcode: format!("{pcn:0len$}"),
        country: country as u16,
        service: service as u16,
    })
}

/// Reconstruct the alphanumeric Structured Carrier (Mode 3) primary fields.
fn decode_primary_3(cw: &[u8; TOTAL_CW]) -> Carrier {
    let g = |i: usize| cw[i] as u32;
    let vals = [
        ((g(5) >> 4) & 0x03) | ((g(6) & 0x0F) << 2), // p0
        ((g(4) >> 4) & 0x03) | ((g(5) & 0x0F) << 2), // p1
        ((g(3) >> 4) & 0x03) | ((g(4) & 0x0F) << 2), // p2
        ((g(2) >> 4) & 0x03) | ((g(3) & 0x0F) << 2), // p3
        ((g(1) >> 4) & 0x03) | ((g(2) & 0x0F) << 2), // p4
        ((g(0) >> 4) & 0x03) | ((g(1) & 0x0F) << 2), // p5
    ];
    let sets = tables::sets();
    let mut postcode = String::with_capacity(6);
    for v in vals {
        if let Cw::Byte(b) = sets[0][(v & 0x3F) as usize] {
            postcode.push(b as char);
        }
    }
    let country = ((g(6) >> 4) & 0x03) | (g(7) << 2) | ((g(8) & 0x03) << 8);
    let service = ((g(8) >> 2) & 0x0F) | (g(9) << 4);
    Carrier {
        postcode,
        country: country as u16,
        service: service as u16,
    }
}

/// Run the code-set state machine over a data-codeword body, recovering the payload
/// bytes as [`Segment`]s (byte runs split by ECI switches). Trailing padding is
/// dropped. This is the shared source of truth used by both the decoder and the
/// encoder's `build*` helpers, keeping their segments identical.
pub(crate) fn decode_body(body: &[u8]) -> Vec<Segment> {
    let sets = tables::sets();
    let mut segments: Vec<Segment> = Vec::new();
    let mut bytes: Vec<u8> = Vec::new();

    let mut set = 0usize;
    let mut last_set = 0usize;
    let mut shift: i32 = -1;
    let mut i = 0;

    while i < body.len() {
        let value = (body[i] & 0x3F) as usize;
        i += 1;
        match sets[set][value] {
            Cw::LatchA => {
                set = 0;
                shift = -1;
            }
            Cw::LatchB => {
                set = 1;
                shift = -1;
            }
            Cw::ShiftA => shift_to(&mut set, &mut last_set, &mut shift, 0, 1),
            Cw::ShiftB => shift_to(&mut set, &mut last_set, &mut shift, 1, 1),
            Cw::ShiftC => shift_to(&mut set, &mut last_set, &mut shift, 2, 1),
            Cw::ShiftD => shift_to(&mut set, &mut last_set, &mut shift, 3, 1),
            Cw::ShiftE => shift_to(&mut set, &mut last_set, &mut shift, 4, 1),
            Cw::TwoShiftA => shift_to(&mut set, &mut last_set, &mut shift, 0, 2),
            Cw::ThreeShiftA => shift_to(&mut set, &mut last_set, &mut shift, 0, 3),
            Cw::Lock => shift = -1,
            Cw::Pad => {} // trailing padding — ignored
            Cw::Ns => {
                if let Some(chunk) = body.get(i..i + 5) {
                    let v = (chunk[0] as u32) << 24
                        | (chunk[1] as u32) << 18
                        | (chunk[2] as u32) << 12
                        | (chunk[3] as u32) << 6
                        | (chunk[4] as u32);
                    i += 5;
                    for d in format!("{v:09}").bytes() {
                        bytes.push(d);
                    }
                } else {
                    break;
                }
            }
            Cw::Eci => {
                if let Some((eci, consumed)) = read_eci(&body[i..]) {
                    i += consumed;
                    if !bytes.is_empty() {
                        segments.push(Segment::byte(std::mem::take(&mut bytes)));
                    }
                    segments.push(Segment::eci(eci));
                } else {
                    break;
                }
            }
            Cw::Byte(b) => bytes.push(b),
        }
        // ZXing's `if (shift-- == 0) set = lastset;`: a shift lasts exactly its span.
        let expired = shift == 0;
        shift -= 1;
        if expired {
            set = last_set;
        }
    }

    if !bytes.is_empty() {
        segments.push(Segment::byte(bytes));
    }
    if segments.is_empty() {
        segments.push(Segment::byte(Vec::new()));
    }
    segments
}

/// Apply a shift of `span` characters to code set `target`.
fn shift_to(set: &mut usize, last_set: &mut usize, shift: &mut i32, target: usize, span: i32) {
    *last_set = *set;
    *set = target;
    *shift = span;
}

/// Decode an ECI assignment number from the codewords following the ECI marker,
/// returning `(eci, codewords_consumed)`.
fn read_eci(rest: &[u8]) -> Option<(u32, usize)> {
    let c1 = *rest.first()? as u32;
    if c1 < 0x20 {
        Some((c1, 1))
    } else if c1 < 0x30 {
        let c2 = *rest.get(1)? as u32;
        Some((((c1 & 0x0F) << 6) | c2, 2))
    } else if c1 < 0x38 {
        let c2 = *rest.get(1)? as u32;
        let c3 = *rest.get(2)? as u32;
        Some((((c1 & 0x07) << 12) | (c2 << 6) | c3, 3))
    } else {
        let c2 = *rest.get(1)? as u32;
        let c3 = *rest.get(2)? as u32;
        let c4 = *rest.get(3)? as u32;
        Some((((c1 & 0x03) << 18) | (c2 << 12) | (c3 << 6) | c4, 4))
    }
}
