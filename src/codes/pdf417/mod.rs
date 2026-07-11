//! PDF417 (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`Pdf417Encoder`, `Pdf417Decoder`, `Pdf417Meta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a PDF417 symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pdf417Meta;

/// PDF417 encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Pdf417Encoder;

impl Pdf417Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for Pdf417Encoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "PDF417 encoding",
        })
    }
}

/// PDF417 decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Pdf417Decoder;

impl Pdf417Decoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for Pdf417Decoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "PDF417 decoding",
        })
    }
}
