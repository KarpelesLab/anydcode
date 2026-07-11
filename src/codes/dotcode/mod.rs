//! DotCode (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`DotCodeEncoder`, `DotCodeDecoder`, `DotCodeMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a DotCode symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DotCodeMeta;

/// DotCode encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DotCodeEncoder;

impl DotCodeEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for DotCodeEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "DotCode encoding",
        })
    }
}

/// DotCode decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DotCodeDecoder;

impl DotCodeDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for DotCodeDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "DotCode decoding",
        })
    }
}
