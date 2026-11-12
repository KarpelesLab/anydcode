//! Data Matrix ECC 200 decoding: [`BitMatrix`] → [`Symbol`].
//!
//! This is the *structural* decoder: it consumes a clean, already-sampled module
//! grid (as produced by the encoder or an image-sampling front-end), corrects errors
//! with Reed–Solomon, and recovers the exact segments and [`DataMatrixMeta`] so the
//! result re-encodes identically.

use super::placement::{mapping_to_symbol, occupancy_bytes, place};
use super::tables::{SymbolSpec, spec_by_size};
use super::{DataMatrixMeta, Encodation};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;
use alloc::{vec, vec::Vec};

use super::{
    BASE256_LATCH, C40_LATCH, ECI, EDIFACT_LATCH, EDIFACT_UNLATCH, FNC1, GS, MACRO_TRAILER,
    MACRO05, MACRO06, PAD, READER_PROGRAMMING, STRUCTURED_APPEND, TEXT_LATCH, UNLATCH, UPPER_SHIFT,
    X12_LATCH, macro_header,
};

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
        let spec = spec_by_size(matrix.width(), matrix.height())
            .ok_or_else(|| Error::undecodable("grid is not a valid Data Matrix size"))?;

        // Read codewords back out of the mapping matrix.
        let (mr, mc) = (spec.mapping_rows(), spec.mapping_cols());
        let mut full = vec![0u8; spec.total_cw()];
        let mut occupied = vec![0u8; occupancy_bytes(mr, mc)];
        place(mr, mc, &mut occupied, |c, bit, r, col| {
            let (x, y) = mapping_to_symbol(&spec, r, col);
            if matrix.get(x, y) {
                full[c] |= 1 << (7 - bit);
            }
        });

        let data = deinterleave_and_correct(&full, &spec)?;
        let (segments, encodations) = parse_codewords(&data)?;

        let meta = DataMatrixMeta {
            width: spec.width,
            height: spec.height,
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
fn deinterleave_and_correct(full: &[u8], spec: &SymbolSpec) -> Result<Vec<u8>> {
    let bc = spec.blocks;
    let epb = spec.ec_per_block();
    let (data_part, ec_part) = full.split_at(spec.data_cw);
    let mut corrected = data_part.to_vec();

    for b in 0..bc {
        let block_data: Vec<u8> = data_part.iter().skip(b).step_by(bc).copied().collect();
        let block_ec: Vec<u8> = (0..epb)
            .map(|e| ec_part[spec.ec_slot(b) + e * bc])
            .collect();
        let mut block = block_data.clone();
        block.extend_from_slice(&block_ec);
        let fixed = super::gf::decode(&block, epb).ok_or(Error::ErrorCorrectionFailed)?;
        for (i, &val) in fixed[..block_data.len()].iter().enumerate() {
            corrected[b + i * bc] = val;
        }
    }
    Ok(corrected)
}

/// The segments of a symbol and their encodations, in step.
#[derive(Default)]
struct Parsed {
    segments: Vec<Segment>,
    encodations: Vec<Encodation>,
}

impl Parsed {
    fn push(&mut self, encodation: Encodation, bytes: Vec<u8>) {
        self.segments.push(Segment::byte(bytes));
        self.encodations.push(encodation);
    }
}

/// Parse corrected data codewords into segments and their encodations, stopping at
/// the first pad codeword.
///
/// ASCII is the base state; every latch opens a segment of its own and the run
/// returns to ASCII when it ends. An ASCII run is one segment unless it holds digits
/// that were not paired the way the encoder pairs them, in which case it is split
/// there so that it re-encodes codeword for codeword. The segmentation is the one
/// [`DataMatrixEncoder`](super::DataMatrixEncoder) writes back identically; a stream
/// it could not reproduce (a dangling shift, stray fill bits, an over-long ECI) is
/// reported as undecodable rather than read approximately.
fn parse_codewords(data: &[u8]) -> Result<(Vec<Segment>, Vec<Encodation>)> {
    let mut out = Parsed::default();
    let mut ascii: Vec<u8> = Vec::new();
    // Whether the previous codeword was a digit encoded on its own. The encoder
    // packs two adjacent digits of one ASCII segment into a pair codeword, so a lone
    // digit followed by more digits can only be reproduced as the end of a segment.
    let mut lone_digit = false;
    // The C40 or Text run the segments so far leave open as the encoder sees them,
    // and whether it ends in an FNC1. The encoder writes an FNC1 that follows such a
    // run inside it, and takes a same-set segment after that FNC1 for more of the
    // run; a symbol that unlatched instead is told apart by an empty ASCII segment.
    let mut open_run: Option<(Encodation, bool)> = None;
    // A C40, Text or X12 run stopped one codeword short of the symbol's end without
    // an unlatch: that last codeword is read as ASCII.
    let mut implicit = false;
    let mut macro_enc = None;
    let mut i = 0;
    while i < data.len() {
        let cw = data[i];
        i += 1;
        let after_lone_digit = core::mem::take(&mut lone_digit);
        if after_lone_digit && matches!(cw, 49..=58 | 130..=229) {
            out.push(Encodation::Ascii, core::mem::take(&mut ascii));
        }
        let was_implicit = core::mem::take(&mut implicit);
        if !matches!(cw, 1..=128 | 130..=229 | UPPER_SHIFT | PAD) && !ascii.is_empty() {
            out.push(Encodation::Ascii, core::mem::take(&mut ascii));
        }
        match cw {
            PAD => {
                // The encoder closes a final run with an unlatch when a codeword is
                // left for it; a pad there instead is an (empty) ASCII segment.
                if was_implicit {
                    out.push(Encodation::Ascii, Vec::new());
                }
                break;
            }
            1..=128 => {
                ascii.push(cw - 1);
                lone_digit = matches!(cw, 49..=58);
            }
            130..=229 => {
                let v = cw - 130;
                ascii.push(b'0' + v / 10);
                ascii.push(b'0' + v % 10);
            }
            UPPER_SHIFT => {
                let n = *data.get(i).ok_or_else(trunc)?;
                i += 1;
                if !(1..=128).contains(&n) {
                    return Err(Error::undecodable("invalid Data Matrix upper shift"));
                }
                ascii.push(n - 1 + 128);
            }
            BASE256_LATCH => {
                let (bytes, counted) = read_base256(data, &mut i)?;
                let short = bytes.len() < 250;
                out.push(Encodation::Base256, bytes);
                // A run that reaches the end of the symbol is normally written with
                // a zero length field, and that is what the encoder does for a final
                // Base256 segment. One that carries its byte count instead is told
                // apart by an empty ASCII segment after it.
                if counted && short && i == data.len() {
                    out.push(Encodation::Ascii, Vec::new());
                }
            }
            C40_LATCH | TEXT_LATCH | X12_LATCH => {
                let set = match cw {
                    C40_LATCH => Encodation::C40,
                    TEXT_LATCH => Encodation::Text,
                    _ => Encodation::X12,
                };
                if open_run == Some((set, true)) {
                    out.push(Encodation::Ascii, Vec::new());
                }
                let (after_fnc1, short) = read_triplets(data, &mut i, set, &mut out)?;
                implicit = short;
                open_run = (set != Encodation::X12).then_some((set, after_fnc1));
                continue;
            }
            EDIFACT_LATCH => read_edifact(data, &mut i, &mut out)?,
            FNC1 => {
                if open_run.is_some() {
                    out.push(Encodation::Ascii, Vec::new());
                }
                // In the first or second position FNC1 flags the data format (GS1,
                // or an AIM industry format) and is not data; anywhere else it is a
                // field separator, transmitted as GS.
                let bytes = if i <= 2 { Vec::new() } else { vec![GS] };
                out.push(Encodation::Fnc1, bytes);
            }
            ECI => {
                let n = read_eci(data, &mut i)?;
                out.segments.push(Segment::eci(n));
                out.encodations.push(Encodation::Ascii);
            }
            MACRO05 | MACRO06 if i == 1 => {
                let set = if cw == MACRO05 {
                    Encodation::Macro05
                } else {
                    Encodation::Macro06
                };
                out.push(set, macro_header(set).to_vec());
                macro_enc = Some(set);
            }
            STRUCTURED_APPEND => {
                return Err(Error::Unsupported {
                    what: "Data Matrix Structured Append",
                });
            }
            READER_PROGRAMMING => {
                return Err(Error::Unsupported {
                    what: "Data Matrix Reader Programming",
                });
            }
            _ => return Err(Error::undecodable("invalid Data Matrix codeword")),
        }
        open_run = None;
    }
    if !ascii.is_empty() {
        out.push(Encodation::Ascii, ascii);
    }
    match macro_enc {
        Some(set) => out.push(set, MACRO_TRAILER.to_vec()),
        // An all-padding symbol corresponds to an empty ASCII payload.
        None if out.segments.is_empty() => out.push(Encodation::Ascii, Vec::new()),
        None => {}
    }
    Ok((out.segments, out.encodations))
}

/// Read one C40, Text or X12 run starting at `*i` (just after the latch), advancing
/// `*i` past it and its unlatch. The run is one segment, split around each FNC1 it
/// holds. Returns whether the run ends in an FNC1, and whether it stopped without an
/// unlatch one codeword short of the end of the symbol.
fn read_triplets(
    data: &[u8],
    i: &mut usize,
    set: Encodation,
    out: &mut Parsed,
) -> Result<(bool, bool)> {
    let bad = || Error::undecodable("malformed Data Matrix C40/Text/X12 run");
    let mut cur: Vec<u8> = Vec::new();
    // The segment being collected opens the run: it is kept even if empty.
    let mut opening = true;
    let mut after_fnc1 = false;
    let (mut shift, mut upper) = (0u8, false);
    let short = loop {
        let left = data.len() - *i;
        if left == 0 {
            break false;
        }
        if data[*i] == UNLATCH {
            *i += 1;
            break false;
        }
        if left == 1 {
            break true;
        }
        let v = (u16::from(data[*i]) << 8 | u16::from(data[*i + 1]))
            .checked_sub(1)
            .ok_or_else(bad)?;
        *i += 2;
        if v >= 64000 {
            return Err(bad());
        }
        for c in [(v / 1600) as u8, (v / 40 % 40) as u8, (v % 40) as u8] {
            let byte = if set == Encodation::X12 {
                match c {
                    0 => b'\r',
                    1 => b'*',
                    2 => b'>',
                    3 => b' ',
                    4..=13 => b'0' + c - 4,
                    _ => b'A' + c - 14,
                }
            } else {
                let text = set == Encodation::Text;
                match (core::mem::take(&mut shift), c) {
                    (0, 0..=2) => {
                        shift = c + 1;
                        continue;
                    }
                    (0, 3) => b' ',
                    (0, 4..=13) => b'0' + c - 4,
                    (0, _) => (if text { b'a' } else { b'A' }) + c - 14,
                    (1, 0..=31) => c,
                    (2, 0..=14) => b'!' + c,
                    (2, 15..=21) => b':' + c - 15,
                    (2, 22..=26) => b'[' + c - 22,
                    (2, 27) if !upper => {
                        if opening || !cur.is_empty() {
                            out.push(set, core::mem::take(&mut cur));
                        }
                        opening = false;
                        out.push(Encodation::Fnc1, vec![GS]);
                        after_fnc1 = true;
                        continue;
                    }
                    (2, 30) if !upper => {
                        upper = true;
                        continue;
                    }
                    (3, 0) => b'`',
                    (3, 1..=26) if text => b'A' + c - 1,
                    (3, 1..=31) => b'`' + c,
                    _ => return Err(bad()),
                }
            };
            cur.push(byte | if core::mem::take(&mut upper) { 0x80 } else { 0 });
            after_fnc1 = false;
        }
    };
    // A lone Shift 1 pads out the last triplet; any other dangling shift cannot be
    // written back.
    if upper || shift > 1 {
        return Err(bad());
    }
    if opening || !cur.is_empty() {
        out.push(set, cur);
    }
    Ok((after_fnc1, short))
}

/// Read one EDIFACT run starting at `*i` (just after the latch), advancing `*i` past
/// it: four 6-bit values to three codewords, ended by the unlatch value (after which
/// the rest of that codeword is zero fill) or by coming within two codewords of the
/// end of the symbol.
fn read_edifact(data: &[u8], i: &mut usize, out: &mut Parsed) -> Result<()> {
    let mut bytes = Vec::new();
    while data.len() - *i > 2 {
        let (b0, b1, b2) = (data[*i], data[*i + 1], data[*i + 2]);
        let vals = [
            b0 >> 2,
            (b0 & 3) << 4 | b1 >> 4,
            (b1 & 15) << 2 | b2 >> 6,
            b2 & 63,
        ];
        let end = vals.iter().position(|&v| v == EDIFACT_UNLATCH);
        for &v in &vals[..end.unwrap_or(4)] {
            bytes.push(if v < 32 { v | 0x40 } else { v });
        }
        let Some(k) = end else {
            *i += 3;
            continue;
        };
        let fill = match k {
            0 => b0 & 3,
            1 => b1 & 15,
            2 => b2 & 63,
            _ => 0,
        };
        if fill != 0 {
            return Err(Error::undecodable("malformed Data Matrix EDIFACT run"));
        }
        *i += ((k + 1) * 6).div_ceil(8);
        break;
    }
    out.push(Encodation::Edifact, bytes);
    Ok(())
}

/// Read the ECI assignment number after an ECI codeword, advancing `*i` past it.
fn read_eci(data: &[u8], i: &mut usize) -> Result<u32> {
    let bad = || Error::undecodable("invalid Data Matrix ECI");
    let mut next = || -> Result<u32> {
        let v = *data.get(*i).ok_or_else(trunc)?;
        *i += 1;
        Ok(u32::from(v))
    };
    let c1 = next()?;
    let eci = match c1 {
        1..=127 => c1 - 1,
        128..=191 => {
            let c2 = next()?;
            if !(1..=254).contains(&c2) {
                return Err(bad());
            }
            (c1 - 128) * 254 + (c2 - 1) + 127
        }
        192..=207 => {
            let (c2, c3) = (next()?, next()?);
            if !(1..=254).contains(&c2) || !(1..=254).contains(&c3) {
                return Err(bad());
            }
            (c1 - 192) * 64516 + (c2 - 1) * 254 + (c3 - 1) + 16383
        }
        _ => return Err(bad()),
    };
    if eci > 999_999 {
        return Err(bad());
    }
    Ok(eci)
}

/// Read one Base256 run starting at `*i` (positioned just after the latch),
/// advancing `*i` past it. Length field and payload are de-randomized. Also returns
/// whether the length field held a byte count (rather than 0, "to the end of the
/// symbol").
fn read_base256(data: &[u8], i: &mut usize) -> Result<(Vec<u8>, bool)> {
    let pos0 = *i + 1;
    let l1 = unrandomize_255(*data.get(*i).ok_or_else(trunc)?, pos0);
    *i += 1;
    let count = if l1 == 0 {
        // The run takes every remaining data codeword of the symbol.
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
    Ok((bytes, l1 != 0))
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

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;
    use crate::codes::datamatrix::DataMatrixEncoder;
    use crate::codes::datamatrix::tables::all_specs;
    use crate::traits::Encode;

    /// Flip one module of each listed codeword in `m`.
    fn corrupt(m: &mut BitMatrix, spec: &SymbolSpec, codewords: &[usize]) {
        let (mr, mc) = (spec.mapping_rows(), spec.mapping_cols());
        let mut occupied = vec![0u8; occupancy_bytes(mr, mc)];
        let mut cells = Vec::new();
        place(mr, mc, &mut occupied, |c, bit, r, col| {
            if bit == c % 8 && codewords.contains(&c) {
                cells.push(mapping_to_symbol(spec, r, col));
            }
        });
        assert_eq!(cells.len(), codewords.len());
        for (x, y) in cells {
            let dark = m.get(x, y);
            m.set(x, y, !dark);
        }
    }

    /// `count` distinct codeword indices of RS block `b`, spread evenly over its data
    /// and EC codewords.
    fn block_codewords(spec: &SymbolSpec, b: usize, count: usize) -> Vec<usize> {
        let bc = spec.blocks;
        let mut all: Vec<usize> = (b..spec.data_cw).step_by(bc).collect();
        all.extend((0..spec.ec_per_block()).map(|e| spec.data_cw + spec.ec_slot(b) + e * bc));
        (0..count).map(|k| all[k * all.len() / count]).collect()
    }

    /// In every size, every block corrects `ec/2` codeword errors simultaneously, and
    /// one error more in any single block is reported rather than mis-decoded.
    #[test]
    fn corrects_up_to_capacity_in_every_block() {
        let enc = DataMatrixEncoder::new();
        for spec in all_specs() {
            let payload: Vec<u8> = (0..spec.data_cw / 2)
                .map(|i| b'A' + (i % 26) as u8)
                .collect();
            let symbol = enc
                .build_rectangular(&payload, spec.width, spec.height)
                .unwrap();
            let Encoding::Matrix(clean) = enc.encode(&symbol).unwrap() else {
                panic!("expected a matrix");
            };
            let t = spec.ec_per_block() / 2;

            let mut m = clean.clone();
            for b in 0..spec.blocks {
                corrupt(&mut m, spec, &block_codewords(spec, b, t));
            }
            let decoded = DataMatrixDecoder::new().decode_matrix(&m).unwrap();
            assert_eq!(decoded, symbol, "{spec:?}");

            for b in 0..spec.blocks {
                let mut m = clean.clone();
                corrupt(&mut m, spec, &block_codewords(spec, b, t + 1));
                assert!(
                    DataMatrixDecoder::new().decode_matrix(&m).is_err(),
                    "{spec:?} block {b}"
                );
            }
        }
    }

    /// Arbitrary codeword streams (what a mis-corrected or hostile symbol yields) must
    /// parse or fail cleanly.
    #[test]
    fn garbage_codeword_streams_never_panic() {
        let mut seed = 0xDA7A_0A7E_1C5E_ED01u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..4000 {
            let len = (next() % 40) as usize + usize::from(round % 7 == 0) * 1500;
            let data: Vec<u8> = (0..len)
                .map(|_| match next() % 4 {
                    // Bias towards the control codewords.
                    0 => [
                        PAD,
                        BASE256_LATCH,
                        UPPER_SHIFT,
                        C40_LATCH,
                        TEXT_LATCH,
                        X12_LATCH,
                        EDIFACT_LATCH,
                        FNC1,
                        ECI,
                        UNLATCH,
                        MACRO05,
                        0,
                        255,
                    ][(next() % 13) as usize],
                    _ => next() as u8,
                })
                .collect();
            let _ = parse_codewords(&data);
        }
    }

    /// Whatever codeword stream parses, the encoder writes back codeword for
    /// codeword: random streams, weighted towards the latches and their unlatches so
    /// that runs open, close and meet the end of the symbol in every way.
    #[test]
    fn every_stream_that_parses_is_written_back_identically() {
        use crate::codes::datamatrix::encode::codewords_of;
        let mut seed = 0x5EED_DA7A_3A7C_0DE5u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let (mut parsed, mut by_set) = (0, [0usize; 9]);
        for round in 0..400_000 {
            let len = (next() % 14) as usize + 1;
            let mut data: Vec<u8> = (0..len)
                .map(|_| match next() % 16 {
                    0 => C40_LATCH,
                    1 => TEXT_LATCH,
                    2 => X12_LATCH,
                    3 => EDIFACT_LATCH,
                    4 => UNLATCH,
                    5 => FNC1,
                    6 => [BASE256_LATCH, ECI, UPPER_SHIFT, MACRO05, MACRO06][(next() % 5) as usize],
                    7 => (next() % 10) as u8 + 49,
                    // An EDIFACT unlatch in each of its four positions.
                    8 => [0x7C, 0xF0, 0xC0, 0x1F, 0x07][(next() % 5) as usize],
                    9 | 10 => (next() % 128) as u8 + 1,
                    _ => next() as u8,
                })
                .collect();
            // Half the streams end in padding.
            if round % 2 == 0 {
                data.push(PAD);
            }
            let Ok((segments, encodations)) = parse_codewords(&data) else {
                continue;
            };
            parsed += 1;
            let written = codewords_of(&segments, &encodations, data.len())
                .unwrap_or_else(|e| panic!("{data:?} -> {segments:?} {encodations:?}: {e}"));
            let n = written.len();
            assert!(
                data.starts_with(&written) && (n == data.len() || data[n] == PAD),
                "{data:?} -> {segments:?} {encodations:?} -> {written:?}"
            );
            for e in encodations {
                by_set[e as usize] += 1;
            }
        }
        assert!(parsed > 20_000, "{parsed}");
        assert!(by_set.iter().all(|&n| n > 300), "{by_set:?}");
    }
}
