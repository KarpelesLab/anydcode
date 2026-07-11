//! Han Xin encoding: [`Symbol`] → [`BitMatrix`].
//!
//! The [`HanXinEncoder`] either reproduces an exact symbol from a fully-specified
//! [`HanXinMeta`] (the round-trip path) or builds a fresh symbol from segments,
//! choosing the smallest fitting version (1–3) and the lowest-penalty mask.
//!
//! The data pipeline mirrors zint `backend/hanxin.c`: mode bit-stream → zero-padded
//! data codewords → per-batch GF(2^8) Reed–Solomon (`hx_add_ecc`) → picket-fence
//! interleave (`hx_make_picket_fence`) → row-major module placement → data mask →
//! function information.

use super::gf::GaloisField;
use super::matrix::Grid;
use super::tables::{data_codewords, ec_blocks, total_codewords};
use super::{EcLevel, HanXinMeta, Mask, Version};
use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// Han Xin Code encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct HanXinEncoder;

impl HanXinEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        HanXinEncoder
    }

    /// Build a reproducible [`Symbol`] from `segments` at EC `level`, choosing the
    /// smallest fitting version (1–3) and the lowest-penalty mask. The returned
    /// symbol's [`HanXinMeta`] pins those choices so re-encoding is identical.
    pub fn build(&self, segments: Vec<Segment>, level: EcLevel) -> Result<Symbol> {
        let version = choose_version(&segments, level)?;
        let mask = choose_mask(&segments, version, level)?;
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

    /// Convenience: build a symbol from UTF-8 `text` as a single byte segment.
    pub fn build_text(&self, text: &str, level: EcLevel) -> Result<Symbol> {
        self.build(vec![Segment::byte(text.as_bytes().to_vec())], level)
    }

    /// Convenience: build a numeric symbol from ASCII `digits`.
    pub fn build_numeric(&self, digits: &str, level: EcLevel) -> Result<Symbol> {
        self.build(vec![Segment::numeric(digits.as_bytes().to_vec())], level)
    }
}

impl Encode for HanXinEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::HanXin {
            return Err(Error::invalid_parameter(
                "HanXinEncoder given a non-Han-Xin symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::HanXin(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Han Xin symbol missing HanXinMeta",
                ));
            }
        };
        let grid = render(&symbol.segments, meta.version, meta.ec_level, meta.mask)?;
        Ok(Encoding::Matrix(grid.to_bitmatrix()))
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

/// Which Han Xin text sub-mode a byte belongs to: 1 (digits/letters) or 2 (rest).
fn text_submode(b: u8) -> u8 {
    if b.is_ascii_digit() || b.is_ascii_uppercase() || b.is_ascii_lowercase() {
        1
    } else {
        2
    }
}

/// Text sub-mode 1 value (Table 3), or `None` if not representable.
fn text1_value(b: u8) -> Option<u32> {
    let v = match b {
        b'0'..=b'9' => b - b'0',
        b'A'..=b'Z' => b - b'A' + 10,
        b'a'..=b'z' => b - b'a' + 36,
        _ => return None,
    };
    Some(v as u32)
}

/// Text sub-mode 2 value (Table 4), or `None` if not representable.
fn text2_value(b: u8) -> Option<u32> {
    let v = match b {
        0..=27 => b,
        b' '..=b'/' => b - b' ' + 28,
        b':'..=b'@' => b - b':' + 44,
        b'['..=b'`' => b - b'[' + 51,
        b'{'..=127 => b - b'{' + 57,
        _ => return None,
    };
    Some(v as u32)
}

/// Write one segment's Han Xin mode header and payload bits.
fn write_segment(w: &mut BitWriter, seg: &Segment) -> Result<()> {
    match &seg.mode {
        Mode::Numeric => {
            w.push(0b0001, 4);
            let data = &seg.data;
            let n = data.len();
            let mut i = 0;
            let mut last_count = 0u32;
            while i < n {
                if !data[i].is_ascii_digit() {
                    return Err(Error::invalid_data("non-digit in numeric segment"));
                }
                let mut value = (data[i] - b'0') as u32;
                let mut count = 1u32;
                if i + 1 < n && data[i + 1].is_ascii_digit() {
                    value = value * 10 + (data[i + 1] - b'0') as u32;
                    count = 2;
                    if i + 2 < n && data[i + 2].is_ascii_digit() {
                        value = value * 10 + (data[i + 2] - b'0') as u32;
                        count = 3;
                    }
                }
                w.push(value, 10);
                last_count = count;
                i += count as usize;
            }
            // Terminator encodes the digit count of the final group.
            let term = match last_count {
                1 => 1021,
                2 => 1022,
                _ => 1023,
            };
            w.push(term, 10);
        }
        Mode::Alphanumeric => {
            // Han Xin Text mode (sub-modes Text 1 / Text 2).
            w.push(0b0010, 4);
            let mut submode = 1u8;
            for &b in &seg.data {
                let want = text_submode(b);
                if want != submode {
                    w.push(62, 6); // sub-mode switch
                    submode = want;
                }
                let value = if submode == 1 {
                    text1_value(b)
                } else {
                    text2_value(b)
                }
                .ok_or_else(|| {
                    Error::invalid_data("byte not representable in Han Xin text mode")
                })?;
                w.push(value, 6);
            }
            w.push(63, 6); // terminator
        }
        Mode::Byte => {
            // Han Xin Binary mode.
            w.push(0b0011, 4);
            w.push(seg.data.len() as u32, 13);
            for &b in &seg.data {
                w.push(b as u32, 8);
            }
        }
        Mode::Kanji | Mode::Eci(_) => {
            return Err(Error::Unsupported {
                what: "Han Xin Kanji/ECI/region modes",
            });
        }
    }
    Ok(())
}

