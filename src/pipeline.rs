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
use alloc::{boxed::Box, vec::Vec};

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
/// Micro QR, rMQR, PDF417, MicroPDF417, App Clip Code) from `frame`.
///
/// These samplers *self-localize* — they find their own finder / start patterns — so
/// they are meant to run on a whole frame, not a pre-cropped region, and they carry the
/// decoded [`Symbol::location`]. Splitting this out lets a live front-end decode 2D codes
/// directly off each frame (fast, and independent of any coarse locator) while reserving
/// the heavier, crop-dependent [`scan_1d`] pass for located regions.
pub fn scan_2d(frame: &crate::image::GrayFrame<'_>) -> Vec<Symbol> {
    #[allow(unused_mut)]
    let mut found: Vec<Symbol> = Vec::new();
    let _ = frame; // unused when no 2D symbology is enabled
    #[cfg(feature = "qr")]
    if let Ok(s) = crate::codes::qr::scan(frame) {
        dedup_push(&mut found, s);
    }
    #[cfg(feature = "datamatrix")]
    if let Ok(s) = crate::codes::datamatrix::scan(frame) {
        dedup_push(&mut found, s);
    }
    #[cfg(feature = "aztec")]
    if let Ok(s) = crate::codes::aztec::scan(frame) {
        dedup_push(&mut found, s);
    }
    #[cfg(feature = "microqr")]
    if let Ok(s) = crate::codes::microqr::scan(frame) {
        dedup_push(&mut found, s);
    }
    #[cfg(feature = "rmqr")]
    if let Ok(s) = crate::codes::rmqr::scan(frame) {
        dedup_push(&mut found, s);
    }
    #[cfg(feature = "pdf417")]
    if let Some(s) = crate::codes::pdf417::scan(frame) {
        dedup_push(&mut found, s);
    }
    #[cfg(feature = "pdf417")]
    if let Ok(s) = crate::codes::pdf417::scan_micro(frame) {
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
/// the remaining linear decoders, whose readings are accepted by cross-scanline consensus.
///
/// The code's orientation in the crop is arbitrary. Each scan sweep only covers a few
/// degrees around its base axis, so: the horizontal sweep runs first (the common case);
/// if it reads nothing, the crop's edge-orientation peaks
/// ([`crate::imgproc::orient::gradient_angle_peaks`]) propose further axes — a barcode
/// is the most orientation-coherent texture there is — and finally the vertical axis is
/// tried regardless. Every candidate pattern is also tried mirrored (an upside-down
/// code scans in reverse). Scan lines are sampled directly along each axis; nothing is
/// resampled. A caller that already knows the reading axis (the locator reports it in
/// [`Location::rotation`]) should use [`scan_1d_at`] instead.
pub fn scan_1d(frame: &crate::image::GrayFrame<'_>) -> Vec<Symbol> {
    let found = scan_1d_sweep(frame, &crate::scan1d::ScanOptions::default());
    if !found.is_empty() {
        return found;
    }
    let mut tried: Vec<f32> = alloc::vec![0.0];
    let proposals = crate::imgproc::orient::gradient_angle_peaks(frame, 2)
        .into_iter()
        .map(f32::to_degrees)
        .chain([90.0]);
    for deg in proposals {
        // Skip an axis a previous sweep (±6° around its base) already covered.
        if tried
            .iter()
            .any(|&t| axis_distance_deg(t, deg) < SWEEP_COVER_DEG)
        {
            continue;
        }
        tried.push(deg);
        let deg = crate::scan1d::refine_axis(frame, deg);
        let found = scan_1d_sweep(frame, &crate::scan1d::ScanOptions::around(deg));
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

/// [`scan_1d`] for a code whose reading axis is already known: `angle` in radians,
/// clockwise from the +x axis (the convention of [`Location::rotation`]). Falls back to
/// the full [`scan_1d`] search if nothing reads along that axis.
pub fn scan_1d_at(frame: &crate::image::GrayFrame<'_>, angle: f32) -> Vec<Symbol> {
    if angle.is_finite() {
        let deg = crate::scan1d::refine_axis(frame, angle.to_degrees());
        let opts = crate::scan1d::ScanOptions::around(deg);
        let found = scan_1d_sweep(frame, &opts);
        if !found.is_empty() {
            return found;
        }
    }
    scan_1d(frame)
}

/// Decode a region the locator classed as **linear** — a 1D code *or* a stacked one —
/// whose reading axis is `angle` (radians, clockwise from +x, as in
/// [`Location::rotation`]): [`scan_1d_at`] first, then [`scan_stacked_at`].
///
/// A stacked symbol (PDF417, MicroPDF417) is rows of bars, so to a texture-based
/// locator it *is* a linear region; only the decoders can tell the two apart.
pub fn scan_linear_at(frame: &crate::image::GrayFrame<'_>, angle: f32) -> Vec<Symbol> {
    let found = scan_1d_at(frame, angle);
    if !found.is_empty() {
        return found;
    }
    scan_stacked_at(frame, angle)
}

/// Decode the stacked symbologies (PDF417, MicroPDF417) from a crop whose rows run
/// along `angle` (radians, clockwise from +x).
///
/// Their samplers trace the start/stop guard columns down the image rows, which only
/// works with the symbol within about ±12° of upright — so beyond a few degrees the crop
/// is derotated by the known axis first, and the decoded outline mapped back.
pub fn scan_stacked_at(frame: &crate::image::GrayFrame<'_>, angle: f32) -> Vec<Symbol> {
    #[allow(unused_mut)]
    let mut found: Vec<Symbol> = Vec::new();
    #[cfg(feature = "pdf417")]
    {
        let stacked = |f: &crate::image::GrayFrame<'_>| {
            let mut out: Vec<Symbol> = Vec::new();
            out.extend(crate::codes::pdf417::scan(f));
            out.extend(crate::codes::pdf417::scan_micro(f).ok());
            out
        };
        if !angle.is_finite() || angle.abs() < MIN_DEROTATE_ANGLE {
            found = stacked(frame);
        }
        if found.is_empty() && angle.is_finite() && angle.abs() >= MIN_DEROTATE_ANGLE {
            let (w, h) = (frame.width(), frame.height());
            let mut data = Vec::with_capacity(w * h);
            for y in 0..h {
                data.extend_from_slice(frame.row(y).expect("row in range"));
            }
            let image = crate::image::GrayImage::from_raw(w, h, data);
            // An axis is mod 180°: the derotated symbol may be upside down, which the
            // samplers cannot read, so try both.
            for turn in [0.0, core::f32::consts::PI] {
                let by = -angle + turn;
                let rotated = crate::transform::rotate(&image, by);
                found = stacked(&rotated.as_frame());
                if found.is_empty() {
                    continue;
                }
                // Map outlines from the derotated image back into the crop.
                let (s, c) = by.sin_cos();
                let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
                let (ncx, ncy) = (rotated.width() as f32 / 2.0, rotated.height() as f32 / 2.0);
                for sym in &mut found {
                    if let Some(loc) = &mut sym.location {
                        for p in &mut loc.outline.corners {
                            let (dx, dy) = (p.x - ncx, p.y - ncy);
                            p.x = c * dx + s * dy + cx;
                            p.y = -s * dx + c * dy + cy;
                        }
                        loc.rotation = Some(loc.rotation.unwrap_or(0.0) - by);
                    }
                }
                break;
            }
        }
    }
    let _ = (frame, angle); // unused when no stacked symbology is enabled
    found
}

/// Smallest rotation (radians) worth derotating a stacked-code crop for; their samplers
/// tolerate about ±12° on their own.
#[cfg(feature = "pdf417")]
const MIN_DEROTATE_ANGLE: f32 = 8.0 * core::f32::consts::PI / 180.0;

/// Half-width (degrees) of the band of reading axes one sweep covers: its outermost
/// scan angle plus the slant a full-height scan line tolerates.
const SWEEP_COVER_DEG: f32 = 8.0;

/// Distance in degrees between two axes (orientations mod 180°).
fn axis_distance_deg(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(180.0);
    d.min(180.0 - d)
}

/// Scanlines that must independently agree on a `(symbology, text)` reading before it
/// is reported. One line is not evidence: every decoder is tried on every span of every
/// scanline in both directions — thousands of attempts per frame — and only Code 128,
/// Code 93 and EAN/UPC carry a check character at all. A printed code is crossed by
/// dozens of lines that all read the same thing; noise that slips past a decoder does
/// so differently on each line.
const MIN_LINE_VOTES: usize = 2;

/// The vote floor for a UPC-E. Its 51 modules are exactly the left half of an EAN-13
/// (start guard, six L/G digits, then `010101` = centre guard + one bar), so a slanted
/// scan line that leaves an EAN-13's bars just past the centre guard — into the white
/// above or below them, which looks like a quiet zone — reads a structurally perfect
/// UPC-E, check digit and all, for one EAN-13 in ten. Only the two or three lines that
/// exit at exactly that module do so, whereas a real UPC-E is read by every line that
/// crosses it.
const MIN_UPCE_VOTES: usize = 5;

/// Light margin (modules) a span must show on *both* sides to be decoded. Specified
/// quiet zones are 10 modules; a bar pattern butting against print or the crop edge is
/// a fragment of something, and fragments are where misreads come from.
const MIN_QUIET_MODULES: usize = 4;

/// Cap on distinct module patterns decoded per sweep, most-voted first: bounds the work
/// on a crop full of text-like spans.
const MAX_PATTERNS: usize = 96;

/// Whether a decoded linear symbol is long enough to be believed from a camera frame.
///
/// The symbologies without a check character accept any well-formed run sequence, and
/// short well-formed sequences occur by chance in text, in other symbologies' bars and
/// — above all — at the *ends of a real symbol*: a scanline that leaves an ITF's bars
/// early still finds a valid start, some digit pairs and something stop-like. Real
/// labels in these symbologies are never this short.
fn plausible_length(sym: &Symbol) -> bool {
    use crate::symbology::Symbology as S;
    let len = sym.payload_bytes().len();
    match sym.symbology {
        S::Itf => len >= 6,
        S::Codabar => len >= 3,
        S::Code39 | S::Code93 => len >= 2,
        // A lone add-on is never what a camera is pointed at; its parity rule is far
        // too weak to stand without the main symbol beside it.
        S::Ean2 | S::Ean5 => false,
        _ => len >= 1,
    }
}

/// One near-horizontal 1D pass: the EAN/UPC edge reader plus the quantized `scan1d`
/// front-end feeding every linear decoder, each candidate tried in both reading
/// directions, accepted by cross-scanline consensus.
fn scan_1d_sweep(
    frame: &crate::image::GrayFrame<'_>,
    scan_opts: &crate::scan1d::ScanOptions,
) -> Vec<Symbol> {
    use crate::traits::Decode;

    let mut found: Vec<Symbol> = Vec::new();
    #[cfg(feature = "ean")]
    if let Some(s) = crate::codes::ean::scan(frame, scan_opts) {
        found.push(s);
    }
    let linear: Vec<Box<dyn Decode>> = alloc::vec![
        #[cfg(feature = "code128")]
        Box::new(crate::codes::code128::Code128Decoder::new()),
        #[cfg(feature = "ean")]
        Box::new(crate::codes::ean::EanDecoder::new()),
        #[cfg(feature = "code93")]
        Box::new(crate::codes::code93::Code93Decoder::new()),
        #[cfg(feature = "code39")]
        Box::new(crate::codes::code39::Code39Decoder::new()),
        #[cfg(feature = "itf")]
        Box::new(crate::codes::itf::ItfDecoder::new()),
        #[cfg(feature = "codabar")]
        Box::new(crate::codes::codabar::CodabarDecoder::new()),
    ];
    if linear.is_empty() {
        return found;
    }

    // Group the per-scanline spans by module pattern: identical patterns decode
    // identically, so each is decoded once and its line count carried as votes.
    let mut patterns: Vec<(crate::scan1d::LinearCandidate, usize)> = Vec::new();
    for cand in crate::scan1d::scan_spans(frame, scan_opts) {
        if cand.pattern.quiet_zone < MIN_QUIET_MODULES {
            continue;
        }
        match patterns
            .iter_mut()
            .find(|(c, _)| c.pattern.modules == cand.pattern.modules)
        {
            Some((best, votes)) => {
                *votes += 1;
                if cand.confidence > best.confidence {
                    *best = cand;
                }
            }
            None => patterns.push((cand, 1)),
        }
    }
    patterns.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(b.0.confidence.total_cmp(&a.0.confidence))
    });
    patterns.truncate(MAX_PATTERNS);

    // Decode each pattern both ways and tally votes per reading. A scanline as easily
    // runs against the code's reading direction as with it (and a 180°-rotated code
    // always does), hence the mirrored attempt.
    let mut readings: Vec<(Symbol, usize)> = Vec::new();
    for (cand, votes) in &patterns {
        let mut mirrored = cand.clone();
        mirrored.pattern.modules.reverse();
        mirrored.edges.clear();
        for dec in &linear {
            for c in [cand, &mirrored] {
                let Some(sym) = crate::scan1d::try_decode(c, dec.as_ref()) else {
                    continue;
                };
                if !plausible_length(&sym) {
                    continue;
                }
                match readings.iter_mut().find(|(s, _)| same_reading(s, &sym)) {
                    Some((_, n)) => *n += votes,
                    None => readings.push((sym, *votes)),
                }
            }
        }
    }
    readings.retain(|(sym, n)| {
        *n >= if sym.symbology == Symbology::UpcE {
            MIN_UPCE_VOTES
        } else {
            MIN_LINE_VOTES
        }
    });

    // A reading contained in a longer reading of the same symbology that is at least
    // as well supported is that longer symbol seen through a scanline that left its
    // bars early — not a second code.
    let partial: Vec<bool> = readings
        .iter()
        .map(|(a, an)| {
            let at = a.payload_bytes();
            readings.iter().any(|(b, bn)| {
                let bt = b.payload_bytes();
                b.symbology == a.symbology
                    && bt.len() > at.len()
                    && 2 * bn >= *an
                    && bt.windows(at.len().max(1)).any(|w| w == at.as_slice())
            })
        })
        .collect();
    let mut keep = partial.iter();
    readings.retain(|_| !*keep.next().expect("one flag per reading"));

    readings.sort_by_key(|(_, votes)| core::cmp::Reverse(*votes));
    for (sym, _) in readings {
        dedup_push(&mut found, sym);
    }
    found
}

/// Whether two symbols are the same reading: same symbology, same payload bytes.
fn same_reading(a: &Symbol, b: &Symbol) -> bool {
    a.symbology == b.symbology && a.payload_bytes() == b.payload_bytes()
}

/// Push `sym` unless an equal `(symbology, payload)` is already present. Keyed on the
/// payload bytes, not the text: two different non-UTF-8 payloads have the same (absent)
/// text and must not collapse into one.
fn dedup_push(found: &mut Vec<Symbol>, sym: Symbol) {
    if !found.iter().any(|s| same_reading(s, &sym)) {
        found.push(sym);
    }
}

/// De-duplicating [`Vec::extend`] by `(symbology, payload)`.
fn dedup_extend(found: &mut Vec<Symbol>, more: Vec<Symbol>) {
    for sym in more {
        dedup_push(found, sym);
    }
}
