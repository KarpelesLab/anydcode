//! Generic 1D (linear) image front-end.
//!
//! This module turns a region of a [`GrayFrame`] that contains a linear barcode
//! into normalized [`LinearPattern`] candidates that *any* linear decoder can
//! consume. It is deliberately symbology-agnostic: it recovers the run structure
//! (bars and spaces expressed in narrow-module units) and hands it off; it does
//! **not** know Code 39 from Code 128 from EAN. A concrete symbology decoder
//! (an implementor of [`Decode`]) plugs in via [`try_decode`].
//!
//! # Pipeline
//!
//! 1. **Scanline extraction** (`sample` helpers). One or more horizontal — and
//!    optionally slightly rotated — lines are sampled across the frame's height.
//!    We work on the raw luminance *profile* along each line; there is no global
//!    image binarization. Each scan is a straight line in image space whose slope
//!    is `tan(angle)`, sampled one column per pixel of width with vertical linear
//!    interpolation.
//! 2. **Edge detection & run-lengths** (`extract_runs`). Dark/light is decided
//!    against a *sliding* local threshold — the midpoint of the luminance range in a
//!    window around each sample — so an illumination falloff along the line, or a
//!    dark surface beyond the label, does not shift the split. Transitions are
//!    located by a hysteresis state machine, then each edge is re-placed at the
//!    sub-pixel crossing of the *local* midpoint between the plateau levels of its
//!    two adjacent runs, which keeps edge positions stable under blur.
//! 3. **Span segmentation** (`split_spans`). A scanline through a real scene crosses
//!    more than the barcode: the label's edge, the surface it sits on, print beside
//!    it. The run sequence is cut at every run too wide to be a symbol element — the
//!    quiet zones and background — and each remaining stretch of bars becomes its own
//!    candidate, carrying the light margins measured on either side of it.
//! 4. **Module quantization** (`quantize`). Per span, the narrow-module pixel width
//!    and the bar/space width bias (ink spread, blur and threshold offset fatten bars
//!    at the expense of spaces, or the reverse) are fitted by least squares against
//!    the assigned integer counts. Each run is expanded into that many equal `bool`s,
//!    yielding a [`LinearPattern`] plus a quiet-zone module count.
//!
//! # Robustness envelope
//!
//! - **Module scale.** Any narrow-module width from ~1 px upward; validated from
//!   2 px/module to large scales.
//! - **Blur.** Mild defocus (separable box blur up to a few pixels radius) — edges
//!   are recovered from midpoint crossings which are blur-stable.
//! - **Rotation.** Scan lines run at whatever angles [`ScanOptions::angles_deg`] lists
//!   — any angle, sampled directly out of the frame with no image resampling — but a
//!   line must still cross every bar, so the angles have to be within a few degrees of
//!   the code's reading axis. The default sweep covers ±`MAX_ANGLE_DEG` around
//!   horizontal; finding the axis of an arbitrarily rotated code is the caller's job
//!   ([`crate::pipeline::scan_1d`] measures the crop's dominant texture orientation,
//!   [`ScanOptions::around`] builds the sweep), as is trying each pattern mirrored for
//!   a code read against its direction.
//! - **Framing.** The barcode need not span the frame, and the scan line may start
//!   or end on anything — dark background, other print, a second barcode. Each symbol
//!   only needs its own light quiet zone on both sides.
//!
//! Vertical extent is *not* measured (a single scan line has no height); the
//! reported [`Location`] outline is a thin band centered on the scan line.

use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::output::{Encoding, LinearPattern};
use crate::symbol::Symbol;
use crate::traits::Decode;
use alloc::{vec, vec::Vec};

/// Default half-set of scan angles in degrees; scans run at `0` and `±` each step.
const MAX_ANGLE_DEG: f32 = 6.0;

/// Minimum peak-to-peak luminance amplitude (0..255) for a scanline to be
/// considered to contain a barcode rather than flat background.
const MIN_AMPLITUDE: f32 = 30.0;

/// Hysteresis half-band of the threshold extractor, in units of the profile noise.
const HYST_NOISE_K: f32 = 2.5;

/// Options controlling scanline extraction and candidate generation.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Number of scan positions distributed across the central band of the frame.
    pub scan_count: usize,
    /// Scan angles in degrees, measured clockwise from the +x axis (`0.0` =
    /// horizontal, `90.0` = straight down). Any angle is allowed; a scan line must
    /// cross every bar, so these should bracket the code's reading axis.
    pub angles_deg: Vec<f32>,
    /// Smoothing radius (box filter) applied to each profile before edge finding.
    /// `0` disables smoothing.
    pub smooth_radius: usize,
    /// Minimum number of bar/space runs (excluding quiet zones) for a candidate to
    /// be emitted.
    pub min_runs: usize,
    /// Maximum number of candidates returned (highest confidence first).
    pub max_candidates: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            scan_count: 16,
            angles_deg: vec![0.0, -3.0, 3.0, -MAX_ANGLE_DEG, MAX_ANGLE_DEG],
            // Off by default: the hysteresis edge finder already tolerates noise, and
            // smoothing erodes the smallest (1-2 px) modules. Raise it for very noisy
            // input where module width comfortably exceeds the smoothing window.
            smooth_radius: 0,
            min_runs: 3,
            max_candidates: 8,
        }
    }
}

impl ScanOptions {
    /// Default options with the angle sweep centred on `base_deg` instead of
    /// horizontal: `base_deg` and `±3°`, `±6°` around it.
    pub fn around(base_deg: f32) -> Self {
        let mut opts = ScanOptions::default();
        for a in &mut opts.angles_deg {
            *a += base_deg;
        }
        opts
    }
}

/// Refine an estimate of a code's reading axis (degrees) by image sharpness.
///
/// A profile averaged across a band *along the bars* is crisp only when the band really
/// does run along the bars: a few degrees off and each sample mixes bar with space, so
/// the profile's edge energy collapses. Trying the angles within ±10° of `deg` on wide
/// bands through the frame and keeping the crispest recovers the axis to within the
/// step — which matters because gradient-based axis estimates (the locator's, the
/// orientation histogram's) are biased by several degrees for bars near the pixel limit,
/// and a scan line more than a few degrees off leaves a long symbol before its end.
pub fn refine_axis(frame: &GrayFrame<'_>, deg: f32) -> f32 {
    let (w, h) = (frame.width(), frame.height());
    if w < 16 || h < 16 {
        return deg;
    }
    // Three lines through the middle half of the frame, wide bands.
    let opts = |a: f32| ScanOptions {
        scan_count: 3,
        angles_deg: vec![a],
        ..ScanOptions::default()
    };
    let energy = |a: f32| -> f32 {
        scan_lines_of(w, h, &opts(a))
            .iter()
            .map(|line| {
                let p = sample_band(frame, line, AXIS_BAND_HALF).samples;
                p.windows(2)
                    .map(|v| (v[1] - v[0]) * (v[1] - v[0]))
                    .sum::<f32>()
                    / p.len().max(1) as f32
            })
            .sum()
    };
    let mut best = (energy(deg), deg);
    let mut step = -10.0f32;
    while step <= 10.0 {
        if step != 0.0 {
            let e = energy(deg + step);
            if e > best.0 {
                best = (e, deg + step);
            }
        }
        step += 2.0;
    }
    best.1
}

/// Band half-width used by [`refine_axis`]: wide, so a small misalignment smears.
const AXIS_BAND_HALF: i32 = 8;

/// One recovered linear-barcode candidate: the normalized module pattern, where it
/// was scanned, and a confidence score.
#[derive(Debug, Clone)]
pub struct LinearCandidate {
    /// The recovered module pattern, ready to feed any linear [`Decode`]r.
    pub pattern: LinearPattern,
    /// Sub-pixel transition positions along the scan line, in pixels. Consecutive
    /// pairs bound the bar/space runs (the first run is a bar). Unlike
    /// [`LinearCandidate::pattern`] these are *not* quantized to an integer module
    /// grid, so a width-ratio decoder can resolve thin (~1 module) features — guards
    /// and single-module elements — that hard quantization merges under blur.
    pub edges: Vec<f32>,
    /// Leading light-margin width in pixels (the quiet zone before the first bar).
    pub lead_px: f32,
    /// Trailing light-margin width in pixels (the quiet zone after the last bar).
    pub trail_px: f32,
    /// Scanline geometry: a thin band along the scan line spanning the bar region.
    pub location: Location,
    /// Confidence in `[0, 1]`: how cleanly the runs quantized to integer modules,
    /// scaled by luminance amplitude.
    pub confidence: f32,
}

/// Scan `frame` for linear barcodes and return normalized candidates.
///
/// Candidates are deduplicated by identical module pattern, ordered by how many scan
/// lines recovered that exact pattern and then by descending
/// [`LinearCandidate::confidence`], and truncated to [`ScanOptions::max_candidates`].
/// Agreement between lines comes first because confidence only measures how cleanly
/// the runs quantized, which a short fragment of bars does more easily than a whole
/// symbol — whereas a fragment is cut differently on every line.
pub fn scan_lines(frame: &GrayFrame<'_>, opts: &ScanOptions) -> Vec<LinearCandidate> {
    // (best candidate, lines that produced the pattern) per distinct pattern.
    let mut kept: Vec<(LinearCandidate, Vec<usize>)> = Vec::new();
    for (line, cand) in scan_all(frame, opts) {
        match kept
            .iter_mut()
            .find(|(k, _)| k.pattern.modules == cand.pattern.modules)
        {
            Some((best, lines)) => {
                if !lines.contains(&line) {
                    lines.push(line);
                }
                if cand.confidence > best.confidence {
                    *best = cand;
                }
            }
            None => kept.push((cand, vec![line])),
        }
    }
    kept.sort_by(|a, b| {
        b.1.len()
            .cmp(&a.1.len())
            .then(b.0.confidence.total_cmp(&a.0.confidence))
    });
    kept.truncate(opts.max_candidates);
    kept.into_iter().map(|(c, _)| c).collect()
}

