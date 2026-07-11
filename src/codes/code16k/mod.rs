//! Code 16K (BS EN 12323:2005): a stacked symbology built on the Code 128 character
//! set, holding 2 to 16 rows of five symbol characters each.
//!
//! Layout:
//! - `tables`  — the Code 128 symbol patterns and the Code 16K row start/stop
//!   delimiters and row-number lookup tables.
//! - `encode`  — [`Symbol`] -> [`BitMatrix`], plus the fresh-input `build` path.
//! - `decode`  — [`BitMatrix`] -> [`Symbol`].
//!
//! ## Symbol structure
//!
//! Each of the `rows` stacked rows is 70 modules wide: a four-element start delimiter,
//! a one-module guard bar, five Code 128 symbol characters (11 modules each), and a
//! four-element stop delimiter. The start/stop delimiters are chosen from an eight-entry
//! table by the row number (EN 12323 Table 5), so a decoder can recover each row's
//! position. The first symbol character of the whole symbol is a mode/row-count
//! character `7*(rows-2)+m`, where `m` selects the initial Code 128 code set; the last
//! two symbol characters are the two modulo-107 check characters.
//!
//! ## Lossless round-trip
//!
//! [`Code16kMeta`] stores the exact symbol-value sequence (mode character, data and
//! padding — the two derived check characters are recomputed on render). Re-encoding
//! renders straight from that sequence, so `encode(decode(x)) == x` regardless of how
//! the payload could otherwise have been encoded. The payload [`Segment`]s carried
//! alongside are the decoded bytes and are not consulted when re-encoding.
//!
//! ## Deviations
//!
//! The fresh-input planner ([`Code16kEncoder::build`]) uses Code 128 sets A and B only
//! (no Code C numeric compression) and supports bytes `0..=127`; the decoder handles
//! Code C, FNC1/GS1 and shifts so it can also read symbols that use them.
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix
//! [`Segment`]: crate::Segment

mod decode;
mod encode;
mod tables;

pub use decode::Code16kDecoder;
pub use encode::Code16kEncoder;

/// Parameters required to re-encode a Code 16K symbol identically (lossless round-trip).
///
/// [`Code16kMeta::values`] is the exact symbol-value sequence — the mode/row-count
/// character (index 0) followed by every data and padding value — of length
/// `rows*5 - 2`. The two modulo-107 check characters are derived on encode and are
/// **not** stored here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Code16kMeta {
    /// Number of stacked rows (`2..=16`).
    pub rows: usize,
    /// The mode/row-count character followed by every data and padding symbol value.
    pub values: Vec<u8>,
}
