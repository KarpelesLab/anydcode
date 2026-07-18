//! QR decoding: [`BitMatrix`] → [`Symbol`].
//!
//! This is the *structural* decoder: it consumes a clean, already-sampled module
//! grid (as produced by the encoder or an image-sampling front-end) and recovers the
//! exact segments and [`QrMeta`], so the result re-encodes identically.

use super::matrix::Canvas;
use super::tables::{char_count_bits, ec_blocks, mode_from_indicator, remainder_bits};
use super::{EcLevel, Mask, QrMeta, Version};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// QR Code structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct QrDecoder;

impl QrDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        QrDecoder
    }

    /// Decode a sampled QR module grid into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        let version = version_from_size(matrix.width(), matrix.height())?;
        let canvas = Canvas::from_matrix(version, matrix);

        // The format information (EC level + mask) is the most fragile part of a real
        // capture: it lives in single-module features hugging the finder rings, exactly
        // where blur bleeds worst. Read it if possible — but a failed (or damaged and
        // silently mis-corrected) read is not fatal, because only 4 levels × 8 masks
        // exist: try the read result first, then every other combination, and let
        // Reed–Solomon arbitrate. RS makes a false accept astronomically unlikely, and
        // a failing combination is rejected in ~µs, so the exhaustive pass costs almost
        // nothing on grids that were going to fail anyway.
        let read = canvas.read_format();
        let mut last = Error::undecodable("unreadable format information");
        if let Some((level, mask)) = read {
            match decode_with_format(&canvas, version, level, mask) {
                Ok(sym) => return Ok(sym),
                Err(e) => last = e,
            }
        }
        // The exhaustive pass only makes sense on a grid that is plausibly a QR at all:
        // image-sampling front-ends throw thousands of garbage grids from false finder
        // triples at this decoder, and 32 RS attempts on each would dominate scan time.
        // A real symbol keeps most of its timing-pattern alternation even when blur has
        // wrecked the format modules; garbage reads as a coin flip.
        if timing_score(&canvas, version) < 0.75 {
            return Err(last);
        }
        for (level, mask) in [EcLevel::L, EcLevel::M, EcLevel::Q, EcLevel::H]
            .into_iter()
            .flat_map(|level| (0..8).filter_map(move |m| Mask::new(m).map(|mk| (level, mk))))
            .filter(|&combo| Some(combo) != read)
        {
            match decode_with_format(&canvas, version, level, mask) {
                Ok(sym) => return Ok(sym),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

/// Fraction of the two timing patterns (grid row 6 and column 6 between the finders)
/// whose modules carry the expected dark/light alternation. `1.0` on a clean symbol;
/// ≈`0.5` on a grid sampled from something that is not a QR.
fn timing_score(canvas: &Canvas, version: Version) -> f32 {
    let dim = version.size();
    let mut ok = 0u32;
    let mut total = 0u32;
    for i in 8..dim - 8 {
        let expected = i % 2 == 0;
        ok += u32::from(canvas.get(i, 6) == expected);
        ok += u32::from(canvas.get(6, i) == expected);
        total += 2;
    }
    if total == 0 {
        return 0.0;
    }
    ok as f32 / total as f32
}

/// Decode the canvas under one assumed `(level, mask)` interpretation.
fn decode_with_format(
    canvas: &Canvas,
    version: Version,
    level: EcLevel,
    mask: Mask,
) -> Result<Symbol> {
    // Read the masked data modules along the path, unmasking as we go.
    let path = canvas.data_path();
    let mut bits: Vec<bool> = Vec::with_capacity(path.len());
    for &(x, y) in &path {
        bits.push(canvas.get(x, y) ^ Canvas::mask_bit(mask, x, y));
    }

    let ecb = ec_blocks(version, level);
    let total_cw = ecb.total_codewords();
    // Pack to codewords, dropping trailing remainder bits.
    if bits.len() < total_cw * 8 {
        return Err(Error::undecodable("not enough data modules"));
    }
    let mut codewords = Vec::with_capacity(total_cw);
    for i in 0..total_cw {
        let mut byte = 0u8;
        for k in 0..8 {
            byte = (byte << 1) | bits[i * 8 + k] as u8;
        }
        codewords.push(byte);
    }
    let _ = remainder_bits(version); // remainder bits already excluded above

    let data = deinterleave_and_correct(&codewords, &ecb)?;
    let segments = parse_segments(&data, version)?;

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

impl Decode for QrDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "QR decode of a linear pattern",
            }),
        }
    }
}