/// Total encoded bit length of all segments.
fn segments_bit_len(segments: &[Segment]) -> Result<usize> {
    let mut w = BitWriter::new();
    for seg in segments {
        write_segment(&mut w, seg)?;
    }
    Ok(w.len())
}

/// The smallest supported version (1–3) whose data capacity holds `segments`.
fn choose_version(segments: &[Segment], level: EcLevel) -> Result<Version> {
    let need = segments_bit_len(segments)?;
    for v in 1..=super::tables::MAX_VERSION {
        let version = Version::new(v).unwrap();
        if need <= data_codewords(version, level) * 8 {
            return Ok(version);
        }
    }
    Err(Error::capacity(
        "data does not fit in Han Xin versions 1–3 at this EC level (larger versions unsupported)",
    ))
}

/// Pack the segment bit-stream into zero-padded data codewords.
fn build_datastream(segments: &[Segment], version: Version, level: EcLevel) -> Result<Vec<u8>> {
    let capacity = data_codewords(version, level);
    let mut w = BitWriter::new();
    for seg in segments {
        write_segment(&mut w, seg)?;
    }
    if w.len() > capacity * 8 {
        return Err(Error::capacity("segments exceed selected version capacity"));
    }
    let mut datastream = vec![0u8; capacity];
    for (i, &bit) in w.bits.iter().enumerate() {
        if bit {
            datastream[i >> 3] |= 0x80 >> (i & 0x07);
        }
    }
    Ok(datastream)
}

/// Apply per-batch Reed–Solomon (GF(2^8), first root `α^1`) producing the full
/// codeword stream (data + EC), mirroring zint `hx_add_ecc`.
fn add_ecc(datastream: &[u8], version: Version, level: EcLevel) -> Vec<u8> {
    let gf = GaloisField::data();
    let ecb = ec_blocks(version, level);
    let mut fullstream = Vec::with_capacity(total_codewords(version));
    let mut input = 0usize;
    for &(blocks, dlen, eclen) in &ecb.batches {
        for _ in 0..blocks {
            let mut block = vec![0u8; dlen];
            for b in block.iter_mut() {
                if input < datastream.len() {
                    *b = datastream[input];
                }
                input += 1;
            }
            fullstream.extend_from_slice(&block);
            let ecc = gf.rs_encode(&block, eclen, 1);
            fullstream.extend_from_slice(&ecc);
        }
    }
    fullstream
}

/// zint `hx_make_picket_fence`: reorder the full stream into 13 interleaved columns.
fn picket_fence(fullstream: &[u8]) -> Vec<u8> {
    let n = fullstream.len();
    let mut out = Vec::with_capacity(n);
    for start in 0..13 {
        let mut i = start;
        while i < n {
            out.push(fullstream[i]);
            i += 13;
        }
    }
    out
}

/// Render a grid for the given parameters, applying `mask`.
fn render(segments: &[Segment], version: Version, level: EcLevel, mask: Mask) -> Result<Grid> {
    let datastream = build_datastream(segments, version, level)?;
    let fullstream = add_ecc(&datastream, version, level);
    let picket = picket_fence(&fullstream);

    let mut grid = Grid::new(version);
    let path = grid.data_path();
    let jmax = total_codewords(version) * 8;
    for (k, &(x, y)) in path.iter().enumerate() {
        if k >= jmax {
            break;
        }
        let bit = picket[k >> 3] & (0x80 >> (k & 0x07)) != 0;
        grid.set_data(x, y, bit);
    }
    grid.apply_mask(mask.index());
    grid.place_function_info(version, level, mask);
    Ok(grid)
}

/// Choose the lowest-penalty mask (ties resolved toward the lower index).
fn choose_mask(segments: &[Segment], version: Version, level: EcLevel) -> Result<Mask> {
    // Build the unmasked data grid once; evaluate each candidate mask against it.
    let datastream = build_datastream(segments, version, level)?;
    let fullstream = add_ecc(&datastream, version, level);
    let picket = picket_fence(&fullstream);
    let mut grid = Grid::new(version);
    let path = grid.data_path();
    let jmax = total_codewords(version) * 8;
    for (k, &(x, y)) in path.iter().enumerate() {
        if k >= jmax {
            break;
        }
        let bit = picket[k >> 3] & (0x80 >> (k & 0x07)) != 0;
        grid.set_data(x, y, bit);
    }

    let mut best = Mask::new(0).unwrap();
    let mut best_penalty = u32::MAX;
    for m in 0..4 {
        let mask = Mask::new(m).unwrap();
        let penalty = grid.penalty(version, level, mask);
        if penalty < best_penalty {
            best_penalty = penalty;
            best = mask;
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact data codewords for numeric "1234" at V1-L4: mode 0001, groups
    /// 123 / 4, terminator 1021, zero-padded to 9 data codewords.
    #[test]
    fn numeric_datastream_1234() {
        let segs = vec![Segment::numeric(b"1234".to_vec())];
        let v = Version::new(1).unwrap();
        let ds = build_datastream(&segs, v, EcLevel::L4).unwrap();
        assert_eq!(
            ds,
            vec![0x11, 0xEC, 0x04, 0xFF, 0x40, 0x00, 0x00, 0x00, 0x00]
        );
    }

    /// Full stream = 9 data + 16 EC codewords = 25 total for V1-L4.
    #[test]
    fn fullstream_length_v1_l4() {
        let segs = vec![Segment::numeric(b"1234".to_vec())];
        let v = Version::new(1).unwrap();
        let ds = build_datastream(&segs, v, EcLevel::L4).unwrap();
        let fs = add_ecc(&ds, v, EcLevel::L4);
        assert_eq!(fs.len(), 25);
        assert_eq!(&fs[..9], &ds[..9]);
    }
}
