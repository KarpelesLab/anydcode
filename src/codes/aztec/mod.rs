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
//! encoder/decoder. ECI/FLG escapes are not implemented.
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix

mod decode;
mod encode;
pub mod gf;
mod highlevel;
mod layout;
mod rune;
mod tables;

pub use decode::AztecDecoder;
pub use encode::AztecEncoder;

/// Aztec Code has no mandatory quiet zone.
const QUIET_ZONE: usize = 0;

/// The physical side length of a full-range symbol with `layers` layers, including
/// the inserted reference grid.
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