/// Scan `frame` and return **every** per-scanline span candidate, without the dedup and
/// truncation [`scan_lines`] applies.
///
/// This is what a consensus decoder wants: the same physical code is crossed by many
/// scanlines, and *how many* of them agree on a reading is the strongest evidence
/// available that the reading is real (see [`crate::pipeline::scan_1d`]). Ordered by
/// scan position, then angle, then left to right along the line.
pub fn scan_spans(frame: &GrayFrame<'_>, opts: &ScanOptions) -> Vec<LinearCandidate> {
    scan_all(frame, opts).into_iter().map(|(_, c)| c).collect()
}

/// Prominence thresholds (as a fraction of the scanline amplitude) used by the fine
/// peak-based extractor behind [`scan_edges`]. A low value resolves the shallow dips
/// that heavy blur leaves at thin (~1 module) features; a higher one rejects noise on
/// cleaner captures. Every level is tried; a width-ratio decoder validates by checksum.
const FINE_PROMINENCE: &[f32] = &[0.06, 0.11];

/// Upper bound on luminance extrema in a single scanline profile before it is rejected as
/// scene texture rather than a barcode. Even the longest linear symbologies produce a few
/// hundred runs; thousands of extrema mean a cluttered full-frame line, and the
/// prominence prune below is superlinear in the extrema count, so this cap keeps a busy
/// image (e.g. a whole 1080p photo) from stalling the scan. A located crop, which is what
/// the 1D path is meant to receive, stays far under it.
const MAX_FINE_EXTREMA: usize = 1024;

/// Scan `frame` and return per-scanline candidates carrying **fine** sub-pixel
/// [`LinearCandidate::edges`], without the dedup and truncation [`scan_lines`] applies.
///
/// Unlike [`scan_lines`] (whose edges come from a hysteresis run finder tuned for the
/// quantized [`LinearPattern`] path), these edges come from a peak-based extractor that
/// resolves sub-threshold thin features — the single-module guards and elements that
/// blur otherwise merges. Two scanlines that hard-quantize to the same pattern can still
/// carry materially different edges, and blur corrupts *different* thin features on
/// different lines, so a width-ratio decoder (e.g. EAN/UPC) wants to try every one and
/// vote. Ordered by descending [`LinearCandidate::confidence`].
pub fn scan_edges(frame: &GrayFrame<'_>, opts: &ScanOptions) -> Vec<LinearCandidate> {
    analyze(frame, opts)
        .edge_candidates()
        .into_iter()
        .map(|(_, c)| c)
        .collect()
}

/// Run every scan position/angle and collect the raw span candidates (no dedup/sort).
fn scan_all(frame: &GrayFrame<'_>, opts: &ScanOptions) -> Vec<(usize, LinearCandidate)> {
    analyze(frame, opts).spans()
}

/// Every scan line of one frame, sampled and edge-extracted once, for the several
/// readers that consume them (the span/quantize path, the EAN width reader).
///
/// Candidates come tagged with the index of the line they were read from: one line
/// yields several candidates for the same stretch of bars — one per edge extractor —
/// and they are not independent evidence. A consensus decoder must count *lines* that
/// agree, not candidates.
#[derive(Debug)]
pub(crate) struct Analysis {
    lines: Vec<LineRuns>,
    min_runs: usize,
    /// Frame size, in pixels.
    size: (usize, usize),
}

/// One scan line's profile and the edges each extractor found on it.
#[derive(Debug)]
struct LineRuns {
    line: ScanLine,
    profile: Profile,
    /// Threshold-crossing edges (exact on a sharp capture).
    hyst: Option<Runs>,
    /// Gradient-peak edges, blur-corrected (survive blur and low contrast).
    grad: Option<Runs>,
}

/// Sample every scan line of `frame` and extract its edges.
pub(crate) fn analyze(frame: &GrayFrame<'_>, opts: &ScanOptions) -> Analysis {
    let (w, h) = (frame.width(), frame.height());
    let mut lines = Vec::new();
    if w >= 4 && h > 0 {
        for line in scan_lines_of(w, h, opts) {
            let profile = sample_profile(frame, &line, opts.smooth_radius);
            let hyst = extract_runs(&profile);
            let grad = extract_runs_grad(&profile);
            lines.push(LineRuns {
                line,
                profile,
                hyst,
                grad,
            });
        }
    }
    Analysis {
        lines,
        min_runs: opts.min_runs,
        size: (w, h),
    }
}

impl Analysis {
    /// One candidate per barcode-shaped span of every line, from both extractors,
    /// tagged with the line's index. Ordered by line, then left to right.
    pub(crate) fn spans(&self) -> Vec<(usize, LinearCandidate)> {
        let mut found = Vec::new();
        for (idx, l) in self.lines.iter().enumerate() {
            // Two independent readings of where the edges are.
            for runs in [&l.hyst, &l.grad].into_iter().flatten() {
                for mut span in split_spans(runs, self.min_runs) {
                    let n = span.edges.len();
                    span.lead_px = span.lead_px.min(quiet_extent(
                        &l.profile,
                        span.edges[0],
                        span.edges[1],
                        span.lead_px,
                    ));
                    span.trail_px = span.trail_px.min(quiet_extent(
                        &l.profile,
                        span.edges[n - 1],
                        span.edges[n - 2],
                        span.trail_px,
                    ));
                    if let Some(cand) = candidate_of(span, &l.line, self.min_runs) {
                        found.push((idx, cand));
                    }
                }
            }
        }
        found
    }

    /// One whole-line candidate per fine edge reading of every line (the peak
    /// extractor at each prominence, plus the gradient extractor), tagged with the
    /// line's index and ordered by descending confidence.
    pub(crate) fn edge_candidates(&self) -> Vec<(usize, LinearCandidate)> {
        let mut found = Vec::new();
        for (idx, l) in self.lines.iter().enumerate() {
            let fine = FINE_PROMINENCE
                .iter()
                .filter_map(|&prom| extract_runs_fine(&l.profile.samples, prom));
            for runs in fine.chain(l.grad.clone()) {
                if let Some(cand) = candidate_of(runs, &l.line, self.min_runs) {
                    found.push((idx, cand));
                }
            }
        }
        found.sort_by(|a, b| b.1.confidence.total_cmp(&a.1.confidence));
        found
    }

    /// The raw edge positions of every line, one sequence per edge extractor that
    /// found any, tagged with the line's index: sub-pixel sample positions along the
    /// line (map them into the frame with [`Analysis::point`]). Consecutive pairs bound
    /// alternating runs, of which the first and the last are *light*: the margins
    /// outside the line's first and last bar, each reaching to the frame's edge or to
    /// a dark run the frame clips.
    ///
    /// Unlike [`Analysis::spans`] nothing is cut at wide runs or quantized, and the
    /// margins are kept as runs: this is for a reader that finds its own symbol inside
    /// the line by its finder patterns (GS1 DataBar), which may have no quiet zone at
    /// all — a symbol cropped to its outermost bar has its first space there.
    #[cfg_attr(not(feature = "databar"), allow(dead_code))]
    pub(crate) fn line_edges(&self) -> impl Iterator<Item = (usize, Vec<f32>)> {
        self.lines.iter().enumerate().flat_map(|(idx, l)| {
            [&l.hyst, &l.grad].into_iter().flatten().map(move |runs| {
                let n = runs.edges.len();
                let mut edges = Vec::with_capacity(n + 2);
                edges.push(runs.edges[0] - runs.lead_px);
                edges.extend_from_slice(&runs.edges);
                edges.push(runs.edges[n - 1] + runs.trail_px);
                (idx, edges)
            })
        })
    }

    /// Frame position of (fractional) sample `pos` along line `line`. A slanted line
    /// starts and ends outside the frame, where it samples the border pixels: such
    /// positions are reported on the border.
    #[cfg_attr(not(feature = "databar"), allow(dead_code))]
    pub(crate) fn point(&self, line: usize, pos: f32) -> Point {
        let p = self.lines[line].line.point(pos);
        let (w, h) = self.size;
        Point::new(
            p.x.clamp(0.0, w.saturating_sub(1) as f32),
            p.y.clamp(0.0, h.saturating_sub(1) as f32),
        )
    }
}

/// Feed a candidate's pattern to a linear decoder, returning the decoded [`Symbol`]
/// on success. The candidate's [`Location`] is attached if the decoder left it unset.
///
/// This is generic over `&dyn Decode`, so it works with any linear symbology decoder
/// without this front-end depending on a specific one.
pub fn try_decode(candidate: &LinearCandidate, decoder: &dyn Decode) -> Option<Symbol> {
    let encoding = Encoding::Linear(candidate.pattern.clone());
    let mut symbol = decoder.decode(&encoding).ok()?;
    if symbol.location.is_none() {
        symbol.location = Some(candidate.location.clone());
    }
    Some(symbol)
}

// --- Scanline sampling -----------------------------------------------------

/// One scan line through the frame: sample `i` sits at `(ox + i·sx, oy + i·sy)`, with
/// `(sx, sy)` a unit vector — one sample per pixel of *line* length at every angle, so
/// a module spans as many samples on a diagonal line as on a horizontal one.
#[derive(Debug, Clone, Copy)]
struct ScanLine {
    ox: f32,
    oy: f32,
    sx: f32,
    sy: f32,
    /// Number of samples.
    len: usize,
}

impl ScanLine {
    /// Frame position of (fractional) sample index `i`.
    fn point(&self, i: f32) -> Point {
        Point::new(self.ox + i * self.sx, self.oy + i * self.sy)
    }
}

