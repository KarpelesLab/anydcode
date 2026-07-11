//! Micro QR Code (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`MicroQrEncoder`, `MicroQrDecoder`, `MicroQrMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Micro QR Code symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MicroQrMeta;

/// Micro QR Code encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MicroQrEncoder;

impl MicroQrEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for MicroQrEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported { what: "Micro QR Code encoding" })
    }
}

/// Micro QR Code decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MicroQrDecoder;

impl MicroQrDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for MicroQrDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported { what: "Micro QR Code decoding" })
    }
}
