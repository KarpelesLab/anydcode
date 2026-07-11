//! Data Matrix ECC 200 encoding: [`Symbol`] → [`BitMatrix`].
//!
//! The [`DataMatrixEncoder`] builds a reproducible symbol from raw bytes (choosing a
//! canonical ASCII/Base256 segmentation and the smallest fitting square size) or
//! renders an already-specified [`Symbol`] exactly from its [`DataMatrixMeta`].

use super::placement::{placement_map, render_borders};
use super::tables::{SquareSpec, smallest_square_for, square_by_size};
use super::{DataMatrixMeta, Encodation};
use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// ASCII codeword: pad / end-of-message.
const PAD: u8 = 129;
/// ASCII codeword: latch to Base256 encodation.
const BASE256_LATCH: u8 = 231;
/// ASCII codeword: upper shift (the next ASCII value gets 128 added).
const UPPER_SHIFT: u8 = 235;

/// Data Matrix encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DataMatrixEncoder;

impl DataMatrixEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        DataMatrixEncoder
    }

    /// Build a reproducible [`Symbol`] from raw `data`, choosing a canonical
    /// ASCII/Base256 segmentation and the smallest fitting square symbol size.
    pub fn build(&self, data: &[u8]) -> Result<Symbol> {
        self.build_inner(data, None)
    }

    /// Like [`DataMatrixEncoder::build`] but forcing a specific square symbol size
    /// (full side length in modules, e.g. `20` for a 20×20 symbol).
    pub fn build_sized(&self, data: &[u8], symbol_size: usize) -> Result<Symbol> {
        self.build_inner(data, Some(symbol_size))
    }

    /// Convenience: build from UTF-8 `text`.
    pub fn build_text(&self, text: &str) -> Result<Symbol> {
        self.build(text.as_bytes())
    }

    fn build_inner(&self, data: &[u8], forced: Option<usize>) -> Result<Symbol> {
        let segs = canonical_segments(data);
        let encodations: Vec<Encodation> = segs.iter().map(|(e, _)| *e).collect();
        let cw = data_codewords(&segs)?;
        let spec = match forced {
            Some(sz) => {
                let s = square_by_size(sz)
                    .ok_or_else(|| Error::invalid_parameter("unknown Data Matrix square size"))?;
                if cw.len() > s.data_cw {
                    return Err(Error::capacity("data does not fit in the requested size"));
                }
                s
            }
            None => smallest_square_for(cw.len())
                .ok_or_else(|| Error::capacity("data exceeds the largest square Data Matrix"))?,
        };
        let segments: Vec<Segment> = segs.into_iter().map(|(_, b)| Segment::byte(b)).collect();
        let meta = DataMatrixMeta {
            symbol_size: spec.symbol_size,
            encodations,
        };
        Ok(Symbol::new(
            Symbology::DataMatrix,
            segments,
            SymbolMeta::DataMatrix(meta),
        ))
    }
}

impl Encode for DataMatrixEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::DataMatrix {
            return Err(Error::invalid_parameter(
                "DataMatrixEncoder given a non-Data-Matrix symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::DataMatrix(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Data Matrix symbol missing DataMatrixMeta",
                ));
            }
        };
        if meta.encodations.len() != symbol.segments.len() {
            return Err(Error::invalid_parameter(
                "encodation count does not match segment count",
            ));
        }
        let spec = square_by_size(meta.symbol_size)
            .ok_or_else(|| Error::invalid_parameter("unknown Data Matrix square size"))?;

        // Re-pair (encodation, bytes) from the symbol's segments.
        let segs: Vec<(Encodation, Vec<u8>)> = symbol
            .segments
            .iter()
            .zip(&meta.encodations)
            .map(|(seg, enc)| {
                if !matches!(seg.mode, Mode::Byte) {
                    return Err(Error::Unsupported {
                        what: "Data Matrix supports only Byte-mode segments",
                    });
                }
                Ok((*enc, seg.data.clone()))
            })
            .collect::<Result<_>>()?;

        let mut cw = data_codewords(&segs)?;
        if cw.len() > spec.data_cw {
            return Err(Error::capacity("data does not fit in the symbol size"));
        }
        pad_codewords(&mut cw, spec.data_cw);
        let full = add_error_correction(&cw, &spec);
        let matrix = render(&spec, &full);
        Ok(Encoding::Matrix(matrix))
    }
}

/// Split `data` into a canonical alternating sequence of ASCII (bytes `< 128`) and
/// Base256 (bytes `>= 128`) runs. This is exactly the segmentation the decoder
/// reconstructs, keeping the round-trip lossless.
fn canonical_segments(data: &[u8]) -> Vec<(Encodation, Vec<u8>)> {
    let mut segs = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let high = data[i] >= 128;
        let start = i;
        while i < data.len() && (data[i] >= 128) == high {
            i += 1;
        }
        let enc = if high {
            Encodation::Base256
        } else {
            Encodation::Ascii
        };
        segs.push((enc, data[start..i].to_vec()));
    }
    if segs.is_empty() {
        segs.push((Encodation::Ascii, Vec::new()));
    }
    segs
}