/// The scan lines for a `w`×`h` frame: [`ScanOptions::scan_count`] positions spread
/// across the central 10%..90% band (so angled lines stay mostly inside the frame),
/// each at every angle in [`ScanOptions::angles_deg`].
///
/// Lines pivot about the frame's centre line *across* their direction — the vertical
/// centre line for angles within 45° of horizontal, the horizontal one otherwise — and
/// run from frame edge to frame edge along their major axis.
fn scan_lines_of(w: usize, h: usize, opts: &ScanOptions) -> Vec<ScanLine> {
    let count = opts.scan_count.max(1);
    let mut lines = Vec::with_capacity(count * opts.angles_deg.len());
    for i in 0..count {
        let frac = if count == 1 {
            0.5
        } else {
            0.1 + 0.8 * (i as f32) / ((count - 1) as f32)
        };
        for &deg in &opts.angles_deg {
            // An orientation, not a direction: fold into (-90°, 90°].
            let mut deg = deg.rem_euclid(180.0);
            if deg > 90.0 {
                deg -= 180.0;
            }
            let (sin, cos) = deg.to_radians().sin_cos();
            lines.push(if deg.abs() <= 45.0 {
                // Spans every column: x from 0 to w-1.
                let cy = frac * (h.saturating_sub(1)) as f32;
                ScanLine {
                    ox: 0.0,
                    oy: cy - (w as f32) / 2.0 * sin / cos,
                    sx: cos,
                    sy: sin,
                    len: ((w as f32) / cos).floor() as usize,
                }
            } else {
                // Spans every row: y from 0 to h-1, reading downward.
                let (sin, cos) = if sin < 0.0 { (-sin, -cos) } else { (sin, cos) };
                let cx = frac * (w.saturating_sub(1)) as f32;
                ScanLine {
                    ox: cx - (h as f32) / 2.0 * cos / sin,
                    oy: 0.0,
                    sx: cos,
                    sy: sin,
                    len: ((h as f32) / sin).floor() as usize,
                }
            });
        }
    }
    lines
}

/// Half-width, in pixels, of the band of image rows averaged into each scan line.
const BAND_HALF: i32 = 2;

/// Band half-width tried on noisy frames (13 rows instead of 5).
const WIDE_BAND_HALF: i32 = 6;
/// Profile noise (grey levels) from which the wide band is worth sampling.
const WIDE_BAND_MIN_NOISE: f32 = 2.5;
/// The wide band is used if its noise is below this fraction of the narrow band's
/// (√(5/13) ≈ 0.62 when the line is perfectly aligned).
const WIDE_BAND_GAIN: f32 = 0.8;

/// A luminance profile along a scan line, with the noise on it.
#[derive(Debug)]
struct Profile {
    /// One sample per pixel of line length.
    samples: Vec<f32>,
    /// Standard deviation of the noise on each sample, measured *across* the band: a
    /// bar is constant along its length, so pixels of one band column differ only by
    /// noise (and a little by any misalignment of the line with the bars).
    noise: f32,
}

/// Build the luminance profile along `line`, one sample per pixel of line length, then
/// optionally box-smooth it.
///
/// Each sample is the mean of the *actual pixels* in a thin band around the line whose
/// centres project into that sample's unit bin — no interpolation. Interpolating between
/// pixels is a low-pass filter exactly at the scale of a two-pixel module, which is
/// where a camera barcode lives; binning real pixels instead keeps an axis-aligned
/// line as sharp as the image, lets a diagonal line draw on pixels at several sub-pixel
/// phases, and averages sensor noise across the band (bars are constant along their
/// length, so nothing but noise is lost). Band pixels outside the frame are left out;
/// where the line itself is outside, it follows the frame's border.
fn sample_profile(frame: &GrayFrame<'_>, line: &ScanLine, smooth_radius: usize) -> Profile {
    let mut profile = sample_band(frame, line, BAND_HALF);
    // On a noisy frame, average more of each bar's length — if the line runs along
    // the bars well enough to allow it. A wider band on a misaligned line smears edges
    // across its samples, which shows up as *more* scatter across the band, not less;
    // so the measured noise itself says which band is the better profile.
    if profile.noise >= WIDE_BAND_MIN_NOISE {
        let wide = sample_band(frame, line, WIDE_BAND_HALF);
        if wide.noise < WIDE_BAND_GAIN * profile.noise {
            profile = wide;
        }
    }
    if smooth_radius > 0 {
        profile.samples = smooth(&profile.samples, smooth_radius);
        profile.noise /= ((2 * smooth_radius + 1) as f32).sqrt();
    }
    profile
}

/// The band-averaged profile along `line`, averaging `band_half` pixels either side.
fn sample_band(frame: &GrayFrame<'_>, line: &ScanLine, band_half: i32) -> Profile {
    let (w, h) = (frame.width() as i32, frame.height() as i32);
    let n = line.len;
    let mut sum = vec![0.0f32; n];
    let mut sum_sq = vec![0.0f32; n];
    let mut count = vec![0u16; n];
    // Unit normal to the line.
    let (nx, ny) = (-line.sy, line.sx);
    let mut add = |x: i32, y: i32| {
        // Bin by where this pixel's centre falls along the line.
        let t = (x as f32 - line.ox) * line.sx + (y as f32 - line.oy) * line.sy;
        let bin = t.round();
        if bin >= 0.0 && (bin as usize) < n {
            let v = f32::from(frame.get_unchecked(x as usize, y as usize));
            sum[bin as usize] += v;
            sum_sq[bin as usize] += v * v;
            count[bin as usize] += 1;
        }
    };
    for i in 0..n {
        let p = line.point(i as f32);
        let mut inside = false;
        for k in -band_half..=band_half {
            let x = (p.x + k as f32 * nx).round() as i32;
            let y = (p.y + k as f32 * ny).round() as i32;
            if (0..w).contains(&x) && (0..h).contains(&y) {
                add(x, y);
                inside = true;
            }
        }
        // Where the whole band has left the frame the line follows the frame's border
        // instead — one pixel, counted once, so the sample is known to be a noisy one.
        if !inside {
            add(
                (p.x.round() as i32).clamp(0, w - 1),
                (p.y.round() as i32).clamp(0, h - 1),
            );
        }
    }
    // A bin no pixel centre fell into (possible on a diagonal) takes its neighbour.
    let mut samples = Vec::with_capacity(n);
    let mut last = 0.0f32;
    // Per-pixel variance within each well-populated bin; the median over bins is the
    // pixel noise (bins straddling an edge on a slightly misaligned line are the
    // outliers). A sample averaging `c` pixels then carries 1/c of it — and on a
    // diagonal line the bins are unevenly filled, so that is averaged over the bins.
    let mut variances: Vec<f32> = Vec::with_capacity(n);
    let (mut inv_count, mut dof) = (0.0f32, 0.0f32);
    for i in 0..n {
        if count[i] > 0 {
            let c = f32::from(count[i]);
            last = sum[i] / c;
            inv_count += 1.0 / c;
            if count[i] > 2 {
                variances.push((sum_sq[i] - sum[i] * last).max(0.0) / (c - 1.0));
                dof += c - 1.0;
            }
        } else {
            inv_count += 1.0;
            if let Some(j) = (i + 1..n).find(|&j| count[j] > 0)
                && i == 0
            {
                last = sum[j] / f32::from(count[j]);
            }
        }
        samples.push(last);
    }
    let noise = if variances.is_empty() || n == 0 {
        0.0
    } else {
        let mid = variances.len() / 2;
        let median = *variances.select_nth_unstable_by(mid, f32::total_cmp).1;
        // The median of a sample variance on k degrees of freedom sits below the
        // variance itself, by (1 − 2/9k)³ (Wilson–Hilferty).
        let k = (dof / variances.len() as f32).max(1.0);
        let pixel_var = median / (1.0 - 2.0 / (9.0 * k)).powi(3);
        (pixel_var * inv_count / n as f32).sqrt()
    };
    Profile { samples, noise }
}

/// Separable box smoothing with the given radius (`0` returns a copy).
fn smooth(profile: &[f32], radius: usize) -> Vec<f32> {
    if radius == 0 {
        return profile.to_vec();
    }
    let n = profile.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let lo = i.saturating_sub(radius);
        let hi = (i + radius).min(n - 1);
        let mut sum = 0.0;
        for &v in &profile[lo..=hi] {
            sum += v;
        }
        out.push(sum / ((hi - lo + 1) as f32));
    }
    out
}

// --- Edge detection & run extraction ---------------------------------------

/// The run structure recovered from a profile: sub-pixel edge positions plus the
/// dark/light levels used.
#[derive(Debug, Clone)]
struct Runs {
    /// Sub-pixel positions (fractional column index) of every transition, from the
    /// leading edge of the first bar to the trailing edge of the last: an even count,
    /// consecutive pairs bounding alternating bar/space runs.
    edges: Vec<f32>,
    /// Peak-to-peak amplitude (light minus dark level).
    amplitude: f32,
    /// Leading light margin width in pixels (quiet zone side).
    lead_px: f32,
    /// Trailing light margin width in pixels (quiet zone side).
    trail_px: f32,
}

/// Robust low/high luminance levels from the profile: the mean of the darkest and
/// brightest tenth of samples. Tolerant of noise and a few outlier runs.
fn levels(profile: &[f32]) -> (f32, f32) {
    let mut sorted = profile.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let n = sorted.len();
    let tenth = (n / 10).max(1);
    let low: f32 = sorted[..tenth].iter().sum::<f32>() / (tenth as f32);
    let high: f32 = sorted[n - tenth..].iter().sum::<f32>() / (tenth as f32);
    (low, high)
}

/// Interpolate the fractional column where the segment from index `i-1` (value `a`)
/// to index `i` (value `b`) crosses `thr`.
fn crossing(i: usize, a: f32, b: f32, thr: f32) -> f32 {
    let denom = b - a;
    if denom.abs() < 1e-6 {
        (i as f32) - 0.5
    } else {
        ((i - 1) as f32) + (thr - a) / denom
    }
}

