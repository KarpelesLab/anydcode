//! Micro QR decoding: [`BitMatrix`] → [`Symbol`].
//!
//! Structural decoder: it consumes a clean, already-sampled module grid and recovers
//! the exact segments and [`MicroQrMeta`], so the result re-encodes identically.

use super::matrix::Canvas;
use super::tables::{
    char_count_bits, data_module_count, ec_params, has_short_final_codeword, mode_from_micro_value,
    mode_indicator_bits, version_ec_from_symbol_number,
};
use super::{MicroEcLevel, MicroMask, MicroQrMeta, MicroVersion};
use crate::codes::qr::gf;
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// Micro QR Code structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MicroQrDecoder;

impl MicroQrDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        MicroQrDecoder
    }

    /// Decode a sampled Micro QR module grid into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        if matrix.width() != matrix.height() {
            return Err(Error::undecodable("Micro QR grid is not square"));
        }
        let version = MicroVersion::from_size(matrix.width())
            .ok_or_else(|| Error::undecodable("grid size is not a valid Micro QR version"))?;
        let canvas = Canvas::from_matrix(version, matrix);
        let (sn, mask_idx) = canvas
            .read_format()
            .ok_or_else(|| Error::undecodable("unreadable Micro QR format information"))?;
        let (fmt_version, ec_level) = version_ec_from_symbol_number(sn)
            .ok_or_else(|| Error::undecodable("invalid Micro QR symbol number"))?;
        if fmt_version != version {
            return Err(Error::undecodable(
                "Micro QR format version disagrees with grid size",
            ));
        }
        let mask = MicroMask::new(mask_idx).unwrap();

        // Read masked data modules along the path, unmasking as we go.
        let path = canvas.data_path();
        let mut bits: Vec<bool> = Vec::with_capacity(path.len());
        for &(x, y) in &path {
            bits.push(canvas.get(x, y) ^ Canvas::mask_bit(mask, x, y));
        }
        debug_assert_eq!(bits.len(), data_module_count(version));

        let data = deinterleave_and_correct(&bits, version, ec_level)?;
        let segments = parse_segments(&data, version)?;

        let meta = MicroQrMeta {
            version,
            ec_level,
            mask,
        };
        Ok(Symbol::new(
            Symbology::MicroQrCode,
            segments,
            SymbolMeta::MicroQr(meta),
        ))
    }
}

impl Decode for MicroQrDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "Micro QR decode of a linear pattern",
            }),
        }
    }
}

/// Unpack the data-region bits into RS codewords, correct them, and return the
/// decoded content bits (before segment parsing).
fn deinterleave_and_correct(
    bits: &[bool],
    version: MicroVersion,
    ec: MicroEcLevel,
) -> Result<Vec<bool>> {
    let params =
        ec_params(version, ec).ok_or_else(|| Error::undecodable("bad Micro QR version/EC"))?;
    let short = has_short_final_codeword(version);

    let mut block: Vec<u8> = Vec::with_capacity(params.data_cw + params.ec_cw);
    let mut pos = 0;
    let full_data = if short {
        params.data_cw - 1
    } else {
        params.data_cw
    };
    for _ in 0..full_data {
        block.push(read_byte(bits, &mut pos));
    }
    if short {
        // Final 4-bit data codeword occupies the high nibble.
        let mut nibble = 0u8;
        for _ in 0..4 {
            nibble = (nibble << 1) | bits[pos] as u8;
            pos += 1;
        }
        block.push(nibble << 4);
    }
    for _ in 0..params.ec_cw {
        block.push(read_byte(bits, &mut pos));
    }

    let corrected = gf::decode(&block, params.ec_cw).ok_or(Error::ErrorCorrectionFailed)?;

    // Rebuild content bits from the corrected data codewords.
    let mut out = Vec::with_capacity(params.data_cw * 8);
    for &b in &corrected[..full_data] {
        for k in (0..8).rev() {
            out.push((b >> k) & 1 != 0);
        }
    }
    if short {
        let last = corrected[params.data_cw - 1];
        for k in (4..8).rev() {
            out.push((last >> k) & 1 != 0);
        }
    }
    Ok(out)
}

fn read_byte(bits: &[bool], pos: &mut usize) -> u8 {
    let mut byte = 0u8;
    for _ in 0..8 {
        byte = (byte << 1) | bits[*pos] as u8;
        *pos += 1;
    }
    byte
}

/// A bit reader over a boolean slice, most-significant-first as written.
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

/// Parse the corrected content bits into segments, stopping at the terminator (a
/// zero character-count) or when the remaining bits are exhausted.
fn parse_segments(content: &[bool], version: MicroVersion) -> Result<Vec<Segment>> {
    let mut r = BitReader::new(content);
    let mut segments = Vec::new();
    let mib = mode_indicator_bits(version);

    loop {
        if r.remaining() < mib {
            break;
        }
        let mode_val = if mib == 0 {
            0 // M1: numeric only, no mode indicator
        } else {
            r.read(mib).unwrap() as u8
        };
        let Some(mode) = mode_from_micro_value(mode_val) else {
            break;
        };
        let Some(ccb) = char_count_bits(version, &mode) else {
            break;
        };
        let Some(count) = r.read(ccb) else {
            break;
        };
        let count = count as usize;
        if count == 0 {
            break; // terminator / padding
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
    Error::undecodable("truncated Micro QR data stream")
}
