//! Codablock F (AIM ISS-X-24): a stacked Code 128 symbology. Each row is a Code 128
//! line carrying row-count/row-number indicator columns; the whole symbol is protected
//! by the K1/K2 data check characters.
//!
//! Layout:
//! - `tables`  — the Code 128 symbol patterns and Stop pattern.
//! - `encode`  — [`Symbol`] -> [`BitMatrix`], plus the fresh-input `build` path.
//! - `decode`  — [`BitMatrix`] -> [`Symbol`].
//!
//! ## Symbol structure
//!
//! Every row is `columns` Code 128 symbols wide: Start Code A, a code-set selector, a
//! row indicator (the total row count in row 0, the row number elsewhere), `columns-5`
//! data symbols, a modulo-103 Code 128 row check, and the Stop pattern. The two K1/K2
//! data check characters (Annex F, computed over the whole payload) occupy the last two
//! data positions of the final row.
//!
//! ## Lossless round-trip
//!
//! [`CodablockFMeta`] stores the exact Code 128 codeword grid (`rows * columns`).
//! Re-encoding renders straight from that grid, so `encode(decode(x)) == x`. The payload
//! [`Segment`](crate::Segment) carried alongside is the decoded bytes and is not
//! consulted when re-encoding.
//!
//! ## Deviations
//!
//! The fresh-input layout ([`CodablockFEncoder::build`]) uses Code 128 sets A and B
//! (no Code C numeric compression, no extended-ASCII FNC4) for bytes `0..=127`, chooses
//! a roughly-square column count automatically, and always appends a dedicated final row
//! for the K1/K2 checks. The decoder is more general: it validates any A/B/C row layout,
//! so it can read symbols (e.g. zint's) that pack the checks into a data row.
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix

mod decode;
mod encode;
mod tables;

pub use decode::CodablockFDecoder;
pub use encode::CodablockFEncoder;

/// Parameters required to re-encode a Codablock F symbol identically (lossless
/// round-trip).
///
/// [`CodablockFMeta::codewords`] is the full row-major grid of Code 128 symbol values
/// (`rows * columns` entries), including the per-row start/selector/indicator columns,
/// the K1/K2 checks, the per-row modulo-103 check and the Stop value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodablockFMeta {
    /// Number of stacked rows (`2..=44`).
    pub rows: usize,
    /// Total Code 128 columns per row (`9..=67`), i.e. `data columns + 5`.
    pub columns: usize,
    /// Row-major Code 128 codeword grid (`rows * columns`).
    pub codewords: Vec<u8>,
}
