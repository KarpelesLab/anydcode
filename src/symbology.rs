//! The catalog of barcode symbologies AnyDCode intends to support.
//!
//! Every variant is modeled here even when no encoder/decoder exists yet, so the
//! type system and the public API are stable as implementations land. Use
//! [`Symbology::is_implemented`] to query current coverage at runtime.
//!
//! The catalog is grouped by [`Dimension`] (how a detector must locate it): linear
//! bars, height-modulated postal codes, stacked linear rows, and 2D matrices. Some
//! symbologies are historically known by other names (e.g. GS1 DataBar was "RSS");
//! those aliases are noted in each variant's docs.

use core::fmt;

/// How a symbology is laid out — the primary axis a detector uses to find it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dimension {
    /// Linear (1D) codes: a single row of variable-width bars.
    Linear,
    /// Height-modulated postal codes: equal-width bars whose height/position encodes
    /// data (POSTNET, RM4SCC, ...). Located differently from width-modulated 1D codes.
    Postal,
    /// Stacked codes: several linear rows read top-to-bottom (PDF417, DataBar Stacked).
    Stacked,
    /// 2D matrix codes: a grid of cells (QR, Aztec, Data Matrix, ...).
    Matrix,
}

/// A barcode symbology. This enum is intentionally broad — it models AnyDCode's whole
/// roadmap. Unimplemented variants return [`crate::Error::Unsupported`] from
/// encode/decode; check [`Symbology::is_implemented`] first.
///
/// Every variant carries an **explicit, stable discriminant** (`symbology as u8`):
/// the values are append-only API, safe to persist or send over a wire — new
/// symbologies get fresh numbers at the end of the space and existing ones never
/// renumber, regardless of where a variant sits in the (grouped-by-family)
/// declaration order. The enum is `#[non_exhaustive]` for the same reason: match
/// with a wildcard arm so added variants cannot break downstream code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[repr(u8)]
pub enum Symbology {
    // ======== 2D: matrix ========
    /// QR Code (ISO/IEC 18004), versions 1–40.
    QrCode = 0,
    /// Micro QR Code, versions M1–M4.
    MicroQrCode = 1,
    /// Rectangular Micro QR Code (rMQR, ISO/IEC 23941).
    RectMicroQrCode = 2,
    /// Aztec Code (ISO/IEC 24778).
    Aztec = 3,
    /// Aztec Runes: the tiny fixed 11×11 Aztec message set (values 0–255).
    AztecRunes = 4,
    /// Data Matrix (ISO/IEC 16022), ECC 200.
    DataMatrix = 5,
    /// MaxiCode (ISO/IEC 16023), the fixed hexagonal-grid "bullseye" code.
    MaxiCode = 6,
    /// Han Xin Code (ISO/IEC 20830), Chinese-optimized 2D matrix.
    HanXin = 7,
    /// DotCode: a sparse dot matrix for high-speed printing.
    DotCode = 8,
    /// Grid Matrix, a Chinese 2D matrix symbology.
    GridMatrix = 9,
    /// Apple App Clip Code: the circular five-ring code scanned by iOS to launch App
    /// Clips. Proprietary (no published specification); modeled after the public
    /// reverse engineering of Apple's generator (see `codes::appclip`).
    AppClipCode = 51,

    // ======== 2D: stacked ========
    /// PDF417 (ISO/IEC 15438).
    Pdf417 = 10,
    /// MicroPDF417.
    MicroPdf417 = 11,
    /// Code 16K: stacked Code 128 rows.
    Code16k = 12,
    /// Code 49: stacked alphanumeric symbology.
    Code49 = 13,
    /// Codablock F: stacked Code 128.
    CodablockF = 14,

    // ======== 1D: EAN/UPC family ========
    /// EAN-13.
    Ean13 = 15,
    /// EAN-8.
    Ean8 = 16,
    /// UPC-A.
    UpcA = 17,
    /// UPC-E (zero-suppressed UPC).
    UpcE = 18,
    /// EAN/UPC 2-digit add-on (UPC/EAN Extension 2).
    Ean2 = 19,
    /// EAN/UPC 5-digit add-on (UPC/EAN Extension 5).
    Ean5 = 20,

    // ======== 1D: Code 128 / GS1-128 ========
    /// Code 128 (ISO/IEC 15417).
    Code128 = 21,
    /// GS1-128 (Application Identifiers carried over Code 128).
    Gs1_128 = 22,

    // ======== 1D: Code 39 / 93 / 11 ========
    /// Code 39 (ISO/IEC 16388), incl. the full-ASCII extension.
    Code39 = 23,
    /// Code 93.
    Code93 = 24,
    /// Code 11 (USD-8), telecom labeling.
    Code11 = 25,

    // ======== 1D: 2-of-5 family ========
    /// Interleaved 2 of 5 (ITF, ISO/IEC 16390).
    Itf = 26,
    /// Standard / industrial 2 of 5.
    Std2of5 = 27,
    /// IATA 2 of 5 (air cargo).
    Iata2of5 = 28,
    /// Matrix 2 of 5.
    Matrix2of5 = 29,