/// Minimum and maximum of `profile` within `radius` samples of each sample, via
/// monotonic deques: O(n).
fn sliding_range(profile: &[f32], radius: usize) -> (Vec<f32>, Vec<f32>) {
    let n = profile.len();
    let mut lo = vec![0.0f32; n];
    let mut hi = vec![0.0f32; n];
    let mut min_q: alloc::collections::VecDeque<usize> = alloc::collections::VecDeque::new();
    let mut max_q: alloc::collections::VecDeque<usize> = alloc::collections::VecDeque::new();
    let mut next = 0usize;
    for i in 0..n {
        let right = (i + radius).min(n - 1);
        while next <= right {
            while min_q.back().is_some_and(|&b| profile[b] >= profile[next]) {
                min_q.pop_back();
            }
            min_q.push_back(next);
            while max_q.back().is_some_and(|&b| profile[b] <= profile[next]) {
                max_q.pop_back();
            }
            max_q.push_back(next);
            next += 1;
        }
        let left = i.saturating_sub(radius);
        while min_q.front().is_some_and(|&f| f < left) {
            min_q.pop_front();
        }
        while max_q.front().is_some_and(|&f| f < left) {
            max_q.pop_front();
        }
        lo[i] = profile[*min_q.front().expect("window is never empty")];
        hi[i] = profile[*max_q.front().expect("window is never empty")];
    }
    (lo, hi)
}

/// Per-sample local threshold and hysteresis half-band for a profile.
///
/// Each sample is compared against the midpoint of the luminance range within
/// `radius` samples of it. Where that range is too small to hold a bar edge — inside a
/// quiet zone, a wide bar, a flat dark background — the nearest *informative* threshold
/// is carried over, so flat stretches are classified by the contrast next to them
/// rather than by noise. Returns `None` for a profile that is flat throughout.
fn local_thresholds(profile: &[f32], radius: usize, floor: f32) -> Option<Vec<(f32, f32)>> {
    let n = profile.len();
    let (lo, hi) = sliding_range(profile, radius);

    let mut out: Vec<Option<(f32, f32)>> = (0..n)
        .map(|i| {
            let amp = hi[i] - lo[i];
            (amp >= floor).then(|| ((lo[i] + hi[i]) / 2.0, amp * 0.12))
        })
        .collect();
    // Carry informative thresholds across flat stretches: forward, then back-fill the
    // leading stretch from the first informative sample.
    let first = out.iter().position(|t| t.is_some())?;
    let mut last = out[first];
    for t in out.iter_mut().skip(first) {
        match t {
            Some(_) => last = *t,
            None => *t = last,
        }
    }
    let lead = out[first];
    for t in out.iter_mut().take(first) {
        *t = lead;
    }
    Some(out.into_iter().map(|t| t.expect("filled above")).collect())
}

/// Extract transitions from a profile using hysteresis around a sliding local
/// mid-level, placing each confirmed edge at its sub-pixel midpoint crossing. Returns
/// `None` for flat profiles or ones holding no complete bar.
///
/// The scan line may begin or end on anything (a dark background, a clipped bar): the
/// returned edges run from the first light→dark transition to the last dark→light one,
/// with the light margins outside them reported as the lead/trail widths.
fn extract_runs(profile: &Profile) -> Option<Runs> {
    // Contrast is only contrast if it stands clear of the noise: on a grainy frame a
    // flat surface swings by more than the absolute floor.
    let noise = profile.noise;
    let floor = MIN_AMPLITUDE.max(GRAD_RANGE_K * noise);
    let profile = profile.samples.as_slice();
    let n = profile.len();
    if n < 4 {
        return None;
    }
    let (low, high) = levels(profile);
    let amplitude = high - low;
    if amplitude < floor {
        return None;
    }
    // Wide enough to see past the widest symbol element at any plausible module size,
    // narrow enough to track an illumination falloff along the line.
    let radius = (n / 24).clamp(8, 48);
    let mut thresholds = local_thresholds(profile, radius, floor)?;
    // The hysteresis band must also clear the noise, or a flat stretch flickers.
    for (_, hyst) in &mut thresholds {
        *hyst = hyst.max(HYST_NOISE_K * noise);
    }

    let start_dark = profile[0] < thresholds[0].0;
    let mut edges: Vec<f32> = Vec::new();
    let mut dark = start_dark;
    let mut cand: Option<f32> = None;

    for i in 1..n {
        let a = profile[i - 1];
        let b = profile[i];
        let (thr, hyst) = thresholds[i];
        if dark {
            // Seeking a rising edge, confirmed once the signal passes `thr + hyst`.
            if a < thr && b >= thr {
                cand = Some(crossing(i, a, b, thr));
            } else if b < thr {
                cand = None;
            }
            if b >= thr + hyst {
                edges.push(cand.take().unwrap_or((i as f32) - 0.5));
                dark = false;
            }
        } else {
            // Seeking a falling edge, confirmed once the signal drops below `thr - hyst`.
            if a > thr && b <= thr {
                cand = Some(crossing(i, a, b, thr));
            } else if b > thr {
                cand = None;
            }
            if b <= thr - hyst {
                edges.push(cand.take().unwrap_or((i as f32) - 0.5));
                dark = true;
            }
        }
    }
    if edges.is_empty() {
        return None;
    }

    // Refine edge positions against *local* levels: each edge is placed at the midpoint
    // between the plateau levels of its two adjacent runs, which is stable under blur
    // where a threshold crossing is biased toward the wider neighbour.
    let refined = refine_edges(profile, &edges, start_dark);
    bar_bounded(refined, start_dark, n, amplitude)
}

/// Trim an alternating edge list so it starts on a light→dark edge and ends on a
/// dark→light one, measuring the light margins outside. `start_dark` says whether the
/// profile began inside a dark run (so the first edge is a dark→light one).
fn bar_bounded(mut edges: Vec<f32>, start_dark: bool, n: usize, amplitude: f32) -> Option<Runs> {
    // A leading dark run is clipped by the frame (or is background): its far side is
    // unknown, so it cannot be a bar. The light run after it is the lead margin.
    let lead_from = if start_dark {
        if edges.is_empty() {
            return None;
        }
        edges.remove(0)
    } else {
        0.0
    };
    // Likewise an odd count now means the profile ended inside a dark run.
    let trail_to = if edges.len() % 2 == 1 {
        edges.pop().expect("odd count is non-empty")
    } else {
        (n - 1) as f32
    };
    if edges.len() < 2 {
        return None;
    }
    let lead_px = edges[0] - lead_from;
    let trail_px = trail_to - edges[edges.len() - 1];
    Some(Runs {
        edges,
        amplitude,
        lead_px,
        trail_px,
    })
}

/// Re-place each approximate edge at the crossing of the local midpoint between the
/// plateau (min for dark, max for light) of the two runs it separates.
fn refine_edges(profile: &[f32], approx: &[f32], start_dark: bool) -> Vec<f32> {
    let n = profile.len();
    // Run boundaries: 0, each edge, n-1. Run k spans bounds[k]..bounds[k+1]; runs
    // alternate from the polarity the profile starts in.
    let mut bounds = Vec::with_capacity(approx.len() + 2);
    bounds.push(0.0f32);
    bounds.extend_from_slice(approx);
    bounds.push((n - 1) as f32);

    // Plateau extreme of each run (min if dark, max if light).
    let mut ext = Vec::with_capacity(bounds.len() - 1);
    for k in 0..bounds.len() - 1 {
        let lo = (bounds[k].ceil() as usize).min(n - 1);
        let hi = (bounds[k + 1].floor() as usize).min(n - 1);
        let dark_run = (k % 2 == 1) != start_dark;
        let (a, b) = if lo <= hi {
            (lo, hi)
        } else {
            (lo.min(hi), lo.max(hi))
        };
        let mut acc = profile[a];
        for &v in &profile[a..=b] {
            if dark_run {
                acc = acc.min(v);
            } else {
                acc = acc.max(v);
            }
        }
        ext.push(acc);
    }

    let mut out = Vec::with_capacity(approx.len());
    for (j, &e) in approx.iter().enumerate() {
        // Edge j separates run j and run j+1.
        let lthr = (ext[j] + ext[j + 1]) / 2.0;
        let left_center = (bounds[j] + bounds[j + 1]) / 2.0;
        let right_center = (bounds[j + 1] + bounds[j + 2]) / 2.0;
        out.push(local_crossing(profile, left_center, right_center, lthr, e));
    }
    out
}

/// Find the sample-space crossing of `lthr` between `left` and `right` (run centers),
/// choosing the straddle nearest `approx` when several exist.
fn local_crossing(profile: &[f32], left: f32, right: f32, lthr: f32, approx: f32) -> f32 {
    let n = profile.len();
    let lo = (left.floor().max(0.0) as usize).max(1);
    let hi = (right.ceil() as usize).min(n - 1);
    let mut best: Option<f32> = None;
    let mut best_d = f32::INFINITY;
    for i in lo..=hi {
        let a = profile[i - 1];
        let b = profile[i];
        if (a - lthr) * (b - lthr) <= 0.0 && (a - b).abs() > 1e-6 {
            let pos = crossing(i, a, b, lthr);
            let d = (pos - approx).abs();
            if d < best_d {
                best_d = d;
                best = Some(pos);
            }
        }
    }
    best.unwrap_or(approx)
}

/// A three-tap box smooth (radius 1) used to steady peak detection against per-pixel
/// noise without eroding the ~2 px features the fine extractor must keep.
fn smooth3(profile: &[f32]) -> Vec<f32> {
    let n = profile.len();
    (0..n)
        .map(|i| {
            let a = profile[i.saturating_sub(1)];
            let c = profile[(i + 1).min(n - 1)];
            (a + profile[i] + c) / 3.0
        })
        .collect()
}

