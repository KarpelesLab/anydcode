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
//! 2. **Edge detection & run-lengths** (`extract_runs`). Robust dark/light
//!    levels are estimated per scanline from luminance percentiles (tolerant of
//!    noise and a bar-or-two of outliers). Transitions are located by a hysteresis
//!    state machine, then each edge is re-placed at the sub-pixel crossing of the
//!    *local* midpoint between the plateau levels of its two adjacent runs. This
//!    local refinement avoids the bias a single global threshold suffers when wide
//!    quiet zones saturate the bright level above the interior space peaks, and it
//!    keeps edge positions stable under blur. The result is a run sequence in
//!    pixels bracketed by light quiet zones at both ends.
//! 3. **Module quantization** (`quantize`). The narrow-module pixel width is
//!    estimated from the run-width distribution (seeded by the smallest run, then
//!    refined by least-squares against the assigned integer counts). Each run is
//!    expanded into that many equal `bool`s, yielding a [`LinearPattern`] plus a
//!    quiet-zone module count.
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
//! - **Framing.** The barcode need not span the full frame width, but a light
//!   quiet zone must be present on both sides of the scan line.
//!
//! Vertical extent is *not* measured (a single scan line has no height); the
//! reported [`Location`] outline is a thin band centered on the scan line.

use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::output::{Encoding, LinearPattern};
use crate::symbol::Symbol;
use crate::traits::Decode;

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
            if let Some(cand) = analyze_profile(&profile, cy, tan, deg, w, opts.min_runs) {
                found.push(cand);
            }
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
    /// Sub-pixel positions (fractional column index) of every transition.
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

/// Extract transitions from a profile using hysteresis around the mid-level, placing
/// each confirmed edge at its sub-pixel midpoint crossing. Returns `None` for flat
/// profiles or ones that do not begin and end in a light quiet zone.
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
    let thr = (low + high) / 2.0;
    let hyst = amplitude * 0.12;
    let hi = thr + hyst;
    let lo = thr - hyst;

    // The scan must start in a light margin (quiet zone), else we clipped a bar.
    let start_light = profile[0] >= thr;
    if !start_light {
        return None;
    }

    let mut edges: Vec<f32> = Vec::new();
    let mut dark = false; // start light
    let mut cand: Option<f32> = None;

    for i in 1..n {
        let a = profile[i - 1];
        let b = profile[i];
        if dark {
            // Seeking a rising edge, confirmed once the signal passes `hi`.
            if a < thr && b >= thr {
                cand = Some(crossing(i, a, b, thr));
            } else if b < thr {
                cand = None;
            }
            if b >= hi {
                edges.push(cand.take().unwrap_or((i as f32) - 0.5));
                dark = false;
            }
        } else {
            // Seeking a falling edge, confirmed once the signal drops below `lo`.
            if a > thr && b <= thr {
                cand = Some(crossing(i, a, b, thr));
            } else if b > thr {
                cand = None;
            }
            if b <= lo {
                edges.push(cand.take().unwrap_or((i as f32) - 0.5));
                dark = true;
            }
        }
    }

    // Must end in a light quiet zone: an even number of transitions.
    if dark || edges.len() < 2 || !edges.len().is_multiple_of(2) {
        return None;
    }

    // Refine edge positions against *local* levels. A single global threshold is
    // biased when wide quiet zones saturate the bright level above the interior
    // space peaks, which would widen bars and shrink spaces. Each edge is instead
    // placed at the midpoint between the plateau levels of its two adjacent runs.
    let refined = refine_edges(profile, &edges);

    let lead_px = refined[0];
    let trail_px = ((n - 1) as f32) - refined[refined.len() - 1];
    Some(Runs {
        edges: refined,
        amplitude,
        lead_px,
        trail_px,
    })
}

/// Re-place each approximate edge at the crossing of the local midpoint between the
/// plateau (min for dark, max for light) of the two runs it separates.
fn refine_edges(profile: &[f32], approx: &[f32]) -> Vec<f32> {
    let n = profile.len();
    // Run boundaries: 0, each edge, n-1. Run k spans bounds[k]..bounds[k+1];
    // run 0 is the leading light quiet zone, so even runs are light, odd are dark.
    let mut bounds = Vec::with_capacity(approx.len() + 2);
    bounds.push(0.0f32);
    bounds.extend_from_slice(approx);
    bounds.push((n - 1) as f32);

    // Plateau extreme of each run (min if dark, max if light).
    let mut ext = Vec::with_capacity(bounds.len() - 1);
    for k in 0..bounds.len() - 1 {
        let lo = (bounds[k].ceil() as usize).min(n - 1);
        let hi = (bounds[k + 1].floor() as usize).min(n - 1);
        let dark_run = k % 2 == 1;
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
    let thr = (low + high) / 2.0;
    // Must begin and end in a light quiet zone.
    if profile[0] < thr || profile[n - 1] < thr {
        return None;
    }

    // Collect extrema (position, value, kind: +1 max / -1 min) from trend reversals,
    // bracketed by virtual light peaks at both ends so the flat quiet zones (which carry
    // no detected peak of their own) still bound the first and last edge.
    let sm = smooth3(profile);
    let mut ext: Vec<(usize, f32, i8)> = vec![(0, profile[0], 1)];
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
    ext.push((n - 1, profile[n - 1], 1));

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
    if edges.len() < 2 || !edges.len().is_multiple_of(2) {
        return None;
    }

    let lead_px = edges[0];
    let trail_px = ((n - 1) as f32) - edges[edges.len() - 1];
    Some(Runs {
        edges,
        amplitude,
        lead_px,
        trail_px,
    })
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

/// Estimate the narrow-module width from the inner run widths and expand each run
/// into its integer module count. Runs alternate bar/space starting with a bar.
fn quantize(runs: &Runs, min_runs: usize) -> Option<Quantized> {
    let edges = &runs.edges;
    // Inner runs are the gaps between consecutive edges.
    let inner: Vec<f32> = edges.windows(2).map(|w| w[1] - w[0]).collect();
    if inner.len() < min_runs {
        return None;
    }

    // Seed the module estimate with the smallest run, then refine by least squares
    // against the assigned integer counts.
    let mut module = inner
        .iter()
        .cloned()
        .fold(f32::INFINITY, f32::min)
        .max(1e-3);
    for _ in 0..6 {
        let mut sum_w = 0.0f32;
        let mut sum_n = 0.0f32;
        for &wpx in &inner {
            let n = (wpx / module).round().max(1.0);
            sum_w += wpx;
            sum_n += n;
        }
        if sum_n > 0.0 {
            module = sum_w / sum_n;
        }
    }

    // Expand runs into modules and accumulate quantization error for the fit score.
    let mut modules: Vec<bool> = Vec::new();
    let mut err_sum = 0.0f32;
    for (idx, &wpx) in inner.iter().enumerate() {
        let exact = wpx / module;
        let n = exact.round().max(1.0);
        err_sum += (exact - n).abs();
        let is_bar = idx % 2 == 0;
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

/// Run the full extract -> quantize -> locate pipeline on one profile.
fn analyze_profile(
    profile: &[f32],
    cy: f32,
    tan: f32,
    deg: f32,
    width: usize,
    min_runs: usize,
) -> Option<LinearCandidate> {
    let runs = extract_runs(profile)?;
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
