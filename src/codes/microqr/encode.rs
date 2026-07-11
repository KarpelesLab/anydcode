//! Micro QR encoding: [`Symbol`] → [`BitMatrix`].
//!
//! The [`MicroQrEncoder`] reproduces an exact symbol from a fully-specified
//! [`MicroQrMeta`] (the round-trip path), or builds a fresh symbol from segments,
//! choosing the smallest fitting version and the highest-scoring mask.

use super::matrix::Canvas;
use super::tables::{
    char_count_bits, data_bit_capacity, ec_params, has_short_final_codeword, micro_mode_value,
    mode_indicator_bits, symbol_number, terminator_bits,
};
use super::{MicroEcLevel, MicroMask, MicroQrMeta, MicroVersion};
use crate::codes::qr::gf;
use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// Micro QR Code encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MicroQrEncoder;

impl MicroQrEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        MicroQrEncoder
    }

    /// Build a reproducible [`Symbol`] from `segments` at error-correction `level`,
    /// choosing the smallest fitting version and the highest-scoring mask. `level`
    /// must be supported by the chosen version (use [`MicroEcLevel::Detection`] to
    /// permit the tiny M1 numeric-only symbol).
    pub fn build(&self, segments: Vec<Segment>, level: MicroEcLevel) -> Result<Symbol> {
        let version = choose_version(&segments, level)?;
        let mask = choose_mask(&segments, version, level)?;
        let meta = MicroQrMeta {
            version,
            ec_level: level,
            mask,
        };
        Ok(Symbol::new(
            Symbology::MicroQrCode,
            segments,
            SymbolMeta::MicroQr(meta),
        ))
    }

    /// Convenience: build a numeric M1-friendly / higher symbol from ASCII digits.
    pub fn build_numeric(&self, digits: &[u8], level: MicroEcLevel) -> Result<Symbol> {
        self.build(vec![Segment::numeric(digits.to_vec())], level)
    }
}

impl Encode for MicroQrEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::MicroQrCode {
            return Err(Error::invalid_parameter(
                "MicroQrEncoder given a non-Micro-QR symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::MicroQr(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Micro QR symbol missing MicroQrMeta",
                ));
            }
        };
        let canvas = render(&symbol.segments, meta.version, meta.ec_level, meta.mask)?;
        Ok(Encoding::Matrix(canvas.to_bitmatrix()))
    }
}

/// Accumulates bits most-significant-first.
struct BitWriter {
    bits: Vec<bool>,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter { bits: Vec::new() }
    }

    fn push(&mut self, value: u32, len: usize) {
        for k in (0..len).rev() {
            self.bits.push((value >> k) & 1 != 0);
        }
    }

    fn len(&self) -> usize {
        self.bits.len()
    }
}

/// The alphanumeric value of a character, or `None` if outside the set.
fn alnum_value(b: u8) -> Option<u32> {
    let v = match b {
        b'0'..=b'9' => b - b'0',
        b'A'..=b'Z' => b - b'A' + 10,
        b' ' => 36,
        b'$' => 37,
        b'%' => 38,
        b'*' => 39,
        b'+' => 40,
        b'-' => 41,
        b'.' => 42,
        b'/' => 43,
        b':' => 44,
        _ => return None,
    };
    Some(v as u32)
}

/// Convert a Shift-JIS double byte to the 13-bit QR Kanji value, if in range.
fn kanji_value(hi: u8, lo: u8) -> Option<u32> {
    let code = ((hi as u32) << 8) | lo as u32;
    let base = if (0x8140..=0x9FFC).contains(&code) {
        code - 0x8140
    } else if (0xE040..=0xEBBF).contains(&code) {
        code - 0xC140
    } else {
        return None;
    };
    Some((base >> 8) * 0xC0 + (base & 0xFF))
}

