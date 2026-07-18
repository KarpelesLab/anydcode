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

/// Decode **every** symbology this library can read from a full (already-binarizable)
/// luminance frame, returning one [`Symbol`] per distinct code found.
///
/// This is the single analysis entry point shared by every front-end — the `anyd` CLI,
/// the WebAssembly `decode` export, and callers embedding the library — so they can never
/// drift out of sync over which decoders run or in what order. It runs the 2D image
/// samplers (QR, Data Matrix, PDF417) and the 1D pipeline (the width-ratio EAN/UPC edge
/// reader that handles curved, blurred camera captures, plus the quantized `scan1d`
/// front-end feeding the remaining checksummed linear decoders), de-duplicated by
/// `(symbology, text)`. It never returns an error: a frame with no code yields an empty
/// vec. Intended for a located crop or a whole still image; on a live stream, gate it
/// behind [`crate::detect::locate`] and decode only the regions it returns.
pub fn scan_all(frame: &crate::image::GrayFrame<'_>) -> Vec<Symbol> {
    let mut found = scan_2d(frame);
    dedup_extend(&mut found, scan_1d(frame));
    found
}

/// Decode only the **2D / self-localizing** symbologies (QR, Data Matrix, Aztec,
/// Micro QR, PDF417, App Clip Code) from `frame`.
///
/// These samplers *self-localize* — they find their own finder / start patterns — so
/// they are meant to run on a whole frame, not a pre-cropped region, and they carry the
/// decoded [`Symbol::location`]. Splitting this out lets a live front-end decode 2D codes
/// directly off each frame (fast, and independent of any coarse locator) while reserving
/// the heavier, crop-dependent [`scan_1d`] pass for located regions.
pub fn scan_2d(frame: &crate::image::GrayFrame<'_>) -> Vec<Symbol> {
    let mut found: Vec<Symbol> = Vec::new();
    if let Ok(s) = crate::codes::qr::scan(frame) {
        found.push(s);
    }
    if let Ok(s) = crate::codes::datamatrix::scan(frame) {
        dedup_push(&mut found, s);
    }
    if let Ok(s) = crate::codes::aztec::scan(frame) {
        dedup_push(&mut found, s);
    }
    if let Ok(s) = crate::codes::microqr::scan(frame) {
        dedup_push(&mut found, s);
    }
    if let Some(s) = crate::codes::pdf417::scan(frame) {
        dedup_push(&mut found, s);
    }
    #[cfg(feature = "appclip")]
    if let Ok(s) = crate::codes::appclip::scan(frame) {
        dedup_push(&mut found, s);
    }
    found
}

/// Decode the **1D / linear** symbologies from `frame`.
///
/// Unlike the 2D samplers, a linear scan reads along image rows, so it wants a region
/// that contains (mostly) just the barcode — feed it a located crop, not a whole cluttered
/// frame. The EAN/UPC edge reader (width ratios, voted across scanlines) reads curved and
/// blurred captures the quantized grid cannot; the quantized `scan1d` front-end then feeds
/// the remaining checksummed linear decoders.
///
/// The scanline sweep itself only covers a few degrees around horizontal, but the code's
/// orientation in the crop is arbitrary: each candidate pattern is also tried mirrored
/// (an upside-down code scans in reverse), and when the sweep finds nothing the frame's
/// dominant texture orientation is measured ([`crate::imgproc::orient`]) and the whole
/// crop is derotated and rescanned — so a 1D code is read at any rotation, not just the
/// near-horizontal band.
pub fn scan_1d(frame: &crate::image::GrayFrame<'_>) -> Vec<Symbol> {
    let scan_opts = crate::scan1d::ScanOptions::default();
    let found = scan_1d_sweep(frame, &scan_opts);
    if !found.is_empty() {
        return found;
    }

    // Nothing near horizontal. If the crop's texture has one dominant orientation —
    // the signature of a rotated barcode — derotate the whole crop and rescan. The
    // sweep already covers ±6°, so only meaningfully rotated codes are worth the
    // resample.
    let Some(angle) = crate::imgproc::orient::dominant_gradient_angle(frame) else {
        return found;
    };
    if angle.abs() < MIN_DEROTATE_ANGLE {
        return found;
    }
    let image = to_image(frame);
    let rotated = crate::transform::rotate(&image, -angle);
    let mut found = scan_1d_sweep(&rotated.as_frame(), &scan_opts);

    // Map each symbol's outline from derotated coordinates back into the frame.
    let (s, c) = (-angle).sin_cos();
    let (cx, cy) = (frame.width() as f32 / 2.0, frame.height() as f32 / 2.0);
    let (ncx, ncy) = (rotated.width() as f32 / 2.0, rotated.height() as f32 / 2.0);
    for sym in &mut found {
        if let Some(loc) = &mut sym.location {
            for p in &mut loc.outline.corners {
                let (dx, dy) = (p.x - ncx, p.y - ncy);
                p.x = c * dx + s * dy + cx;
                p.y = -s * dx + c * dy + cy;
            }
            loc.rotation = Some(loc.rotation.unwrap_or(0.0) + angle);
        }
    }
    found
}

