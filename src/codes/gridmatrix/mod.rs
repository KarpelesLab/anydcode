//! Grid Matrix (AIMD014 / GB/T 27766) encoder and structural decoder.
//!
//! Grid Matrix is a Chinese national 2D matrix symbology. A symbol is a square array
//! of `2·v + 1` **macromodules** per side (version `v` = 1…13); each macromodule is a
//! 6×6 block whose fixed perimeter forms the alternating "grid" and whose 4×4 interior
//! carries a 2-bit layer-id plus two 7-bit codewords. Error correction is Reed–Solomon
//! over GF(2^7).
//!
//! Layout:
//! - [`gf`]     — GF(2^7) arithmetic (primitive `0x89`) and Reed–Solomon.
//! - `tables`   — versions, EC levels, and the per-version RS block structure.
//! - `data`     — mode-switched data encodation and canonical segmentation.
//! - `layout`   — macromodule placement, the frame, and the layer-id metadata.
//! - `encode`   — [`Symbol`] → [`BitMatrix`].
//! - `decode`   — [`BitMatrix`] → [`Symbol`].
//!
//! # Scope
//!
//! All 13 versions are implemented, along with the full five-level Reed–Solomon block
//! structure (transcribed from zint). Data encodation covers **Numeric**, **Upper**,
//! **Lower** and **Byte** modes; **Mixed** and **GB2312 Chinese** modes are modelled
//! but not produced by the encoder (the decoder reports them as unsupported). See
//! `data` for the exact per-mode bit formats and the small, documented deviations in
//! byte mode.
//!
//! # Losslessness and metadata
//!
//! [`GridMatrixEncoder::build`] splits the payload into a canonical mode segmentation
//! that the decoder reconstructs exactly, so `encode(decode(x)) == x`. [`GridMatrixMeta`]
//! records the version, EC level, and per-segment mode sequence needed to re-encode
//! identically.
//!
//! Grid Matrix defines **no** QR-style data mask: the alternating macromodule frame
//! already balances the module pattern, and the version/EC metadata rides in redundant
//! layer-id rings rather than a masked format field. `GridMatrixMeta` therefore stores
//! no mask (a deliberate deviation from the task's generic "version/EC/mask" shape).
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix

mod data;
mod decode;
mod encode;
pub mod gf;
mod layout;
mod tables;

pub use data::GmMode;
pub use decode::GridMatrixDecoder;
pub use encode::GridMatrixEncoder;
pub use tables::{EcLevel, Version};

/// Grid Matrix parameters needed to re-encode a symbol identically (lossless
/// round-trip).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridMatrixMeta {
    /// Symbol version (1–13), which fixes the module size.
    pub version: Version,
    /// Reed–Solomon error-correction level.
    pub ec_level: EcLevel,
    /// The Grid Matrix encodation mode of each data segment, in symbol order.
    /// Parallel to [`crate::Symbol::segments`].
    pub modes: Vec<GmMode>,
}

impl Default for GridMatrixMeta {
    fn default() -> Self {
        GridMatrixMeta {
            version: Version::new(1).unwrap(),
            ec_level: EcLevel::L5,
            modes: Vec::new(),
        }
    }
}