/// Fine run extraction by peak detection. Where [`extract_runs`] uses a hysteresis band
/// that merges thin low-contrast features, this locates every local extremum, prunes the
/// ones whose prominence falls below `prom` × amplitude, and places a sub-pixel edge at
/// the local-midpoint crossing between each surviving adjacent pair. Sub-threshold dips
/// (a blurred single-module bar between two spaces) therefore survive as their own run,
/// which is what lets a width-ratio decoder recover guards and single-module elements.
fn extract_runs_fine(profile: &[f32], prom: f32) -> Option<Runs> {
    let n = profile.len();
    if n < 8 {
        return None;
    }
    let (low, high) = levels(profile);
    let amplitude = high - low;
    if amplitude < MIN_AMPLITUDE {
        return None;
    }
    // The line may begin or end on anything; judge each end against its own
    // neighbourhood (a global split misreads the dim end of an unevenly lit line).
    let thresholds = local_thresholds(profile, (n / 24).clamp(8, 48), MIN_AMPLITUDE)?;
    let end_kind = |i: usize| if profile[i] < thresholds[i].0 { -1 } else { 1 };

    // Collect extrema (position, value, kind: +1 max / -1 min) from trend reversals,
    // bracketed by a virtual extremum at each end so a flat margin (which carries no
    // detected peak of its own) still bounds the first and last edge.
    let sm = smooth3(profile);
    let mut ext: Vec<(usize, f32, i8)> = vec![(0, profile[0], end_kind(0))];
    let mut dir: i8 = 0;
    for i in 1..n {
        let dv = sm[i] - sm[i - 1];
        let nd = if dv > 0.0 {
            1
        } else if dv < 0.0 {
            -1
        } else {
            dir
        };
        if dir != 0 && nd != dir {
            ext.push((i - 1, sm[i - 1], dir));
        }
        dir = nd;
    }
    ext.push((n - 1, profile[n - 1], end_kind(n - 1)));

    // Bail on texture before the superlinear prune: a barcode line never has this many
    // swings, but a cluttered full-frame profile does, and pruning them one at a time
    // would stall the scan.
    if ext.len() > MAX_FINE_EXTREMA {
        return None;
    }

    // Collapse consecutive same-kind extrema, keeping the more extreme.
    let mut j = 1;
    while j < ext.len() {
        if ext[j].2 == ext[j - 1].2 {
            let keep_prev = if ext[j].2 > 0 {
                ext[j - 1].1 >= ext[j].1
            } else {
                ext[j - 1].1 <= ext[j].1
            };
            ext.remove(if keep_prev { j } else { j - 1 });
        } else {
            j += 1;
        }
    }

    // Prune the lowest-prominence extremum until every interior swing clears the floor.
    let min_prom = amplitude * prom;
    loop {
        if ext.len() < 3 {
            return None;
        }
        let mut worst: Option<(usize, f32)> = None;
        for k in 1..ext.len() - 1 {
            let p = (ext[k].1 - ext[k - 1].1)
                .abs()
                .min((ext[k].1 - ext[k + 1].1).abs());
            if p < min_prom && worst.is_none_or(|(_, wp)| p < wp) {
                worst = Some((k, p));
            }
        }
        let Some((k, _)) = worst else { break };
        ext.remove(k);
        // Removing k leaves its two neighbours the same kind: keep the more extreme.
        if k < ext.len() && ext[k - 1].2 == ext[k].2 {
            let keep_prev = if ext[k].2 > 0 {
                ext[k - 1].1 >= ext[k].1
            } else {
                ext[k - 1].1 <= ext[k].1
            };
            ext.remove(if keep_prev { k } else { k - 1 });
        }
    }
    // Place a sub-pixel edge at the local-midpoint crossing between each adjacent pair.
    let mut edges: Vec<f32> = Vec::with_capacity(ext.len().saturating_sub(1));
    for w in ext.windows(2) {
        let (p0, v0, _) = w[0];
        let (p1, v1, _) = w[1];
        let mid = (v0 + v1) / 2.0;
        let mut pos = (p0 as f32 + p1 as f32) / 2.0;
        for i in (p0 + 1)..=p1 {
            let a = profile[i - 1];
            let b = profile[i];
            if (a - mid) * (b - mid) <= 0.0 && (a - b).abs() > 1e-6 {
                pos = (i - 1) as f32 + (mid - a) / (b - a);
                break;
            }
        }
        edges.push(pos);
    }
    let start_dark = ext[0].2 < 0;
    bar_bounded(edges, start_dark, n, amplitude)
}

// --- Gradient-peak edge extraction -------------------------------------------

/// Least luminance range (0..255) of a scanline for the gradient extractor to look at
/// it. Far below [`MIN_AMPLITUDE`]: this extractor never compares against an absolute
/// level, so faint print is as readable as its signal-to-noise allows.
const MIN_GRAD_AMPLITUDE: f32 = 10.0;

/// An edge must be at least this fraction of the steepest edge near it. Blur flattens
/// the edges of single-module elements to a fraction of an isolated edge's slope.
const GRAD_REL: f32 = 0.16;

/// ... and at least this many times the noise on a slope sample.
const GRAD_NOISE_K: f32 = 2.5;

/// The whole level change across an edge must be this many times the noise on a
/// difference of two samples: what keeps sensor noise from reading as bars.
const GRAD_STEP_K: f32 = 4.0;

/// A line's luminance range must be this many times its noise to hold a barcode.
const GRAD_RANGE_K: f32 = 6.0;

/// ... and at least this steep in absolute terms (luminance per pixel).
const GRAD_MIN_STEP: f32 = 2.0;

/// Two same-polarity gradient lobes at most this far apart are one edge seen through
/// a ripple (a blurred ramp with noise on it), not two edges.
const GRAD_MERGE_PX: f32 = 2.5;

/// Run extraction from gradient peaks: an edge is where the luminance changes fastest.
///
/// A threshold crossing ([`extract_runs`]) places an edge where the profile passes the
/// midpoint between two plateaus — but under blur a narrow element never reaches its
/// plateau, so its edges are placed too far apart and it steals width from its
/// neighbours; and "midpoint" needs an absolute notion of contrast that faint print
/// does not meet. The point of steepest change has neither problem: convolving a step
/// with any symmetric blur leaves its steepest point where the step was, and closely
/// spaced edges pull each other's peaks apart far less than they do their half-height
/// crossings. Each maximal same-sign lobe of the first difference is one edge, placed
/// at the centroid of its upper half; lobes weaker than the local and noise floors are
/// ignored.
fn extract_runs_grad(profile: &Profile) -> Option<Runs> {
    let noise = profile.noise;
    let profile = profile.samples.as_slice();
    let n = profile.len();
    if n < 8 {
        return None;
    }
    let (low, high) = levels(profile);
    let amplitude = high - low;
    // The spread between the darkest and brightest tenth of pure noise is ~3.5σ.
    if amplitude < MIN_GRAD_AMPLITUDE.max(GRAD_RANGE_K * noise) {
        return None;
    }
    // Forward differences: g[i] is the slope between samples i and i+1, at i + 0.5.
    let g: Vec<f32> = profile.windows(2).map(|w| w[1] - w[0]).collect();
    // A difference of two samples carries √2 times the noise of one.
    let floor = (GRAD_NOISE_K * core::f32::consts::SQRT_2 * noise).max(GRAD_MIN_STEP);
    let step_floor = (GRAD_STEP_K * core::f32::consts::SQRT_2 * noise).max(GRAD_MIN_STEP);

    // Steepest slope per block; an edge is judged against its own and adjacent blocks.
    let block = (n / 24).clamp(8, 48);
    let bmax: Vec<f32> = g
        .chunks(block)
        .map(|c| c.iter().fold(0.0f32, |m, v| m.max(v.abs())))
        .collect();
    let threshold = |i: usize| {
        let b = i / block;
        let lo = b.saturating_sub(1);
        let hi = (b + 1).min(bmax.len() - 1);
        let local = bmax[lo..=hi].iter().fold(0.0f32, |m, &v| m.max(v));
        (GRAD_REL * local).max(floor)
    };

    // One accepted lobe per edge.
    let mut edges: Vec<GradEdge> = Vec::new();
    let mut i = 0;
    while i < g.len() {
        if g[i] == 0.0 {
            i += 1;
            continue;
        }
        let rising = g[i] > 0.0;
        let start = i;
        let (mut peak, mut peak_at) = (0.0f32, i);
        while i < g.len() && g[i] != 0.0 && (g[i] > 0.0) == rising {
            if g[i].abs() > peak {
                peak = g[i].abs();
                peak_at = i;
            }
            i += 1;
        }
        // Steep enough at its peak, and a large enough step in total: a noise spike
        // is one steep sample, whereas a real edge — however blurred — changes the
        // level by the whole lobe.
        let step = (profile[i] - profile[start]).abs();
        if peak < threshold(peak_at) || step < step_floor {
            continue;
        }
        let (mut weight, mut moment) = (0.0f32, 0.0f32);
        for (k, v) in g.iter().enumerate().take(i).skip(start) {
            let a = v.abs();
            if a >= 0.5 * peak {
                weight += a;
                moment += a * (k as f32 + 0.5);
            }
        }
        let pos = moment / weight;
        match edges.last_mut() {
            Some(last) if last.rising == rising => {
                if pos - last.pos <= GRAD_MERGE_PX {
                    last.pos = (last.pos * last.weight + pos * weight) / (last.weight + weight);
                    last.weight += weight;
                    last.peak = last.peak.max(peak);
                } else if weight > last.weight {
                    // An edge of the other polarity was missed between them; the
                    // stronger of the two is the one to believe.
                    *last = GradEdge {
                        pos,
                        rising,
                        weight,
                        peak,
                    };
                }
            }
            _ => edges.push(GradEdge {
                pos,
                rising,
                weight,
                peak,
            }),
        }
    }
    let start_dark = edges.first()?.rising;
    deblur_edges(profile, &mut edges, block);
    let edges: Vec<f32> = edges.into_iter().map(|e| e.pos).collect();
    bar_bounded(edges, start_dark, n, amplitude)
}

