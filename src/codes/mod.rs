//! Per-symbology implementations. Each submodule provides encode/decode (and, with
//! the `scan` feature, image detection) for one symbology family, and is compiled
//! only when its cargo feature is enabled (`qr`, `ean`, `pdf417`, ... — or the
//! `matrix` / `stacked` / `linear` / `postal` / `all-codes` groups). Query
//! [`crate::Symbology::is_implemented`] for the coverage of the current build.
//!
//! Inside each module the encoder is gated on the `encode` feature, the structural
//! decoder on `decode`, and pixel samplers on `scan`.

// 2D matrix families.
#[cfg(all(feature = "alloc", feature = "appclip"))]
pub mod appclip;
#[cfg(all(feature = "alloc", feature = "aztec"))]
pub mod aztec;
#[cfg(all(feature = "alloc", feature = "datamatrix"))]
pub mod datamatrix;
#[cfg(all(feature = "alloc", feature = "dotcode"))]
pub mod dotcode;
#[cfg(all(feature = "alloc", feature = "gridmatrix"))]
pub mod gridmatrix;
#[cfg(all(feature = "alloc", feature = "hanxin"))]
pub mod hanxin;
#[cfg(all(feature = "alloc", feature = "maxicode"))]
pub mod maxicode;
#[cfg(all(feature = "alloc", feature = "microqr"))]
pub mod microqr;
#[cfg(feature = "qr")]
pub mod qr;
/// QR's GF(256) Reed–Solomon alone, for Micro QR / rMQR builds without QR itself.
#[cfg(all(
    feature = "alloc",
    not(feature = "qr"),
    any(feature = "microqr", feature = "rmqr")
))]
pub(crate) mod qr {
    #[allow(dead_code)]
    pub mod gf;
}
#[cfg(all(feature = "alloc", feature = "rmqr"))]
pub mod rmqr;

// 2D stacked families.
#[cfg(all(feature = "alloc", feature = "codablockf"))]
pub mod codablockf;
#[cfg(all(feature = "alloc", feature = "code16k"))]
pub mod code16k;
#[cfg(all(feature = "alloc", feature = "code49"))]
pub mod code49;
#[cfg(all(feature = "alloc", feature = "pdf417"))]
pub mod pdf417;

// 1D linear families.
#[cfg(feature = "codabar")]
pub mod codabar;
#[cfg(feature = "code11")]
pub mod code11;
#[cfg(feature = "code128")]
pub mod code128;
#[cfg(feature = "code39")]
pub mod code39;
#[cfg(feature = "code93")]
pub mod code93;
#[cfg(all(feature = "alloc", feature = "dxfilm"))]
pub mod dxfilm;
#[cfg(feature = "ean")]
pub mod ean;
#[cfg(feature = "itf")]
pub mod itf;
#[cfg(feature = "msi")]
pub mod msi;
#[cfg(all(feature = "alloc", feature = "pharmacode"))]
pub mod pharmacode;
#[cfg(all(feature = "alloc", feature = "telepen"))]
pub mod telepen;
#[cfg(feature = "twoof5")]
pub mod twoof5;

// GS1 DataBar (formerly RSS).
#[cfg(all(feature = "alloc", feature = "databar"))]
pub mod databar;

// Postal (height-modulated / 4-state).
#[cfg(all(feature = "alloc", feature = "postal"))]
pub mod postal;
