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
//! - **Rotation.** A few degrees, handled by scanning at several small angles
//!   (default ±`MAX_ANGLE_DEG`). Full rotation invariance is deliberately **not**
//!   this module's job: a scan line must cross all bars, so near-perpendicular
//!   capture is assumed here. [`crate::pipeline::scan_1d`] supplies it instead, by
//!   trying every candidate pattern mirrored (180°) and — when the sweep finds
//!   nothing — measuring the crop's dominant texture orientation and derotating the
//!   whole crop before rescanning (any other angle).
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

/// Options controlling scanline extraction and candidate generation.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Number of scan positions distributed across the central band of the frame.
    pub scan_count: usize,
    /// Scan angles in degrees (`0.0` = horizontal). Small magnitudes only; a scan
    /// line must still cross every bar.
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
/// Candidates are sorted by descending [`LinearCandidate::confidence`], deduplicated
/// by identical module pattern, and truncated to [`ScanOptions::max_candidates`].
pub fn scan_lines(frame: &GrayFrame<'_>, opts: &ScanOptions) -> Vec<LinearCandidate> {
    let mut found = scan_all(frame, opts);
    dedupe(&mut found, opts.max_candidates);
    found
}

/// Scan `frame` and return **every** per-scanline span candidate, without the dedup and
/// truncation [`scan_lines`] applies.
///
/// This is what a consensus decoder wants: the same physical code is crossed by many
/// scanlines, and *how many* of them agree on a reading is the strongest evidence
/// available that the reading is real (see [`crate::pipeline::scan_1d`]). Ordered by
/// scan position, then angle, then left to right along the line.
pub fn scan_spans(frame: &GrayFrame<'_>, opts: &ScanOptions) -> Vec<LinearCandidate> {
    scan_all(frame, opts)
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
    let w = frame.width();
    let h = frame.height();
    if w < 4 || h == 0 {
        return Vec::new();
    }
    let mut found: Vec<LinearCandidate> = Vec::new();
    let count = opts.scan_count.max(1);
    for i in 0..count {
        let frac = if count == 1 {
            0.5
        } else {
            0.1 + 0.8 * (i as f32) / ((count - 1) as f32)
        };
        let cy = frac * (h.saturating_sub(1)) as f32;
        for &deg in &opts.angles_deg {
            let tan = (deg.to_radians()).tan();
            let profile = sample_profile(frame, cy, tan, opts.smooth_radius);
            for &prom in FINE_PROMINENCE {
                if let Some(cand) =
                    analyze_profile_fine(&profile, cy, tan, deg, w, opts.min_runs, prom)
                {
                    found.push(cand);
                }
            }
        }
    }
    found.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(core::cmp::Ordering::Equal)
    });
    found
}