/// Smallest texture rotation (radians) worth a derotate-and-rescan; the plain sweep
/// already covers a ±6° band around horizontal.
const MIN_DEROTATE_ANGLE: f32 = 8.0 * core::f32::consts::PI / 180.0;

/// One near-horizontal 1D pass: the EAN/UPC edge reader plus the quantized `scan1d`
/// front-end feeding every checksummed linear decoder, each candidate tried in both
/// reading directions.
fn scan_1d_sweep(
    frame: &crate::image::GrayFrame<'_>,
    scan_opts: &crate::scan1d::ScanOptions,
) -> Vec<Symbol> {
    use crate::traits::Decode;

    let mut found: Vec<Symbol> = Vec::new();
    if let Some(s) = crate::codes::ean::scan(frame, scan_opts) {
        found.push(s);
    }
    let candidates = crate::scan1d::scan_lines(frame, scan_opts);
    let linear: [Box<dyn Decode>; 6] = [
        Box::new(crate::codes::code128::Code128Decoder::new()),
        Box::new(crate::codes::ean::EanDecoder::new()),
        Box::new(crate::codes::code93::Code93Decoder::new()),
        Box::new(crate::codes::code39::Code39Decoder::new()),
        Box::new(crate::codes::itf::ItfDecoder::new()),
        Box::new(crate::codes::codabar::CodabarDecoder::new()),
    ];
    for cand in &candidates {
        // A scanline as easily runs against the code's reading direction as with it
        // (and a 180°-rotated code always does): try the mirrored pattern too. The
        // decoders are all checksummed, so a mirrored misread cannot false-accept.
        let mut mirrored = cand.clone();
        mirrored.pattern.modules.reverse();
        mirrored.edges.clear();
        for dec in &linear {
            if let Some(s) = crate::scan1d::try_decode(cand, dec.as_ref()) {
                dedup_push(&mut found, s);
            }
            if let Some(s) = crate::scan1d::try_decode(&mirrored, dec.as_ref()) {
                dedup_push(&mut found, s);
            }
        }
    }
    found
}

/// Copy a borrowed frame into an owned [`crate::image::GrayImage`] (for resampling).
fn to_image(frame: &crate::image::GrayFrame<'_>) -> crate::image::GrayImage {
    let (w, h) = (frame.width(), frame.height());
    let mut data = Vec::with_capacity(w * h);
    for y in 0..h {
        data.extend_from_slice(frame.row(y).expect("row in range"));
    }
    crate::image::GrayImage::from_raw(w, h, data)
}

/// Push `sym` unless an equal `(symbology, text)` is already present.
fn dedup_push(found: &mut Vec<Symbol>, sym: Symbol) {
    let key = (sym.symbology, sym.text().unwrap_or_default());
    if !found
        .iter()
        .any(|s| (s.symbology, s.text().unwrap_or_default()) == key)
    {
        found.push(sym);
    }
}

/// De-duplicating [`Vec::extend`] by `(symbology, text)`.
fn dedup_extend(found: &mut Vec<Symbol>, more: Vec<Symbol>) {
    for sym in more {
        dedup_push(found, sym);
    }
}
