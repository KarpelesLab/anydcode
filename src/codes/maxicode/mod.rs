//! MaxiCode (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`MaxiCodeEncoder`, `MaxiCodeDecoder`, `MaxiCodeMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a MaxiCode symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MaxiCodeMeta;

/// MaxiCode encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MaxiCodeEncoder;

impl MaxiCodeEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for MaxiCodeEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported { what: "MaxiCode encoding" })
    }
}

/// MaxiCode decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MaxiCodeDecoder;

impl MaxiCodeDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for MaxiCodeDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported { what: "MaxiCode decoding" })
    }
}
