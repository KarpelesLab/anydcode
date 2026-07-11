//! QR encoding: [`Symbol`] → [`BitMatrix`].
//!
//! The [`QrEncoder`] can either reproduce an exact symbol from a fully-specified
//! [`QrMeta`] (the round-trip path) or build a fresh symbol from segments, choosing
//! the smallest fitting version and the lowest-penalty mask.

use super::matrix::Canvas;
use super::tables::{char_count_bits, ec_blocks, mode_indicator, remainder_bits};
use super::{EcLevel, Mask, QrMeta, Version};
use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// QR Code encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct QrEncoder;

impl QrEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        QrEncoder
    }

    /// Build a reproducible [`Symbol`] from `segments` at error-correction `level`,
    /// choosing the smallest fitting version and the lowest-penalty mask. The
    /// returned symbol's [`QrMeta`] pins those choices so re-encoding is identical.
    pub fn build(&self, segments: Vec<Segment>, level: EcLevel) -> Result<Symbol> {
        let version = choose_version(&segments, level)?;
        let (_, mask) = choose_mask(&segments, version, level)?;
        let meta = QrMeta {
            version,
            ec_level: level,
            mask,
        };
        Ok(Symbol::new(
            Symbology::QrCode,
            segments,
            SymbolMeta::Qr(meta),
        ))
    }

    /// Convenience: build a symbol from UTF-8 `text` as a single byte segment.
    pub fn build_text(&self, text: &str, level: EcLevel) -> Result<Symbol> {
        self.build(vec![Segment::byte(text.as_bytes().to_vec())], level)
    }
}

impl Encode for QrEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::QrCode {
            return Err(Error::invalid_parameter("QrEncoder given a non-QR symbol"));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Qr(m) => m,
            _ => return Err(Error::invalid_parameter("QR symbol missing QrMeta")),
        };
        let canvas = render(&symbol.segments, meta.version, meta.ec_level, meta.mask)?;
        Ok(Encoding::Matrix(canvas.to_bitmatrix()))
    }
}

/// Accumulates bits most-significant-first for the QR data stream.
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

/// Encode the ECI assignment number using the variable-length QR encoding.
fn write_eci_assignment(w: &mut BitWriter, n: u32) -> Result<()> {
    if n < 128 {
        w.push(n, 8);
    } else if n < 16384 {
        w.push(0b10 << 14 | n, 16);
    } else if n < 1_000_000 {
        w.push(0b110 << 21 | n, 24);
    } else {
        return Err(Error::invalid_data("ECI assignment out of range"));
    }
    Ok(())
}

/// Write one segment's header and payload bits.
fn write_segment(w: &mut BitWriter, seg: &Segment, version: Version) -> Result<()> {
    // Mode indicator (4 bits) for every segment including ECI.
    w.push(mode_indicator(&seg.mode) as u32, 4);
    match &seg.mode {
        Mode::Eci(n) => {
            write_eci_assignment(w, *n)?;
            // ECI carries no payload of its own; the following segment does.
        }
        Mode::Numeric => {
            let n = seg.data.len();
            w.push(n as u32, char_count_bits(version, &seg.mode));
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
            w.push(seg.data.len() as u32, char_count_bits(version, &seg.mode));
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
            w.push(seg.data.len() as u32, char_count_bits(version, &seg.mode));
            for &b in &seg.data {
                w.push(b as u32, 8);
            }
        }
        Mode::Kanji => {
            if !seg.data.len().is_multiple_of(2) {
                return Err(Error::invalid_data("odd-length kanji segment"));
            }
            w.push(
                (seg.data.len() / 2) as u32,
                char_count_bits(version, &seg.mode),
            );
            for pair in seg.data.chunks(2) {
                let v = kanji_value(pair[0], pair[1])
                    .ok_or_else(|| Error::invalid_data("bad kanji char"))?;
                w.push(v, 13);
            }
        }
    }
    Ok(())
}

/// Total encoded bit length of all segments at a version (headers + payload).
fn segments_bit_len(segments: &[Segment], version: Version) -> Result<usize> {
    let mut w = BitWriter::new();
    for seg in segments {
        write_segment(&mut w, seg, version)?;
    }
    Ok(w.len())
}

/// The smallest version (at `level`) whose data capacity holds `segments`.
fn choose_version(segments: &[Segment], level: EcLevel) -> Result<Version> {
    for v in 1..=40 {
        let version = Version::new(v).unwrap();
        let capacity = ec_blocks(version, level).total_data() * 8;
        // char-count sizes change across version bands; recompute per version.
        if let Ok(len) = segments_bit_len(segments, version)
            && len <= capacity
        {
            return Ok(version);
        }
    }
    Err(Error::capacity(
        "data does not fit in any QR version at this EC level",
    ))
}

