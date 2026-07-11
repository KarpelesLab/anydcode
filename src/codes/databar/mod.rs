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
//! - **GS1 DataBar Expanded** ([`Symbology::DataBarExpanded`], RSS Expanded): a GS1
//!   element string of Application Identifier data, encoded with the general-purpose
//!   compaction (§7.2.5) into a variable number of finder patterns and symbol
//!   characters. Encoding methods 1 and 2 are produced; the weight/date/price
//!   shortcut methods 3–14 are not (a spec-valid choice; see the `expanded` module).
//!
//! ## Not yet implemented
//! - All stacked variants (Stacked, Stacked Omnidirectional, Expanded Stacked)
//!   return [`Error::Unsupported`].
//!
//! ## Lossless model
//! Omnidirectional and Limited encode a canonical 14-digit GTIN (13-digit body plus
//! its GS1 mod-10 check digit) as a single [`Segment::numeric`]. Expanded stores its
//! reduced element string as a single [`Segment::byte`]. In every case decoding
//! recovers the exact segment(s), so `encode(decode(x)) == x` byte-for-byte.
//!
//! [`Symbol`]: crate::Symbol
//! [`Segment::numeric`]: crate::segment::Segment::numeric
//! [`Segment::byte`]: crate::segment::Segment::byte
//! [`LinearPattern`]: crate::output::LinearPattern
//! [`Error::Unsupported`]: crate::error::Error::Unsupported
//! [`Symbology::DataBarOmni`]: crate::symbology::Symbology::DataBarOmni
//! [`Symbology::DataBarLimited`]: crate::symbology::Symbology::DataBarLimited
//! [`Symbology::DataBarExpanded`]: crate::symbology::Symbology::DataBarExpanded

mod decode;
mod encode;
mod expanded;
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
    /// GS1 DataBar Expanded (RSS Expanded), variable length.
    Expanded,
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
