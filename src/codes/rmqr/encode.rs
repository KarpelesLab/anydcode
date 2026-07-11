//! rMQR encoding: [`Symbol`] → [`BitMatrix`].
//!
//! The [`RmqrEncoder`] reproduces an exact symbol from a fully-specified
//! [`RmqrMeta`] (the round-trip path), or builds a fresh symbol from segments,
//! choosing the smallest fitting size.

use super::matrix::Canvas;
use super::tables::{char_count_bits, info, mode_value};
use super::{RmqrEcLevel, RmqrMeta, RmqrSize};
use crate::codes::qr::gf;
use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// Rectangular Micro QR encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct RmqrEncoder;

impl RmqrEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        RmqrEncoder
    }

    /// Build a reproducible [`Symbol`] from `segments` at EC `level`, choosing the
    /// smallest-area fitting size.
    pub fn build(&self, segments: Vec<Segment>, level: RmqrEcLevel) -> Result<Symbol> {
        let size = choose_size(&segments, level)?;
        let meta = RmqrMeta {
            size,
            ec_level: level,
        };
        Ok(Symbol::new(
            Symbology::RectMicroQrCode,
            segments,
            SymbolMeta::Rmqr(meta),
        ))
    }

    /// Convenience: build a symbol from UTF-8 `text` as a single byte segment.
    pub fn build_text(&self, text: &str, level: RmqrEcLevel) -> Result<Symbol> {
        self.build(vec![Segment::byte(text.as_bytes().to_vec())], level)
    }
}

impl Encode for RmqrEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::RectMicroQrCode {
            return Err(Error::invalid_parameter(
                "RmqrEncoder given a non-rMQR symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Rmqr(m) => m,
            _ => return Err(Error::invalid_parameter("rMQR symbol missing RmqrMeta")),
        };
        let canvas = render(&symbol.segments, meta.size, meta.ec_level)?;
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

/// Write one segment's 3-bit mode indicator, character count and payload bits.
fn write_segment(w: &mut BitWriter, seg: &Segment, size: RmqrSize) -> Result<()> {
    let mv = mode_value(&seg.mode).ok_or_else(|| Error::invalid_data("ECI unsupported in rMQR"))?;
    w.push(mv as u32, 3);
    let ccb = char_count_bits(size, &seg.mode)
        .ok_or_else(|| Error::invalid_data("ECI unsupported in rMQR"))?;
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
        Mode::Eci(_) => return Err(Error::invalid_data("ECI unsupported in rMQR")),
    }
    Ok(())
}

/// Encoded bit length of the segments at a size, or `None` if a mode is invalid.
fn segments_bit_len(segments: &[Segment], size: RmqrSize) -> Option<usize> {
    let mut w = BitWriter::new();
    for seg in segments {
        write_segment(&mut w, seg, size).ok()?;
    }
    Some(w.len())
}

/// The smallest-area size (at `level`) whose data capacity holds `segments`.
fn choose_size(segments: &[Segment], level: RmqrEcLevel) -> Result<RmqrSize> {
    let mut best: Option<(RmqrSize, usize)> = None;
    for size in RmqrSize::all() {
        let cap = info(size).data_bit_capacity(level);
        if let Some(len) = segments_bit_len(segments, size)
            && len <= cap
        {
            let area = size.width() * size.height();
            if best.as_ref().is_none_or(|&(_, a)| area < a) {
                best = Some((size, area));
            }
        }
    }
    best.map(|(s, _)| s)
        .ok_or_else(|| Error::capacity("data does not fit any rMQR size at this EC level"))
}

/// Build the full interleaved codeword bit stream (data + EC + remainder).
fn codeword_bits(segments: &[Segment], size: RmqrSize, ec: RmqrEcLevel) -> Result<Vec<bool>> {
    let si = info(size);
    let cap = si.data_bit_capacity(ec);

    let mut w = BitWriter::new();
    for seg in segments {
        write_segment(&mut w, seg, size)?;
    }
    if w.len() > cap {
        return Err(Error::capacity("segments exceed selected rMQR capacity"));
    }
    // Terminator (3 bits) if it fits.
    if w.len() + 3 <= cap {
        w.push(0, 3);
    }
    while !w.len().is_multiple_of(8) {
        w.bits.push(false);
    }
    let mut data: Vec<u8> = w
        .bits
        .chunks(8)
        .map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | b as u8))
        .collect();
    let total_data = si.total_data_codewords(ec);
    let pad = [0xEC_u8, 0x11];
    let mut pi = 0;
    while data.len() < total_data {
        data.push(pad[pi % 2]);
        pi += 1;
    }

    // Split into RS blocks and compute EC codewords.
    let mut data_blocks: Vec<Vec<u8>> = Vec::new();
    let mut ec_blocks: Vec<Vec<u8>> = Vec::new();
    let mut pos = 0;
    for group in si.blocks(ec) {
        let ecn = group.total - group.data;
        for _ in 0..group.num {
            let d = data[pos..pos + group.data].to_vec();
            pos += group.data;
            let e = gf::encode(&d, ecn);
            data_blocks.push(d);
            ec_blocks.push(e);
        }
    }

    // Interleave data codewords, then EC codewords.
    let mut out: Vec<u8> = Vec::with_capacity(si.total_codewords(ec));
    let max_k = data_blocks.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..max_k {
        for b in &data_blocks {
            if i < b.len() {
                out.push(b[i]);
            }
        }
    }
    let max_c = ec_blocks.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..max_c {
        for b in &ec_blocks {
            if i < b.len() {
                out.push(b[i]);
            }
        }
    }

    let mut bits = Vec::with_capacity(out.len() * 8 + si.remainder);
    for byte in out {
        for k in (0..8).rev() {
            bits.push((byte >> k) & 1 != 0);
        }
    }
    bits.extend(std::iter::repeat_n(false, si.remainder));
    Ok(bits)
}

