//! Postal 4-state / height-modulated (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`PostalEncoder`, `PostalDecoder`, `PostalMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Postal 4-state / height-modulated symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PostalMeta;

/// Postal 4-state / height-modulated encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct PostalEncoder;

impl PostalEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for PostalEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported { what: "Postal 4-state / height-modulated encoding" })
    }
}

/// Postal 4-state / height-modulated decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct PostalDecoder;

impl PostalDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for PostalDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported { what: "Postal 4-state / height-modulated decoding" })
    }
}
