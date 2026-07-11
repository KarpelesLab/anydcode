//! MSI Plessey (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`MsiEncoder`, `MsiDecoder`, `MsiMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a MSI Plessey symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MsiMeta;

/// MSI Plessey encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MsiEncoder;

impl MsiEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for MsiEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "MSI Plessey encoding",
        })
    }
}

/// MSI Plessey decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct MsiDecoder;

impl MsiDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for MsiDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "MSI Plessey decoding",
        })
    }
}
