//! PDF417 encoding: [`Symbol`] → [`BitMatrix`].
//!
//! The [`Pdf417Encoder`] reproduces an exact symbol from a fully-specified
//! [`Pdf417Meta`] (the round-trip path) or builds a fresh symbol from segments,
//! choosing a fitting row/column geometry for the requested error-correction level.

use super::tables::{
    CODEWORD_PATTERNS, CODEWORD_WIDTH, START_PATTERN, START_WIDTH, STOP_PATTERN, STOP_WIDTH,
};
use super::{EcLevel, Pdf417Meta, compaction, ec};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// Number of identical module rows rendered per codeword row (PDF417 rows are at
/// least three modules tall). The decoder derives the row count from this.
pub(super) const ROW_HEIGHT: usize = 3;

/// Required light-module quiet zone around the symbol.
const QUIET_ZONE: usize = 2;

/// Maximum total codewords (data + EC + symbol-length descriptor).
const MAX_CODEWORDS: usize = 929;

/// PDF417 encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Pdf417Encoder;

impl Pdf417Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        Pdf417Encoder
    }

    /// Build a reproducible [`Symbol`] from `segments` at error-correction `level`,
    /// auto-selecting the column count. The returned [`Pdf417Meta`] pins the chosen
    /// geometry so re-encoding is identical.
    pub fn build(&self, segments: Vec<Segment>, level: EcLevel) -> Result<Symbol> {
        self.build_sized(segments, level, None)
    }

    /// Like [`Pdf417Encoder::build`] but with an explicit data-column count
    /// (`1..=30`).
    pub fn build_sized(
        &self,
        segments: Vec<Segment>,
        level: EcLevel,
        columns: Option<usize>,
    ) -> Result<Symbol> {
        let stream = compaction::encode_segments(&segments)?;
        let (columns, rows) = choose_dimensions(stream.len(), level, columns)?;
        let meta = Pdf417Meta {
            rows,
            columns,
            ec_level: level,
        };
        Ok(Symbol::new(
            Symbology::Pdf417,
            segments,
            SymbolMeta::Pdf417(meta),
        ))
    }

    /// Convenience: build a symbol from UTF-8 `text` as a single text segment.
    pub fn build_text(&self, text: &str, level: EcLevel) -> Result<Symbol> {
        self.build(vec![Segment::alphanumeric(text.as_bytes().to_vec())], level)
    }
}

impl Encode for Pdf417Encoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::Pdf417 {
            return Err(Error::invalid_parameter(
                "Pdf417Encoder given a non-PDF417 symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Pdf417(m) => m,
            _ => return Err(Error::invalid_parameter("PDF417 symbol missing Pdf417Meta")),
        };
        let matrix = render(&symbol.segments, meta)?;
        Ok(Encoding::Matrix(matrix))
    }
}

/// Assemble the full codeword grid (data + EC), one entry per data-region cell in
/// row-major order.
pub(super) fn build_codewords(segments: &[Segment], meta: &Pdf417Meta) -> Result<Vec<u32>> {
    let k = meta.ec_level.ec_codewords();
    let total = meta.rows * meta.columns;
    if total < k + 1 {
        return Err(Error::invalid_parameter(
            "PDF417 geometry too small for this EC level",
        ));
    }
    let n = total - k; // data codewords including the symbol-length descriptor
    let stream = compaction::encode_segments(segments)?;
    if stream.len() + 1 > n {
        return Err(Error::capacity(
            "PDF417 segments do not fit the chosen geometry",
        ));
    }
    if n + k > MAX_CODEWORDS {
        return Err(Error::capacity("PDF417 message exceeds 929 codewords"));
    }

    let mut data = Vec::with_capacity(n);
    data.push(n as u32); // symbol-length descriptor
    data.extend_from_slice(&stream);
    while data.len() < n {
        data.push(900); // pad codeword
    }

    let ecc = ec::encode(&data, k);
    data.extend_from_slice(&ecc);
    debug_assert_eq!(data.len(), total);
    Ok(data)
}

