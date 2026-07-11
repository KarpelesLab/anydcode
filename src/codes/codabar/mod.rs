//! Codabar (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`CodabarEncoder`, `CodabarDecoder`, `CodabarMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Codabar symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodabarMeta;

/// Codabar encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodabarEncoder;

impl CodabarEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for CodabarEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "Codabar encoding",
        })
    }
}

/// Codabar decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodabarDecoder;

impl CodabarDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for CodabarDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "Codabar decoding",
        })
    }
}
