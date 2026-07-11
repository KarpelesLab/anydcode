//! [`Symbol`] — the decoded representation of a barcode, and the exact input the
//! encoder consumes to reproduce it.
//!
//! A `Symbol` is designed to be *lossless*: decoding a scanned code and feeding the
//! result straight back into the encoder yields a byte-for-byte identical symbol.
//! It therefore records not just the payload but the encoding decisions
//! (segmentation, version/size, error-correction level, mask, ...) via
//! [`SymbolMeta`].

use crate::geometry::Location;
use crate::segment::{Mode, Segment};
use crate::symbology::Symbology;

/// Symbology-specific parameters required to re-encode a symbol identically.
///
/// One variant per implemented symbology; `Generic` is a placeholder used while a
/// symbology has segments modeled but no bespoke metadata yet.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SymbolMeta {
    /// QR Code parameters (version, EC level, mask, structural flags).
    Qr(crate::codes::qr::QrMeta),
    /// No symbology-specific metadata captured.
    Generic,
}

/// A fully decoded barcode symbol.
///
/// `PartialEq` compares payload and metadata (including [`Symbol::location`]); note
/// it is not `Eq` because [`Location`] carries floating-point geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    /// Which symbology this is.
    pub symbology: Symbology,
    /// The payload, split into mode-tagged segments in symbol order.
    pub segments: Vec<Segment>,
    /// Symbology-specific re-encoding parameters.
    pub meta: SymbolMeta,
    /// Where the symbol was found in the source frame, if produced by detection.
    /// Not part of the payload and ignored by the encoder.
    pub location: Option<Location>,
}

impl Symbol {
    /// Build a symbol from its parts, with no location.
    pub fn new(symbology: Symbology, segments: Vec<Segment>, meta: SymbolMeta) -> Self {
        Symbol {
            symbology,
            segments,
            meta,
            location: None,
        }
    }

    /// Best-effort concatenation of the payload bytes across all data segments.
    ///
    /// This flattens segmentation and ignores ECI switches; it is a convenience for
    /// callers that only want the raw content and do not care how it was encoded.
    /// For lossless work, inspect [`Symbol::segments`] directly.
    pub fn payload_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for seg in &self.segments {
            if seg.mode.is_data() {
                out.extend_from_slice(&seg.data);
            }
        }
        out
    }

    /// Best-effort UTF-8 decoding of [`Symbol::payload_bytes`].
    ///
    /// Returns `None` if the concatenated payload is not valid UTF-8.
    pub fn text(&self) -> Option<String> {
        String::from_utf8(self.payload_bytes()).ok()
    }

    /// The ordered list of data modes used, ignoring control segments. Handy for tests
    /// and diagnostics.
    pub fn modes(&self) -> Vec<Mode> {
        self.segments.iter().map(|s| s.mode.clone()).collect()
    }
}