/// One edge found by the gradient extractor.
#[derive(Debug, Clone, Copy)]
struct GradEdge {
    /// Sub-pixel position along the profile.
    pos: f32,
    /// Whether luminance rises across it (dark → light).
    rising: bool,
    /// Summed slope of the lobe's upper half (its evidence).
    weight: f32,
    /// Steepest slope in the lobe.
    peak: f32,
}

/// The standard normal cumulative distribution (Abramowitz & Stegun 7.1.26, |ε| < 2e-7).
fn normal_cdf(u: f32) -> f32 {
    let x = u * core::f32::consts::FRAC_1_SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let poly = ((((1.061_405_4 * t - 1.453_152_1) * t + 1.421_413_8) * t - 0.284_496_72) * t
        + 0.254_829_6)
        * t;
    let erf = 1.0 - poly * (-x * x).exp();
    0.5 * (1.0 + if x >= 0.0 { erf } else { -erf })
}

/// Blur (Gaussian σ, pixels) below which edges are where they appear to be.
const DEBLUR_MIN_SIGMA: f32 = 0.6;
/// Largest blur modelled; beyond it single modules are gone, not displaced.
const DEBLUR_MAX_SIGMA: f32 = 3.5;
/// Blur widths tried, as fractions of the estimate read off the sharpest edges. That
/// estimate can only be too large — an edge with a close neighbour is less steep than
/// an isolated one — and blur width trades off against the width of narrow elements,
/// so the fit that leaves the smallest residual decides.
const DEBLUR_SIGMA_TRIALS: [f32; 4] = [1.0, 0.85, 0.7, 0.55];
/// Gauss–Newton sweeps per trial blur width, and further sweeps at the chosen one.
const DEBLUR_TRIAL_SWEEPS: usize = 2;
const DEBLUR_FINAL_SWEEPS: usize = 3;
/// Half-width of the windows the levels are re-estimated over, in envelope radii.
const RELEVEL_REACH: usize = 3;
/// Modelled darkness within this of 0 or 1 counts as being on a plateau.
const RELEVEL_BAND: f32 = 0.15;
/// Samples either side of an edge its blurred step is evaluated over, in σ.
const DEBLUR_REACH: f32 = 3.5;

/// Move each edge to where it must be for a *blurred* bar pattern to reproduce the
/// profile.
///
/// Blur does not just soften an edge, it displaces the apparent position of edges that
/// have a neighbour within the blur radius: a one-module bar spreads into the spaces
/// beside it, so by any local measure — threshold crossing or steepest slope — it reads
/// wide and its neighbours narrow, by up to half a module once the blur is as wide as a
/// module. No per-edge rule can undo that, but the forward model is simple: a bar
/// pattern is a sum of steps, each blurred into a normal CDF, between a local light and
/// a local dark level. Given the edges found (which exist, even if misplaced), fit their
/// positions to the profile by least squares — each edge in turn takes the Gauss–Newton
/// step against the residual around it. The blur width starts from the sharpest edges
/// (an isolated blurred step of height `A` has peak slope `A / (σ√2π)`) and is settled
/// by which candidate width fits best.
fn deblur_edges(profile: &[f32], edges: &mut [GradEdge], radius: usize) {
    let n = profile.len();
    if edges.len() < 4 {
        return;
    }
    let (lo, hi) = sliding_range(profile, radius);
    let at = |x: f32| (x.round().max(0.0) as usize).min(n - 1);

    let mut sigmas: Vec<f32> = edges
        .iter()
        .filter_map(|e| {
            let i = at(e.pos);
            let a = hi[i] - lo[i];
            (a > 0.0 && e.peak > 0.0).then(|| a / (e.peak * SQRT_TAU))
        })
        .collect();
    if sigmas.is_empty() {
        return;
    }
    sigmas.sort_by(f32::total_cmp);
    let estimate = sigmas[sigmas.len() / 6].min(DEBLUR_MAX_SIGMA);
    if estimate < DEBLUR_MIN_SIGMA {
        return;
    }

    let start: Vec<f32> = edges.iter().map(|e| e.pos).collect();
    let signs: Vec<f32> = edges
        .iter()
        .map(|e| if e.rising { -1.0 } else { 1.0 })
        .collect();
    let mut best: Option<(f32, BlurFit<'_>)> = None;
    for scale in DEBLUR_SIGMA_TRIALS {
        let sigma = estimate * scale;
        if sigma < 0.45 {
            break;
        }
        let mut fit = BlurFit::new(
            profile,
            lo.clone(),
            hi.clone(),
            &signs,
            start.clone(),
            sigma,
        );
        for _ in 0..DEBLUR_TRIAL_SWEEPS {
            fit.sweep();
        }
        fit.relevel(radius);
        for _ in 0..DEBLUR_TRIAL_SWEEPS {
            fit.sweep();
        }
        let sse = fit.residual();
        match &best {
            // The residual has one minimum over the blur width: past it, stop.
            Some((b, _)) if sse >= *b => break,
            _ => best = Some((sse, fit)),
        }
    }
    let Some((_, mut fit)) = best else { return };
    for _ in 0..DEBLUR_FINAL_SWEEPS {
        fit.sweep();
    }
    for (e, &pos) in edges.iter_mut().zip(&fit.pos) {
        e.pos = pos;
    }
}

/// √(2π).
const SQRT_TAU: f32 = 2.506_628_3;

/// A blurred-bar-pattern model of a profile, being fitted (see [`deblur_edges`]).
struct BlurFit<'a> {
    profile: &'a [f32],
    /// Local dark and light levels per sample.
    lo: Vec<f32>,
    hi: Vec<f32>,
    /// +1 for an edge into a bar, −1 for one out of it.
    signs: &'a [f32],
    /// Edge positions.
    pos: Vec<f32>,
    sigma: f32,
    /// Modelled darkness per sample, in [0, 1]: each light→dark edge adds a blurred
    /// step, each dark→light edge removes one. Predicted luminance is hi − (hi − lo)·D.
    dark: Vec<f32>,
    /// Φ(u) tabulated over ±[`DEBLUR_REACH`] in steps of 1/16.
    cdf: Vec<f32>,
}

impl<'a> BlurFit<'a> {
    fn new(
        profile: &'a [f32],
        lo: Vec<f32>,
        hi: Vec<f32>,
        signs: &'a [f32],
        pos: Vec<f32>,
        sigma: f32,
    ) -> Self {
        let n = profile.len();
        let steps = (DEBLUR_REACH * 16.0) as i32;
        let cdf: Vec<f32> = (-steps..=steps)
            .map(|i| normal_cdf(i as f32 / 16.0))
            .collect();
        let mut fit = BlurFit {
            profile,
            lo,
            hi,
            signs,
            pos,
            sigma,
            dark: vec![0.0; n],
            cdf,
        };
        // A step well past its edge is a constant: those go in as a prefix sum, the
        // blurred part of each step is added over its window.
        let mut diff = vec![0.0f32; n + 1];
        if signs[0] < 0.0 {
            diff[0] = 1.0; // the profile starts inside a dark run
        }
        for (k, &sign) in signs.iter().enumerate() {
            let (x0, x1) = fit.window(fit.pos[k]);
            for x in x0..=x1 {
                fit.dark[x] += sign * fit.step(x as f32 - fit.pos[k]);
            }
            diff[x1 + 1] += sign;
        }
        let mut acc = 0.0f32;
        for (d, step) in fit.dark.iter_mut().zip(&diff) {
            acc += step;
            *d += acc;
        }
        fit
    }

    /// Sample range a step at `pos` is blurred over.
    fn window(&self, pos: f32) -> (usize, usize) {
        let reach = DEBLUR_REACH * self.sigma;
        let last = self.profile.len() - 1;
        (
            ((pos - reach).ceil().max(0.0) as usize).min(last),
            ((pos + reach).floor().max(0.0) as usize).min(last),
        )
    }

    /// The blurred unit step, `d` pixels past its edge: Φ(d/σ), exactly 0 and 1 beyond
    /// the reach.
    fn step(&self, d: f32) -> f32 {
        let t = (d / self.sigma + DEBLUR_REACH) * 16.0;
        if t <= 0.0 {
            return 0.0;
        }
        let i = t as usize;
        if i + 1 >= self.cdf.len() {
            return 1.0;
        }
        let f = t - i as f32;
        self.cdf[i] + f * (self.cdf[i + 1] - self.cdf[i])
    }

    /// One Gauss–Newton pass: each edge in turn steps to where it best explains the
    /// residual around it.
    fn sweep(&mut self) {
        let n = self.profile.len();
        for k in 0..self.pos.len() {
            let (e, s) = (self.pos[k], self.signs[k]);
            let (x0, x1) = self.window(e);
            let (mut num, mut den) = (0.0f32, 0.0f32);
            for x in x0..=x1 {
                let a = self.hi[x] - self.lo[x];
                let u = (x as f32 - e) / self.sigma;
                // ∂(predicted)/∂(edge position).
                let j = a * s * (-0.5 * u * u).exp() / (self.sigma * SQRT_TAU);
                let r = self.profile[x] - (self.hi[x] - a * self.dark[x]);
                num += r * j;
                den += j * j;
            }
            if den <= 1e-6 {
                continue;
            }
            let lower = if k > 0 { self.pos[k - 1] + 0.25 } else { 0.0 };
            let upper = self.pos.get(k + 1).map_or((n - 1) as f32, |p| p - 0.25);
            if lower >= upper {
                continue;
            }
            let new = (e + (num / den).clamp(-1.0, 1.0)).clamp(lower, upper);
            // The old and new steps differ only within the union of their windows.
            let (y0, y1) = (self.window(e.min(new)).0, self.window(e.max(new)).1);
            for x in y0..=y1 {
                let xf = x as f32;
                self.dark[x] += s * (self.step(xf - new) - self.step(xf - e));
            }
            self.pos[k] = new;
        }
    }

