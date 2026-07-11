//! Code 11 (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`Code11Encoder`, `Code11Decoder`, `Code11Meta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Code 11 symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Code11Meta;

/// Code 11 encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code11Encoder;

impl Code11Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for Code11Encoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "Code 11 encoding",
        })
    }
}

/// Code 11 decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code11Decoder;

impl Code11Decoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for Code11Decoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "Code 11 decoding",
        })
    }
}
