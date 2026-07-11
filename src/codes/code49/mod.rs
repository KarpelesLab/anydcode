//! Code 49 (ANSI/AIM BC6-2000): a stacked alphanumeric symbology of 2 to 8 rows, each
//! row holding four 16-module symbol characters that pack seven base-49 codewords plus a
//! per-row check character.
//!
//! Layout:
//! - `tables`  — the 49-character chart, the even/odd 16-bit encodation patterns, the
//!   row-parity table and the symbol-check weight tables (extracted from the zint
//!   reference implementation, which follows the standard's Appendix E).
//! - `encode`  — [`Symbol`] -> [`BitMatrix`], plus the fresh-input `build` path.
//! - `decode`  — [`BitMatrix`] -> [`Symbol`].
//!
//! ## Symbol structure
//!
//! Each row is 70 modules wide: a two-module start ("10"), four symbol characters
//! (16 modules each), and a four-module stop ("1111"). A symbol character encodes a
//! value `0..=2400` (two base-49 code characters) as one of two 16-bit patterns chosen
//! by a per-row, per-column parity (Table 4); the last row uses even parity throughout.
//! The last row also carries the mode/row-count character and the X/Y/Z symbol check
//! characters (modulo 2401), and every row ends with a modulo-49 row check character.
//!
//! ## Lossless round-trip
//!
//! [`Code49Meta`] stores the exact code-character grid (`rows * 8`). Re-encoding renders
//! straight from that grid, so `encode(decode(x)) == x`. The payload
//! [`Segment`](crate::Segment) carried alongside is the decoded bytes and is not
//! consulted when re-encoding.
//!
//! ## Deviations
//!
//! The fresh-input planner ([`Code49Encoder::build`]) encodes each byte as one or two
//! base-49 codewords and does **not** use the Numeric Encodation optimization (5-digit
//! blocks); digits therefore encode one codeword each. Symbols remain valid and fully
//! lossless. Reconstruction of the payload for numeric-mode symbols is correspondingly
//! not implemented.
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix

mod decode;
mod encode;
mod tables;

pub use decode::Code49Decoder;
pub use encode::Code49Encoder;

/// Parameters required to re-encode a Code 49 symbol identically (lossless round-trip).
///
/// [`Code49Meta::grid`] is the row-major code-character grid (`rows * 8` values, each
/// `0..=48`): seven data/pad codewords plus one row check per row, with the last row's
/// columns repurposed for the symbol check and mode/row-count characters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Code49Meta {
    /// Number of stacked rows (`2..=8`).
    pub rows: usize,
    /// Row-major code-character grid (`rows * 8`).
    pub grid: Vec<u8>,
}
