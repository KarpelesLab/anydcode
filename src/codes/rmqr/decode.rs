//! rMQR decoding: [`BitMatrix`] → [`Symbol`].
//!
//! Structural decoder: it consumes a clean, already-sampled module grid and recovers
//! the exact segments and [`RmqrMeta`], so the result re-encodes identically.

use super::matrix::Canvas;
use super::tables::{char_count_bits, info, mode_from_value};
use super::{RmqrEcLevel, RmqrMeta, RmqrSize};
use crate::codes::qr::gf;
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// Rectangular Micro QR structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct RmqrDecoder;

impl RmqrDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        RmqrDecoder
    }

    /// Decode a sampled rMQR module grid into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        let size = RmqrSize::from_dimensions(matrix.width(), matrix.height())
            .ok_or_else(|| Error::undecodable("grid size is not a valid rMQR size"))?;
        let canvas = Canvas::from_matrix(size, matrix);
        let (fmt_size, ec) = canvas
            .read_format()
            .ok_or_else(|| Error::undecodable("unreadable rMQR format information"))?;
        if fmt_size != size {
            return Err(Error::undecodable(
                "rMQR format size disagrees with grid dimensions",
            ));
        }

        let path = canvas.data_path(canvas.data_cell_count(ec));
        let mut bits: Vec<bool> = Vec::with_capacity(path.len());
        for &(x, y) in &path {
            bits.push(canvas.get(x, y) ^ Canvas::mask_bit(x, y));
        }

        let si = info(size);
        let total_cw = si.total_codewords(ec);
        if bits.len() < total_cw * 8 {
            return Err(Error::undecodable("not enough rMQR data modules"));
        }
        let mut codewords = Vec::with_capacity(total_cw);
        for i in 0..total_cw {
            let mut byte = 0u8;
            for k in 0..8 {
                byte = (byte << 1) | bits[i * 8 + k] as u8;
            }
            codewords.push(byte);
        }

        let data = deinterleave_and_correct(&codewords, size, ec)?;
        let content: Vec<bool> = data
            .iter()
            .flat_map(|&b| (0..8).rev().map(move |k| (b >> k) & 1 != 0))
            .collect();
        let segments = parse_segments(&content, size)?;

        Ok(Symbol::new(
            Symbology::RectMicroQrCode,
            segments,
            SymbolMeta::Rmqr(RmqrMeta { size, ec_level: ec }),
        ))
    }
}

impl Decode for RmqrDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "rMQR decode of a linear pattern",
            }),
        }
    }
}

/// De-interleave codewords into blocks, RS-correct each, and concatenate the
/// corrected data codewords in original block order.
fn deinterleave_and_correct(codewords: &[u8], size: RmqrSize, ec: RmqrEcLevel) -> Result<Vec<u8>> {
    let si = info(size);
    // Expand block groups to a flat list of (data_len, ec_len) per block.
    let mut specs: Vec<(usize, usize)> = Vec::new();
    for g in si.blocks(ec) {
        for _ in 0..g.num {
            specs.push((g.data, g.total - g.data));
        }
    }
    let total_data = si.total_data_codewords(ec);
    let (data_part, ec_part) = codewords.split_at(total_data);

    // De-interleave data codewords column-by-column.
    let mut block_data: Vec<Vec<u8>> = vec![Vec::new(); specs.len()];
    let max_k = specs.iter().map(|&(k, _)| k).max().unwrap_or(0);
    let mut di = 0;
    for col in 0..max_k {
        for (bi, &(k, _)) in specs.iter().enumerate() {
            if col < k {
                block_data[bi].push(data_part[di]);
                di += 1;
            }
        }
    }
    // De-interleave EC codewords.
    let mut block_ec: Vec<Vec<u8>> = vec![Vec::new(); specs.len()];
    let max_e = specs.iter().map(|&(_, e)| e).max().unwrap_or(0);
    let mut ei = 0;
    for col in 0..max_e {
        for (bi, &(_, e)) in specs.iter().enumerate() {
            if col < e {
                block_ec[bi].push(ec_part[ei]);
                ei += 1;
            }
        }
    }

    let mut out = Vec::with_capacity(total_data);
    for (bi, &(k, e)) in specs.iter().enumerate() {
        let mut full = block_data[bi].clone();
        full.extend_from_slice(&block_ec[bi]);
        let corrected = gf::decode(&full, e).ok_or(Error::ErrorCorrectionFailed)?;
        out.extend_from_slice(&corrected[..k]);
    }
    Ok(out)
}

/// A bit reader over a boolean slice.
struct BitReader<'a> {
    bits: &'a [bool],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(bits: &'a [bool]) -> Self {
        BitReader { bits, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bits.len() - self.pos
    }

    fn read(&mut self, n: usize) -> Option<u32> {
        if self.remaining() < n {
            return None;
        }
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bits[self.pos] as u32;
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

fn kanji_bytes(v: u32) -> (u8, u8) {
    let base = (v / 0xC0) << 8 | (v % 0xC0);
    let code = if base < 0x1F00 {
        base + 0x8140
    } else {
        base + 0xC140
    };
    ((code >> 8) as u8, (code & 0xFF) as u8)
}

/// Parse the corrected content bits into segments, stopping at the terminator (an
/// invalid 3-bit mode indicator) or when the remaining bits are exhausted.
fn parse_segments(content: &[bool], size: RmqrSize) -> Result<Vec<Segment>> {
    let mut r = BitReader::new(content);
    let mut segments = Vec::new();

    while let Some(mv) = r.read(3) {
        let Some(mode) = mode_from_value(mv as u8) else {
            break; // terminator / padding
        };
        let ccb = char_count_bits(size, &mode).unwrap();
        let Some(count) = r.read(ccb) else { break };
        let count = count as usize;
        if count == 0 {
            break;
        }
        match mode {
            Mode::Numeric => {
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
                let mut out = Vec::with_capacity(count);
                let mut remaining = count;
                while remaining >= 2 {
                    let v = r.read(11).ok_or_else(trunc)?;
                    out.push(alnum_char(v / 45).ok_or_else(|| Error::undecodable("bad alnum"))?);
                    out.push(alnum_char(v % 45).ok_or_else(|| Error::undecodable("bad alnum"))?);
                    remaining -= 2;
                }
                if remaining == 1 {
                    let v = r.read(6).ok_or_else(trunc)?;
                    out.push(alnum_char(v).ok_or_else(|| Error::undecodable("bad alnum"))?);
                }
                segments.push(Segment::alphanumeric(out));
            }
            Mode::Byte => {
                let mut out = Vec::with_capacity(count);
                for _ in 0..count {
                    out.push(r.read(8).ok_or_else(trunc)? as u8);
                }
                segments.push(Segment::byte(out));
            }
            Mode::Kanji => {
                let mut out = Vec::with_capacity(count * 2);
                for _ in 0..count {
                    let v = r.read(13).ok_or_else(trunc)?;
                    let (hi, lo) = kanji_bytes(v);
                    out.push(hi);
                    out.push(lo);
                }
                segments.push(Segment::kanji(out));
            }
            Mode::Eci(_) => break,
        }
    }
    Ok(segments)
}

fn trunc() -> Error {
    Error::undecodable("truncated rMQR data stream")
}
