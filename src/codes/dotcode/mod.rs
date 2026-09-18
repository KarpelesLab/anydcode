//! DotCode (AIM ISS DotCode Rev 4.0) encoder and structural decoder.
//!
//! DotCode is a sparse 2D *dot matrix* designed for high-speed industrial printing
//! (thermal/inkjet). Data is compacted into a stream of base-113 codewords through a
//! Code 128-style Code Set A/B/C state machine (with a binary/byte mode reached via
//! an upper-shift or binary latch), protected by Reed–Solomon error correction over
//! the prime field GF(113), then folded onto a checkerboard of dots.
//!
//! Layout:
//! - `tables` — the 9-bit Annex C dot patterns and their reverse lookup.
//! - `rs`     — GF(113) prime-field arithmetic and Reed–Solomon encoding.
//! - [`encode`] — the A/B/C/binary encodation, sizing, padding, masking and fold.
//! - [`decode`] — the inverse: unfold, un-mask, RS-verify and segment reconstruction.
//!
//! ## Coordinate / checkerboard convention
//!
//! The output is an [`Encoding::Matrix`](crate::output::Encoding::Matrix) of
//! `width × height` modules where a **dark module is a printed dot**. Only cells with
//! `(x + y)` even carry a dot ("checkerboard"); all other cells are always light.
//! `width + height` is always odd, so exactly one dimension is odd. The codeword bit
//! stream (2 mask bits followed by 9 bits per codeword, then lit-dot padding) is
//! folded horizontally when the height is odd and vertically when the width is odd,
//! with six reserved corner dots carrying the final six bits — matching AIM ISS
//! DotCode Rev 4.0 Annex A. [`BitMatrix::get(x, y)`](crate::output::BitMatrix::get)
//! is column `x`, row `y`.
//!
//! ## Lossless round-trip
//!
//! [`DotCodeMeta`] pins the exact **unmasked data codewords** (post-padding),
//! the symbol `width`/`height`, and the data `mask` (`0..=7`). The encoder renders
//! straight from these — applying the mask, appending the Reed–Solomon check words,
//! and folding — so `encode(decode(x)) == x` byte-for-byte regardless of how the
//! same payload could otherwise have been encoded. The payload [`Segment`]s carried
//! alongside are a human-readable reconstruction and are not consulted on re-encode.
//!
//! ## Masks and error correction
//!
//! Mask selection follows the reference scoring: the best of masks `0..=3`, and when
//! even that scores poorly (typically an unlit edge, which would hide the symbol's
//! extent) the best of masks `4..=7` — the same four with the six corner dots forced
//! lit, the overwritten dots being left to error correction. The decoder corrects
//! errors and erasures (a dot group that is no valid pattern) with the GF(113)
//! Reed–Solomon code, per interleaved block.
//!
//! ## Deviations / cut scope
//!
//! - **Structured Append** and multi-segment **ECI** planning are not implemented in
//!   the fresh-input builder (single-segment ECI is). Reader-init (FNC3) is decodable
//!   but not exposed as a build option.
//!
//! [`Symbol`]: crate::Symbol
//! [`Segment`]: crate::Segment
//! [`BitMatrix`]: crate::output::BitMatrix

// With neither `encode` nor `decode` only the metadata types remain; their shared
// helpers are then unused.
#![cfg_attr(
    not(any(feature = "encode", feature = "decode")),
    allow(dead_code, unused_imports)
)]

use alloc::vec::Vec;

#[cfg(feature = "decode")]
pub mod decode;
#[cfg(all(feature = "alloc", feature = "encode"))]
pub mod encode;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
mod rs;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
mod tables;

#[cfg(feature = "decode")]
pub use decode::DotCodeDecoder;
#[cfg(all(feature = "alloc", feature = "encode"))]
pub use encode::DotCodeEncoder;

/// Minimum symbol dimension (modules) in either axis.
pub const MIN_SIZE: usize = 5;
/// Maximum symbol dimension (modules) in either axis.
pub const MAX_SIZE: usize = 200;

/// Parameters required to re-encode a DotCode symbol identically (lossless
/// round-trip).
///
/// [`DotCodeMeta::codewords`] is the exact sequence of **unmasked** data codewords
/// (each `0..=112`), including any padding codewords, in symbol order. Together with
/// the geometry and mask this fully determines the module matrix: the encoder
/// applies the mask, appends the GF(113) Reed–Solomon check words and folds the
/// result onto the checkerboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DotCodeMeta {
    /// Symbol width in modules (`5..=200`).
    pub width: usize,
    /// Symbol height in modules (`5..=200`); `width + height` is odd.
    pub height: usize,
    /// The data mask: `0..=3`, or `4..=7` for the same masks with the six corner
    /// dots forced lit.
    pub mask: u8,
    /// The unmasked data codewords (post-padding), each `0..=112`.
    pub codewords: Vec<u8>,
}

impl Default for DotCodeMeta {
    fn default() -> Self {
        DotCodeMeta {
            width: 7,
            height: 5,
            mask: 0,
            codewords: Vec::new(),
        }
    }
}
