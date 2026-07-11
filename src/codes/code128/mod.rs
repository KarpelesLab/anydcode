//! Code 128 (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`Code128Encoder`, `Code128Decoder`, `Code128Meta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Code 128 symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Code128Meta;

/// Code 128 encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code128Encoder;

impl Code128Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for Code128Encoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "Code 128 encoding",
        })
    }
}

/// Code 128 decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct Code128Decoder;

impl Code128Decoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for Code128Decoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "Code 128 decoding",
        })
    }
}
