//! [`Symbol`] — the decoded representation of a barcode, and the exact input the
//! encoder consumes to reproduce it.
//!
//! A `Symbol` is designed to be *lossless*: decoding a scanned code and feeding the
//! result straight back into the encoder yields a byte-for-byte identical symbol.
//! It therefore records not just the payload but the encoding decisions
//! (segmentation, version/size, error-correction level, mask, ...) via
//! [`SymbolMeta`].

use alloc::string::String;
use alloc::vec::Vec;

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
    #[cfg(feature = "qr")]
    Qr(crate::codes::qr::QrMeta),
    /// EAN/UPC family parameters (which variant, add-on).
    #[cfg(feature = "ean")]
    Ean(crate::codes::ean::EanMeta),
    /// Code 39 parameters.
    #[cfg(feature = "code39")]
    Code39(crate::codes::code39::Code39Meta),
    /// Code 93 parameters.
    #[cfg(feature = "code93")]
    Code93(crate::codes::code93::Code93Meta),
    /// Code 11 parameters.
    #[cfg(feature = "code11")]
    Code11(crate::codes::code11::Code11Meta),
    /// Code 128 / GS1-128 parameters (code-set sequence).
    #[cfg(feature = "code128")]
    Code128(crate::codes::code128::Code128Meta),
    /// Interleaved 2 of 5 parameters.
    #[cfg(feature = "itf")]
    Itf(crate::codes::itf::ItfMeta),
    /// 2-of-5 family (standard/IATA/matrix) parameters.
    #[cfg(feature = "twoof5")]
    TwoOf5(crate::codes::twoof5::TwoOf5Meta),
    /// Codabar parameters (start/stop characters).
    #[cfg(feature = "codabar")]
    Codabar(crate::codes::codabar::CodabarMeta),
    /// MSI Plessey / Plessey parameters (check-digit scheme).
    #[cfg(feature = "msi")]
    Msi(crate::codes::msi::MsiMeta),
    /// Telepen parameters.
    #[cfg(feature = "telepen")]
    Telepen(crate::codes::telepen::TelepenMeta),
    /// Pharmacode parameters.
    #[cfg(feature = "pharmacode")]
    Pharmacode(crate::codes::pharmacode::PharmacodeMeta),
    /// GS1 DataBar family parameters.
    #[cfg(feature = "databar")]
    DataBar(crate::codes::databar::DataBarMeta),
    /// PDF417 / MicroPDF417 parameters.
    #[cfg(feature = "pdf417")]
    Pdf417(crate::codes::pdf417::Pdf417Meta),
    /// Data Matrix parameters.
    #[cfg(feature = "datamatrix")]
    DataMatrix(crate::codes::datamatrix::DataMatrixMeta),
    /// Aztec / Aztec Runes parameters.
    #[cfg(feature = "aztec")]
    Aztec(crate::codes::aztec::AztecMeta),
    /// MaxiCode parameters.
    #[cfg(feature = "maxicode")]
    MaxiCode(crate::codes::maxicode::MaxiCodeMeta),
    /// Micro QR Code parameters.
    #[cfg(feature = "microqr")]
    MicroQr(crate::codes::microqr::MicroQrMeta),
    /// Rectangular Micro QR (rMQR) parameters.
    #[cfg(feature = "rmqr")]
    Rmqr(crate::codes::rmqr::RmqrMeta),
    /// Postal (4-state / height-modulated) parameters.
    #[cfg(feature = "postal")]
    Postal(crate::codes::postal::PostalMeta),
    /// Han Xin Code parameters.
    #[cfg(feature = "hanxin")]
    HanXin(crate::codes::hanxin::HanXinMeta),
    /// Grid Matrix parameters.
    #[cfg(feature = "gridmatrix")]
    GridMatrix(crate::codes::gridmatrix::GridMatrixMeta),
    /// DotCode parameters.
    #[cfg(feature = "dotcode")]
    DotCode(crate::codes::dotcode::DotCodeMeta),
    /// Code 16K parameters.
    #[cfg(feature = "code16k")]
    Code16k(crate::codes::code16k::Code16kMeta),
    /// Code 49 parameters.
    #[cfg(feature = "code49")]
    Code49(crate::codes::code49::Code49Meta),
    /// Codablock F parameters.
    #[cfg(feature = "codablockf")]
    CodablockF(crate::codes::codablockf::CodablockFMeta),
    /// DX Film Edge parameters.
    #[cfg(feature = "dxfilm")]
    DxFilm(crate::codes::dxfilm::DxFilmMeta),
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
    /// For lossless work, inspect `Symbol::segments` directly.
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

    /// The mode of every segment in symbol order, [`Mode::Eci`] switches included.
    /// Handy for tests and diagnostics.
    pub fn modes(&self) -> Vec<Mode> {
        self.segments.iter().map(|s| s.mode).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Point, Quad};
    use alloc::vec;

    fn symbol(segments: Vec<Segment>) -> Symbol {
        Symbol::new(Symbology::QrCode, segments, SymbolMeta::Generic)
    }

    #[test]
    fn payload_concatenates_data_segments_and_skips_eci() {
        let s = symbol(vec![
            Segment::eci(26),
            Segment::numeric(b"123".to_vec()),
            Segment::alphanumeric(b"AB".to_vec()),
            Segment::eci(3),
            Segment::byte(b"c\xc3\xa9".to_vec()),
            Segment::kanji(vec![]),
        ]);
        assert_eq!(s.payload_bytes(), b"123ABc\xc3\xa9");
        assert_eq!(s.text().as_deref(), Some("123ABc\u{e9}"));
        assert_eq!(
            s.modes(),
            [
                Mode::Eci(26),
                Mode::Numeric,
                Mode::Alphanumeric,
                Mode::Eci(3),
                Mode::Byte,
                Mode::Kanji
            ]
        );
        assert_eq!(s.location, None);
        // An ECI segment that (wrongly) carries data still contributes nothing.
        let mut eci = Segment::eci(26);
        eci.data = b"junk".to_vec();
        assert_eq!(symbol(vec![eci]).payload_bytes(), b"");
    }

    #[test]
    fn empty_symbol_has_empty_text() {
        let s = symbol(Vec::new());
        assert!(s.payload_bytes().is_empty() && s.modes().is_empty());
        assert_eq!(s.text().as_deref(), Some(""));
    }

    /// `text()` is strict UTF-8 over the *whole* payload: Latin-1, Shift-JIS, a lone
    /// surrogate, or a character split across the payload's end are all `None` (the
    /// bytes stay available through `payload_bytes`).
    #[test]
    fn text_is_none_unless_the_whole_payload_is_utf8() {
        for bad in [
            &b"caf\xe9"[..],
            b"\x93\xfa\x96\x7b",
            b"\xed\xa0\x80",
            b"ok \xf0\x9f\x98",
            b"\xff",
        ] {
            let s = symbol(vec![Segment::byte(bad.to_vec())]);
            assert_eq!(s.text(), None, "{bad:?}");
            assert_eq!(s.payload_bytes(), bad);
        }
        // A UTF-8 character split across two segments is still one character.
        let split = symbol(vec![
            Segment::byte(b"\xe6\x97".to_vec()),
            Segment::byte(b"\xa5".to_vec()),
        ]);
        assert_eq!(split.text().as_deref(), Some("\u{65e5}"));
    }

    #[test]
    fn equality_covers_payload_meta_and_location() {
        let a = symbol(vec![Segment::byte(b"x".to_vec())]);
        let mut b = a.clone();
        assert_eq!(a, b);
        b.location = Some(Location {
            outline: Quad::new([Point::new(0.0, 0.0); 4]),
            rotation: None,
            module_size: Some(2.0),
        });
        assert_ne!(a, b);
        let mut c = a.clone();
        c.symbology = Symbology::Aztec;
        assert_ne!(a, c);
        // Same bytes, different segmentation: not the same symbol.
        let d = symbol(vec![Segment::alphanumeric(b"x".to_vec())]);
        assert_ne!(a, d);
        assert_eq!(a.payload_bytes(), d.payload_bytes());
    }
}