    /// Re-estimate the local light and dark levels from the model.
    ///
    /// The levels start as the profile's local maximum and minimum — but under blur
    /// only wide elements reach their level, so where the nearby bars are all narrow
    /// the dark level comes out too light, and the fit makes up for the missing depth
    /// by widening every narrow bar (depth and width of a blurred narrow element are
    /// all but interchangeable). The model knows how far short of its plateau each
    /// sample is: regress the profile on the modelled darkness, `p = light − (light −
    /// dark)·D`, over the samples that are nearly on a plateau (`D` near 0 or 1), in
    /// windows wide enough to hold wide elements of both kinds. Samples in between are
    /// left out — they are where width and depth trade off.
    fn relevel(&mut self, radius: usize) {
        let n = self.profile.len();
        let reach = RELEVEL_REACH * radius;
        // Prefix sums of the regression terms over plateau samples.
        let mut sums = vec![[0.0f64; 5]; n + 1];
        for x in 0..n {
            let d = self.dark[x];
            let mut row = sums[x];
            if !(RELEVEL_BAND..=1.0 - RELEVEL_BAND).contains(&d) {
                let (d, p) = (d as f64, self.profile[x] as f64);
                for (slot, term) in row.iter_mut().zip([1.0, d, d * d, p, p * d]) {
                    *slot += term;
                }
            }
            sums[x + 1] = row;
        }
        // Levels at block centres, then interpolated per sample.
        let centres: Vec<usize> = (0..n).step_by(radius.max(1)).collect();
        let fitted: Vec<Option<(f32, f32)>> = centres
            .iter()
            .map(|&c| {
                let (a, b) = (c.saturating_sub(reach), (c + reach + 1).min(n));
                let t: [f64; 5] = core::array::from_fn(|i| sums[b][i] - sums[a][i]);
                let [cnt, sd, sdd, sp, spd] = t;
                // p = h − amp·d  ⇒  normal equations in (h, −amp).
                let det = cnt * sdd - sd * sd;
                // Both plateaus must be represented: Var(d)·count² well above zero.
                if cnt < 8.0 || det < 0.02 * cnt * cnt {
                    return None;
                }
                let h = (sp * sdd - spd * sd) / det;
                let amp = -(cnt * spd - sd * sp) / det;
                Some((h as f32, amp as f32))
            })
            .collect();
        for x in 0..n {
            let i = (x / radius.max(1)).min(centres.len() - 1);
            let j = (i + 1).min(centres.len() - 1);
            let f = if j > i {
                (x - centres[i]) as f32 / (centres[j] - centres[i]) as f32
            } else {
                0.0
            };
            let (Some((h0, a0)), Some((h1, a1))) = (fitted[i], fitted[j]) else {
                continue;
            };
            let (h, amp) = (h0 + f * (h1 - h0), a0 + f * (a1 - a0));
            // The envelope can only have fallen short of a plateau, and not by much.
            let old = self.hi[x] - self.lo[x];
            let hi = h.clamp(self.hi[x] - 0.05 * old, self.hi[x] + 0.35 * old);
            let lo = (h - amp).clamp(self.lo[x] - 0.35 * old, self.lo[x] + 0.05 * old);
            (self.hi[x], self.lo[x]) = (hi, lo);
        }
    }

    /// Sum of squared residuals over the stretch the edges span.
    fn residual(&self) -> f32 {
        let (x0, _) = self.window(self.pos[0]);
        let (_, x1) = self.window(self.pos[self.pos.len() - 1]);
        (x0..=x1)
            .map(|x| {
                let a = self.hi[x] - self.lo[x];
                let r = self.profile[x] - (self.hi[x] - a * self.dark[x]);
                r * r
            })
            .sum()
    }
}

// --- Span segmentation -------------------------------------------------------

/// Widest a genuine symbol element may be, in units of the span's narrow run width,
/// before the run is taken for a quiet zone or background instead. The widest element
/// of any linear symbology read here is 4 modules (Code 128, Code 93, EAN/UPC); blur
/// shrinks narrow runs and swells wide ones, hence the slack. Specified quiet zones are
/// 10 modules, and ~7 in practice.
const MAX_ELEMENT_NARROWS: f32 = 5.75;

/// Narrow-run width of a run-width list: its lower-quartile value. Every symbology read
/// here has at least a quarter of its elements one module wide, and the quartile shrugs
/// off the odd noise sliver that the minimum would latch onto.
fn narrow_width(widths: &[f32]) -> f32 {
    let mut sorted = widths.to_vec();
    sorted.sort_by(f32::total_cmp);
    sorted[sorted.len() / 4]
}

/// Cut a scanline's runs into barcode-shaped spans: maximal stretches that start and
/// end on a bar and contain no run wider than a symbol element can be.
///
/// A scanline through a scene crosses the label edge, the surface behind it, text and
/// possibly several codes; treating it all as one pattern makes the module estimate and
/// every decoder fail. The over-wide runs *are* the quiet zones and background, so they
/// are exactly where to cut. "Too wide" is relative to the local narrow-run width, which
/// itself is only meaningful once unrelated content has been cut away — so the split
/// recurses, re-measuring each piece, until every span is self-consistent.
fn split_spans(runs: &Runs, min_runs: usize) -> Vec<Runs> {
    let widths: Vec<f32> = runs.edges.windows(2).map(|w| w[1] - w[0]).collect();
    let mut out = Vec::new();
    // Work list of inclusive run-index ranges, each starting and ending on a bar (even
    // index: run 0 is a bar).
    let mut work: Vec<(usize, usize)> = vec![(0, widths.len() - 1)];
    while let Some((lo, hi)) = work.pop() {
        if hi < lo || hi - lo + 1 < min_runs.max(1) {
            continue;
        }
        let limit = MAX_ELEMENT_NARROWS * narrow_width(&widths[lo..=hi]);
        // Cut at the widest offending run first; the pieces are re-measured.
        let cut = (lo..=hi)
            .filter(|&k| widths[k] > limit)
            .max_by(|&a, &b| widths[a].total_cmp(&widths[b]));
        let Some(k) = cut else {
            let lead_px = if lo == 0 {
                runs.lead_px
            } else {
                widths[lo - 1]
            };
            let trail_px = if hi + 1 == widths.len() {
                runs.trail_px
            } else {
                widths[hi + 1]
            };
            out.push(Runs {
                edges: runs.edges[lo..=hi + 1].to_vec(),
                amplitude: runs.amplitude,
                lead_px,
                trail_px,
            });
            continue;
        };
        // A light run is dropped on its own; an over-wide *dark* run is background or
        // a solid graphic, and takes its neighbouring spaces with it.
        let (left_hi, right_lo) = if k % 2 == 1 {
            (k - 1, k + 1)
        } else {
            (k.wrapping_sub(2), k + 2)
        };
        if k >= 2 || k % 2 == 1 {
            work.push((lo, left_hi));
        }
        work.push((right_lo, hi));
    }
    // Left-to-right order, for determinism.
    out.sort_by(|a, b| a.edges[0].total_cmp(&b.edges[0]));
    out
}

/// A light margin stops being quiet where it dips this fraction of the adjacent bar's
/// depth below its own brightest level...
const QUIET_DIP: f32 = 0.15;
/// ... or this many times the profile noise, whichever is more.
const QUIET_NOISE_K: f32 = 3.0;

/// How far the light margin beside a span really is quiet, in pixels (at most `limit`).
///
/// A margin is found as a wide light *run* — but a run is only as trustworthy as the
/// edge finder: a narrow bar between two wide spaces is the first element blur erases,
/// and the two spaces then merge into one run wide enough to pass for a quiet zone, in
/// the middle of a symbol. The bars on one side of it are then a *fragment* that starts
/// and ends cleanly, which a symbology without a fixed length or a check character
/// (Interleaved 2 of 5 above all) reads as a shorter symbol. The erased bar is still in
/// the signal, though, as a dip too shallow to be called an edge; a real quiet zone has
/// none. So walk outward from the span's outer `edge` (whose bar extends to `bar_end`)
/// and stop at the first sample that falls clearly below the margin's light level.
fn quiet_extent(profile: &Profile, edge: f32, bar_end: f32, limit: f32) -> f32 {
    let p = profile.samples.as_slice();
    let n = p.len() as isize;
    if n < 3 || limit.is_nan() || limit <= 0.0 {
        return 0.0;
    }
    // Three-tap mean: steadies single-sample noise without hiding a one-module dip.
    let at = |i: isize| -> f32 {
        let c = i.clamp(0, n - 1);
        (p[(c - 1).max(0) as usize] + p[c as usize] + p[(c + 1).min(n - 1) as usize]) / 3.0
    };
    let step: isize = if bar_end > edge { -1 } else { 1 };
    let first = if step < 0 {
        edge.floor() as isize
    } else {
        edge.ceil() as isize
    };
    let count = limit.ceil() as isize;
    // Light level of the margin, and the depth of the bar it borders.
    let light = (0..=count)
        .map(|k| at(first + step * k))
        .fold(f32::MIN, f32::max);
    let (b0, b1) = (edge.min(bar_end), edge.max(bar_end));
    let dark = (b0.floor() as isize..=b1.ceil() as isize)
        .map(at)
        .fold(f32::MAX, f32::min);
    let tolerance = (QUIET_DIP * (light - dark)).max(QUIET_NOISE_K * profile.noise / 1.73);
    let floor = light - tolerance;
    // The blurred shoulder of the bar's own edge comes first; allow it the bar's width
    // (blur wider than the bar would have erased it) plus a little.
    let shoulder = (b1 - b0).ceil() as isize + 2;
    let mut k = 0;
    while k <= count && k <= shoulder && at(first + step * k) < floor {
        k += 1;
    }
    if k > shoulder {
        return 0.0;
    }
    while k <= count && at(first + step * k) >= floor {
        k += 1;
    }
    ((k - 1).max(0) as f32).min(limit)
}

