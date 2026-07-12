//! EAN/UPC (ISO/IEC 15420) encoder and structural decoder.
//!
//! Covers the whole retail linear family: EAN-13, EAN-8, UPC-A, UPC-E, and the
//! UPC/EAN 2- and 5-digit add-ons. Layout mirrors the QR module
//! ([`crate::codes::qr`]):
//! - `tables`  — L/G/R element patterns, parity tables, guards, check digits and
//!   the UPC-E ⟷ UPC-A expansion.
//! - `encode`  — [`Symbol`] → [`LinearPattern`].
//! - `decode`  — [`LinearPattern`] → [`Symbol`] (structural, from a clean module grid).
//! - `edge`    — [`Symbol`] from per-scanline sub-pixel run *widths* ([`scan`],
//!   [`decode_edges`]), the robust path for curved, blurred camera captures where hard
//!   quantization merges the single-module guards and elements.
//!
//! ## Lossless representation
//!
//! The human-readable digits (including the check digit) live verbatim in a single
//! [`Segment::numeric`] on the [`Symbol`]. Everything else needed to reproduce the
//! exact module pattern lives in [`EanMeta`]:
//! - [`EanMeta::variant`] pins which member of the family the digits belong to. In
//!   particular a scanned UPC-E keeps its compressed 8-digit form (number system +
//!   six payload digits + check) so it re-encodes to the UPC-E pattern rather than
//!   its expanded UPC-A equivalent.
//! - [`EanMeta::addon`] carries an optional 2- or 5-digit supplement attached to a
//!   main symbol, so `main + add-on` round-trips as one [`LinearPattern`].
//!
//! Standalone add-ons are represented as their own [`Symbology::Ean2`] /
//! [`Symbology::Ean5`] symbols with the digits in the segment and `addon = None`.
//!
//! ## UPC-A vs. leading-zero EAN-13
//!
//! A UPC-A symbol is bit-for-bit identical to an EAN-13 whose leading digit is `0`.
//! The decoder resolves this unavoidable ambiguity by reporting a 95-module symbol
//! with an implied leading `0` as [`EanVariant::UpcA`] (12 digits) and any other
//! leading digit as [`EanVariant::Ean13`] (13 digits). Consequently an EAN-13 built
//! with an explicit leading `0` re-encodes to the identical pattern but is *labelled*
//! UPC-A after a decode round-trip; every other input is label-stable.
//!
//! [`Symbol`]: crate::Symbol
//! [`Segment::numeric`]: crate::Segment::numeric
//! [`LinearPattern`]: crate::output::LinearPattern
//! [`Symbology::Ean2`]: crate::Symbology::Ean2
//! [`Symbology::Ean5`]: crate::Symbology::Ean5

mod decode;
mod edge;
mod encode;
mod tables;

pub use decode::EanDecoder;
pub use edge::{decode_edges, scan};
pub use encode::EanEncoder;

/// Which member of the EAN/UPC family a [`crate::Symbol`] represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EanVariant {
    /// EAN-13 (13 digits).
    Ean13,
    /// EAN-8 (8 digits).
    Ean8,
    /// UPC-A (12 digits).
    UpcA,
    /// UPC-E (8 digits: number system + six payload digits + check).
    UpcE,
    /// Standalone EAN-2 add-on (2 digits).
    Ean2,
    /// Standalone EAN-5 add-on (5 digits).
    Ean5,
}

/// Whether an add-on carries two or five digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOnKind {
    /// Two-digit supplement (checksum by value mod 4).
    Two,
    /// Five-digit supplement (weighted mod-10 checksum).
    Five,
}

/// A 2- or 5-digit supplemental add-on attached to a main EAN/UPC symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddOn {
    /// Whether this is a 2- or 5-digit supplement.
    pub kind: AddOnKind,
    /// The add-on digits as ASCII bytes (`b'0'..=b'9'`).
    pub digits: Vec<u8>,
}

/// EAN/UPC parameters needed to re-encode a symbol identically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EanMeta {
    /// Which family member the [`crate::Symbol`]'s digits belong to.
    pub variant: EanVariant,
    /// An optional 2- or 5-digit supplement following a main symbol.
    pub addon: Option<AddOn>,
}

impl EanMeta {
    /// Metadata for a plain main symbol with no add-on.
    pub fn new(variant: EanVariant) -> Self {
        EanMeta {
            variant,
            addon: None,
        }
    }
}
