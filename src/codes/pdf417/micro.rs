//! MicroPDF417 (ISO/IEC 24728): a compact, column-limited relative of PDF417.
//!
//! MicroPDF417 reuses PDF417's high-level compaction (Text/Byte/Numeric) and its
//! GF(929) Reed–Solomon error correction, but replaces the start/stop guards and row
//! indicators with **Row Address Patterns** (RAPs) and drops the symbol-length
//! descriptor. Only 34 fixed sizes exist (1–4 data columns), each pinning the row
//! count and the number of error-correction codewords; there is no user-selectable
//! error-correction level.
//!
//! ## Layout
//!
//! Every row is a fixed-width string of 17-module data symbol characters framed by
//! 10-module RAPs, closed by a single dark stop module:
//!
//! | cols | row structure                                            | width |
//! |------|----------------------------------------------------------|-------|
//! | 1    | L · d0 · R · stop                                        | 38    |
//! | 2    | L · d0 · d1 · R · stop                                    | 55    |
//! | 3    | L · d0 · C · d1 · d2 · R · stop                           | 82    |
//! | 4    | L · d0 · d1 · C · d2 · d3 · R · stop                      | 99    |
//!
//! `L`/`R` are the left/right side RAPs (same 52-entry table); `C` is the centre RAP
//! (a distinct 52-entry table). Each RAP index and the data cluster advance by one
//! per row (RAPs wrap modulo 52, the cluster modulo 3); the per-variant starting
//! values come from [`RAP_LEFT`], [`RAP_CENTRE`], [`RAP_RIGHT`] and [`START_CLUSTER`].
//!
//! The data symbol characters are the *same* three cluster tables as PDF417
//! ([`CODEWORD_PATTERNS`]); MicroPDF417 merely chooses a per-variant starting cluster.
//!
//! ## Lossless round-trip
//!
//! The width and height of a rendered symbol pin the column and row counts, which
//! uniquely identify the size variant; the variant fixes the error-correction
//! codeword count. The decoder therefore recovers the exact geometry and — via the
//! shared compaction inverse — the exact segments, so `encode(decode(x)) == x`.
//!
//! The tables here (size variants, RAP start indices and the two 52-entry RAP pattern
//! tables) are those of ISO/IEC 24728, cross-checked against the (BSD-3-Clause) zint
//! project's `pdf417_tabs.h`.

use std::collections::HashMap;

use super::tables::CODEWORD_PATTERNS;
use super::{EcLevel, Pdf417Meta, Pdf417Variant, compaction, ec};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::{Decode, Encode};

/// Number of identical module rows rendered per codeword row. The decoder derives the
/// row count from this, so any consistent value round-trips; 2 is MicroPDF417's
/// nominal default row height.
pub(super) const MICRO_ROW_HEIGHT: usize = 2;

/// Required light-module quiet zone around the symbol (ISO/IEC 24728 minimum is 1X).
const QUIET_ZONE: usize = 1;

/// Width in modules of a data symbol character.
const CW_WIDTH: usize = 17;
/// Width in modules of a Row Address Pattern.
const RAP_WIDTH: usize = 10;

/// The pad codeword used to fill unused data-codeword slots (PDF417 Text latch).
const PAD: u32 = 900;

/// Number of size variants defined by ISO/IEC 24728.
pub(super) const NUM_VARIANTS: usize = 34;

/// Data columns (`1..=4`) for each of the 34 size variants.
pub(super) const VAR_COLS: [u8; NUM_VARIANTS] = [
    1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    4, 4,
];

/// Row count for each size variant.
pub(super) const VAR_ROWS: [u8; NUM_VARIANTS] = [
    11, 14, 17, 20, 24, 28, 8, 11, 14, 17, 20, 23, 26, 6, 8, 10, 12, 15, 20, 26, 32, 38, 44, 4, 6,
    8, 10, 12, 15, 20, 26, 32, 38, 44,
];

