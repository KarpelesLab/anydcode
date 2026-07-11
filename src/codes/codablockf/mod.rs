//! Codablock F (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`CodablockFEncoder`, `CodablockFDecoder`, `CodablockFMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Codablock F symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodablockFMeta;

/// Codablock F encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodablockFEncoder;

impl CodablockFEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for CodablockFEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "Codablock F encoding",
        })
    }
}

/// Codablock F decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodablockFDecoder;

impl CodablockFDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for CodablockFDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "Codablock F decoding",
        })
    }
}
