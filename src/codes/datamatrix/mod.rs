//! Data Matrix (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`DataMatrixEncoder`, `DataMatrixDecoder`, `DataMatrixMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Data Matrix symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DataMatrixMeta;

/// Data Matrix encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DataMatrixEncoder;

impl DataMatrixEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for DataMatrixEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "Data Matrix encoding",
        })
    }
}

/// Data Matrix decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DataMatrixDecoder;

impl DataMatrixDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for DataMatrixDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "Data Matrix decoding",
        })
    }
}