/// Number of Reed–Solomon error-correction codewords for each size variant.
const VAR_EC: [u8; NUM_VARIANTS] = [
    7, 7, 7, 8, 8, 8, 8, 9, 9, 10, 11, 13, 15, 12, 14, 16, 18, 21, 26, 32, 38, 44, 50, 8, 12, 14,
    16, 18, 21, 26, 32, 38, 44, 50,
];

/// 1-based starting index of the left side RAP for each size variant.
const RAP_LEFT: [u8; NUM_VARIANTS] = [
    1, 8, 36, 19, 9, 25, 1, 1, 8, 36, 19, 9, 27, 1, 7, 15, 25, 37, 1, 1, 21, 15, 1, 47, 1, 7, 15,
    25, 37, 1, 1, 21, 15, 1,
];

/// 1-based starting index of the centre RAP for each size variant (0 where unused).
const RAP_CENTRE: [u8; NUM_VARIANTS] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 7, 15, 25, 37, 17, 9, 29, 31, 25, 19, 1, 7, 15, 25,
    37, 17, 9, 29, 31, 25,
];

/// 1-based starting index of the right side RAP for each size variant.
const RAP_RIGHT: [u8; NUM_VARIANTS] = [
    9, 8, 36, 19, 17, 33, 1, 9, 8, 36, 19, 17, 35, 1, 7, 15, 25, 37, 33, 17, 37, 47, 49, 43, 1, 7,
    15, 25, 37, 33, 17, 37, 47, 49,
];

/// Starting data cluster (0, 1 or 2 — i.e. clusters 0, 3 and 6) for each size variant.
const START_CLUSTER: [u8; NUM_VARIANTS] = [
    0, 1, 2, 0, 2, 0, 0, 0, 1, 2, 0, 2, 2, 0, 0, 2, 0, 0, 0, 0, 2, 2, 0, 1, 0, 0, 2, 0, 0, 0, 0, 2,
    2, 0,
];

/// The 52 side-RAP module patterns (left and right), 10 modules each, MSB first.
const RAP_SIDE: [u32; 52] = [
    0x322, 0x3A2, 0x3B2, 0x332, 0x372, 0x37A, 0x33A, 0x3BA, 0x39A, 0x3DA, 0x3CA, 0x38A, 0x30A,
    0x31A, 0x312, 0x392, 0x3D2, 0x3D6, 0x3D4, 0x394, 0x3B4, 0x3A4, 0x3A6, 0x3AE, 0x3AC, 0x3A8,
    0x328, 0x32C, 0x32E, 0x326, 0x336, 0x3B6, 0x396, 0x316, 0x314, 0x334, 0x374, 0x364, 0x366,
    0x36E, 0x36C, 0x368, 0x348, 0x358, 0x35C, 0x35E, 0x34E, 0x34C, 0x344, 0x346, 0x342, 0x362,
];

/// The 52 centre-RAP module patterns, 10 modules each, MSB first.
const RAP_CENTRE_PAT: [u32; 52] = [
    0x2CE, 0x24E, 0x26E, 0x22E, 0x226, 0x236, 0x216, 0x212, 0x21A, 0x23A, 0x232, 0x222, 0x262,
    0x272, 0x27A, 0x2FA, 0x2F2, 0x2F6, 0x276, 0x274, 0x264, 0x266, 0x246, 0x242, 0x2C2, 0x2E2,
    0x2E6, 0x2E4, 0x2EC, 0x26C, 0x22C, 0x228, 0x268, 0x2E8, 0x2C8, 0x2CC, 0x2C4, 0x2C6, 0x286,
    0x28E, 0x28C, 0x29C, 0x298, 0x2B8, 0x2B0, 0x290, 0x2D0, 0x250, 0x258, 0x25C, 0x2DC, 0x2DE,
];

