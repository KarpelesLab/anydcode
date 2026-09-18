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
use alloc::{format, vec, vec::Vec};

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

/// Parse the corrected content bits into segments, stopping at the terminator (the
/// `000` mode indicator) or when the remaining bits are exhausted.
fn parse_segments(content: &[bool], size: RmqrSize) -> Result<Vec<Segment>> {
    let mut r = BitReader::new(content);
    let mut segments = Vec::new();

    while let Some(mv) = r.read(3) {
        if mv == 0 {
            break; // terminator
        }
        let mode = mode_from_value(mv as u8).ok_or(Error::Unsupported {
            what: "rMQR FNC1 modes",
        })?;
        if let Mode::Eci(_) = mode {
            // No character count: the assignment number follows directly.
            segments.push(Segment::eci(read_eci_assignment(&mut r)?));
            continue;
        }
        let ccb = char_count_bits(size, &mode).unwrap();
        let Some(count) = r.read(ccb) else { break };
        // A zero count is an (empty) segment: only the `000` indicator terminates.
        let count = count as usize;
        match mode {
            Mode::Numeric => {
                let mut out = Vec::with_capacity(count);
                let mut remaining = count;
                let bad = || Error::undecodable("bad numeric value");
                while remaining >= 3 {
                    let v = r.read(10).ok_or_else(trunc)?;
                    if v >= 1000 {
                        return Err(bad());
                    }
                    out.extend_from_slice(format!("{v:03}").as_bytes());
                    remaining -= 3;
                }
                if remaining == 2 {
                    let v = r.read(7).ok_or_else(trunc)?;
                    if v >= 100 {
                        return Err(bad());
                    }
                    out.extend_from_slice(format!("{v:02}").as_bytes());
                } else if remaining == 1 {
                    let v = r.read(4).ok_or_else(trunc)?;
                    if v >= 10 {
                        return Err(bad());
                    }
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
            Mode::Eci(_) => unreachable!("handled above"),
        }
    }
    Ok(segments)
}

/// The variable-length (1–3 byte) ECI assignment number, as in QR.
fn read_eci_assignment(r: &mut BitReader<'_>) -> Result<u32> {
    let first = r.read(8).ok_or_else(trunc)?;
    if first & 0x80 == 0 {
        Ok(first)
    } else if first & 0xC0 == 0x80 {
        let rest = r.read(8).ok_or_else(trunc)?;
        Ok(((first & 0x3F) << 8) | rest)
    } else if first & 0xE0 == 0xC0 {
        let rest = r.read(16).ok_or_else(trunc)?;
        // 21 bits reach 2_097_151, but assignment numbers stop at 999_999 (and the
        // encoder rejects anything above).
        Some(((first & 0x1F) << 16) | rest)
            .filter(|&n| n < 1_000_000)
            .ok_or_else(|| Error::undecodable("ECI assignment out of range"))
    } else {
        Err(Error::undecodable("invalid ECI assignment"))
    }
}

fn trunc() -> Error {
    Error::undecodable("truncated rMQR data stream")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift64 byte source for the no-panic fuzz loops.
    fn noise(state: &mut u64) -> u8 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        (*state >> 32) as u8
    }

    /// Segment parsing sees attacker-controlled bits (anything that passes RS): it
    /// must return `Ok`/`Err` on arbitrary streams, never panic.
    #[test]
    fn parse_segments_never_panics_on_garbage() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for size in RmqrSize::all() {
            for round in 0..200 {
                let len = round * 3;
                let content: Vec<bool> = (0..len).map(|_| noise(&mut state) & 1 != 0).collect();
                if let Ok(segments) = parse_segments(&content, size) {
                    for seg in segments {
                        if seg.mode == Mode::Numeric {
                            assert!(seg.data.iter().all(u8::is_ascii_digit));
                        }
                    }
                }
            }
        }
    }

    fn bits(s: &str) -> Vec<bool> {
        s.bytes()
            .filter(|b| !b.is_ascii_whitespace())
            .map(|b| b == b'1')
            .collect()
    }

    /// The 3-byte ECI form holds 21 bits, but assignment numbers end at 999_999:
    /// anything above cannot be re-encoded and must not decode.
    #[test]
    fn eci_assignment_above_999999_is_rejected() {
        let size = RmqrSize::from_dimensions(43, 7).unwrap();
        assert!(parse_segments(&bits("111 11011111 11111111 11111111 000"), size).is_err());
        let segs = parse_segments(&bits("111 11001111 01000010 00111111 000"), size).unwrap();
        assert_eq!(segs, [Segment::eci(999_999)]);
    }

    /// A numeric group must be a valid 3/2/1-digit number; 1000..=1023, 100..=127
    /// and 10..=15 are malformed, not extra digits.
    #[test]
    fn numeric_group_out_of_range_is_rejected() {
        // R7x43: 4-bit numeric character count.
        let size = RmqrSize::from_dimensions(43, 7).unwrap();
        assert!(parse_segments(&bits("001 0011 1111101000 000"), size).is_err());
        assert!(parse_segments(&bits("001 0010 1100100 000"), size).is_err());
        assert!(parse_segments(&bits("001 0001 1010 000"), size).is_err());
        let segs = parse_segments(&bits("001 0001 0111 000"), size).unwrap();
        assert_eq!(segs, [Segment::numeric(b"7".to_vec())]);
    }
}
