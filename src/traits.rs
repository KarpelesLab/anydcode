//! The core traits every symbology plugs into: [`Detect`], [`Analyze`], [`Decode`]
//! and [`Encode`]. They mirror the two-stage live pipeline (locate cheaply, then
//! decode) while also supporting one-shot still-image decoding.

use crate::error::Result;
use crate::image::GrayFrame;
use crate::output::Encoding;
use crate::pipeline::{Candidate, Hints};
use crate::symbol::Symbol;

/// Fast per-frame detection: locate and roughly classify candidate codes without
/// fully decoding them. Implementations should be cheap enough to run on every
/// video frame and should consult `hints` to reuse prior-frame results.
pub trait Detect {
    /// Find candidate codes in `frame`, using `hints` from previous frames.
    fn detect(&self, frame: &GrayFrame<'_>, hints: &Hints) -> Vec<Candidate>;
}

/// Heavier analysis: fully decode a located [`Candidate`] into a [`Symbol`].
pub trait Analyze {
    /// Decode `candidate` within `frame`. Implementations may return the candidate's
    /// cached `known` symbol immediately when present.
    fn analyze(&self, frame: &GrayFrame<'_>, candidate: &Candidate) -> Result<Symbol>;
}

/// Decode an already-sampled module grid or linear pattern into a [`Symbol`].
///
/// This is the structural half of decoding — it operates on clean module data
/// (`Encoding`) rather than pixels, and is what round-trip tests exercise against
/// the encoder. Image sampling lives in [`Analyze`].
pub trait Decode {
    /// Decode module data into a symbol.
    fn decode(&self, encoding: &Encoding) -> Result<Symbol>;
}

/// Encode a [`Symbol`] into abstract module geometry.
pub trait Encode {
    /// Produce the module geometry for `symbol`.
    fn encode(&self, symbol: &Symbol) -> Result<Encoding>;
}
