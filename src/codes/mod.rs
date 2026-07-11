//! Per-symbology implementations. Each submodule provides encode/decode (and later
//! detection) for one symbology family. Only [`qr`] is implemented so far; the rest
//! of the [`crate::Symbology`] catalog is modeled but not yet built.

pub mod qr;
