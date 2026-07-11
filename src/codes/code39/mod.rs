//! Code 39 (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`Code39Encoder`, `Code39Decoder`, `Code39Meta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Code 39 symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Code39Meta;

/// Code 39 encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code39Encoder;

impl Code39Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for Code39Encoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "Code 39 encoding",
        })
    }
}

/// Code 39 decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code39Decoder;

impl Code39Decoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for Code39Decoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "Code 39 decoding",
        })
    }
}
