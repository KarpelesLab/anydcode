//! Data Matrix (ISO/IEC 16022) ECC 200 encoder and structural decoder.
//!
//! Layout:
//! - [`gf`]        — GF(256) arithmetic (primitive `0x12D`) and Reed–Solomon.
//! - `tables`      — the spec's per-size data/EC codeword and region geometry.
//! - `placement`   — Annex F module placement and finder/timing borders.
//! - `encode`      — [`Symbol`] → [`BitMatrix`].
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
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix
//! [`Mode::Byte`]: crate::segment::Mode::Byte

// With neither `encode` nor `decode` only the metadata types remain; their shared
// helpers are then unused.
#![cfg_attr(
    not(any(feature = "encode", feature = "decode")),
    allow(dead_code, unused_imports)
)]

use alloc::vec::Vec;

#[cfg(feature = "decode")]
mod decode;
#[cfg(all(feature = "alloc", feature = "encode"))]
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
#[cfg(all(feature = "alloc", feature = "encode"))]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataMatrixMeta {
    /// Full square symbol side length in modules (e.g. `10` for a 10×10 symbol).
    pub symbol_size: usize,
    /// The encodation used for each data segment, in symbol order. Parallel to
    /// [`crate::Symbol::segments`].
    pub encodations: Vec<Encodation>,
}