/// Build the full interleaved codeword bit stream (data + EC + remainder) for a
/// version/level, ready to place along the data path.
fn codeword_bits(segments: &[Segment], version: Version, level: EcLevel) -> Result<Vec<bool>> {
    let ecb = ec_blocks(version, level);
    let data_capacity = ecb.total_data() * 8;

    let mut w = BitWriter::new();
    for seg in segments {
        write_segment(&mut w, seg, version)?;
    }
    if w.len() > data_capacity {
        return Err(Error::capacity("segments exceed selected version capacity"));
    }
    // Terminator: up to 4 zero bits, bounded by remaining capacity.
    let term = (data_capacity - w.len()).min(4);
    w.push(0, term);
    // Pad to a byte boundary.
    while !w.len().is_multiple_of(8) {
        w.bits.push(false);
    }
    // Pack to data codewords.
    let mut data: Vec<u8> = w
        .bits
        .chunks(8)
        .map(|c| c.iter().fold(0u8, |acc, &b| (acc << 1) | b as u8))
        .collect();
    // Pad codewords with the alternating 0xEC / 0x11 bytes.
    let pad = [0xEC_u8, 0x11];
    let mut pi = 0;
    while data.len() < ecb.total_data() {
        data.push(pad[pi % 2]);
        pi += 1;
    }

    // Split into blocks and compute EC codewords per block.
    let mut blocks: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(ecb.total_blocks());
    let mut pos = 0;
    for (count, size) in [
        (ecb.group1_blocks, ecb.group1_data),
        (ecb.group2_blocks, ecb.group2_data),
    ] {
        for _ in 0..count {
            let d = data[pos..pos + size].to_vec();
            pos += size;
            let ec = super::gf::encode(&d, ecb.ec_per_block);
            blocks.push((d, ec));
        }
    }

    // Interleave data codewords, then EC codewords.
    let mut out: Vec<u8> = Vec::with_capacity(ecb.total_codewords());
    let max_data = ecb.group1_data.max(ecb.group2_data);
    for i in 0..max_data {
        for (d, _) in &blocks {
            if i < d.len() {
                out.push(d[i]);
            }
        }
    }
    for i in 0..ecb.ec_per_block {
        for (_, ec) in &blocks {
            out.push(ec[i]);
        }
    }

    // To bits, MSB first, then remainder bits.
    let mut bits = Vec::with_capacity(out.len() * 8 + remainder_bits(version));
    for byte in out {
        for k in (0..8).rev() {
            bits.push((byte >> k) & 1 != 0);
        }
    }
    bits.extend(std::iter::repeat_n(false, remainder_bits(version)));
    Ok(bits)
}

/// Render a canvas for the given parameters, applying the given mask.
fn render(segments: &[Segment], version: Version, level: EcLevel, mask: Mask) -> Result<Canvas> {
    let bits = codeword_bits(segments, version, level)?;
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
    canvas.place_format(level, mask);
    canvas.place_version();
    Ok(canvas)
}

/// Choose the lowest-penalty mask for the given data, returning the rendered canvas.
fn choose_mask(segments: &[Segment], version: Version, level: EcLevel) -> Result<(Canvas, Mask)> {
    let mut best: Option<(Canvas, Mask, u32)> = None;
    for m in 0..8 {
        let mask = Mask::new(m).unwrap();
        let canvas = render(segments, version, level, mask)?;
        let penalty = canvas.penalty();
        if best.as_ref().is_none_or(|(_, _, p)| penalty < *p) {
            best = Some((canvas, mask, penalty));
        }
    }
    let (canvas, mask, _) = best.unwrap();
    Ok((canvas, mask))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full ISO/IEC 18004 worked example: numeric "01234567" at V1-M produces
    /// exactly these 16 data + 10 EC codewords. This validates mode encoding,
    /// character-count sizing, terminator and pad bytes, and RS end to end.
    #[test]
    fn iso_example_full_codewords() {
        let segments = vec![Segment::numeric(b"01234567".to_vec())];
        let version = Version::new(1).unwrap();
        let bits = codeword_bits(&segments, version, EcLevel::M).unwrap();
        // Pack back to bytes (V1 has no remainder bits).
        let bytes: Vec<u8> = bits
            .chunks(8)
            .map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | b as u8))
            .collect();
        let expected: [u8; 26] = [
            16, 32, 12, 86, 97, 128, 236, 17, 236, 17, 236, 17, 236, 17, 236, 17, // data
            165, 36, 212, 193, 237, 54, 199, 135, 44, 85, // EC
        ];
        assert_eq!(bytes, expected);
    }
}
