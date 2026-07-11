//! Data Matrix ECC 200 decoding: [`BitMatrix`] → [`Symbol`].
//!
//! This is the *structural* decoder: it consumes a clean, already-sampled module
//! grid (as produced by the encoder or an image-sampling front-end), corrects errors
//! with Reed–Solomon, and recovers the exact segments and [`DataMatrixMeta`] so the
//! result re-encodes identically.

use super::placement::{placement_map, strip_borders};
use super::tables::{SquareSpec, square_by_size};
use super::{DataMatrixMeta, Encodation};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

const PAD: u8 = 129;
const BASE256_LATCH: u8 = 231;
const UPPER_SHIFT: u8 = 235;

/// Data Matrix structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DataMatrixDecoder;

impl DataMatrixDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        DataMatrixDecoder
    }

    /// Decode a sampled Data Matrix module grid into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        if matrix.width() != matrix.height() {
            return Err(Error::undecodable("Data Matrix grid is not square"));
        }
        let spec = square_by_size(matrix.width())
            .ok_or_else(|| Error::undecodable("grid size is not a valid Data Matrix square"))?;

        let mapping = strip_borders(&spec, matrix);
        let ms = spec.mapping_size();
        let pm = placement_map(ms, ms);

        // Read codewords back out of the mapping matrix.
        let mut full = Vec::with_capacity(pm.codewords.len());
        for positions in &pm.codewords {
            let mut byte = 0u8;
            for (bit, &(r, col)) in positions.iter().enumerate() {
                if mapping[r * ms + col] {
                    byte |= 1 << (7 - bit);
                }
            }
            full.push(byte);
        }

        let data = deinterleave_and_correct(&full, &spec)?;
        let (segments, encodations) = parse_codewords(&data)?;

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

impl Decode for DataMatrixDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "Data Matrix decode of a linear pattern",
            }),
        }
    }
}

/// De-interleave the codeword stream into RS blocks, correct each, and return the
/// corrected data codewords in their original (interleaved) order.
fn deinterleave_and_correct(full: &[u8], spec: &SquareSpec) -> Result<Vec<u8>> {
    let bc = spec.blocks;
    let epb = spec.ec_per_block();
    let (data_part, ec_part) = full.split_at(spec.data_cw);
    let mut corrected = data_part.to_vec();

    for b in 0..bc {
        let block_data: Vec<u8> = data_part.iter().skip(b).step_by(bc).copied().collect();
        let block_ec: Vec<u8> = (0..epb).map(|e| ec_part[b + e * bc]).collect();
        let mut block = block_data.clone();
        block.extend_from_slice(&block_ec);
        let fixed = super::gf::decode(&block, epb).ok_or(Error::ErrorCorrectionFailed)?;
        for (i, &val) in fixed[..block_data.len()].iter().enumerate() {
            corrected[b + i * bc] = val;
        }
    }
    Ok(corrected)
}

/// Parse corrected data codewords into segments and their encodations, stopping at
/// the first pad codeword. ASCII is the base state; a Base256 latch introduces a
/// length-prefixed randomized run and then returns to ASCII.
fn parse_codewords(data: &[u8]) -> Result<(Vec<Segment>, Vec<Encodation>)> {
    let mut segments = Vec::new();
    let mut encodations = Vec::new();
    let mut ascii: Vec<u8> = Vec::new();
    let mut i = 0;

    while i < data.len() {
        let cw = data[i];
        i += 1;
        match cw {
            PAD => break,
            1..=128 => ascii.push(cw - 1),
            130..=229 => {
                let v = cw - 130;
                ascii.push(b'0' + v / 10);
                ascii.push(b'0' + v % 10);
            }
            UPPER_SHIFT => {
                let n = *data.get(i).ok_or_else(trunc)?;
                i += 1;
                ascii.push((n.wrapping_sub(1)).wrapping_add(128));
            }
            BASE256_LATCH => {
                if !ascii.is_empty() {
                    segments.push(Segment::byte(std::mem::take(&mut ascii)));
                    encodations.push(Encodation::Ascii);
                }
                let bytes = read_base256(data, &mut i)?;
                segments.push(Segment::byte(bytes));
                encodations.push(Encodation::Base256);
            }
            _ => {
                return Err(Error::Unsupported {
                    what: "Data Matrix codeword (C40/Text/X12/EDIFACT/FNC not supported)",
                });
            }
        }
    }

    if !ascii.is_empty() {
        segments.push(Segment::byte(ascii));
        encodations.push(Encodation::Ascii);
    }
    if segments.is_empty() {
        // An all-padding symbol corresponds to an empty ASCII payload.
        segments.push(Segment::byte(Vec::new()));
        encodations.push(Encodation::Ascii);
    }
    Ok((segments, encodations))
}

/// Read one Base256 run starting at `*i` (positioned just after the latch),
/// advancing `*i` past it. Length field and payload are de-randomized.
fn read_base256(data: &[u8], i: &mut usize) -> Result<Vec<u8>> {
    let pos0 = *i + 1;
    let l1 = unrandomize_255(*data.get(*i).ok_or_else(trunc)?, pos0);
    *i += 1;
    let count = if l1 == 0 {
        // "Rest of the symbol" length; encoder never emits this, but honor it.
        data.len() - *i
    } else if l1 <= 249 {
        l1 as usize
    } else {
        let pos1 = *i + 1;
        let l2 = unrandomize_255(*data.get(*i).ok_or_else(trunc)?, pos1);
        *i += 1;
        (l1 as usize - 249) * 250 + l2 as usize
    };
    let mut bytes = Vec::with_capacity(count);
    for _ in 0..count {
        let pos = *i + 1;
        bytes.push(unrandomize_255(*data.get(*i).ok_or_else(trunc)?, pos));
        *i += 1;
    }
    Ok(bytes)
}

/// Inverse of the Base256 "255-state" randomizing algorithm.
fn unrandomize_255(value: u8, position: usize) -> u8 {
    let pseudo = ((149 * position) % 255) + 1;
    let t = value as isize - pseudo as isize;
    (if t >= 0 { t } else { t + 256 }) as u8
}

fn trunc() -> Error {
    Error::undecodable("truncated Data Matrix codeword stream")
}
