//! The catalog of barcode symbologies AnyDCode intends to support.
//!
//! Every variant is modeled here even when no encoder/decoder exists yet, so the
//! type system and the public API are stable as implementations land. Use
//! [`Symbology::is_implemented`] to query current coverage at runtime.

use core::fmt;

/// Broad family a symbology belongs to. Useful for routing in detectors, since
/// 1D (linear) and 2D (matrix/stacked) codes are located with different methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dimension {
    /// Linear (1D) codes: a single row of bars.
    Linear,
    /// Matrix or stacked (2D) codes.
    Matrix,
}

/// A barcode symbology. This enum is intentionally exhaustive of AnyDCode's roadmap;
/// unimplemented variants return [`crate::Error::Unsupported`] from encode/decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Symbology {
    // ---- 2D: matrix ----
    /// QR Code (ISO/IEC 18004), versions 1–40.
    QrCode,
    /// Micro QR Code, versions M1–M4.
    MicroQrCode,
    /// Rectangular Micro QR Code (rMQR).
    RectMicroQrCode,
    /// Aztec Code (ISO/IEC 24778).
    Aztec,
    /// Data Matrix (ISO/IEC 16022), ECC 200.
    DataMatrix,
    /// MaxiCode.
    MaxiCode,
    /// Han Xin Code.
    HanXin,
    /// DotCode.
    DotCode,

    // ---- 2D: stacked ----
    /// PDF417 (ISO/IEC 15438).
    Pdf417,
    /// MicroPDF417.
    MicroPdf417,

    // ---- 1D: EAN/UPC family ----
    /// EAN-13.
    Ean13,
    /// EAN-8.
    Ean8,
    /// UPC-A.
    UpcA,
    /// UPC-E.
    UpcE,
    /// EAN/UPC 2-digit add-on.
    Ean2,
    /// EAN/UPC 5-digit add-on.
    Ean5,

    // ---- 1D: Code 128 / GS1-128 ----
    /// Code 128.
    Code128,
    /// GS1-128 (application identifiers over Code 128).
    Gs1_128,

    // ---- 1D: Code 39 / 93 ----
    /// Code 39.
    Code39,
    /// Code 93.
    Code93,

    // ---- 1D: 2-of-5 family ----
    /// Interleaved 2 of 5.
    Itf,
    /// Standard (industrial) 2 of 5.
    Std2of5,

    // ---- 1D: other ----
    /// Codabar.
    Codabar,
    /// MSI Plessey.
    MsiPlessey,

    // ---- 1D: GS1 DataBar family ----
    /// GS1 DataBar Omnidirectional.
    DataBarOmni,
    /// GS1 DataBar Expanded.
    DataBarExpanded,
    /// GS1 DataBar Limited.
    DataBarLimited,
}

impl Symbology {
    /// The dimensional family this symbology belongs to.
    pub fn dimension(self) -> Dimension {
        use Symbology::*;
        match self {
            QrCode | MicroQrCode | RectMicroQrCode | Aztec | DataMatrix | MaxiCode | HanXin
            | DotCode | Pdf417 | MicroPdf417 => Dimension::Matrix,
            _ => Dimension::Linear,
        }
    }

    /// Whether an encoder/decoder is currently implemented for this symbology.
    pub fn is_implemented(self) -> bool {
        matches!(self, Symbology::QrCode)
    }

    /// A short, stable, human-readable name.
    pub fn name(self) -> &'static str {
        use Symbology::*;
        match self {
            QrCode => "QR Code",
            MicroQrCode => "Micro QR Code",
            RectMicroQrCode => "Rectangular Micro QR Code",
            Aztec => "Aztec",
            DataMatrix => "Data Matrix",
            MaxiCode => "MaxiCode",
            HanXin => "Han Xin",
            DotCode => "DotCode",
            Pdf417 => "PDF417",
            MicroPdf417 => "MicroPDF417",
            Ean13 => "EAN-13",
            Ean8 => "EAN-8",
            UpcA => "UPC-A",
            UpcE => "UPC-E",
            Ean2 => "EAN-2",
            Ean5 => "EAN-5",
            Code128 => "Code 128",
            Gs1_128 => "GS1-128",
            Code39 => "Code 39",
            Code93 => "Code 93",
            Itf => "Interleaved 2 of 5",
            Std2of5 => "Standard 2 of 5",
            Codabar => "Codabar",
            MsiPlessey => "MSI Plessey",
            DataBarOmni => "GS1 DataBar Omnidirectional",
            DataBarExpanded => "GS1 DataBar Expanded",
            DataBarLimited => "GS1 DataBar Limited",
        }
    }
}

impl fmt::Display for Symbology {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