/// Derive the QR version from the grid dimensions.
fn version_from_size(width: usize, height: usize) -> Result<Version> {
    if width != height {
        return Err(Error::undecodable("QR grid is not square"));
    }
    if width < 21 || !(width - 21).is_multiple_of(4) {
        return Err(Error::undecodable("grid size is not a valid QR version"));
    }
    let v = ((width - 21) / 4 + 1) as u8;
    Version::new(v).ok_or_else(|| Error::undecodable("grid size out of QR version range"))
}

/// De-interleave codewords back into blocks, RS-correct each, and concatenate the
/// corrected data codewords in original block order.
fn deinterleave_and_correct(codewords: &[u8], ecb: &super::tables::EcBlocks) -> Result<Vec<u8>> {
    let total_blocks = ecb.total_blocks();
    let block_sizes: Vec<usize> = (0..ecb.group1_blocks)
        .map(|_| ecb.group1_data)
        .chain((0..ecb.group2_blocks).map(|_| ecb.group2_data))
        .collect();

    let (interleaved_data, interleaved_ec) = codewords.split_at(ecb.total_data());

    // De-interleave data codewords, column by column.
    let mut block_data: Vec<Vec<u8>> = vec![Vec::new(); total_blocks];
    let max_data = ecb.group1_data.max(ecb.group2_data);
    let mut di = 0;
    for col in 0..max_data {
        for (bi, &bsize) in block_sizes.iter().enumerate() {
            if col < bsize {
                block_data[bi].push(interleaved_data[di]);
                di += 1;
            }
        }
    }

    // De-interleave EC codewords.
    let mut block_ec: Vec<Vec<u8>> = vec![Vec::new(); total_blocks];
    let mut ei = 0;
    for _col in 0..ecb.ec_per_block {
        for ec in block_ec.iter_mut() {
            ec.push(interleaved_ec[ei]);
            ei += 1;
        }
    }

    // Correct each block and gather corrected data codewords.
    let mut out = Vec::with_capacity(ecb.total_data());
    for bi in 0..total_blocks {
        let mut full = block_data[bi].clone();
        full.extend_from_slice(&block_ec[bi]);
        let corrected =
            super::gf::decode(&full, ecb.ec_per_block).ok_or(Error::ErrorCorrectionFailed)?;
        out.extend_from_slice(&corrected[..block_sizes[bi]]);
    }
    Ok(out)
}

/// Read bits most-significant-first out of a codeword byte stream.
struct BitReader<'a> {
    bits: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(bits: &'a [u8]) -> Self {
        BitReader { bits, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bits.len() * 8 - self.pos
    }

    fn read(&mut self, n: usize) -> Option<u32> {
        if self.remaining() < n {
            return None;
        }
        let mut v = 0u32;
        for _ in 0..n {
            let byte = self.bits[self.pos / 8];
            let bit = (byte >> (7 - self.pos % 8)) & 1;
            v = (v << 1) | bit as u32;
            self.pos += 1;
        }
        Some(v)
    }
}

fn alnum_char(v: u32) -> Option<u8> {
    let c = match v {
        0..=9 => b'0' + v as u8,
        10..=35 => b'A' + (v - 10) as u8,
        36 => b' ',
        37 => b'$',
        38 => b'%',
        39 => b'*',
        40 => b'+',
        41 => b'-',
        42 => b'.',
        43 => b'/',
        44 => b':',
        _ => return None,
    };
    Some(c)
}

fn kanji_bytes(v: u32) -> Option<(u8, u8)> {
    let base = (v / 0xC0) << 8 | (v % 0xC0);
    let code = if base < 0x1F00 {
        base + 0x8140
    } else {
        base + 0xC140
    };
    Some(((code >> 8) as u8, (code & 0xFF) as u8))
}