/// The symbol width in modules for a variant with `cols` data columns.
pub(super) fn variant_width(cols: usize) -> usize {
    // left RAP + data columns + (a centre RAP when there are 3+ columns) + right RAP
    // + the single stop module.
    let centre = if cols >= 3 { RAP_WIDTH } else { 0 };
    RAP_WIDTH + cols * CW_WIDTH + centre + RAP_WIDTH + 1
}

/// Data-codeword capacity (total cells minus error-correction codewords) of a variant.
fn variant_capacity(variant: usize) -> usize {
    let total = VAR_COLS[variant] as usize * VAR_ROWS[variant] as usize;
    total - VAR_EC[variant] as usize
}

/// Locate the size variant with the given column and row counts, if any.
fn variant_for(cols: usize, rows: usize) -> Option<usize> {
    (0..NUM_VARIANTS).find(|&v| VAR_COLS[v] as usize == cols && VAR_ROWS[v] as usize == rows)
}

/// MicroPDF417 encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MicroPdf417Encoder;

impl MicroPdf417Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        MicroPdf417Encoder
    }

    /// Build a reproducible [`Symbol`] from `segments`, auto-selecting the smallest
    /// size variant that fits.
    pub fn build(&self, segments: Vec<Segment>) -> Result<Symbol> {
        self.build_sized(segments, None)
    }

    /// Like [`MicroPdf417Encoder::build`] but restricted to a specific data-column
    /// count (`1..=4`); the smallest fitting row count for that width is chosen.
    pub fn build_sized(&self, segments: Vec<Segment>, columns: Option<usize>) -> Result<Symbol> {
        let stream = compaction::encode_segments(&segments)?;
        let variant = choose_variant(stream.len(), columns)?;
        let meta = Pdf417Meta {
            rows: VAR_ROWS[variant] as usize,
            columns: VAR_COLS[variant] as usize,
            ec_level: EcLevel::new(0).unwrap(),
            variant: Pdf417Variant::Micro(variant as u8),
        };
        Ok(Symbol::new(
            Symbology::MicroPdf417,
            segments,
            SymbolMeta::Pdf417(meta),
        ))
    }

    /// Convenience: build a symbol from UTF-8 `text` as a single text segment.
    pub fn build_text(&self, text: &str) -> Result<Symbol> {
        self.build(vec![Segment::alphanumeric(text.as_bytes().to_vec())])
    }
}

impl Encode for MicroPdf417Encoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::MicroPdf417 {
            return Err(Error::invalid_parameter(
                "MicroPdf417Encoder given a non-MicroPDF417 symbol",
            ));
        }
        let variant = match &symbol.meta {
            SymbolMeta::Pdf417(m) => match m.variant {
                Pdf417Variant::Micro(v) => v as usize,
                Pdf417Variant::Standard => {
                    return Err(Error::invalid_parameter(
                        "MicroPDF417 symbol carries a standard PDF417 variant",
                    ));
                }
            },
            _ => {
                return Err(Error::invalid_parameter(
                    "MicroPDF417 symbol missing Pdf417Meta",
                ));
            }
        };
        if variant >= NUM_VARIANTS {
            return Err(Error::invalid_parameter("MicroPDF417 variant out of range"));
        }
        let matrix = render(&symbol.segments, variant)?;
        Ok(Encoding::Matrix(matrix))
    }
}

/// Assemble the full codeword grid (padded data followed by error correction) in
/// row-major order for `variant`.
fn build_codewords(segments: &[Segment], variant: usize) -> Result<Vec<u32>> {
    let k = VAR_EC[variant] as usize;
    let capacity = variant_capacity(variant);
    let stream = compaction::encode_segments(segments)?;
    if stream.len() > capacity {
        return Err(Error::capacity(
            "MicroPDF417 segments do not fit the chosen variant",
        ));
    }
    let mut data = Vec::with_capacity(capacity);
    data.extend_from_slice(&stream);
    while data.len() < capacity {
        data.push(PAD);
    }
    let ecc = ec::encode(&data, k);
    data.extend_from_slice(&ecc);
    debug_assert_eq!(
        data.len(),
        VAR_COLS[variant] as usize * VAR_ROWS[variant] as usize
    );
    Ok(data)
}