/// Render the module matrix for a symbol.
fn render(segments: &[Segment], meta: &Pdf417Meta) -> Result<BitMatrix> {
    validate_geometry(meta)?;
    let codewords = build_codewords(segments, meta)?;
    let c = meta.columns;
    let rows = meta.rows;
    let width = 17 * c + 69;
    let height = rows * ROW_HEIGHT;
    let mut matrix = BitMatrix::new(width, height, QUIET_ZONE);

    for y in 0..rows {
        let cluster = y % 3;
        let (left, right) = row_indicators(y, rows, c, meta.ec_level.level() as usize);
        let mut bits: Vec<bool> = Vec::with_capacity(width);
        emit(&mut bits, START_PATTERN, START_WIDTH);
        emit(&mut bits, CODEWORD_PATTERNS[cluster][left], CODEWORD_WIDTH);
        for col in 0..c {
            let cw = codewords[y * c + col] as usize;
            emit(&mut bits, CODEWORD_PATTERNS[cluster][cw], CODEWORD_WIDTH);
        }
        emit(&mut bits, CODEWORD_PATTERNS[cluster][right], CODEWORD_WIDTH);
        emit(&mut bits, STOP_PATTERN, STOP_WIDTH);
        debug_assert_eq!(bits.len(), width);

        for dy in 0..ROW_HEIGHT {
            let yy = y * ROW_HEIGHT + dy;
            for (x, &b) in bits.iter().enumerate() {
                if b {
                    matrix.set(x, yy, true);
                }
            }
        }
    }
    Ok(matrix)
}

/// Append `width` modules of `pattern`, most-significant module first.
fn emit(bits: &mut Vec<bool>, pattern: u32, width: usize) {
    for i in (0..width).rev() {
        bits.push((pattern >> i) & 1 != 0);
    }
}

/// The left and right row-indicator codeword values for row `y` (ISO/IEC 15438 §5.5).
pub(super) fn row_indicators(
    y: usize,
    rows: usize,
    cols: usize,
    ec_level: usize,
) -> (usize, usize) {
    let cluster = y % 3;
    let base = 30 * (y / 3);
    match cluster {
        0 => (base + (rows - 1) / 3, base + (cols - 1)),
        1 => (base + ec_level * 3 + (rows - 1) % 3, base + (rows - 1) / 3),
        _ => (base + (cols - 1), base + ec_level * 3 + (rows - 1) % 3),
    }
}

fn validate_geometry(meta: &Pdf417Meta) -> Result<()> {
    if !(1..=30).contains(&meta.columns) {
        return Err(Error::invalid_parameter("PDF417 columns must be 1..=30"));
    }
    if !(3..=90).contains(&meta.rows) {
        return Err(Error::invalid_parameter("PDF417 rows must be 3..=90"));
    }
    Ok(())
}

/// Choose `(columns, rows)` for `source` high-level codewords at `level`.
fn choose_dimensions(
    source: usize,
    level: EcLevel,
    columns: Option<usize>,
) -> Result<(usize, usize)> {
    let k = level.ec_codewords();
    let need = source + 1 + k; // SLD + source + EC

    let try_cols = |cols: usize| -> Option<usize> {
        if !(1..=30).contains(&cols) {
            return None;
        }
        let rows = need.div_ceil(cols).max(3);
        if rows > 90 {
            return None;
        }
        let total = cols * rows;
        let n = total - k;
        if n < source + 1 || n > 928 {
            return None;
        }
        Some(rows)
    };

    if let Some(cols) = columns {
        let rows = try_cols(cols).ok_or_else(|| {
            Error::capacity("PDF417 data does not fit the requested column count")
        })?;
        return Ok((cols, rows));
    }

    // Auto: minimise wasted (pad) codewords, preferring a wider symbol on ties.
    let mut best: Option<(usize, usize, usize)> = None; // (cols, rows, pad)
    for cols in 1..=30 {
        if let Some(rows) = try_cols(cols) {
            let pad = cols * rows - k - (source + 1);
            // `pad <= bp` lets a later (wider) column win on equal waste.
            if best.as_ref().is_none_or(|&(_, _, bp)| pad <= bp) {
                best = Some((cols, rows, pad));
            }
        }
    }
    best.map(|(c, r, _)| (c, r))
        .ok_or_else(|| Error::capacity("PDF417 data does not fit any valid geometry"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indicator_formulas_match_spec() {
        // Cluster 0 encodes (rows-1)/3 on the left and (cols-1) on the right.
        let (l, r) = row_indicators(0, 6, 4, 2);
        assert_eq!((l, r), ((6 - 1) / 3, 4 - 1));
        // Cluster 1 encodes ecLevel*3 + (rows-1)%3 on the left.
        let (l, _r) = row_indicators(1, 6, 4, 2);
        assert_eq!(l, 2 * 3 + (6 - 1) % 3);
    }

    #[test]
    fn dimensions_fit() {
        let level = EcLevel::new(2).unwrap();
        let (cols, rows) = choose_dimensions(20, level, None).unwrap();
        assert!((1..=30).contains(&cols));
        assert!((3..=90).contains(&rows));
        let n = cols * rows - level.ec_codewords();
        assert!(n > 20);
    }
}