// --- Module quantization ---------------------------------------------------

/// A quantized pattern plus the estimated narrow-module pixel width and a
/// goodness-of-fit score in `[0, 1]`.
#[derive(Debug)]
struct Quantized {
    pattern: LinearPattern,
    module_px: f32,
    fit: f32,
}

/// Mean distance of `widths / module` from the nearest positive integer, with bars
/// narrowed and spaces widened by `bias` pixels first.
fn quantization_error(widths: &[f32], module: f32, bias: f32) -> f32 {
    let mut err = 0.0f32;
    for (idx, &w) in widths.iter().enumerate() {
        let w = if idx % 2 == 0 { w - bias } else { w + bias };
        let exact = w / module;
        err += (exact - exact.round().max(1.0)).abs();
    }
    err / widths.len() as f32
}

/// Fit the module width and bar/space bias of one span and expand each run into its
/// integer module count. Runs alternate bar/space starting with a bar.
///
/// Printing and imaging do not treat bars and spaces alike: ink spread, defocus and any
/// threshold offset move *both* edges of a bar outward (or inward) by the same amount,
/// so every bar reads `bias` pixels wide and every space `bias` narrow, whatever its
/// nominal width. At two or three pixels per module that is the difference between
/// rounding a run to one module or two, so the fit solves for it jointly with the module
/// width: minimise `Σ (wᵢ − nᵢ·m ∓ b)²` over `m, b`, alternating with re-assigning the
/// integer counts `nᵢ`.
fn quantize(runs: &Runs, min_runs: usize) -> Option<Quantized> {
    let edges = &runs.edges;
    // Inner runs are the gaps between consecutive edges.
    let inner: Vec<f32> = edges.windows(2).map(|w| w[1] - w[0]).collect();
    if inner.len() < min_runs.max(1) {
        return None;
    }

    // Seed: the candidate around the narrow-run width that quantizes most cleanly. A
    // plain minimum latches onto a single noise sliver and halves every count; a clean
    // fit also exists at every integer fraction of the true module, so the search is
    // confined to the neighbourhood of the observed narrow runs and prefers the larger
    // of near-equal fits.
    let narrow = narrow_width(&inner).max(0.5);
    let mut module = narrow;
    let mut best = f32::INFINITY;
    let mut m = narrow * 1.45;
    while m >= narrow * 0.65 {
        let e = quantization_error(&inner, m, 0.0);
        if e < best * 0.92 {
            best = e;
            module = m;
        }
        m *= 0.97;
    }

    // Joint least-squares refinement of module width and bar/space bias.
    let mut bias = 0.0f32;
    for _ in 0..6 {
        let (mut snn, mut sns, mut snw, mut ssw) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for (idx, &w) in inner.iter().enumerate() {
            let sign = if idx % 2 == 0 { 1.0 } else { -1.0 };
            let n = ((w - sign * bias) / module).round().max(1.0);
            snn += n * n;
            sns += n * sign;
            snw += n * w;
            ssw += sign * w;
        }
        let count = inner.len() as f32;
        let det = snn * count - sns * sns;
        if det.abs() < 1e-6 {
            break;
        }
        let new_module = (snw * count - sns * ssw) / det;
        let new_bias = (snn * ssw - sns * snw) / det;
        if !(new_module.is_finite() && new_module > 0.0) {
            break;
        }
        module = new_module;
        // A bias beyond half a module means the fit has slipped a whole count.
        bias = new_bias.clamp(-0.45 * module, 0.45 * module);
    }

    // Expand runs into modules and accumulate quantization error for the fit score.
    let expand = |module_at: &dyn Fn(usize) -> f32, bias: f32| -> (Vec<bool>, f32) {
        let mut modules: Vec<bool> = Vec::new();
        let mut err_sum = 0.0f32;
        for (idx, &wpx) in inner.iter().enumerate() {
            let is_bar = idx % 2 == 0;
            let exact = (if is_bar { wpx - bias } else { wpx + bias }) / module_at(idx);
            let n = exact.round().max(1.0);
            err_sum += (exact - n).abs();
            for _ in 0..(n as usize) {
                modules.push(is_bar);
            }
        }
        (modules, err_sum / (inner.len() as f32))
    };
    let (mut modules, mut mean_err) = expand(&|_| module, bias);

    // A symbol seen off-axis is foreshortened: its module pitch shrinks steadily from
    // the near end to the far one, and a single pitch then mis-counts the wide runs at
    // both ends. Refit with the pitch varying linearly along the span — `wᵢ = nᵢ·(m +
    // k·uᵢ) ± b`, `uᵢ` the run's position in [-1, 1] — and keep that reading only if it
    // explains the widths clearly better (it always fits a little better: it has one
    // more parameter to spend).
    if inner.len() >= MIN_TILT_RUNS && mean_err > TILT_MIN_ERR {
        let (first, last) = (edges[0], edges[edges.len() - 1]);
        let (mid, half) = ((first + last) / 2.0, ((last - first) / 2.0).max(1.0));
        let pos: Vec<f32> = edges
            .windows(2)
            .map(|w| ((w[0] + w[1]) / 2.0 - mid) / half)
            .collect();
        let (mut m, mut k, mut b) = (module, 0.0f32, bias);
        for _ in 0..8 {
            // Normal equations for the basis (n, n·u, ±1).
            let mut ata = [[0.0f64; 3]; 3];
            let mut atw = [0.0f64; 3];
            for (idx, &w) in inner.iter().enumerate() {
                let sign = if idx % 2 == 0 { 1.0 } else { -1.0 };
                let n = ((w - sign * b) / (m + k * pos[idx])).round().max(1.0);
                let basis = [n as f64, (n * pos[idx]) as f64, sign as f64];
                for r in 0..3 {
                    for c in 0..3 {
                        ata[r][c] += basis[r] * basis[c];
                    }
                    atw[r] += basis[r] * w as f64;
                }
            }
            let Some([nm, nk, nb]) = solve3(ata, atw) else {
                break;
            };
            let (nm, nk, nb) = (nm as f32, nk as f32, nb as f32);
            if !(nm.is_finite() && nm > 0.0 && nk.is_finite() && nb.is_finite()) {
                break;
            }
            m = nm;
            k = nk.clamp(-MAX_TILT * m, MAX_TILT * m);
            b = nb.clamp(-0.45 * m, 0.45 * m);
        }
        let (tilted, tilted_err) = expand(&|idx| m + k * pos[idx], b);
        if tilted_err < TILT_GAIN * mean_err {
            (modules, mean_err, module) = (tilted, tilted_err, m);
        }
    }
    let fit = (1.0 - 2.0 * mean_err).clamp(0.0, 1.0);

    // Quiet zone = the smaller measured light margin, in modules.
    let qz_px = runs.lead_px.min(runs.trail_px);
    let quiet_zone = (qz_px / module).round().max(0.0) as usize;

    Some(Quantized {
        pattern: LinearPattern {
            modules,
            quiet_zone,
        },
        module_px: module,
        fit,
    })
}

/// Fewest runs for which a pitch gradient is fitted: three parameters need support.
const MIN_TILT_RUNS: usize = 12;
/// Mean quantization error (modules) below which the constant-pitch fit is already
/// clean and a pitch gradient has nothing to explain.
const TILT_MIN_ERR: f32 = 0.08;
/// Largest pitch change from the span's centre to either end, as a fraction of the
/// centre pitch: ±35% covers a symbol turned some 50° away from the camera.
const MAX_TILT: f32 = 0.35;
/// The gradient fit must cut the mean quantization error to this fraction to be kept.
const TILT_GAIN: f32 = 0.7;

/// Solve the 3×3 system `a·x = b` by Cramer's rule; `None` if singular.
fn solve3(a: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det3 = |m: &[[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let det = det3(&a);
    if det.abs() < 1e-9 {
        return None;
    }
    let mut out = [0.0f64; 3];
    for (col, slot) in out.iter_mut().enumerate() {
        let mut m = a;
        for row in 0..3 {
            m[row][col] = b[row];
        }
        *slot = det3(&m) / det;
    }
    Some(out)
}

// --- Candidate assembly ----------------------------------------------------

/// Quantize one run sequence (a whole line or one span of it) into a candidate.
fn candidate_of(runs: Runs, line: &ScanLine, min_runs: usize) -> Option<LinearCandidate> {
    let q = quantize(&runs, min_runs)?;
    let amp_factor = (runs.amplitude / 128.0).clamp(0.0, 1.0);
    let confidence = (q.fit * amp_factor).clamp(0.0, 1.0);
    let location = build_location(&runs, line, q.module_px);
    Some(LinearCandidate {
        pattern: q.pattern,
        edges: runs.edges,
        lead_px: runs.lead_px,
        trail_px: runs.trail_px,
        location,
        confidence,
    })
}

/// Build a thin-band [`Location`] along the scan line spanning the bar region.
fn build_location(runs: &Runs, line: &ScanLine, module_px: f32) -> Location {
    let left = line.point(runs.edges[0]);
    let right = line.point(runs.edges[runs.edges.len() - 1]);

    // Unit perpendicular to the scan direction; +perp points "down" for a horizontal
    // line.
    let half_h = 2.0f32;
    let px = -line.sy * half_h;
    let py = line.sx * half_h;
    let outline = Quad::new([
        Point::new(left.x + px, left.y + py),
        Point::new(right.x + px, right.y + py),
        Point::new(right.x - px, right.y - py),
        Point::new(left.x - px, left.y - py),
    ]);

    Location {
        outline,
        rotation: Some(line.sy.atan2(line.sx)),
        module_size: Some(module_px),
    }
}

#[cfg(test)]
mod tests;