/// Render a canvas for the given parameters.
fn render(segments: &[Segment], size: RmqrSize, ec: RmqrEcLevel) -> Result<Canvas> {
    let bits = codeword_bits(segments, size, ec)?;
    let mut canvas = Canvas::new(size);
    canvas.place_format(ec);
    let path = canvas.data_path(canvas.data_cell_count(ec));
    if bits.len() != path.len() {
        return Err(Error::invalid_parameter(format!(
            "codeword bit count {} != data cell count {}",
            bits.len(),
            path.len()
        )));
    }
    for (&(x, y), &b) in path.iter().zip(&bits) {
        canvas.place_data_bit(x, y, b);
    }
    canvas.apply_mask(&path);
    Ok(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R7x43-M byte "A": derive the data codewords by hand and check the EC via the
    /// shared QR field. mode `011`, count(3)=`001`, byte `01000001` → then terminator
    /// and 0xEC/0x11 padding fill the 6 data codewords.
    #[test]
    fn r7x43_byte_codewords() {
        let size = RmqrSize::from_dimensions(43, 7).unwrap();
        let segments = vec![Segment::byte(b"A".to_vec())];
        let bits = codeword_bits(&segments, size, RmqrEcLevel::M).unwrap();
        // 6 data + 7 EC = 13 codewords, no remainder for R7x43.
        let bytes: Vec<u8> = bits
            .chunks(8)
            .map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | b as u8))
            .collect();
        assert_eq!(bytes.len(), 13);
        // "011"|"001"|"01000001"|"000" terminator, zero-padded to 24 bits = 0x65,
        // 0x04, 0x00, then 0xEC/0x11/0xEC pad codewords to fill 6 data codewords.
        assert_eq!(&bytes[..6], &[0x65, 0x04, 0x00, 0xEC, 0x11, 0xEC]);
    }
}