    // ======== 1D: other linear ========
    /// Codabar (a.k.a. "Coda", NW-7, USD-4).
    Codabar = 30,
    /// MSI Plessey.
    MsiPlessey = 31,
    /// Plessey (the original UK symbology).
    Plessey = 32,
    /// Telepen (full ASCII).
    Telepen = 33,
    /// Pharmacode (Laetus one-track pharmaceutical code).
    Pharmacode = 34,
    /// Two-track Pharmacode.
    PharmacodeTwoTrack = 35,
    /// DX film edge barcode (the code printed along 35mm film edges).
    DxFilmEdge = 36,

    // ======== 1D: GS1 DataBar family (formerly "RSS") ========
    /// GS1 DataBar Omnidirectional (RSS-14). Also covers Truncated.
    DataBarOmni = 37,
    /// GS1 DataBar Stacked (RSS-14 Stacked).
    DataBarStacked = 38,
    /// GS1 DataBar Stacked Omnidirectional.
    DataBarStackedOmni = 39,
    /// GS1 DataBar Limited (RSS Limited).
    DataBarLimited = 40,
    /// GS1 DataBar Expanded (RSS Expanded).
    DataBarExpanded = 41,
    /// GS1 DataBar Expanded Stacked.
    DataBarExpandedStacked = 42,

    // ======== Postal (height-modulated) ========
    /// USPS POSTNET.
    Postnet = 43,
    /// USPS PLANET.
    Planet = 44,
    /// USPS Intelligent Mail Barcode (OneCode, 4-state).
    IntelligentMail = 45,
    /// Royal Mail 4-State Customer Code (RM4SCC).
    RoyalMail = 46,
    /// Royal Mail Mailmark 4-state.
    Mailmark = 47,
    /// Australia Post 4-state.
    AustraliaPost = 48,
    /// Japan Post 4-state.
    JapanPost = 49,
    /// Dutch KIX (Royal TNT Post 4-state).
    KixCode = 50,
}

