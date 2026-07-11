//! The live-video decoding pipeline: a fast per-frame *detection* pass that only
//! locates and classifies candidate codes, separated from a heavier *analysis* pass
//! that fully decodes them.
//!
//! The split exists so that on a video stream we can run detection on every frame
//! but skip re-decoding a code we already analyzed. [`Hints`] carries forward the
//! previous frame's results; a detector can match a new [`Candidate`] against a
//! prior one by position and [`Fingerprint`] and mark it as already known.

use crate::geometry::Location;
use crate::symbol::Symbol;
use crate::symbology::Symbology;

/// A cheap, position-independent signature of a candidate region, used to recognize
/// the same physical code across consecutive frames without decoding it.
///
/// The concrete hashing scheme is detector-defined; equality is the only contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub u64);

/// A located, classified — but not necessarily decoded — code within a frame.
///
/// Produced by the detection pass. If `known` refers into the [`Hints`] from the
/// previous frame, analysis can be skipped and the prior [`Symbol`] reused.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Where the candidate is in the current frame.
    pub location: Location,
    /// Best guess at the symbology, if the detector can tell cheaply.
    pub symbology: Option<Symbology>,
    /// A frame-to-frame matching signature, if the detector computes one.
    pub fingerprint: Option<Fingerprint>,
    /// If this candidate matches a previously analyzed symbol, its cached result.
    pub known: Option<Symbol>,
}

impl Candidate {
    /// A bare candidate with only a location.
    pub fn at(location: Location) -> Self {
        Candidate {
            location,
            symbology: None,
            fingerprint: None,
            known: None,
        }
    }
}

/// Carried-forward state from previously processed frames, used to avoid redundant
/// analysis on a live stream.
#[derive(Debug, Clone, Default)]
pub struct Hints {
    /// Symbols decoded in earlier frames, with their last known locations and
    /// fingerprints (via each symbol's own `location`). Detectors consult these to
    /// short-circuit re-analysis.
    pub previous: Vec<KnownSymbol>,
}

/// A symbol carried forward from a prior frame together with its matching signature.
#[derive(Debug, Clone)]
pub struct KnownSymbol {
    /// The decoded symbol from a previous frame.
    pub symbol: Symbol,
    /// Its fingerprint for cheap re-matching, if one was computed.
    pub fingerprint: Option<Fingerprint>,
}

impl Hints {
    /// Empty hints (first frame).
    pub fn new() -> Self {
        Hints::default()
    }

    /// Look up a previously decoded symbol by fingerprint.
    pub fn find(&self, fp: Fingerprint) -> Option<&KnownSymbol> {
        self.previous.iter().find(|k| k.fingerprint == Some(fp))
    }
}