/// Render the module matrix for `variant`.
fn render(segments: &[Segment], variant: usize) -> Result<BitMatrix> {
    let cols = VAR_COLS[variant] as usize;
    let rows = VAR_ROWS[variant] as usize;
    let codewords = build_codewords(segments, variant)?;

    let width = variant_width(cols);
    let height = rows * MICRO_ROW_HEIGHT;
    let mut matrix = BitMatrix::new(width, height, QUIET_ZONE);

    let mut left = RAP_LEFT[variant] as usize - 1;
    let mut centre = RAP_CENTRE[variant].saturating_sub(1) as usize;
    let mut right = RAP_RIGHT[variant] as usize - 1;
    let mut cluster = START_CLUSTER[variant] as usize;

    for y in 0..rows {
        let mut bits: Vec<bool> = Vec::with_capacity(width);
        emit(&mut bits, RAP_SIDE[left], RAP_WIDTH);
        for col in 0..cols {
            let cw = codewords[y * cols + col] as usize;
            emit(&mut bits, CODEWORD_PATTERNS[cluster][cw], CW_WIDTH);
            // A centre RAP falls after the first column (3 cols) or the second
            // column (4 cols).
            if (cols == 3 && col == 0) || (cols == 4 && col == 1) {
                emit(&mut bits, RAP_CENTRE_PAT[centre], RAP_WIDTH);
            }
        }
        emit(&mut bits, RAP_SIDE[right], RAP_WIDTH);
        bits.push(true); // stop module
        debug_assert_eq!(bits.len(), width);

        for dy in 0..MICRO_ROW_HEIGHT {
            let yy = y * MICRO_ROW_HEIGHT + dy;
            for (x, &b) in bits.iter().enumerate() {
                if b {
                    matrix.set(x, yy, true);
                }
            }
        }

        left = if left == 51 { 0 } else { left + 1 };
        centre = if centre == 51 { 0 } else { centre + 1 };
        right = if right == 51 { 0 } else { right + 1 };
        cluster = if cluster == 2 { 0 } else { cluster + 1 };
    }
    Ok(matrix)
}

/// Append `width` modules of `pattern`, most-significant module first.
fn emit(bits: &mut Vec<bool>, pattern: u32, width: usize) {
    for i in (0..width).rev() {
        bits.push((pattern >> i) & 1 != 0);
    }
}

/// Choose the size variant for a `stream` of high-level codewords, optionally pinned
/// to a specific data-column count.
fn choose_variant(stream_len: usize, columns: Option<usize>) -> Result<usize> {
    let mut best: Option<(usize, usize)> = None; // (variant, total cells)
    for v in 0..NUM_VARIANTS {
        if let Some(c) = columns
            && VAR_COLS[v] as usize != c
        {
            continue;
        }
        if variant_capacity(v) < stream_len {
            continue;
        }
        let total = VAR_COLS[v] as usize * VAR_ROWS[v] as usize;
        // Smallest symbol area wins; ties (none in this table) keep the earlier one.
        if best.is_none_or(|(_, bt)| total < bt) {
            best = Some((v, total));
        }
    }
    best.map(|(v, _)| v).ok_or_else(|| {
        if columns.is_some() {
            Error::capacity("MicroPDF417 data does not fit the requested column count")
        } else {
            Error::capacity("MicroPDF417 data exceeds the largest variant")
        }
    })
}

/// MicroPDF417 structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MicroPdf417Decoder;

impl MicroPdf417Decoder {
    /// A new decoder.
    pub fn new() -> Self {
        MicroPdf417Decoder
    }

