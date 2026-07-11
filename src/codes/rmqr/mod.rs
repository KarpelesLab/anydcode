//! Rectangular Micro QR (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`RmqrEncoder`, `RmqrDecoder`, `RmqrMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Rectangular Micro QR symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RmqrMeta;

/// Rectangular Micro QR encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct RmqrEncoder;

impl RmqrEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for RmqrEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported { what: "Rectangular Micro QR encoding" })
    }
}

/// Rectangular Micro QR decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct RmqrDecoder;

impl RmqrDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for RmqrDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported { what: "Rectangular Micro QR decoding" })
    }
}
