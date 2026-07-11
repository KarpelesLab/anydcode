//! Code 16K (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`Code16kEncoder`, `Code16kDecoder`, `Code16kMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Code 16K symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Code16kMeta;

/// Code 16K encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code16kEncoder;

impl Code16kEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for Code16kEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "Code 16K encoding",
        })
    }
}

/// Code 16K decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code16kDecoder;

impl Code16kDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for Code16kDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "Code 16K decoding",
        })
    }
}