/// Encode all segments into the data codeword stream (before padding).
fn data_codewords(segs: &[(Encodation, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut cw = Vec::new();
    for (enc, bytes) in segs {
        match enc {
            Encodation::Ascii => ascii_encode(&mut cw, bytes)?,
            Encodation::Base256 => base256_encode(&mut cw, bytes),
        }
    }
    Ok(cw)
}

/// Append `bytes` in ASCII encodation: digit pairs pack into one codeword, other
/// bytes below 128 map directly, and bytes `>= 128` use an upper shift.
fn ascii_encode(cw: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_digit() && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let val = (b - b'0') * 10 + (bytes[i + 1] - b'0');
            cw.push(130 + val);
            i += 2;
        } else if b < 128 {
            cw.push(b + 1);
            i += 1;
        } else {
            cw.push(UPPER_SHIFT);
            cw.push((b - 128) + 1);
            i += 1;
        }
    }
    Ok(())
}

/// Append `bytes` in Base256 encodation (latch, length field, randomized payload).
fn base256_encode(cw: &mut Vec<u8>, bytes: &[u8]) {
    cw.push(BASE256_LATCH);
    let count = bytes.len();
    let mut field = Vec::new();
    if count <= 249 {
        field.push(count as u8);
    } else {
        field.push((count / 250 + 249) as u8);
        field.push((count % 250) as u8);
    }
    for &b in field.iter().chain(bytes.iter()) {
        let pos = cw.len() + 1;
        cw.push(randomize_255(b, pos));
    }
}

/// The Base256 "255-state" randomizing algorithm (ISO/IEC 16022 §5.2.6).
fn randomize_255(value: u8, position: usize) -> u8 {
    let pseudo = ((149 * position) % 255) + 1;
    let t = value as usize + pseudo;
    (if t <= 255 { t } else { t - 256 }) as u8
}

/// Pad the data codewords out to `capacity` using the "253-state" pad algorithm.
fn pad_codewords(cw: &mut Vec<u8>, capacity: usize) {
    if cw.len() < capacity {
        cw.push(PAD);
    }
    while cw.len() < capacity {
        let pos = cw.len() + 1;
        let pseudo = ((149 * pos) % 253) + 1;
        let mut t = 129 + pseudo;
        if t > 254 {
            t -= 254;
        }
        cw.push(t as u8);
    }
}

/// Append interleaved Reed–Solomon error codewords to the (padded) data stream.
fn add_error_correction(data: &[u8], spec: &SquareSpec) -> Vec<u8> {
    let bc = spec.blocks;
    let epb = spec.ec_per_block();
    let mut ec_stream = vec![0u8; spec.ec_cw];
    for b in 0..bc {
        let block_data: Vec<u8> = data.iter().skip(b).step_by(bc).copied().collect();
        let ecc = super::gf::encode(&block_data, epb);
        for (e, &val) in ecc.iter().enumerate() {
            ec_stream[b + e * bc] = val;
        }
    }
    let mut full = data.to_vec();
    full.extend_from_slice(&ec_stream);
    full
}

/// Place the full codeword stream into the mapping matrix and add borders.
fn render(spec: &SquareSpec, full: &[u8]) -> crate::output::BitMatrix {
    let ms = spec.mapping_size();
    let pm = placement_map(ms, ms);
    debug_assert_eq!(full.len(), spec.total_cw());
    debug_assert_eq!(full.len(), pm.codewords.len());
    let mut mapping = vec![false; ms * ms];
    for (c, positions) in pm.codewords.iter().enumerate() {
        let byte = full[c];
        for (bit, &(r, col)) in positions.iter().enumerate() {
            mapping[r * ms + col] = (byte >> (7 - bit)) & 1 != 0;
        }
    }
    for &(r, col) in &pm.fixed_dark {
        mapping[r * ms + col] = true;
    }
    render_borders(spec, &mapping)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ISO/IEC 16022 worked example: "123456" ASCII-encodes to three digit-pair
    /// codewords 142, 164, 186 which exactly fill the 10×10 symbol's data capacity.
    #[test]
    fn iso_example_data_codewords() {
        let segs = canonical_segments(b"123456");
        let cw = data_codewords(&segs).unwrap();
        assert_eq!(cw, vec![142, 164, 186]);
    }

    #[test]
    fn base256_length_field_and_latch() {
        let cw = data_codewords(&[(Encodation::Base256, vec![0x80, 0x81])]).unwrap();
        // latch + 1-byte length + 2 randomized payload bytes.
        assert_eq!(cw.len(), 4);
        assert_eq!(cw[0], BASE256_LATCH);
    }
}
