//! Per-symbology implementations. Each submodule provides encode/decode (and later
//! detection) for one symbology family. Only [`qr`] is implemented so far; the other
//! modules are pre-declared stubs whose encode/decode return
//! [`crate::Error::Unsupported`] until filled in. Query
//! [`crate::Symbology::is_implemented`] for current coverage.

pub mod qr;

// 1D linear families.
pub mod codabar;
pub mod code11;
pub mod code128;
pub mod code39;
pub mod code93;
pub mod dxfilm;
pub mod ean;
pub mod itf;
pub mod msi;
pub mod pharmacode;
pub mod telepen;
pub mod twoof5;

// GS1 DataBar (formerly RSS).
pub mod databar;

// 2D matrix families.
pub mod aztec;
pub mod datamatrix;
pub mod dotcode;
pub mod gridmatrix;
pub mod hanxin;
pub mod maxicode;
pub mod microqr;
pub mod rmqr;

// 2D stacked families.
pub mod codablockf;
pub mod code16k;
pub mod code49;
pub mod pdf417;

// Postal (height-modulated / 4-state).
pub mod postal;
