//! Han Xin Code (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`HanXinEncoder`, `HanXinDecoder`, `HanXinMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a Han Xin Code symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HanXinMeta;

/// Han Xin Code encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct HanXinEncoder;

impl HanXinEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for HanXinEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "Han Xin Code encoding",
        })
    }
}

/// Han Xin Code decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct HanXinDecoder;

impl HanXinDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for HanXinDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "Han Xin Code decoding",
        })
    }
}
