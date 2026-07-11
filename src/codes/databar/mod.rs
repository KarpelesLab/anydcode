//! GS1 DataBar (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`DataBarEncoder`, `DataBarDecoder`, `DataBarMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a GS1 DataBar symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DataBarMeta;

/// GS1 DataBar encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DataBarEncoder;

impl DataBarEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for DataBarEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "GS1 DataBar encoding",
        })
    }
}

/// GS1 DataBar decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DataBarDecoder;

impl DataBarDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for DataBarDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "GS1 DataBar decoding",
        })
    }
}
