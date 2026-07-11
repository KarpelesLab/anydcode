//! Interleaved 2 of 5 (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`ItfEncoder`, `ItfDecoder`, `ItfMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Interleaved 2 of 5 symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItfMeta;

/// Interleaved 2 of 5 encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct ItfEncoder;

impl ItfEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for ItfEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "Interleaved 2 of 5 encoding",
        })
    }
}

/// Interleaved 2 of 5 decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct ItfDecoder;

impl ItfDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for ItfDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "Interleaved 2 of 5 decoding",
        })
    }
}
