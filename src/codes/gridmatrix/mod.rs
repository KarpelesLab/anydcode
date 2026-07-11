//! Grid Matrix (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`GridMatrixEncoder`, `GridMatrixDecoder`, `GridMatrixMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Grid Matrix symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GridMatrixMeta;

/// Grid Matrix encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct GridMatrixEncoder;

impl GridMatrixEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for GridMatrixEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "Grid Matrix encoding",
        })
    }
}

/// Grid Matrix decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct GridMatrixDecoder;

impl GridMatrixDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for GridMatrixDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "Grid Matrix decoding",
        })
    }
}
