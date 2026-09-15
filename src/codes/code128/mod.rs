//! Code 128 (ISO/IEC 15417) and GS1-128 encoder and structural decoder.
//!
//! Layout:
//! - `tables`  — the 107 symbol patterns, the Stop pattern and code-set constants.
//! - `encode`  — [`Symbol`] → [`LinearPattern`], plus the fresh-input `build` path.
//! - `decode`  — [`LinearPattern`] → [`Symbol`].
//!
//! ## Losslessness
//!
//! The same text can be encoded through many different code-set sequences (Start A
//! vs B, when to latch to Code C, Shift vs latch, ...). To reproduce a scanned symbol
//! byte-for-byte, [`Code128Meta`] stores the **exact symbol-value sequence** — the
//! Start character followed by every data symbol value, excluding the derived
//! modulo-103 check character and the Stop. Re-encoding renders straight from that
//! sequence, so `encode(decode(x)) == x` regardless of how the data could have been
//! encoded. The payload [`Segment`]s carried alongside are the decoded human-readable
//! bytes and are not consulted when re-encoding from meta.
//!
//! [`Symbol`]: crate::Symbol
//! [`Segment`]: crate::Segment
//! [`LinearPattern`]: crate::output::LinearPattern

// With neither `encode` nor `decode` only the metadata types remain; their shared
// helpers are then unused.
#![cfg_attr(
    not(any(feature = "encode", feature = "decode")),
    allow(dead_code, unused_imports)
)]

use alloc::vec::Vec;

#[cfg(feature = "decode")]
mod decode;
#[cfg(all(feature = "alloc", feature = "encode"))]
mod encode;
#[cfg_attr(not(all(feature = "encode", feature = "decode")), allow(dead_code))]
mod tables;

#[cfg(feature = "decode")]
pub use decode::Code128Decoder;
#[cfg(all(feature = "alloc", feature = "encode"))]
pub use encode::{Code128Encoder, Code128Input};
pub use tables::CodeSet;

/// Parameters required to re-encode a Code 128 / GS1-128 symbol identically.
///
/// [`Code128Meta::symbols`] is the exact symbol-value sequence from the Start
/// character (index 0, one of `103`/`104`/`105`) through the last data value. The
/// modulo-103 check character and the Stop pattern are derived on encode and are
/// **not** stored here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Code128Meta {
    /// Whether this is a GS1-128 symbol (FNC1 in the first data position).
    pub gs1: bool,
    /// The Start symbol value followed by every data symbol value (no check, no Stop).
    pub symbols: Vec<u8>,
}
