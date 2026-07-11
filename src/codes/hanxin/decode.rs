//! Han Xin decoding: [`BitMatrix`] → [`Symbol`].
//!
//! The structural decoder consumes a clean, already-sampled module grid (from the
//! encoder or an image front-end) and recovers the exact segments and
//! [`HanXinMeta`], so the result re-encodes identically. It inverts every stage of
//! [`super::encode`]: read function info, un-mask, reverse the picket-fence
//! interleave, RS-correct each block, then parse the mode bit-stream.

use super::gf::GaloisField;
use super::matrix::Grid;
use super::tables::{data_codewords, ec_blocks, total_codewords};
use super::{HanXinMeta, Version};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// Han Xin Code structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct HanXinDecoder;

impl HanXinDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        HanXinDecoder
    }

    /// Decode a sampled Han Xin module grid into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        let version = version_from_size(matrix.width(), matrix.height())?;
        let grid = Grid::from_matrix(version, matrix);
        let (version, level, mask) = grid
            .read_function_info(version)
            .ok_or_else(|| Error::undecodable("unreadable Han Xin function information"))?;

        // Read the data path, un-masking, into the picket-fence codeword stream.
        let path = grid.data_path();
        let jmax = total_codewords(version) * 8;
        let mut picket = vec![0u8; total_codewords(version)];
        for (k, &(x, y)) in path.iter().enumerate() {
            if k >= jmax {
                break;
            }
            let bit = grid.dark(x, y) ^ Grid::mask_flip(mask.index(), x, y);
            if bit {
                picket[k >> 3] |= 0x80 >> (k & 0x07);
            }
        }

        let fullstream = un_picket_fence(&picket);
        let datastream = deinterleave_and_correct(&fullstream, version, level)?;
        let segments = parse_segments(&datastream)?;

        let meta = HanXinMeta {
            version,
            ec_level: level,
            mask,
        };
        Ok(Symbol::new(
            Symbology::HanXin,
            segments,
            SymbolMeta::HanXin(meta),
        ))
    }
}

impl Decode for HanXinDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "Han Xin decode of a linear pattern",
            }),
        }
    }
}

/// Derive the Han Xin version (1–3) from the grid dimensions.
fn version_from_size(width: usize, height: usize) -> Result<Version> {
    if width != height {
        return Err(Error::undecodable("Han Xin grid is not square"));
    }
    if width < 23 || !(width - 23).is_multiple_of(2) {
        return Err(Error::undecodable(
            "grid size is not a valid Han Xin version",
        ));
    }
    let v = ((width - 23) / 2 + 1) as u8;
    Version::new(v).ok_or_else(|| Error::undecodable("grid size out of supported Han Xin range"))
}

/// Invert `hx_make_picket_fence`: `picket[k]` came from `fullstream[order[k]]`.
fn un_picket_fence(picket: &[u8]) -> Vec<u8> {
    let n = picket.len();
    let mut order = Vec::with_capacity(n);
    for start in 0..13 {
        let mut i = start;
        while i < n {
            order.push(i);
            i += 13;
        }
    }
    let mut full = vec![0u8; n];
    for (k, &pos) in order.iter().enumerate() {
        full[pos] = picket[k];
    }
    full
}

/// Split the full stream into RS blocks (per the batch layout), correct each, and
/// concatenate the corrected data codewords.
fn deinterleave_and_correct(
    fullstream: &[u8],
    version: Version,
    level: super::EcLevel,
) -> Result<Vec<u8>> {
    let gf = GaloisField::data();
    let ecb = ec_blocks(version, level);
    let mut out = Vec::with_capacity(data_codewords(version, level));
    let mut pos = 0usize;
    for &(blocks, dlen, eclen) in &ecb.batches {
        for _ in 0..blocks {
            let end = pos + dlen + eclen;
            if end > fullstream.len() {
                return Err(Error::undecodable("truncated Han Xin codeword stream"));
            }
            let block = &fullstream[pos..end];
            let corrected = gf
                .rs_decode(block, eclen, 1)
                .ok_or(Error::ErrorCorrectionFailed)?;
            out.extend_from_slice(&corrected[..dlen]);
            pos = end;
        }
    }
    Ok(out)
}

/// Read bits most-significant-first from a codeword byte stream.
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

fn text1_char(v: u32) -> Option<u8> {
    let c = match v {
        0..=9 => b'0' + v as u8,
        10..=35 => b'A' + (v - 10) as u8,
        36..=61 => b'a' + (v - 36) as u8,
        _ => return None,
    };
    Some(c)
}

fn text2_char(v: u32) -> Option<u8> {
    let c = match v {
        0..=27 => v as u8,
        28..=43 => b' ' + (v - 28) as u8,
        44..=50 => b':' + (v - 44) as u8,
        51..=56 => b'[' + (v - 51) as u8,
        57..=61 => b'{' + (v - 57) as u8,
        _ => return None,
    };
    Some(c)
}

/// Parse the corrected data codewords into segments, stopping at the zero padding.
fn parse_segments(data: &[u8]) -> Result<Vec<Segment>> {
    let mut r = BitReader::new(data);
    let mut segments = Vec::new();

    while r.remaining() >= 4 {
        let indicator = r.read(4).unwrap();
        match indicator {
            0b0001 => segments.push(parse_numeric(&mut r)?),
            0b0010 => segments.push(parse_text(&mut r)?),
            0b0011 => segments.push(parse_binary(&mut r)?),
            0 => break, // zero padding after the last segment
            _ => {
                return Err(Error::Unsupported {
                    what: "Han Xin region / GB 18030 modes",
                });
            }
        }
    }
    Ok(segments)
}

fn parse_numeric(r: &mut BitReader<'_>) -> Result<Segment> {
    let mut groups = Vec::new();
    let last_count;
    loop {
        let v = r.read(10).ok_or_else(trunc)?;
        if v >= 1021 {
            last_count = (v - 1020) as usize; // 1021→1, 1022→2, 1023→3
            break;
        }
        groups.push(v);
    }
    let mut out = Vec::new();
    let n = groups.len();
    for (idx, &g) in groups.iter().enumerate() {
        let width = if idx + 1 == n { last_count } else { 3 };
        match width {
            3 => out.extend_from_slice(format!("{g:03}").as_bytes()),
            2 => out.extend_from_slice(format!("{g:02}").as_bytes()),
            1 => out.push(b'0' + g as u8),
            _ => {}
        }
    }
    Ok(Segment::numeric(out))
}

fn parse_text(r: &mut BitReader<'_>) -> Result<Segment> {
    let mut submode = 1u8;
    let mut out = Vec::new();
    loop {
        let v = r.read(6).ok_or_else(trunc)?;
        if v == 63 {
            break; // terminator
        }
        if v == 62 {
            submode = if submode == 1 { 2 } else { 1 };
            continue;
        }
        let ch = if submode == 1 {
            text1_char(v)
        } else {
            text2_char(v)
        }
        .ok_or_else(|| Error::undecodable("bad Han Xin text value"))?;
        out.push(ch);
    }
    Ok(Segment::alphanumeric(out))
}

fn parse_binary(r: &mut BitReader<'_>) -> Result<Segment> {
    let count = r.read(13).ok_or_else(trunc)? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(r.read(8).ok_or_else(trunc)? as u8);
    }
    Ok(Segment::byte(out))
}

fn trunc() -> Error {
    Error::undecodable("truncated Han Xin data stream")
}
