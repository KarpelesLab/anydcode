//! Data Matrix (ISO/IEC 16022) ECC 200 encoder and structural decoder.
//!
//! Layout:
//! - [`gf`]        — GF(256) arithmetic (primitive `0x12D`) and Reed–Solomon.
//! - `tables`      — the spec's per-size data/EC codeword and region geometry.
//! - `placement`   — Annex F module placement and finder/timing borders.
//! - `encode`      — [`Symbol`] → [`BitMatrix`], or heap-free into a `MatrixBuf`.
//! - `decode`      — [`BitMatrix`] → [`Symbol`].
//!
//! # Scope
//!
//! Square symbol sizes 10×10 … 144×144 are implemented, including the multi-region
//! larger sizes and interleaved Reed–Solomon blocks. Data encodation covers **ASCII**
//! (with digit-pair packing and upper shift) and **Base256** for arbitrary bytes.
//! C40 / Text / X12 / EDIFACT encodation and the rectangular symbol sizes are not
//! implemented.
//!
//! # Losslessness
//!
//! [`DataMatrixEncoder::build`] splits the payload into a *canonical* alternating
//! sequence of ASCII runs (bytes `< 128`) and Base256 runs (bytes `>= 128`). This is
//! exactly the segmentation the decoder reconstructs from the codeword stream, so
//! `encode(decode(encoding)) == encoding` holds. Segments always use [`Mode::Byte`];
//! [`DataMatrixMeta`] records the square size and the per-segment encodation.
//!
//! A symbol segmented any other way re-encodes identically too. The decoder merges
//! adjacent ASCII runs, except where that would change how digits pair up: a digit
//! encoded on its own and followed by another digit ends its segment, since the
//! encoder packs two adjacent digits of one segment into a single codeword.
//!
//! A Base256 length field of 0 means "to the end of the symbol". A Base256 run that
//! is the last segment and ends on the last data codeword is written that way, as
//! zint, ZXing and libdmtx do (so their symbols re-encode identically); a run that
//! carries its byte count there instead decodes with an empty ASCII segment after
//! it. For the same reason an empty Base256 segment is only accepted as the last
//! codewords of a symbol it fills exactly.
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix
//! [`Mode::Byte`]: crate::segment::Mode::Byte

// With neither `encode` nor `decode` only the metadata types remain; their shared
// helpers are then unused.
#![cfg_attr(
    not(any(feature = "encode", feature = "decode")),
    allow(dead_code, unused_imports)
)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[cfg(feature = "decode")]
mod decode;
#[cfg(feature = "encode")]
mod encode;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
pub mod gf;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
mod placement;
#[cfg(feature = "scan")]
mod sample;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
mod tables;

#[cfg(feature = "decode")]
pub use decode::DataMatrixDecoder;
#[cfg(feature = "encode")]
pub use encode::DataMatrixEncoder;
#[cfg(feature = "scan")]
pub use sample::{DataMatrixScanner, sample_grid, scan};

/// The data-encodation scheme used for one segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encodation {
    /// ASCII encodation: digit pairs, direct byte values, and upper shift.
    Ascii,
    /// Base256 encodation: a latch, length field, and randomized raw bytes.
    Base256,
}

/// Data Matrix parameters needed to re-encode a symbol identically.
///
/// Heap-free encoding takes the same information as borrowed arguments of
/// `DataMatrixEncoder::encode_into` (the symbol size and an `&[Encodation]`).
#[cfg(feature = "alloc")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataMatrixMeta {
    /// Full square symbol side length in modules (e.g. `10` for a 10×10 symbol).
    pub symbol_size: usize,
    /// The encodation used for each data segment, in symbol order. Parallel to
    /// [`crate::Symbol::segments`].
    pub encodations: Vec<Encodation>,
}
