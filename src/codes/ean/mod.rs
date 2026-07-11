//! EAN/UPC (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`EanEncoder`, `EanDecoder`, `EanMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a EAN/UPC symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EanMeta;

/// EAN/UPC encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct EanEncoder;

impl EanEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for EanEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "EAN/UPC encoding",
        })
    }
}

/// EAN/UPC decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct EanDecoder;

impl EanDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for EanDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "EAN/UPC decoding",
        })
    }
}
