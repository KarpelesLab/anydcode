//! PDF417 (ISO/IEC 15438) encoder and structural decoder.
//!
//! PDF417 is a *stacked* linear symbology: the payload is compacted into a stream
//! of base-929 codewords, protected with Reed–Solomon error correction over
//! GF(929), then laid out as a grid of rows. Every row is a sequence of
//! 17-module-wide symbol characters framed by a start pattern, a left row
//! indicator, the data columns, a right row indicator and a stop pattern.
//!
//! Layout:
//! - [`ec`]        — GF(929) prime-field arithmetic and Reed–Solomon encode/decode.
//! - `tables`      — the three cluster pattern tables and the start/stop patterns.
//! - `compaction`  — high-level Text/Byte/Numeric compaction and its inverse.
//! - `encode`      — [`Symbol`] → [`BitMatrix`].
//! - `decode`      — [`BitMatrix`] → [`Symbol`].
//!
//! ## Lossless round-trip
//!
//! Each data [`Segment`](crate::Segment) maps to exactly one compaction mode:
//! [`Mode::Numeric`](crate::Mode::Numeric) → Numeric, [`Mode::Byte`](crate::Mode::Byte)
//! → Byte, and [`Mode::Alphanumeric`](crate::Mode::Alphanumeric) → Text (PDF417 Text
//! compaction covers the full printable-ASCII set plus tab/CR/LF). The encoder emits
//! an explicit mode latch before every segment, so the decoder recovers the exact
//! segment boundaries. Adjacent segments of the *same* mode are the one case that
//! cannot be distinguished after the fact and are therefore not produced by the
//! decoder; callers should merge same-mode runs (the round-trip identity
//! `encode(decode(x)) == x` still holds because the decoder's canonical output
//! re-encodes to the identical matrix).
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix

mod compaction;
mod decode;
pub mod ec;
mod encode;
mod tables;

pub use decode::Pdf417Decoder;
pub use encode::Pdf417Encoder;

use crate::error::Error;

/// PDF417 error-correction level, `0..=8`.
///
/// Level `n` appends `2^(n+1)` Reed–Solomon check codewords (2, 4, 8, … 512),
/// trading symbol size for damage tolerance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EcLevel(u8);

impl EcLevel {
    /// Construct a level, validating the `0..=8` range.
    pub fn new(level: u8) -> Option<Self> {
        (level <= 8).then_some(EcLevel(level))
    }

    /// The level number, `0..=8`.
    pub fn level(self) -> u8 {
        self.0
    }

    /// The number of Reed–Solomon error-correction codewords this level adds.
    pub fn ec_codewords(self) -> usize {
        1usize << (self.0 as usize + 1)
    }
}

impl TryFrom<u8> for EcLevel {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self, Error> {
        EcLevel::new(value).ok_or_else(|| Error::invalid_parameter("PDF417 EC level must be 0..=8"))
    }
}

/// Parameters required to re-encode a PDF417 symbol identically (lossless round-trip).
///
/// The compaction-mode sequence itself lives in the symbol's ordered
/// [`Segment`](crate::Segment)s (one mode latch per segment); this metadata pins the
/// symbol geometry and error-correction strength, which together with the segments
/// fully determine the module matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pdf417Meta {
    /// Number of rows (`3..=90`).
    pub rows: usize,
    /// Number of data columns (`1..=30`), excluding start/stop and row indicators.
    pub columns: usize,
    /// Error-correction level.
    pub ec_level: EcLevel,
}

impl Default for Pdf417Meta {
    fn default() -> Self {
        Pdf417Meta {
            rows: 3,
            columns: 1,
            ec_level: EcLevel(0),
        }
    }
}