/// Run every scan position/angle and collect the raw candidates (no dedup/sort).
fn scan_all(frame: &GrayFrame<'_>, opts: &ScanOptions) -> Vec<LinearCandidate> {
    let w = frame.width();
    let h = frame.height();
    if w < 4 || h == 0 {
        return Vec::new();
    }

    let mut found: Vec<LinearCandidate> = Vec::new();

    // Distribute scan rows across the central 10%..90% band so slightly angled
    // lines stay inside the frame.
    let count = opts.scan_count.max(1);
    for i in 0..count {
        let frac = if count == 1 {
            0.5
        } else {
            0.1 + 0.8 * (i as f32) / ((count - 1) as f32)
        };
        let cy = frac * (h.saturating_sub(1)) as f32;
        for &deg in &opts.angles_deg {
            let tan = (deg.to_radians()).tan();
            let profile = sample_profile(frame, cy, tan, opts.smooth_radius);
            found.extend(analyze_profile(&profile, cy, tan, deg, w, opts.min_runs));
        }
    }
    found
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

/// Sample one column `x` at fractional row `y` with vertical linear interpolation.
/// `y` is clamped to the valid row range.
fn sample_column(frame: &GrayFrame<'_>, x: usize, y: f32) -> f32 {
    let h = frame.height();
    let yc = y.clamp(0.0, (h - 1) as f32);
    let y0 = yc.floor() as usize;
    let y1 = (y0 + 1).min(h - 1);
    let fy = yc - (y0 as f32);
    let a = frame.get_unchecked(x, y0) as f32;
    let b = frame.get_unchecked(x, y1) as f32;
    a + (b - a) * fy
}

/// Build the luminance profile for a scan line centered at row `cy` with slope
/// `tan` (`dy/dx`), one sample per column, then optionally box-smooth it.
fn sample_profile(frame: &GrayFrame<'_>, cy: f32, tan: f32, smooth_radius: usize) -> Vec<f32> {
    let w = frame.width();
    let half_w = (w as f32) / 2.0;
    let mut profile = Vec::with_capacity(w);
    for x in 0..w {
        let y = cy + ((x as f32) - half_w) * tan;
        profile.push(sample_column(frame, x, y));
    }
    smooth(&profile, smooth_radius)
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
#[derive(Debug)]
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

/// Per-sample local threshold and hysteresis half-band for a profile.
///
/// Each sample is compared against the midpoint of the luminance range within
/// `radius` samples of it. Where that range is too small to hold a bar edge — inside a
/// quiet zone, a wide bar, a flat dark background — the nearest *informative* threshold
/// is carried over, so flat stretches are classified by the contrast next to them
/// rather than by noise. Returns `None` for a profile that is flat throughout.
fn local_thresholds(profile: &[f32], radius: usize) -> Option<Vec<(f32, f32)>> {
    let n = profile.len();
    // Sliding-window min and max via monotonic deques: O(n).
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

    let mut out: Vec<Option<(f32, f32)>> = (0..n)
        .map(|i| {
            let amp = hi[i] - lo[i];
            (amp >= MIN_AMPLITUDE).then(|| ((lo[i] + hi[i]) / 2.0, amp * 0.12))
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
fn extract_runs(profile: &[f32]) -> Option<Runs> {
    let n = profile.len();
    if n < 4 {
        return None;
    }
    let (low, high) = levels(profile);
    let amplitude = high - low;
    if amplitude < MIN_AMPLITUDE {
        return None;
    }
    // Wide enough to see past the widest symbol element at any plausible module size,
    // narrow enough to track an illumination falloff along the line.
    let radius = (n / 24).clamp(8, 48);
    let thresholds = local_thresholds(profile, radius)?;

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
    let thresholds = local_thresholds(profile, (n / 24).clamp(8, 48))?;
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
    let mut modules: Vec<bool> = Vec::new();
    let mut err_sum = 0.0f32;
    for (idx, &wpx) in inner.iter().enumerate() {
        let is_bar = idx % 2 == 0;
        let exact = (if is_bar { wpx - bias } else { wpx + bias }) / module;
        let n = exact.round().max(1.0);
        err_sum += (exact - n).abs();
        for _ in 0..(n as usize) {
            modules.push(is_bar);
        }
    }
    let mean_err = err_sum / (inner.len() as f32);
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

// --- Candidate assembly ----------------------------------------------------

/// Run the full extract -> segment -> quantize -> locate pipeline on one profile,
/// yielding one candidate per barcode-shaped span found along it.
fn analyze_profile(
    profile: &[f32],
    cy: f32,
    tan: f32,
    deg: f32,
    width: usize,
    min_runs: usize,
) -> Vec<LinearCandidate> {
    let Some(runs) = extract_runs(profile) else {
        return Vec::new();
    };
    split_spans(&runs, min_runs)
        .into_iter()
        .filter_map(|span| {
            let q = quantize(&span, min_runs)?;
            let amp_factor = (span.amplitude / 128.0).clamp(0.0, 1.0);
            let confidence = (q.fit * amp_factor).clamp(0.0, 1.0);
            let location = build_location(&span, cy, tan, deg, width, q.module_px);
            Some(LinearCandidate {
                pattern: q.pattern,
                edges: span.edges,
                lead_px: span.lead_px,
                trail_px: span.trail_px,
                location,
                confidence,
            })
        })
        .collect()
}

/// Like [`analyze_profile`] but using the fine peak-based [`extract_runs_fine`] at the
/// given prominence, so the resulting candidate's [`LinearCandidate::edges`] resolve
/// thin features. The quantized [`LinearCandidate::pattern`] is still filled (from the
/// same fine edges) so the candidate remains a drop-in for [`try_decode`].
fn analyze_profile_fine(
    profile: &[f32],
    cy: f32,
    tan: f32,
    deg: f32,
    width: usize,
    min_runs: usize,
    prom: f32,
) -> Option<LinearCandidate> {
    let runs = extract_runs_fine(profile, prom)?;
    let q = quantize(&runs, min_runs)?;

    let amp_factor = (runs.amplitude / 128.0).clamp(0.0, 1.0);
    let confidence = (q.fit * amp_factor).clamp(0.0, 1.0);

    let location = build_location(&runs, cy, tan, deg, width, q.module_px);
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
fn build_location(
    runs: &Runs,
    cy: f32,
    tan: f32,
    deg: f32,
    width: usize,
    module_px: f32,
) -> Location {
    let theta = deg.to_radians();
    let (sin, cos) = theta.sin_cos();
    let half_w = (width as f32) / 2.0;

    let x0 = runs.edges[0];
    let x1 = runs.edges[runs.edges.len() - 1];
    let point_on = |x: f32| Point::new(x, cy + (x - half_w) * tan);
    let left = point_on(x0);
    let right = point_on(x1);

    // Perpendicular to the scan direction (cos, sin); +perp points downward.
    let half_h = 2.0f32;
    let px = -sin * half_h;
    let py = cos * half_h;
    let outline = Quad::new([
        Point::new(left.x + px, left.y + py),
        Point::new(right.x + px, right.y + py),
        Point::new(right.x - px, right.y - py),
        Point::new(left.x - px, left.y - py),
    ]);

    Location {
        outline,
        rotation: Some(theta),
        module_size: Some(module_px),
    }
}

/// Sort by descending confidence, drop duplicate module patterns, and truncate.
fn dedupe(cands: &mut Vec<LinearCandidate>, max: usize) {
    cands.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(core::cmp::Ordering::Equal)
    });
    let mut kept: Vec<LinearCandidate> = Vec::new();
    for c in cands.drain(..) {
        if kept.iter().any(|k| k.pattern.modules == c.pattern.modules) {
            continue;
        }
        kept.push(c);
        if kept.len() >= max {
            break;
        }
    }
    *cands = kept;
}

#[cfg(test)]
mod tests;
