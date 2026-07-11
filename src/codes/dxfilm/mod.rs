//! DX Film Edge (stub).
//!
//! Encoding and decoding are not yet implemented; both entry points return
//! [`Error::Unsupported`]. The public surface (`DxFilmEncoder`, `DxFilmDecoder`, `DxFilmMeta`) is
//! pre-declared so the implementation can be filled in without touching shared
//! files. Follow the QR module ([`crate::codes::qr`]) as the reference pattern.

use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::symbol::Symbol;
use crate::traits::{Decode, Encode};

/// Parameters required to re-encode a DX Film Edge symbol identically (lossless
/// round-trip). Fields are defined by the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DxFilmMeta;

/// DX Film Edge encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DxFilmEncoder;

impl DxFilmEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl Encode for DxFilmEncoder {
    fn encode(&self, _symbol: &Symbol) -> Result<Encoding> {
        Err(Error::Unsupported {
            what: "DX Film Edge encoding",
        })
    }
}

/// DX Film Edge decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DxFilmDecoder;

impl DxFilmDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

impl Decode for DxFilmDecoder {
    fn decode(&self, _encoding: &Encoding) -> Result<Symbol> {
        Err(Error::Unsupported {
            what: "DX Film Edge decoding",
        })
    }
}