    /// Decode a sampled MicroPDF417 module grid into a [`Symbol`].
    pub fn decode_matrix(&self, matrix: &BitMatrix) -> Result<Symbol> {
        let width = matrix.width();
        let height = matrix.height();

        let cols = match width {
            38 => 1,
            55 => 2,
            82 => 3,
            99 => 4,
            _ => {
                return Err(Error::undecodable(
                    "width is not a valid MicroPDF417 geometry",
                ));
            }
        };
        if !height.is_multiple_of(MICRO_ROW_HEIGHT) {
            return Err(Error::undecodable(
                "height is not a multiple of the MicroPDF417 row height",
            ));
        }
        let rows = height / MICRO_ROW_HEIGHT;
        let variant = variant_for(cols, rows)
            .ok_or_else(|| Error::undecodable("no MicroPDF417 variant for this geometry"))?;

        let reverse = reverse_tables();
        let mut cluster = START_CLUSTER[variant] as usize;
        let mut codewords = Vec::with_capacity(cols * rows);
        let mut misses = 0usize;
        for y in 0..rows {
            let sample_row = y * MICRO_ROW_HEIGHT;
            for col in 0..cols {
                // Skip the left RAP, prior data columns and (for 3+ columns) a centre
                // RAP that precedes this column.
                let mut x0 = RAP_WIDTH + col * CW_WIDTH;
                if (cols == 3 && col >= 1) || (cols == 4 && col >= 2) {
                    x0 += RAP_WIDTH;
                }
                let pattern = read_pattern(matrix, x0, sample_row);
                let cw = match reverse[cluster].get(&pattern) {
                    Some(&cw) => cw,
                    None => {
                        misses += 1;
                        0
                    }
                };
                codewords.push(cw);
            }
            cluster = if cluster == 2 { 0 } else { cluster + 1 };
        }

        let k = VAR_EC[variant] as usize;
        // A pattern absent from the codeword tables is a guaranteed error; once they
        // exceed what Reed–Solomon can repair the grid cannot be genuine, and an
        // all-miss grid would otherwise "decode" perfectly (misses read as codeword
        // 0, and the all-zero vector is a valid RS codeword). Image samplers throw
        // many wrong geometry hypotheses at this decoder, so reject early and hard.
        if misses * 2 > k {
            return Err(Error::undecodable(
                "too many unrecognized MicroPDF417 codeword patterns",
            ));
        }
        let capacity = variant_capacity(variant);
        let corrected = ec::decode(&codewords, k).ok_or(Error::ErrorCorrectionFailed)?;
        // MicroPDF417 has no symbol-length descriptor; synthesise one so the shared
        // segment parser (which treats index 0 as a length) consumes exactly the
        // data-codeword region and drops the trailing pad codewords.
        let mut synthetic = Vec::with_capacity(capacity + 1);
        synthetic.push((capacity + 1) as u32);
        synthetic.extend_from_slice(&corrected[..capacity]);
        let segments = compaction::decode_segments(&synthetic)?;

        let meta = Pdf417Meta {
            rows,
            columns: cols,
            ec_level: EcLevel::new(0).unwrap(),
            variant: Pdf417Variant::Micro(variant as u8),
        };
        Ok(Symbol::new(
            Symbology::MicroPdf417,
            segments,
            SymbolMeta::Pdf417(meta),
        ))
    }
}

impl Decode for MicroPdf417Decoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "MicroPDF417 decode of a linear pattern",
            }),
        }
    }
}

/// Build the inverse `pattern -> codeword` maps for the three data clusters.
fn reverse_tables() -> [HashMap<u32, u32>; 3] {
    std::array::from_fn(|cluster| {
        CODEWORD_PATTERNS[cluster]
            .iter()
            .enumerate()
            .map(|(cw, &pat)| (pat, cw as u32))
            .collect()
    })
}

