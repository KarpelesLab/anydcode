//! The core traits every symbology plugs into: [`Detect`], [`Analyze`], [`Decode`]
//! and [`Encode`]. They mirror the two-stage live pipeline (locate cheaply, then
//! decode) while also supporting one-shot still-image decoding.
//!
//! [`Encode`] needs the `encode` and `alloc` features, [`Decode`] needs `decode`,
//! and [`Detect`] / [`Analyze`] need `scan`.

#[cfg(feature = "scan")]
use crate::image::GrayFrame;
#[cfg(feature = "scan")]
use crate::pipeline::{Candidate, Hints};
#[cfg(feature = "scan")]
use alloc::vec::Vec;

/// Fast per-frame detection: locate and roughly classify candidate codes without
/// fully decoding them. Implementations should be cheap enough to run on every
/// video frame and should consult `hints` to reuse prior-frame results.
#[cfg(feature = "scan")]
pub trait Detect {
    /// Find candidate codes in `frame`, using `hints` from previous frames.
    fn detect(&self, frame: &GrayFrame<'_>, hints: &Hints) -> Vec<Candidate>;
}

/// Heavier analysis: fully decode a located [`Candidate`] into a [`Symbol`](crate::Symbol).
#[cfg(feature = "scan")]
pub trait Analyze {
    /// Decode `candidate` within `frame`. Implementations may return the candidate's
    /// cached `known` symbol immediately when present.
    fn analyze(&self, frame: &GrayFrame<'_>, candidate: &Candidate)
    -> crate::Result<crate::Symbol>;
}

/// Decode an already-sampled module grid or linear pattern into a [`Symbol`](crate::Symbol).
///
/// This is the structural half of decoding — it operates on clean module data
/// (`Encoding`) rather than pixels, and is what round-trip tests exercise against
/// the encoder. Image sampling lives in [`Analyze`].
#[cfg(feature = "decode")]
pub trait Decode {
    /// Decode module data into a symbol.
    fn decode(&self, encoding: &crate::output::Encoding) -> crate::Result<crate::Symbol>;
}

/// Encode a [`Symbol`](crate::Symbol) into abstract module geometry.
#[cfg(all(feature = "alloc", feature = "encode"))]
pub trait Encode {
    /// Produce the module geometry for `symbol`.
    fn encode(&self, symbol: &crate::Symbol) -> crate::Result<crate::output::Encoding>;
}
