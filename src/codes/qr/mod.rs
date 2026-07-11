//! QR Code (ISO/IEC 18004) encoder and structural decoder.
//!
//! Layout:
//! - [`gf`]      — GF(256) arithmetic and Reed–Solomon encode/decode.
//! - `tables`    — the spec's per-version capacity, EC-block and alignment tables.
//! - `matrix`    — module placement, masking and format/version information.
//! - `encode`    — [`Symbol`] → [`BitMatrix`].
//! - `decode`    — [`BitMatrix`] → [`Symbol`].
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix

mod decode;
mod encode;
pub mod gf;
mod matrix;
mod sample;
mod tables;

pub use decode::QrDecoder;
pub use encode::QrEncoder;
pub use sample::{QrScanner, sample_grid, scan};

/// QR error-correction level, in ascending order of recovery capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EcLevel {
    /// ~7% recovery.
    L,
    /// ~15% recovery.
    M,
    /// ~25% recovery.
    Q,
    /// ~30% recovery.
    H,
}

impl EcLevel {
    /// The 2-bit field value used in the format information.
    pub fn format_bits(self) -> u8 {
        match self {
            EcLevel::L => 0b01,
            EcLevel::M => 0b00,
            EcLevel::Q => 0b11,
            EcLevel::H => 0b10,
        }
    }

    /// Recover an EC level from its 2-bit format field.
    pub fn from_format_bits(bits: u8) -> Self {
        match bits & 0b11 {
            0b01 => EcLevel::L,
            0b00 => EcLevel::M,
            0b11 => EcLevel::Q,
            _ => EcLevel::H,
        }
    }
}

/// A QR symbol version, 1–40. The module grid is `21 + 4 * (version - 1)` on a side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Version(u8);

impl Version {
    /// Construct a version, validating the `1..=40` range.
    pub fn new(v: u8) -> Option<Self> {
        (1..=40).contains(&v).then_some(Version(v))
    }

    /// The version number, 1–40.
    pub fn number(self) -> u8 {
        self.0
    }

    /// Side length of the module grid.
    pub fn size(self) -> usize {
        21 + 4 * (self.0 as usize - 1)
    }
}

/// The data mask pattern applied to the encoding region, 0–7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mask(u8);

impl Mask {
    /// Construct a mask, validating the `0..=7` range.
    pub fn new(m: u8) -> Option<Self> {
        (m <= 7).then_some(Mask(m))
    }

    /// The mask pattern index, 0–7.
    pub fn index(self) -> u8 {
        self.0
    }
}

/// QR-specific parameters needed to re-encode a symbol identically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrMeta {
    /// Symbol version (size).
    pub version: Version,
    /// Error-correction level.
    pub ec_level: EcLevel,
    /// Applied data mask pattern.
    pub mask: Mask,
}
