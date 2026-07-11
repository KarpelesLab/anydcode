//! GS1 DataBar (formerly RSS) linear symbologies.
//!
//! Layout:
//! - `tables`  — the ISO/IEC 24724:2011 specification tables and the GTIN check digit.
//! - `widths`  — the combinatorial width <-> value algorithms (Annex B).
//! - `encode`  — [`Symbol`] -> [`LinearPattern`].
//! - `decode`  — [`LinearPattern`] -> [`Symbol`].
//!
//! ## Implemented
//! - **GS1 DataBar Omnidirectional** ([`Symbology::DataBarOmni`], RSS-14): a
//!   14-digit GTIN, 96 modules wide. Also covers Truncated, whose module pattern
//!   is identical (it differs only in printed height, which this crate's abstract
//!   module output does not carry).
//! - **GS1 DataBar Limited** ([`Symbology::DataBarLimited`]): a GTIN with
//!   indicator digit 0 or 1, 79 modules wide.
//!
//! ## Not yet implemented
//! - GS1 DataBar Expanded and all stacked variants (Stacked, Stacked
//!   Omnidirectional, Expanded Stacked) return [`Error::Unsupported`].
//!
//! ## Lossless model
//! Both implemented variants encode a canonical 14-digit GTIN (13-digit body plus
//! its GS1 mod-10 check digit) as a single [`Segment::numeric`]. Decoding recovers
//! that exact 14-digit segment, so `encode(decode(x)) == x` byte-for-byte.
//!
//! [`Symbol`]: crate::Symbol
//! [`Segment::numeric`]: crate::segment::Segment::numeric
//! [`LinearPattern`]: crate::output::LinearPattern
//! [`Error::Unsupported`]: crate::error::Error::Unsupported
//! [`Symbology::DataBarOmni`]: crate::symbology::Symbology::DataBarOmni
//! [`Symbology::DataBarLimited`]: crate::symbology::Symbology::DataBarLimited

mod decode;
mod encode;
mod tables;
mod widths;

pub use decode::DataBarDecoder;
pub use encode::DataBarEncoder;

/// Which member of the GS1 DataBar family a symbol is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DataBarVariant {
    /// GS1 DataBar Omnidirectional (RSS-14), 96 modules.
    Omni,
    /// GS1 DataBar Limited, 79 modules.
    Limited,
}

/// Parameters required to re-encode a GS1 DataBar symbol identically.
///
/// The payload (the 14-digit GTIN) lives in the symbol's segments; the only
/// re-encoding parameter is which family member produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataBarMeta {
    /// The DataBar family member.
    pub variant: DataBarVariant,
}

impl DataBarMeta {
    /// Metadata for a given variant.
    pub fn new(variant: DataBarVariant) -> Self {
        DataBarMeta { variant }
    }
}