/// Write one segment's header and payload bits.
fn write_segment(w: &mut BitWriter, seg: &Segment, version: MicroVersion) -> Result<()> {
    let mib = mode_indicator_bits(version);
    if mib > 0 {
        let mv = micro_mode_value(&seg.mode)
            .ok_or_else(|| Error::invalid_data("ECI unsupported in Micro QR"))?;
        w.push(mv as u32, mib);
    }
    let ccb = char_count_bits(version, &seg.mode).ok_or_else(|| {
        Error::invalid_data("segment mode not permitted at this Micro QR version")
    })?;
    match &seg.mode {
        Mode::Numeric => {
            w.push(seg.data.len() as u32, ccb);
            for chunk in seg.data.chunks(3) {
                let mut val = 0u32;
                for &d in chunk {
                    if !d.is_ascii_digit() {
                        return Err(Error::invalid_data("non-digit in numeric segment"));
                    }
                    val = val * 10 + (d - b'0') as u32;
                }
                let bits = match chunk.len() {
                    3 => 10,
                    2 => 7,
                    _ => 4,
                };
                w.push(val, bits);
            }
        }
        Mode::Alphanumeric => {
            w.push(seg.data.len() as u32, ccb);
            for pair in seg.data.chunks(2) {
                if pair.len() == 2 {
                    let a = alnum_value(pair[0])
                        .ok_or_else(|| Error::invalid_data("bad alphanumeric char"))?;
                    let b = alnum_value(pair[1])
                        .ok_or_else(|| Error::invalid_data("bad alphanumeric char"))?;
                    w.push(a * 45 + b, 11);
                } else {
                    let a = alnum_value(pair[0])
                        .ok_or_else(|| Error::invalid_data("bad alphanumeric char"))?;
                    w.push(a, 6);
                }
            }
        }
        Mode::Byte => {
            w.push(seg.data.len() as u32, ccb);
            for &b in &seg.data {
                w.push(b as u32, 8);
            }
        }
        Mode::Kanji => {
            if !seg.data.len().is_multiple_of(2) {
                return Err(Error::invalid_data("odd-length kanji segment"));
            }
            w.push((seg.data.len() / 2) as u32, ccb);
            for pair in seg.data.chunks(2) {
                let v = kanji_value(pair[0], pair[1])
                    .ok_or_else(|| Error::invalid_data("bad kanji char"))?;
                w.push(v, 13);
            }
        }
        Mode::Eci(_) => return Err(Error::invalid_data("ECI unsupported in Micro QR")),
    }
    Ok(())
}

/// Total encoded bit length of all segments at a version, or `None` if any segment's
/// mode is not representable at that version.
fn segments_bit_len(segments: &[Segment], version: MicroVersion) -> Option<usize> {
    let mut w = BitWriter::new();
    for seg in segments {
        write_segment(&mut w, seg, version).ok()?;
    }
    Some(w.len())
}

/// The smallest version (at `level`) whose data capacity holds `segments`.
fn choose_version(segments: &[Segment], level: MicroEcLevel) -> Result<MicroVersion> {
    for v in [
        MicroVersion::M1,
        MicroVersion::M2,
        MicroVersion::M3,
        MicroVersion::M4,
    ] {
        let Some(cap) = data_bit_capacity(v, level) else {
            continue; // level not supported at this version
        };
        if let Some(len) = segments_bit_len(segments, v)
            && len <= cap
        {
            return Ok(v);
        }
    }
    Err(Error::capacity(
        "data does not fit any Micro QR version at this EC level",
    ))
}

