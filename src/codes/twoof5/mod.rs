//! 2 of 5 (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`TwoOf5Encoder`, `TwoOf5Decoder`, `TwoOf5Meta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a 2 of 5 symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TwoOf5Meta;

/// 2 of 5 encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct TwoOf5Encoder;

impl TwoOf5Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for TwoOf5Encoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "2 of 5 encoding",
        })
    }
}

/// 2 of 5 decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct TwoOf5Decoder;

impl TwoOf5Decoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for TwoOf5Decoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "2 of 5 decoding",
        })
    }
}
