//! Aztec Code (ISO/IEC 24778) encoder and structural decoder.
//!
//! Layout:
//! - [`gf`]        — Galois-field arithmetic and Reed–Solomon over GF(16/64/256/1024).
//! - `tables`      — the five high-level character sets, codeword sizes and capacities.
//! - `layout`      — bullseye, orientation marks, reference grid and the spiral path.
//! - `highlevel`   — payload bytes ⇄ mode-switched bit stream, plus bit-stuffing.
//! - `encode`      — [`Symbol`] → [`BitMatrix`].
//! - `decode`      — [`BitMatrix`] → [`Symbol`].
//!
//! Coverage: compact symbols (1–4 layers) and full-range symbols (1–22 layers) are
//! encoded and structurally decoded. The payload round-trips losslessly (as a single
//! byte segment) and re-encodes byte-for-byte identically. Aztec Runes (the fixed
//! 11×11 single-byte symbol, ISO/IEC 24778 Annex A) are also supported via the same
//! encoder/decoder. The decoder follows every high-level construct a third-party
//! symbol may use (latched Punct mode, `FLG(0)` FNC1 reported as GS, `FLG(n)` ECI
//! escapes reported as ECI segments); the encoder does not emit FLG escapes.
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix

// With neither `encode` nor `decode` only the metadata types remain; their shared
// helpers are then unused.
#![cfg_attr(
    not(any(feature = "encode", feature = "decode")),
    allow(dead_code, unused_imports)
)]

#[cfg(feature = "decode")]
mod decode;
#[cfg(all(feature = "alloc", feature = "encode"))]
mod encode;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
pub mod gf;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
mod highlevel;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
mod layout;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
mod rune;
#[cfg(feature = "scan")]
mod sample;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
mod tables;

#[cfg(feature = "decode")]
pub use decode::AztecDecoder;
#[cfg(all(feature = "alloc", feature = "encode"))]
pub use encode::AztecEncoder;
#[cfg(feature = "scan")]
pub use sample::scan;

/// Aztec Code has no mandatory quiet zone.
const QUIET_ZONE: usize = 0;

/// The physical side length of a full-range symbol with `layers` layers, including
/// the inserted reference grid.
#[cfg(feature = "decode")]
pub(crate) fn full_size(layers: usize) -> usize {
    let base = 14 + layers * 4;
    base + 1 + 2 * ((base / 2 - 1) / 15)
}

/// Parameters required to re-encode an Aztec symbol identically (lossless
/// round-trip): the symbol type and its layer count. The high-level encodation is
/// deterministic, so these fields plus the payload fully pin the matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AztecMeta {
    /// `true` for a compact symbol, `false` for a full-range symbol. Runes are
    /// compact.
    pub compact: bool,
    /// Number of data layers (1–4 compact, 1–22 full; `0` for an Aztec Rune).
    pub layers: u8,
    /// `true` for an Aztec Rune (the fixed 11×11 single-byte symbol, ISO/IEC 24778
    /// Annex A), which has no data layers.
    pub rune: bool,
}