/// Build the full codeword bit stream (data + EC) for a version/level.
fn codeword_bits(
    segments: &[Segment],
    version: MicroVersion,
    ec: MicroEcLevel,
) -> Result<Vec<bool>> {
    let params = ec_params(version, ec)
        .ok_or_else(|| Error::invalid_parameter("unsupported Micro QR version/EC combination"))?;
    let cap_bits = data_bit_capacity(version, ec).unwrap();
    let short = has_short_final_codeword(version);

    let mut w = BitWriter::new();
    for seg in segments {
        write_segment(&mut w, seg, version)?;
    }
    if w.len() > cap_bits {
        return Err(Error::capacity(
            "segments exceed selected Micro QR capacity",
        ));
    }
    // Terminator, bounded by remaining capacity.
    let term = (cap_bits - w.len()).min(terminator_bits(version));
    w.push(0, term);

    // Assemble data codewords.
    let mut data: Vec<u8> = Vec::with_capacity(params.data_cw);
    if short {
        // Zero-fill to the (…8-bit + final 4-bit) capacity, then pack.
        while w.len() < cap_bits {
            w.bits.push(false);
        }
        for c in 0..params.data_cw - 1 {
            data.push(pack_byte(&w.bits[c * 8..c * 8 + 8]));
        }
        // Final 4-bit codeword lives in the high nibble of the RS symbol.
        let nib_start = (params.data_cw - 1) * 8;
        let mut nibble = 0u8;
        for k in 0..4 {
            nibble = (nibble << 1) | w.bits[nib_start + k] as u8;
        }
        data.push(nibble << 4);
    } else {
        while !w.len().is_multiple_of(8) {
            w.bits.push(false);
        }
        for chunk in w.bits.chunks(8) {
            data.push(pack_byte(chunk));
        }
        let pad = [0xEC_u8, 0x11];
        let mut pi = 0;
        while data.len() < params.data_cw {
            data.push(pad[pi % 2]);
            pi += 1;
        }
    }

    let ec_bytes = gf::encode(&data, params.ec_cw);

    // Emit final bit stream: data codewords, then EC codewords, MSB first.
    let mut bits = Vec::with_capacity(super::tables::data_module_count(version));
    if short {
        for &b in &data[..params.data_cw - 1] {
            push_byte(&mut bits, b);
        }
        // Only the high nibble of the final codeword is placed.
        let last = data[params.data_cw - 1];
        for k in (4..8).rev() {
            bits.push((last >> k) & 1 != 0);
        }
    } else {
        for &b in &data {
            push_byte(&mut bits, b);
        }
    }
    for &b in &ec_bytes {
        push_byte(&mut bits, b);
    }
    Ok(bits)
}

fn pack_byte(chunk: &[bool]) -> u8 {
    chunk.iter().fold(0u8, |acc, &b| (acc << 1) | b as u8)
}

fn push_byte(bits: &mut Vec<bool>, byte: u8) {
    for k in (0..8).rev() {
        bits.push((byte >> k) & 1 != 0);
    }
}

/// Render a canvas for the given parameters, applying `mask`.
fn render(
    segments: &[Segment],
    version: MicroVersion,
    ec: MicroEcLevel,
    mask: MicroMask,
) -> Result<Canvas> {
    let bits = codeword_bits(segments, version, ec)?;
    let mut canvas = Canvas::new(version);
    let path = canvas.data_path();
    if bits.len() != path.len() {
        return Err(Error::invalid_parameter(format!(
            "codeword bit count {} != data module count {}",
            bits.len(),
            path.len()
        )));
    }
    for (&(x, y), &b) in path.iter().zip(&bits) {
        canvas.place_data_bit(x, y, b);
    }
    canvas.apply_mask(mask);
    let sn = symbol_number(version, ec)
        .ok_or_else(|| Error::invalid_parameter("unsupported Micro QR version/EC combination"))?;
    canvas.place_format(sn, mask);
    Ok(canvas)
}

/// Choose the highest-scoring mask (ISO/IEC 18004 §7.8.3.2), ties resolved to the
/// lowest index.
fn choose_mask(segments: &[Segment], version: MicroVersion, ec: MicroEcLevel) -> Result<MicroMask> {
    let mut best: Option<(MicroMask, u32)> = None;
    for m in 0..4 {
        let mask = MicroMask::new(m).unwrap();
        let canvas = render(segments, version, ec, mask)?;
        let score = canvas.evaluate();
        if best.as_ref().is_none_or(|&(_, s)| score > s) {
            best = Some((mask, score));
        }
    }
    Ok(best.unwrap().0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M2-M numeric "01234567". The 4 data codewords are derived by hand from the
    /// ISO/IEC 18004 bit layout (mode `0`, count `1000`, then 10/10/7-bit digit
    /// groups → 0x40,0x18,0xAC,0xC3); the 6 EC codewords come from the shared QR
    /// Reed–Solomon field.
    #[test]
    fn m2_example_codewords() {
        let segments = vec![Segment::numeric(b"01234567".to_vec())];
        let bits = codeword_bits(&segments, MicroVersion::M2, MicroEcLevel::M).unwrap();
        let bytes: Vec<u8> = bits.chunks(8).map(pack_byte).collect();
        let expected: [u8; 10] = [64, 24, 172, 195, 211, 226, 194, 57, 150, 107];
        assert_eq!(bytes, expected);
    }
}
