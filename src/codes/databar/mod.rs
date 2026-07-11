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
//! - **GS1 DataBar Stacked** ([`Symbology::DataBarStacked`], RSS-14 Stacked): the
//!   two-row form of DataBar-14. The left half (elements 0..23 of the linear
//!   symbol) becomes the top row, the right half (elements 23..46) the bottom row,
//!   with a computed separator row between them. 50 modules wide, 3 module-rows tall.
//! - **GS1 DataBar Stacked Omnidirectional** ([`Symbology::DataBarStackedOmni`],
//!   RSS-14 Stacked Omnidirectional): the omnidirectionally-scannable two-row form.
//!   Same left/right split, but with three separator rows (a top separator, a
//!   central alternating-module separator, and a bottom separator) so the two rows
//!   can be read independently. 50 modules wide, 5 module-rows tall.
//! - **GS1 DataBar Expanded Stacked** ([`Symbology::DataBarExpandedStacked`], RSS
//!   Expanded Stacked): the Expanded encodation (see `expanded`) split across
//!   several stacked rows of a configurable number of finder/data-character *column
//!   pairs* (codeblocks) per row, with separators between rows.
//!
//! ## Stacked layout convention
//! Stacked variants produce an [`Encoding::Matrix`]: a [`BitMatrix`] whose rows are,
//! top to bottom, the data rows interleaved with 1-module-tall separator rows, in
//! the order specified by ISO/IEC 24724:2011 §5.3 and §7.2.8. Each logical row
//! (data or separator) is exactly one module tall; the printed 5:7 / 34X row-height
//! ratios are a rendering concern the abstract module output does not carry (the
//! same choice the linear variants make for Truncated symbol height). The quiet
//! zone is a 1-module margin on all sides, matching the linear encoders. The
//! construction reuses the linear element-width encoders unchanged.
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
//! [`Symbology::DataBarStacked`]: crate::symbology::Symbology::DataBarStacked
//! [`Symbology::DataBarStackedOmni`]: crate::symbology::Symbology::DataBarStackedOmni
//! [`Symbology::DataBarExpandedStacked`]: crate::symbology::Symbology::DataBarExpandedStacked
//! [`Encoding::Matrix`]: crate::output::Encoding::Matrix
//! [`BitMatrix`]: crate::output::BitMatrix

mod decode;
mod encode;
mod expanded;
mod stacked;
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
    /// GS1 DataBar Stacked (RSS-14 Stacked): two rows, 50 modules wide.
    Stacked,
    /// GS1 DataBar Stacked Omnidirectional (RSS-14 Stacked Omnidirectional).
    StackedOmni,
    /// GS1 DataBar Expanded Stacked (RSS Expanded Stacked), multi-row.
    ExpandedStacked,
}

/// Parameters required to re-encode a GS1 DataBar symbol identically.
///
/// The payload (a 14-digit GTIN, or a reduced Expanded element string) lives in the
/// symbol's segments. Beyond the family member, the only stacked-specific parameter
/// is the number of column pairs per row for Expanded Stacked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataBarMeta {
    /// The DataBar family member.
    pub variant: DataBarVariant,
    /// For [`DataBarVariant::ExpandedStacked`]: the number of *column pairs*
    /// (codeblocks — finder + two data characters) placed per stacked row. This is
    /// the "segments per row" of ISO/IEC 24724:2011 §7.2.8 measured in
    /// data-character-pair columns (BWIPP's `segments` option is twice this value).
    /// `None` for every non-stacked-Expanded variant.
    pub columns_per_row: Option<usize>,
}

impl DataBarMeta {
    /// Metadata for a given variant (no stacked column parameter).
    pub fn new(variant: DataBarVariant) -> Self {
        DataBarMeta {
            variant,
            columns_per_row: None,
        }
    }

    /// Metadata for an Expanded Stacked symbol with `columns_per_row` column pairs
    /// (codeblocks) per stacked row.
    pub fn expanded_stacked(columns_per_row: usize) -> Self {
        DataBarMeta {
            variant: DataBarVariant::ExpandedStacked,
            columns_per_row: Some(columns_per_row),
        }
    }
}
