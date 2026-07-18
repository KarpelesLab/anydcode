//! Micro QR Code (ISO/IEC 18004) encoder and structural decoder.
//!
//! Micro QR is the compact single-finder cousin of QR. It defines four sizes,
//! M1 (11×11) through M4 (17×17), a single finder pattern, edge timing patterns,
//! a 15-bit format code with its own BCH mask, four data masks and reduced
//! per-version mode/character-count fields. It reuses QR's GF(256) Reed–Solomon
//! ([`crate::codes::qr::gf`]) for error correction.
//!
//! Layout mirrors the QR module:
//! - `tables`  — per-version codeword/EC counts, field-width tables, format BCH.
//! - `matrix`  — module placement, edge timing, masking and format information.
//! - `encode`  — [`Symbol`] → [`BitMatrix`].
//! - `decode`  — [`BitMatrix`] → [`Symbol`].
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix

mod decode;
mod encode;
mod matrix;
mod sample;
mod tables;

pub use decode::MicroQrDecoder;
pub use encode::MicroQrEncoder;
pub use sample::scan;

/// A Micro QR symbol version, M1–M4. The module grid is `11 + 2 * (n - 1)` on a side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MicroVersion {
    /// 11×11, numeric only, error-detection only (no correctable EC).
    M1,
    /// 13×13, supports EC levels L and M.
    M2,
    /// 15×15, supports EC levels L and M.
    M3,
    /// 17×17, supports EC levels L, M and Q.
    M4,
}

impl MicroVersion {
    /// The version index 1–4 (M1 → 1 … M4 → 4).
    pub fn number(self) -> u8 {
        match self {
            MicroVersion::M1 => 1,
            MicroVersion::M2 => 2,
            MicroVersion::M3 => 3,
            MicroVersion::M4 => 4,
        }
    }

    /// Construct from the index 1–4.
    pub fn from_number(n: u8) -> Option<Self> {
        match n {
            1 => Some(MicroVersion::M1),
            2 => Some(MicroVersion::M2),
            3 => Some(MicroVersion::M3),
            4 => Some(MicroVersion::M4),
            _ => None,
        }
    }

    /// Side length of the (square) module grid.
    pub fn size(self) -> usize {
        11 + 2 * (self.number() as usize - 1)
    }

    /// Recover a version from the grid side length, if valid.
    pub fn from_size(size: usize) -> Option<Self> {
        match size {
            11 => Some(MicroVersion::M1),
            13 => Some(MicroVersion::M2),
            15 => Some(MicroVersion::M3),
            17 => Some(MicroVersion::M4),
            _ => None,
        }
    }
}

/// Micro QR error-correction level. `Detection` is the M1-only mode that carries
/// two error-*detection* codewords but no error *correction* capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MicroEcLevel {
    /// Error detection only (M1).
    Detection,
    /// ~7% recovery.
    L,
    /// ~15% recovery.
    M,
    /// ~25% recovery (M4 only).
    Q,
}

/// A Micro QR data mask pattern, 0–3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MicroMask(u8);

impl MicroMask {
    /// Construct a mask, validating the `0..=3` range.
    pub fn new(m: u8) -> Option<Self> {
        (m <= 3).then_some(MicroMask(m))
    }

    /// The mask pattern index, 0–3.
    pub fn index(self) -> u8 {
        self.0
    }
}

/// Micro QR-specific parameters needed to re-encode a symbol identically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicroQrMeta {
    /// Symbol version (size).
    pub version: MicroVersion,
    /// Error-correction level.
    pub ec_level: MicroEcLevel,
    /// Applied data mask pattern.
    pub mask: MicroMask,
}
