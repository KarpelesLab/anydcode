//! Code 93 (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`Code93Encoder`, `Code93Decoder`, `Code93Meta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Code 93 symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Code93Meta;

/// Code 93 encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code93Encoder;

impl Code93Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for Code93Encoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "Code 93 encoding",
        })
    }
}

/// Code 93 decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code93Decoder;

impl Code93Decoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for Code93Decoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "Code 93 decoding",
        })
    }
}