/// Read a 17-module symbol character starting at `x0` on module row `y` into a
/// 17-bit pattern (most-significant module first).
fn read_pattern(matrix: &BitMatrix, x0: usize, y: usize) -> u32 {
    let mut pattern = 0u32;
    for j in 0..CW_WIDTH {
        if matrix.get(x0 + j, y) {
            pattern |= 1 << (CW_WIDTH - 1 - j);
        }
    }
    pattern
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_have_expected_lengths() {
        assert_eq!(VAR_COLS.len(), NUM_VARIANTS);
        assert_eq!(VAR_ROWS.len(), NUM_VARIANTS);
        assert_eq!(VAR_EC.len(), NUM_VARIANTS);
        assert_eq!(RAP_LEFT.len(), NUM_VARIANTS);
        assert_eq!(RAP_CENTRE.len(), NUM_VARIANTS);
        assert_eq!(RAP_RIGHT.len(), NUM_VARIANTS);
        assert_eq!(START_CLUSTER.len(), NUM_VARIANTS);
        assert_eq!(RAP_SIDE.len(), 52);
        assert_eq!(RAP_CENTRE_PAT.len(), 52);
    }

    #[test]
    fn every_variant_geometry_is_unique() {
        for a in 0..NUM_VARIANTS {
            for b in (a + 1)..NUM_VARIANTS {
                assert!(
                    VAR_COLS[a] != VAR_COLS[b] || VAR_ROWS[a] != VAR_ROWS[b],
                    "variants {a} and {b} share a geometry"
                );
            }
        }
    }

    #[test]
    fn widths_match_layout() {
        assert_eq!(variant_width(1), 38);
        assert_eq!(variant_width(2), 55);
        assert_eq!(variant_width(3), 82);
        assert_eq!(variant_width(4), 99);
    }

    /// The single-character "A" symbol's data-codeword stream, cross-checked against
    /// zint 2.16.0 (`zint -b MICROPDF417 -d A --dump`): Text latch, `A`, then two pad
    /// codewords filling the four-codeword data region of variant 0 (1×11, EC 7).
    #[test]
    fn known_codewords_for_a() {
        let data = build_codewords(&[Segment::alphanumeric(b"A".to_vec())], 0).unwrap();
        assert_eq!(&data[..4], &[900, 29, 900, 900]);
        // Reed–Solomon codewords cross-checked against the same zint dump.
        assert_eq!(&data[4..], &[122, 330, 902, 353, 913, 357, 917]);
    }

    fn roundtrip(segments: Vec<Segment>, columns: Option<usize>) {
        let enc = MicroPdf417Encoder::new();
        let symbol = enc.build_sized(segments, columns).unwrap();
        let Encoding::Matrix(m) = enc.encode(&symbol).unwrap() else {
            panic!("expected a matrix");
        };
        let decoded = MicroPdf417Decoder::new().decode_matrix(&m).unwrap();
        assert_eq!(decoded.meta, symbol.meta);
        assert_eq!(decoded.segments, symbol.segments);
        // Re-encoding the decoded symbol reproduces the identical matrix.
        let Encoding::Matrix(m2) = enc.encode(&decoded).unwrap() else {
            panic!("expected a matrix");
        };
        assert_eq!(m, m2);
    }

    #[test]
    fn roundtrips_across_columns_and_modes() {
        for cols in 1..=4 {
            roundtrip(
                vec![Segment::alphanumeric(b"ABCDEFGH".to_vec())],
                Some(cols),
            );
            roundtrip(vec![Segment::numeric(b"123456789012".to_vec())], Some(cols));
            roundtrip(
                vec![Segment::byte(vec![0, 1, 2, 250, 251, 252])],
                Some(cols),
            );
        }
        roundtrip(
            vec![
                Segment::alphanumeric(b"Micro".to_vec()),
                Segment::numeric(b"42".to_vec()),
            ],
            None,
        );
    }
}
