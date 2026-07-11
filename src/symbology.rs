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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Symbology {
    // ======== 2D: matrix ========
    /// QR Code (ISO/IEC 18004), versions 1–40.
    QrCode,
    /// Micro QR Code, versions M1–M4.
    MicroQrCode,
    /// Rectangular Micro QR Code (rMQR, ISO/IEC 23941).
    RectMicroQrCode,
    /// Aztec Code (ISO/IEC 24778).
    Aztec,
    /// Aztec Runes: the tiny fixed 11×11 Aztec message set (values 0–255).
    AztecRunes,
    /// Data Matrix (ISO/IEC 16022), ECC 200.
    DataMatrix,
    /// MaxiCode (ISO/IEC 16023), the fixed hexagonal-grid "bullseye" code.
    MaxiCode,
    /// Han Xin Code (ISO/IEC 20830), Chinese-optimized 2D matrix.
    HanXin,
    /// DotCode: a sparse dot matrix for high-speed printing.
    DotCode,
    /// Grid Matrix, a Chinese 2D matrix symbology.
    GridMatrix,

    // ======== 2D: stacked ========
    /// PDF417 (ISO/IEC 15438).
    Pdf417,
    /// MicroPDF417.
    MicroPdf417,
    /// Code 16K: stacked Code 128 rows.
    Code16k,
    /// Code 49: stacked alphanumeric symbology.
    Code49,
    /// Codablock F: stacked Code 128.
    CodablockF,

    // ======== 1D: EAN/UPC family ========
    /// EAN-13.
    Ean13,
    /// EAN-8.
    Ean8,
    /// UPC-A.
    UpcA,
    /// UPC-E (zero-suppressed UPC).
    UpcE,
    /// EAN/UPC 2-digit add-on (UPC/EAN Extension 2).
    Ean2,
    /// EAN/UPC 5-digit add-on (UPC/EAN Extension 5).
    Ean5,

    // ======== 1D: Code 128 / GS1-128 ========
    /// Code 128 (ISO/IEC 15417).
    Code128,
    /// GS1-128 (Application Identifiers carried over Code 128).
    Gs1_128,

    // ======== 1D: Code 39 / 93 / 11 ========
    /// Code 39 (ISO/IEC 16388), incl. the full-ASCII extension.
    Code39,
    /// Code 93.
    Code93,
    /// Code 11 (USD-8), telecom labeling.
    Code11,

    // ======== 1D: 2-of-5 family ========
    /// Interleaved 2 of 5 (ITF, ISO/IEC 16390).
    Itf,
    /// Standard / industrial 2 of 5.
    Std2of5,
    /// IATA 2 of 5 (air cargo).
    Iata2of5,
    /// Matrix 2 of 5.
    Matrix2of5,

    // ======== 1D: other linear ========
    /// Codabar (a.k.a. "Coda", NW-7, USD-4).
    Codabar,
    /// MSI Plessey.
    MsiPlessey,
    /// Plessey (the original UK symbology).
    Plessey,
    /// Telepen (full ASCII).
    Telepen,
    /// Pharmacode (Laetus one-track pharmaceutical code).
    Pharmacode,
    /// Two-track Pharmacode.
    PharmacodeTwoTrack,
    /// DX film edge barcode (the code printed along 35mm film edges).
    DxFilmEdge,

    // ======== 1D: GS1 DataBar family (formerly "RSS") ========
    /// GS1 DataBar Omnidirectional (RSS-14). Also covers Truncated.
    DataBarOmni,
    /// GS1 DataBar Stacked (RSS-14 Stacked).
    DataBarStacked,
    /// GS1 DataBar Stacked Omnidirectional.
    DataBarStackedOmni,
    /// GS1 DataBar Limited (RSS Limited).
    DataBarLimited,
    /// GS1 DataBar Expanded (RSS Expanded).
    DataBarExpanded,
    /// GS1 DataBar Expanded Stacked.
    DataBarExpandedStacked,

    // ======== Postal (height-modulated) ========
    /// USPS POSTNET.
    Postnet,
    /// USPS PLANET.
    Planet,
    /// USPS Intelligent Mail Barcode (OneCode, 4-state).
    IntelligentMail,
    /// Royal Mail 4-State Customer Code (RM4SCC).
    RoyalMail,
    /// Royal Mail Mailmark 4-state.
    Mailmark,
    /// Australia Post 4-state.
    AustraliaPost,
    /// Japan Post 4-state.
    JapanPost,
    /// Dutch KIX (Royal TNT Post 4-state).
    KixCode,
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
            | HanXin | DotCode | GridMatrix => Dimension::Matrix,
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
        matches!(
            self,
            QrCode
                | DataMatrix
                | Pdf417
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
                | DataBarOmni
                | DataBarLimited
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
}
