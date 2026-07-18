//! Rectangular Micro QR Code (rMQR, ISO/IEC 23941) encoder and structural decoder.
//!
//! rMQR is a rectangular member of the QR family designed for narrow print areas.
//! It defines 32 sizes (heights 7/9/11/13/15/17 × widths 27…139), a full 7×7 finder
//! plus a 5×5 finder-sub pattern at the opposite corner, corner-finder subpatterns,
//! edge timing patterns and optional interior alignment/timing columns, a single
//! fixed data mask, an 18-bit BCH format code in two copies, and two EC levels
//! (M and H). It reuses QR's GF(256) Reed–Solomon ([`crate::codes::qr::gf`]).
//!
//! **All 32 standard sizes are implemented** for encode and structural decode.
//!
//! Layout mirrors the QR module:
//! - `tables`  — the 32 sizes' dimensions, block layouts and field widths.
//! - `matrix`  — module placement, alignment/timing, format info and masking.
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

pub use decode::RmqrDecoder;
pub use encode::RmqrEncoder;
pub use sample::scan;

/// An rMQR symbol size, identified by its 5-bit version indicator (0..=31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RmqrSize(u8);

impl RmqrSize {
    /// Construct from a version indicator 0..=31.
    pub fn from_indicator(vi: u8) -> Option<Self> {
        (vi < 32).then_some(RmqrSize(vi))
    }

    /// Construct from `(width, height)` module dimensions, if a valid rMQR size.
    pub fn from_dimensions(width: usize, height: usize) -> Option<Self> {
        (0..32u8).find_map(|vi| {
            let info = &tables::SIZES[vi as usize];
            (info.width == width && info.height == height).then_some(RmqrSize(vi))
        })
    }

    /// The 5-bit version indicator.
    pub fn indicator(self) -> u8 {
        self.0
    }

    /// Symbol width in modules.
    pub fn width(self) -> usize {
        tables::SIZES[self.0 as usize].width
    }

    /// Symbol height in modules.
    pub fn height(self) -> usize {
        tables::SIZES[self.0 as usize].height
    }

    /// The `RxHxW` style name, e.g. `"R7x43"`.
    pub fn name(self) -> String {
        format!("R{}x{}", self.height(), self.width())
    }

    /// All 32 sizes in version-indicator order.
    pub fn all() -> impl Iterator<Item = RmqrSize> {
        (0..32u8).map(RmqrSize)
    }
}

/// rMQR error-correction level. Every size supports both M (~15%) and H (~30%).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RmqrEcLevel {
    /// ~15% recovery.
    M,
    /// ~30% recovery.
    H,
}

/// How [`RmqrEncoder::build_with`](crate::codes::rmqr::RmqrEncoder::build_with)
/// chooses among the rMQR sizes that fit the data. rMQR spans flat-and-wide to
/// tall-and-narrow form factors, so the caller can pick the shape for the space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SizeStrategy {
    /// Smallest total module area — the most compact fitting symbol (default).
    #[default]
    Balanced,
    /// Prefer the shortest (flattest, widest) fitting symbol.
    MinHeight,
    /// Prefer the tallest (narrowest) fitting symbol.
    MaxHeight,
}

/// rMQR-specific parameters needed to re-encode a symbol identically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmqrMeta {
    /// Symbol size.
    pub size: RmqrSize,
    /// Error-correction level.
    pub ec_level: RmqrEcLevel,
}