impl Symbology {
    /// Every modeled symbology, in catalog order. Useful for iterating candidate
    /// symbologies during detection or for enumerating coverage.
    pub const ALL: &'static [Symbology] = {
        use Symbology::*;
        &[
            QrCode,
            MicroQrCode,
            RectMicroQrCode,
            Aztec,
            AztecRunes,
            DataMatrix,
            MaxiCode,
            HanXin,
            DotCode,
            GridMatrix,
            AppClipCode,
            Pdf417,
            MicroPdf417,
            Code16k,
            Code49,
            CodablockF,
            Ean13,
            Ean8,
            UpcA,
            UpcE,
            Ean2,
            Ean5,
            Code128,
            Gs1_128,
            Code39,
            Code93,
            Code11,
            Itf,
            Std2of5,
            Iata2of5,
            Matrix2of5,
            Codabar,
            MsiPlessey,
            Plessey,
            Telepen,
            Pharmacode,
            PharmacodeTwoTrack,
            DxFilmEdge,
            DataBarOmni,
            DataBarStacked,
            DataBarStackedOmni,
            DataBarLimited,
            DataBarExpanded,
            DataBarExpandedStacked,
            Postnet,
            Planet,
            IntelligentMail,
            RoyalMail,
            Mailmark,
            AustraliaPost,
            JapanPost,
            KixCode,
        ]
    };

    /// The layout family this symbology belongs to, used to route detection.
    pub fn dimension(self) -> Dimension {
        use Symbology::*;
        match self {
            // Matrix
            QrCode | MicroQrCode | RectMicroQrCode | Aztec | AztecRunes | DataMatrix | MaxiCode
            | HanXin | DotCode | GridMatrix | AppClipCode => Dimension::Matrix,
            // Stacked
            Pdf417
            | MicroPdf417
            | Code16k
            | Code49
            | CodablockF
            | DataBarStacked
            | DataBarStackedOmni
            | DataBarExpandedStacked => Dimension::Stacked,
            // Postal
            Postnet | Planet | IntelligentMail | RoyalMail | Mailmark | AustraliaPost
            | JapanPost | KixCode => Dimension::Postal,
            // Everything else is width-modulated linear.
            _ => Dimension::Linear,
        }
    }

    /// Whether an encoder/decoder is currently implemented for this symbology.
    pub fn is_implemented(self) -> bool {
        use Symbology::*;
        // Gated on its cargo feature: the URL codec embeds ~1.7 MB of trained
        // frequency tables, which lean builds may want to leave out.
        if self == AppClipCode {
            return cfg!(feature = "appclip");
        }
        matches!(
            self,
            QrCode
                | MicroQrCode
                | RectMicroQrCode
                | Aztec
                | AztecRunes
                | HanXin
                | DotCode
                | GridMatrix
                | DataMatrix
                | MaxiCode
                | Pdf417
                | MicroPdf417
                | Code128
                | Gs1_128
                | Code39
                | Code93
                | Code11
                | Ean13
                | Ean8
                | UpcA
                | UpcE
                | Ean2
                | Ean5
                | Itf
                | Std2of5
                | Iata2of5
                | Matrix2of5
                | Codabar
                | MsiPlessey
                | Plessey
                | Telepen
                | Pharmacode
                | PharmacodeTwoTrack
                | DataBarOmni
                | DataBarLimited
                | DataBarExpanded
                | DataBarStacked
                | DataBarStackedOmni
                | DataBarExpandedStacked
                | Code16k
                | Code49
                | CodablockF
                | Postnet
                | Planet
                | IntelligentMail
                | RoyalMail
                | KixCode
                | Mailmark
                | AustraliaPost
                | JapanPost
                | DxFilmEdge
        )
    }

    /// A short, stable, human-readable name.
    pub fn name(self) -> &'static str {
        use Symbology::*;
        match self {
            QrCode => "QR Code",
            MicroQrCode => "Micro QR Code",
            RectMicroQrCode => "Rectangular Micro QR Code",
            Aztec => "Aztec",
            AztecRunes => "Aztec Runes",
            DataMatrix => "Data Matrix",
            MaxiCode => "MaxiCode",
            HanXin => "Han Xin",
            DotCode => "DotCode",
            GridMatrix => "Grid Matrix",
            AppClipCode => "App Clip Code",
            Pdf417 => "PDF417",
            MicroPdf417 => "MicroPDF417",
            Code16k => "Code 16K",
            Code49 => "Code 49",
            CodablockF => "Codablock F",
            Ean13 => "EAN-13",
            Ean8 => "EAN-8",
            UpcA => "UPC-A",
            UpcE => "UPC-E",
            Ean2 => "EAN-2 (add-on)",
            Ean5 => "EAN-5 (add-on)",
            Code128 => "Code 128",
            Gs1_128 => "GS1-128",
            Code39 => "Code 39",
            Code93 => "Code 93",
            Code11 => "Code 11",
            Itf => "Interleaved 2 of 5",
            Std2of5 => "Standard 2 of 5",
            Iata2of5 => "IATA 2 of 5",
            Matrix2of5 => "Matrix 2 of 5",
            Codabar => "Codabar",
            MsiPlessey => "MSI Plessey",
            Plessey => "Plessey",
            Telepen => "Telepen",
            Pharmacode => "Pharmacode",
            PharmacodeTwoTrack => "Pharmacode Two-Track",
            DxFilmEdge => "DX Film Edge",
            DataBarOmni => "GS1 DataBar Omnidirectional",
            DataBarStacked => "GS1 DataBar Stacked",
            DataBarStackedOmni => "GS1 DataBar Stacked Omnidirectional",
            DataBarLimited => "GS1 DataBar Limited",
            DataBarExpanded => "GS1 DataBar Expanded",
            DataBarExpandedStacked => "GS1 DataBar Expanded Stacked",
            Postnet => "POSTNET",
            Planet => "PLANET",
            IntelligentMail => "Intelligent Mail",
            RoyalMail => "Royal Mail 4-State (RM4SCC)",
            Mailmark => "Royal Mail Mailmark",
            AustraliaPost => "Australia Post 4-State",
            JapanPost => "Japan Post 4-State",
            KixCode => "Dutch KIX",
        }
    }
}

impl fmt::Display for Symbology {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalog_is_coherent() {
        // Names are non-empty and unique; every variant routes to a dimension.
        let mut names = HashSet::new();
        for &s in Symbology::ALL {
            assert!(!s.name().is_empty(), "{s:?} has an empty name");
            assert!(names.insert(s.name()), "duplicate name: {}", s.name());
            let _ = s.dimension();
        }
        // At least QR is implemented; coverage grows over time.
        assert!(Symbology::QrCode.is_implemented());
        assert!(Symbology::ALL.iter().any(|s| s.is_implemented()));
    }

    /// The discriminants are persisted-data / wire API: spot-check the anchors of
    /// each family block and prove the whole space stays collision-free, so an
    /// accidental insertion (rather than append) can never renumber a variant again.
    #[test]
    fn discriminants_are_stable() {
        use Symbology::*;
        for (sym, id) in [
            (QrCode, 0u8),
            (GridMatrix, 9),
            (Pdf417, 10),
            (CodablockF, 14),
            (Ean13, 15),
            (Code128, 21),
            (Code39, 23),
            (Itf, 26),
            (Codabar, 30),
            (DxFilmEdge, 36),
            (DataBarOmni, 37),
            (Postnet, 43),
            (KixCode, 50),
            // Declared amid the matrix family but numbered append-only.
            (AppClipCode, 51),
        ] {
            assert_eq!(sym as u8, id, "{sym:?} renumbered");
        }
        let mut seen = std::collections::HashSet::new();
        for &s in Symbology::ALL {
            assert!(seen.insert(s as u8), "duplicate discriminant for {s:?}");
        }
        assert_eq!(seen.len(), Symbology::ALL.len());
    }
}