/// Parse the corrected data codewords into segments, stopping at the terminator or
/// when the remaining bits are all padding.
fn parse_segments(data: &[u8], version: Version) -> Result<Vec<Segment>> {
    let mut r = BitReader::new(data);
    let mut segments = Vec::new();

    while r.remaining() >= 4 {
        let indicator = r.read(4).unwrap() as u8;
        if indicator == 0 {
            break; // terminator (or padding)
        }
        let mode = mode_from_indicator(indicator)
            .ok_or_else(|| Error::undecodable("unknown QR mode indicator"))?;
        match mode {
            Mode::Eci(_) => {
                let assignment = read_eci_assignment(&mut r)?;
                segments.push(Segment::eci(assignment));
            }
            Mode::Numeric => {
                let count = r.read(char_count_bits(version, &mode)).ok_or_else(trunc)? as usize;
                let mut out = Vec::with_capacity(count);
                let mut remaining = count;
                while remaining >= 3 {
                    let v = r.read(10).ok_or_else(trunc)?;
                    out.extend_from_slice(format!("{v:03}").as_bytes());
                    remaining -= 3;
                }
                if remaining == 2 {
                    let v = r.read(7).ok_or_else(trunc)?;
                    out.extend_from_slice(format!("{v:02}").as_bytes());
                } else if remaining == 1 {
                    let v = r.read(4).ok_or_else(trunc)?;
                    out.push(b'0' + v as u8);
                }
                segments.push(Segment::numeric(out));
            }
            Mode::Alphanumeric => {
                let count = r.read(char_count_bits(version, &mode)).ok_or_else(trunc)? as usize;
                let mut out = Vec::with_capacity(count);
                let mut remaining = count;
                while remaining >= 2 {
                    let v = r.read(11).ok_or_else(trunc)?;
                    out.push(
                        alnum_char(v / 45)
                            .ok_or_else(|| Error::undecodable("bad alphanumeric value"))?,
                    );
                    out.push(
                        alnum_char(v % 45)
                            .ok_or_else(|| Error::undecodable("bad alphanumeric value"))?,
                    );
                    remaining -= 2;
                }
                if remaining == 1 {
                    let v = r.read(6).ok_or_else(trunc)?;
                    out.push(
                        alnum_char(v)
                            .ok_or_else(|| Error::undecodable("bad alphanumeric value"))?,
                    );
                }
                segments.push(Segment::alphanumeric(out));
            }
            Mode::Byte => {
                let count = r.read(char_count_bits(version, &mode)).ok_or_else(trunc)? as usize;
                let mut out = Vec::with_capacity(count);
                for _ in 0..count {
                    out.push(r.read(8).ok_or_else(trunc)? as u8);
                }
                segments.push(Segment::byte(out));
            }
            Mode::Kanji => {
                let count = r.read(char_count_bits(version, &mode)).ok_or_else(trunc)? as usize;
                let mut out = Vec::with_capacity(count * 2);
                for _ in 0..count {
                    let v = r.read(13).ok_or_else(trunc)?;
                    let (hi, lo) =
                        kanji_bytes(v).ok_or_else(|| Error::undecodable("bad kanji value"))?;
                    out.push(hi);
                    out.push(lo);
                }
                segments.push(Segment::kanji(out));
            }
        }
    }
    Ok(segments)
}

fn read_eci_assignment(r: &mut BitReader<'_>) -> Result<u32> {
    let first = r.read(8).ok_or_else(trunc)?;
    if first & 0x80 == 0 {
        Ok(first)
    } else if first & 0xC0 == 0x80 {
        let rest = r.read(8).ok_or_else(trunc)?;
        Ok(((first & 0x3F) << 8) | rest)
    } else if first & 0xE0 == 0xC0 {
        let rest = r.read(16).ok_or_else(trunc)?;
        Ok(((first & 0x1F) << 16) | rest)
    } else {
        Err(Error::undecodable("invalid ECI assignment"))
    }
}

fn trunc() -> Error {
    Error::undecodable("truncated QR data stream")
}
