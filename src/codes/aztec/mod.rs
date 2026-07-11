//! Aztec (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`AztecEncoder`, `AztecDecoder`, `AztecMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Aztec symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AztecMeta;

/// Aztec encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct AztecEncoder;

impl AztecEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for AztecEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "Aztec encoding",
        })
    }
}

/// Aztec decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct AztecDecoder;

impl AztecDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for AztecDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "Aztec decoding",
        })
    }
}
