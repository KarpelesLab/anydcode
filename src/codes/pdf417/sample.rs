//! PDF417 image sampling: pixels → [`BitMatrix`] → [`Symbol`].
//!
//! The structural [`Pdf417Decoder`](super::Pdf417Decoder) consumes a clean module
//! grid. This module is the front-end that recovers such a grid from a real image of a
//! PDF417 symbol.
//!
//! PDF417 is a *stacked linear* symbology, so it has no finder patterns like QR. What
//! it does have are two full-height guard columns that are identical on every row:
//!
//! - the **start** pattern `81111113` on the far left (run ratios `[8,1,1,1,1,1,1,3]`),
//!   whose leading 8-module bar is the widest dark run anywhere in the symbol, and
//! - the **stop** pattern `711311121` on the far right (run ratios
//!   `[7,1,1,3,1,1,1,2,1]`), whose leading 7-module bar is the second widest.
//!
//! No data codeword contains a dark run wider than six modules, so these two guards are
//! unambiguous. The sampler binarizes the frame, scans it row by row, and on each row
//! locates the start pattern's left edge and the stop pattern's right edge by matching
//! those run ratios. Those per-row anchors trace the symbol's left and right edges; two
//! RANSAC line fits recover the edges (and thus the skew). The true top and bottom are
//! then found by walking the solid guard bars — the start pattern's 8-module bar and the
//! stop pattern's 7-module bar are dark on every row, so they form full-height columns
//! whose ends mark the symbol's top and bottom even where a diagonal edge clips the rest
//! of the guard pattern. The four corners are assembled into a parallelogram and drive a
//! [`Homography`] from the module grid to pixels; every module centre is sampled into a
//! [`BitMatrix`] of the symbol's exact dimensions and handed to the structural decoder,
//! whose Reed–Solomon layer repairs any residual per-module sampling errors.
//!
//! ## Robustness envelope
//!
//! The sampler targets the transforms in [`crate::transform`] for a symbol that is
//! upright or mildly rotated. The geometry model is **affine** (rotation, scale, shear),
//! recovered as a parallelogram from the two guard edges:
//!
//! - **Rotation:** any in-plane angle to about ±12°, for every symbol shape (wide/short
//!   or tall/narrow). Beyond that a horizontal scan line stops reliably crossing both
//!   guards and the left/right guard is no longer the leading corner.
//! - **Scale / module size:** down to ~4 px per module; no upper limit.
//! - **Blur:** Gaussian σ up to ~1.5 px.
//! - **Noise:** additive σ up to ~25 luminance (global Otsu is essentially immune to
//!   noise of this magnitude on a bi-level render).
//! - **Brightness / contrast:** wide range; Otsu adapts to the histogram.
//! - **Uneven lighting:** the global-Otsu pass is followed by an adaptive (Bradley
//!   local-mean) binarization fallback, so a symbol under a lighting gradient or glare
//!   that a single global threshold cannot separate still locates and decodes.
//!
//! ## Second stage: the grey-level scan-line reader
//!
//! When neither binarization yields a grid that decodes, [`scan`] falls back to a
//! reader that never forms a module grid at all (see the section comment further
//! down): it finds the guards on grey-level edges, reads scan lines along the rows,
//! decodes each symbol character from its edge-to-edge distances, and lets the lines
//! of a row vote. It is what reads a real camera frame:
//!
//! - **Perspective tilt:** the two guards may differ in length and in module pitch
//!   (30% foreshortening is asserted); each row is followed between them and the pitch
//!   tracked along it.
//! - **Module size:** 3 px per module, also when blurred (Gaussian σ 1.6 px, or 4–5 px
//!   of motion blur along the rows — beyond what edge positions survive, characters are
//!   matched as blurred profiles instead).
//! - **Contrast and noise:** a third of full contrast, or grain of σ 16, under a 2:1
//!   lighting falloff; edges are found relative to their neighbourhood, with a
//!   hysteresis scaled to the measured noise.
//! - **Skew:** a few degrees between the image rows and the symbol's, as left by a
//!   derotation on an estimated axis.
//! - **Cropping:** a guard may stand at the frame's very border, without a quiet zone;
//!   and a symbol of which the frame holds only one guard and the columns next to it
//!   is read from that end, the missing columns being left to Reed–Solomon as
//!   erasures — as far as the symbol's error-correction level reaches with three
//!   check codewords to spare.
//!
//! Out of scope, and deliberately not asserted: steep rotation (callers derotate; see
//! [`crate::pipeline::scan_stacked_at`]), multiple symbols per frame, and symbols cut
//! short at the top or bottom.

use super::Pdf417Decoder;
use super::tables::{CODEWORD_PATTERNS, ROW_HEIGHT, START_PATTERN, STOP_PATTERN};
use crate::error::{Error, Result};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::imgproc::homography::Homography;
use crate::imgproc::line::{Line, ransac_line};
use crate::imgproc::rng::Prng;
use crate::imgproc::sample::sample_bilinear;
use crate::imgproc::threshold::{adaptive_binarize_bradley, otsu_threshold};
use crate::output::{BitMatrix, Encoding};
use crate::pipeline::{Candidate, Hints};
use crate::symbol::Symbol;
use crate::symbology::Symbology;
use crate::traits::{Analyze, Decode, Detect};
use alloc::{vec, vec::Vec};

/// Quiet zone (light modules) recorded on the produced [`BitMatrix`]; matches the
/// PDF417 encoder's `Encoding::Matrix` quiet zone so an upright capture round-trips to
/// the identical matrix.
const QUIET_ZONE: usize = 2;

/// Run ratios of the start pattern (`81111113`), dark run first.
const START_RATIOS: [i32; 8] = [8, 1, 1, 1, 1, 1, 1, 3];
/// Run ratios of the stop pattern (`711311121`), dark run first and last.
const STOP_RATIOS: [i32; 9] = [7, 1, 1, 3, 1, 1, 1, 2, 1];

/// Fixed module overhead of a PDF417 row: start (17) + two row indicators (17 each) +
/// stop (18). Data columns add `17` modules each, so `width = 17·columns + 69`.
const ROW_OVERHEAD_MODULES: f32 = 69.0;

/// Per-element relative tolerance when matching a run window to a guard ratio.
const MATCH_TOLERANCE: f32 = 0.45;

/// Image-based PDF417 scanner: finds, samples and decodes a PDF417 symbol in a frame.
#[derive(Debug, Default, Clone, Copy)]
pub struct Pdf417Scanner;

impl Pdf417Scanner {
    /// A new scanner.
    pub fn new() -> Self {
        Pdf417Scanner
    }
}

impl Detect for Pdf417Scanner {
    fn detect(&self, frame: &GrayFrame<'_>, _hints: &Hints) -> Vec<Candidate> {
        match locate_any(frame) {
            Some(loc) => vec![Candidate {
                location: loc.as_location(),
                symbology: Some(Symbology::Pdf417),
                fingerprint: None,
                known: None,
            }],
            None => Vec::new(),
        }
    }
}

impl Analyze for Pdf417Scanner {
    fn analyze(&self, frame: &GrayFrame<'_>, candidate: &Candidate) -> Result<Symbol> {
        if let Some(known) = &candidate.known {
            return Ok(known.clone());
        }
        scan(frame).ok_or_else(|| Error::undecodable("PDF417 sampler could not decode the frame"))
    }
}

/// Locate, sample and structurally decode the PDF417 symbol in `frame`.
///
/// Returns `None` if no symbol is found or nothing decodes. The module grid is sampled
/// under global Otsu first (exact on clean renders), then under an adaptive local
/// binarization; if neither decodes, the grey-level scan-line reader — which copes with
/// blur, noise, low contrast, small modules and perspective — has the last word.
///
/// A small frame that yields nothing is read once more at twice the size: a symbol
/// written to a file at a pixel or two per module leaves the guard search and the
/// edge reader nothing between two edges.
pub fn scan(frame: &GrayFrame<'_>) -> Option<Symbol> {
    if let Some(sym) = scan_at_scale(frame) {
        return Some(sym);
    }
    let (w, h) = (frame.width(), frame.height());
    if w * h > UPSCALE_MAX_PIXELS || w == 0 || h == 0 {
        return None;
    }
    let mut big = vec![0u8; 4 * w * h];
    for y in 0..2 * h {
        for x in 0..2 * w {
            big[y * 2 * w + x] = frame.get_unchecked(x / 2, y / 2);
        }
    }
    let doubled = GrayFrame::new(&big, 2 * w, 2 * h).ok()?;
    let mut sym = scan_at_scale(&doubled)?;
    if let Some(location) = sym.location.as_mut() {
        // Pixel centres sit on whole coordinates, in both frames.
        location.outline = Quad::new(
            location
                .outline
                .corners
                .map(|p| Point::new((p.x - 0.5) / 2.0, (p.y - 0.5) / 2.0)),
        );
        location.module_size = location.module_size.map(|m| m / 2.0);
    }
    Some(sym)
}

/// The largest frame, in pixels, that [`scan`] reads a second time at twice the size.
const UPSCALE_MAX_PIXELS: usize = 320 * 200;

/// [`scan`] on the frame as given.
fn scan_at_scale(frame: &GrayFrame<'_>) -> Option<Symbol> {
    scan_with(frame, &Binary::from_frame(frame))
        .or_else(|| scan_with(frame, &Binary::adaptive(frame)))
        .or_else(|| scan_lines(frame))
}

/// Locate, sample and decode under one already-built binarization.
fn scan_with(frame: &GrayFrame<'_>, bin: &Binary) -> Option<Symbol> {
    let located = locate(frame, bin)?;
    let matrix = located.sample(frame);
    let mut sym = Pdf417Decoder::new()
        .decode(&Encoding::Matrix(matrix))
        .ok()?;
    // Every read says where it is: a live front-end has nothing to draw otherwise.
    sym.location.get_or_insert_with(|| located.as_location());
    Some(sym)
}

/// Locate the PDF417 symbol in `frame` and sample it to a clean [`BitMatrix`].
///
/// This is the geometry-recovery half of [`scan`], exposed so tests can assert the
/// sampled grid directly (e.g. pixel-exact recovery of an upright render). Uses the
/// global-Otsu binarization, falling back to the adaptive one only if that fails to
/// locate the guards.
pub fn sample_grid(frame: &GrayFrame<'_>) -> Option<BitMatrix> {
    locate_any(frame).map(|loc| loc.sample(frame))
}

/// Locate under global Otsu, falling back to adaptive binarization.
fn locate_any(frame: &GrayFrame<'_>) -> Option<Located> {
    locate(frame, &Binary::from_frame(frame)).or_else(|| locate(frame, &Binary::adaptive(frame)))
}

// ===================================================================================
// Binarization
// ===================================================================================

/// A binarized frame: `true` = dark module.
struct Binary {
    bits: Vec<bool>,
    width: usize,
    height: usize,
    threshold: u8,
}

impl Binary {
    fn from_frame(frame: &GrayFrame<'_>) -> Binary {
        let w = frame.width();
        let h = frame.height();
        let threshold = otsu_threshold(frame);
        let mut bits = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                bits[y * w + x] = frame.get_unchecked(x, y) <= threshold;
            }
        }
        Binary {
            bits,
            width: w,
            height: h,
            threshold,
        }
    }

    /// Adaptive (Bradley local-mean) binarization, used as a fallback when the global
    /// Otsu pass fails: it survives the lighting gradients and glare of real captures
    /// that wipe out a single global threshold. The sampling threshold stays the global
    /// Otsu value (module centres are read against it), only the guard-detection bitmap
    /// changes.
    fn adaptive(frame: &GrayFrame<'_>) -> Binary {
        let w = frame.width();
        let h = frame.height();
        let radius = (w.min(h) / 8).clamp(8, 50);
        let img = adaptive_binarize_bradley(frame, radius, 0.10);
        Binary {
            bits: img.bits().to_vec(),
            width: w,
            height: h,
            threshold: otsu_threshold(frame),
        }
    }

    #[inline]
    fn dark(&self, x: usize, y: usize) -> bool {
        self.bits[y * self.width + x]
    }
}

// ===================================================================================
// Per-row guard matching
// ===================================================================================

/// A dark/light run on one image row: `(dark, start_x, length)`.
type Run = (bool, usize, i32);

/// Run-length encode row `y` of the binary image.
fn encode_row(bin: &Binary, y: usize) -> Vec<Run> {
    let w = bin.width;
    let mut runs = Vec::new();
    let mut cur = bin.dark(0, y);
    let mut start = 0usize;
    for x in 1..w {
        let d = bin.dark(x, y);
        if d != cur {
            runs.push((cur, start, (x - start) as i32));
            cur = d;
            start = x;
        }
    }
    runs.push((cur, start, (w - start) as i32));
    runs
}

/// Match a window of consecutive run lengths against `ratios`, returning the estimated
/// module size (px) on success. Both the overall scale and each element are checked, so
/// a data codeword's narrower runs cannot masquerade as a guard.
fn match_ratios(lengths: &[i32], ratios: &[i32]) -> Option<f32> {
    debug_assert_eq!(lengths.len(), ratios.len());
    let total: i32 = lengths.iter().sum();
    let ratio_total: i32 = ratios.iter().sum();
    if total < ratio_total {
        return None; // fewer than one pixel per module
    }
    let unit = total as f32 / ratio_total as f32;
    if unit < 1.0 {
        return None;
    }
    for (&len, &r) in lengths.iter().zip(ratios) {
        let expected = r as f32 * unit;
        // Allow a per-element deviation that scales with the element and never falls
        // below half a module, so unit runs keep a usable absolute tolerance.
        let allowed = (expected * MATCH_TOLERANCE).max(unit * 0.5);
        if (len as f32 - expected).abs() > allowed {
            return None;
        }
    }
    Some(unit)
}

/// Locate the leftmost start guard on row `y`, returning its left-edge `x` (module
/// column 0) and the module size measured along the row.
fn find_start(runs: &[Run]) -> Option<(f32, f32)> {
    if runs.len() < START_RATIOS.len() {
        return None;
    }
    for i in 0..=runs.len() - START_RATIOS.len() {
        if !runs[i].0 {
            continue;
        }
        let lens: Vec<i32> = runs[i..i + START_RATIOS.len()]
            .iter()
            .map(|r| r.2)
            .collect();
        if let Some(unit) = match_ratios(&lens, &START_RATIOS) {
            return Some((runs[i].1 as f32, unit));
        }
    }
    None
}

/// Locate the rightmost stop guard on row `y`, returning its right-edge `x` (module
/// column `width`). Independent of the start guard, so a row that crosses only the right
/// edge still contributes.
fn find_stop(runs: &[Run]) -> Option<f32> {
    if runs.len() < STOP_RATIOS.len() {
        return None;
    }
    for i in (0..=runs.len() - STOP_RATIOS.len()).rev() {
        if !runs[i].0 {
            continue;
        }
        let lens: Vec<i32> = runs[i..i + STOP_RATIOS.len()].iter().map(|r| r.2).collect();
        if match_ratios(&lens, &STOP_RATIOS).is_some() {
            let last = &runs[i + STOP_RATIOS.len() - 1];
            return Some((last.1 as i32 + last.2) as f32);
        }
    }
    None
}

// ===================================================================================
// Geometry recovery
// ===================================================================================

/// A fully located symbol: the four corners plus the derived module dimensions.
struct Located {
    /// Grid corners `[TL, TR, BR, BL]` in image pixels.
    corners: [Point; 4],
    /// Symbol width in modules (`17·columns + 69`).
    grid_w: usize,
    /// Symbol height in modules (`3·rows`).
    grid_h: usize,
    threshold: u8,
    module_size: f32,
}

impl Located {
    fn as_location(&self) -> Location {
        let dx = self.corners[1].x - self.corners[0].x;
        let dy = self.corners[1].y - self.corners[0].y;
        Location {
            outline: Quad::new(self.corners),
            rotation: Some(dy.atan2(dx)),
            module_size: Some(self.module_size),
        }
    }

    /// Sample every module centre into a clean [`BitMatrix`].
    fn sample(&self, frame: &GrayFrame<'_>) -> BitMatrix {
        // Homography mapping module-grid coordinates to image pixels.
        let src = [
            Point::new(0.0, 0.0),
            Point::new(self.grid_w as f32, 0.0),
            Point::new(self.grid_w as f32, self.grid_h as f32),
            Point::new(0.0, self.grid_h as f32),
        ];
        let grid_to_img = match Homography::from_correspondences(src, self.corners) {
            Ok(h) => h,
            Err(_) => return BitMatrix::new(self.grid_w, self.grid_h, QUIET_ZONE),
        };

        let mut matrix = BitMatrix::new(self.grid_w, self.grid_h, QUIET_ZONE);
        for my in 0..self.grid_h {
            for mx in 0..self.grid_w {
                let (px, py) = grid_to_img.map_f64(mx as f64 + 0.5, my as f64 + 0.5);
                if sample_dark(frame, px, py, self.threshold) {
                    matrix.set(mx, my, true);
                }
            }
        }
        matrix
    }
}

/// Sample a 3×3 neighbourhood around `(px, py)` and decide dark vs light against the
/// binarization threshold — averaging suppresses single-pixel noise and blur ringing.
fn sample_dark(frame: &GrayFrame<'_>, px: f64, py: f64, threshold: u8) -> bool {
    let mut sum = 0.0f64;
    let mut n = 0.0f64;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let v = sample_bilinear(frame, px + f64::from(dx), py + f64::from(dy));
            sum += v;
            n += 1.0;
        }
    }
    (sum / n) <= f64::from(threshold)
}

/// Midpoint of two points.
fn midpoint(a: Point, b: Point) -> Point {
    Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
}

/// Unit direction along a line (perpendicular to its unit normal `(a, b)`).
fn down_dir(line: &Line) -> (f32, f32) {
    (-line.b, line.a)
}

/// Centroid of a point set (assumed non-empty).
fn centroid(pts: &[Point]) -> Point {
    let n = pts.len() as f32;
    Point::new(
        pts.iter().map(|p| p.x).sum::<f32>() / n,
        pts.iter().map(|p| p.y).sum::<f32>() / n,
    )
}

/// Walk from `start` along `sign · dir` (a unit vector) while the binary image stays
/// dark, tolerating short gaps, and return the signed distance to the last dark pixel.
/// Used to measure the extent of a solid guard bar (and thus the symbol height).
fn walk_bar(bin: &Binary, start: Point, dir: (f32, f32), sign: f32) -> f32 {
    let mut dist = 0.0f32;
    let mut last_dark = 0.0f32;
    let mut gap = 0;
    let limit = (bin.width + bin.height) as f32;
    while dist <= limit {
        dist += 0.5;
        let x = start.x + sign * dir.0 * dist;
        let y = start.y + sign * dir.1 * dist;
        let xi = x.round();
        let yi = y.round();
        if xi < 0.0 || yi < 0.0 || xi >= bin.width as f32 || yi >= bin.height as f32 {
            break;
        }
        if bin.dark(xi as usize, yi as usize) {
            last_dark = dist;
            gap = 0;
        } else {
            gap += 1;
            if gap > 5 {
                break; // >2.5 px of light: past the end of the bar
            }
        }
    }
    sign * last_dark
}

/// Given a fitted guard edge line and the edge's anchor points, recover the two true
/// corner endpoints by walking a solid guard bar up and down. The walk starts
/// `offset_modules` inside the symbol from the edge (to reach a solid full-height bar:
/// the start pattern's 8-bar sits ~3.5 modules right of the left edge, the stop
/// pattern's 7-bar ~14.5 modules left of the right edge), then follows the edge
/// direction. `into` orients the interior direction; `module` is the module size.
/// Returns the endpoints, projected back onto the edge line, ordered `(top, bottom)`.
fn guard_corners(
    bin: &Binary,
    line: &Line,
    pts: &[Point],
    into: Point,
    module: f32,
    offset_modules: f32,
) -> (Point, Point) {
    let c = centroid(pts);
    // Unit normal (a, b); orient it toward the symbol interior.
    let mut normal = (line.a, line.b);
    if (into.x - c.x) * normal.0 + (into.y - c.y) * normal.1 < 0.0 {
        normal = (-normal.0, -normal.1);
    }
    let off = offset_modules * module;
    let interior = Point::new(c.x + normal.0 * off, c.y + normal.1 * off);
    let dir = (-line.b, line.a);
    let t_a = walk_bar(bin, interior, dir, 1.0);
    let t_b = walk_bar(bin, interior, dir, -1.0);
    let e1 = Point::new(c.x + t_a * dir.0, c.y + t_a * dir.1);
    let e2 = Point::new(c.x + t_b * dir.0, c.y + t_b * dir.1);
    if e1.y <= e2.y { (e1, e2) } else { (e2, e1) }
}

/// Median of a slice (copies and sorts). Empty input yields `0.0`.
fn median(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    if v.len().is_multiple_of(2) {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    }
}

/// Minimum number of rows that must carry both guards for a confident location.
const MIN_ANCHOR_ROWS: usize = 6;

/// Full location pipeline for `frame` under a supplied binarization `bin`.
fn locate(frame: &GrayFrame<'_>, bin: &Binary) -> Option<Located> {
    if frame.width() < 40 || frame.height() < 9 {
        return None;
    }
    let mut left_pts = Vec::new();
    let mut right_pts = Vec::new();
    let mut start_units = Vec::new();
    for y in 0..bin.height {
        let runs = encode_row(bin, y);
        if let Some((lx, unit)) = find_start(&runs) {
            left_pts.push(Point::new(lx, y as f32 + 0.5));
            start_units.push(unit);
        }
        if let Some(rx) = find_stop(&runs) {
            right_pts.push(Point::new(rx, y as f32 + 0.5));
        }
    }
    if left_pts.len() < MIN_ANCHOR_ROWS || right_pts.len() < MIN_ANCHOR_ROWS {
        return None;
    }

    // Robustly fit the two guard edges, rejecting the occasional stray match with
    // RANSAC (seeded for reproducibility). The residual threshold is sub-module.
    let unit_along = median(&start_units);
    let ransac_thr = (unit_along * 0.6).clamp(1.5, 6.0);
    let mut rng = Prng::new(0x9E37_79B9);
    let (left_line, left_idx) = ransac_line(&left_pts, 200, ransac_thr, &mut rng)?;
    let (right_line, right_idx) = ransac_line(&right_pts, 200, ransac_thr, &mut rng)?;
    // Keep only the inlier points; a stray guard mismatch would otherwise bias the
    // corner projection (whose base is the point-cloud centroid).
    if left_idx.len() < MIN_ANCHOR_ROWS || right_idx.len() < MIN_ANCHOR_ROWS {
        return None;
    }
    let left_in: Vec<Point> = left_idx.iter().map(|&i| left_pts[i]).collect();
    let right_in: Vec<Point> = right_idx.iter().map(|&i| right_pts[i]).collect();

    // Recover all four corners by walking the solid guard bars, which reach the true
    // top/bottom even where the diagonal edges clip the full guard pattern. The start
    // pattern's 8-bar is centred ~3.5 modules right of the left edge; the stop pattern's
    // 7-bar ~14.5 modules left of the right edge.
    let interior = centroid(&[centroid(&left_in), centroid(&right_in)]);
    let (tl0, bl0) = guard_corners(bin, &left_line, &left_in, interior, unit_along, 3.5);
    let (tr0, br0) = guard_corners(bin, &right_line, &right_in, interior, unit_along, 14.5);

    // Build a clean parallelogram from the two edge midpoints and a single shared height
    // along the averaged edge direction. Per-corner walk noise (±1 px in where a bar's
    // fuzzy end is detected) would otherwise leave a slight keystone whose spurious
    // perspective bows the middle columns, so under pure rotation (an affine transform)
    // this is both exact and far less noise-sensitive than the raw corners.
    let left_mid = midpoint(tl0, bl0);
    let right_mid = midpoint(tr0, br0);
    let mut d_left = down_dir(&left_line);
    let mut d_right = down_dir(&right_line);
    // Orient both edge directions downward (increasing y).
    if d_left.1 < 0.0 {
        d_left = (-d_left.0, -d_left.1);
    }
    if d_right.1 < 0.0 {
        d_right = (-d_right.0, -d_right.1);
    }
    let mut y_axis = ((d_left.0 + d_right.0) / 2.0, (d_left.1 + d_right.1) / 2.0);
    let yl = (y_axis.0 * y_axis.0 + y_axis.1 * y_axis.1).sqrt().max(1e-6);
    y_axis = (y_axis.0 / yl, y_axis.1 / yl);
    let half_h = (tl0.distance(bl0) + tr0.distance(br0)) / 4.0;
    let tl = Point::new(
        left_mid.x - y_axis.0 * half_h,
        left_mid.y - y_axis.1 * half_h,
    );
    let bl = Point::new(
        left_mid.x + y_axis.0 * half_h,
        left_mid.y + y_axis.1 * half_h,
    );
    let tr = Point::new(
        right_mid.x - y_axis.0 * half_h,
        right_mid.y - y_axis.1 * half_h,
    );
    let br = Point::new(
        right_mid.x + y_axis.0 * half_h,
        right_mid.y + y_axis.1 * half_h,
    );

    // Column count from geometry. The guards measure the module size along each scan row
    // (foreshortened by tilt), so the true module size is that times the cosine of the
    // edges' tilt from vertical (`|a|` of each edge's unit direction). This needs no row
    // to cross both guards, which a wide symbol under rotation rarely offers.
    let width_px = left_mid.distance(right_mid);
    let cos_tilt = (left_line.a.abs() + right_line.a.abs()) / 2.0;
    let module_guard = (unit_along * cos_tilt).max(0.5);
    let width_modules = width_px / module_guard;
    let columns = (((width_modules - ROW_OVERHEAD_MODULES) / 17.0).round() as i64).clamp(1, 30);
    let grid_w = 17 * columns as usize + 69;

    // True module size for the row count, refined from the snapped grid width.
    let module_size = width_px / grid_w as f32;
    if module_size < 1.0 {
        return None;
    }
    let rows = (2.0 * half_h / module_size / ROW_HEIGHT as f32).round() as i64;
    let rows = rows.clamp(3, 90);
    let grid_h = ROW_HEIGHT * rows as usize;

    Some(Located {
        corners: [tl, tr, br, bl],
        grid_w,
        grid_h,
        threshold: bin.threshold,
        module_size,
    })
}

// ===================================================================================
// Grey-level scan-line reader (low-quality captures)
// ===================================================================================
//
// The module-grid sampler above needs a clean binarization and an affine symbol. The
// reader below is the fallback for everything else — soft focus, motion blur, low
// contrast, sensor grain, perspective foreshortening, small modules. It follows the
// reference decode algorithm of ISO/IEC 15438 rather than sampling module centres:
//
// 1. **Edges, not thresholds.** A scan line is turned into bar/space edges by pairing
//    its local extrema (with a noise-scaled hysteresis) and placing each edge where
//    the profile crosses the midpoint between a bar and the light level beside it, at
//    sub-pixel precision. That tracks a lighting gradient and survives blur that
//    leaves a one-module bar as a mere dip.
// 2. **Guards.** The start and stop patterns are matched on those edge runs on every
//    other image row; a RANSAC line through each family gives the symbol's left and
//    right edges, and each guard's wide bar is followed up and down to its ends. The
//    four corners need not form a parallelogram, so a foreshortened symbol is
//    followed as it is.
// 3. **Scan lines along the rows.** One line per pixel of symbol height is read from
//    the left edge to the corresponding point of the right edge, averaged with its
//    neighbours inside the row height to beat the noise down.
// 4. **Codewords from edge-to-edge distances.** Each line is cut into 17-module symbol
//    characters by walking from the start pattern at the locally measured pitch — so
//    the pitch may change along the row — and each character is decoded from the six
//    sums of adjacent element widths, normalised to its own width. Those distances
//    run between like edges, so ink spread and symmetric blur cancel.
// 5. **Voting and cross-checks.** A character's cluster says which row (mod 3) it was
//    read from; together with the row indicators that places every reading in the
//    codeword grid, where the lines covering one row vote. The symbol's size and
//    error-correction level come from the row indicators and must agree with the
//    geometry, and Reed–Solomon must then accept the grid at exactly that level.
// 6. **Template matching.** If it does not, blur has moved the edges themselves; the
//    rows are then re-read by comparing each character's profile with every character
//    of its cluster, blurred as the symbol is (see that section below).

/// A bar/space transition on a scan line.
#[derive(Debug, Clone, Copy)]
struct Edge {
    /// Position along the line, in samples (sub-sample precision).
    pos: f32,
    /// `true` for a light→dark transition (the start of a bar).
    dark: bool,
}

/// One decoded symbol character: its cluster (`0..3`, i.e. row mod 3), codeword value
/// and a confidence weight.
#[derive(Debug, Clone, Copy)]
struct Cw {
    cluster: u8,
    value: u16,
    weight: f32,
}

/// Position of the edge between the extrema of `p` at indices `a < b`, which differ
/// by at least `hyst`: where the profile crosses the midpoint between the valley — the
/// bar — and the light level next to it.
///
/// That light level is normally the peak itself. But the light side of an edge is not
/// always a plateau at one level: past a symbol's last bar the peak is wherever the
/// page is brightest, and a blank canvas around a dim crop turns the fall into its
/// first bar into a staircase. So if, going out from the valley, the profile settles
/// on a shelf well short of the peak, the shelf is the level the bar's edge rises to.
fn crossing(p: &[f32], a: usize, b: usize, hyst: f32) -> f32 {
    /// Samples a shelf must hold its level for.
    const SHELF: usize = 4;
    /// Least rise of a shelf above the bar, as a share of the whole swing: a wide bar
    /// is not evenly black, and the lighter stretch of its ink is not the page.
    const SHELF_RISE: f32 = 0.15;
    let rising = p[b] > p[a];
    let n = b - a;
    // The profile seen from the valley outwards.
    let v = |k: usize| if rising { p[a + k] } else { p[b - k] };
    let (dark, peak) = (v(0), v(n));
    let (mut light, mut end) = (peak, n);
    for k in 1..(n + 1).saturating_sub(SHELF) {
        let (mut lo, mut hi, mut sum) = (f32::MAX, f32::MIN, 0f32);
        for j in k..k + SHELF {
            lo = lo.min(v(j));
            hi = hi.max(v(j));
            sum += v(j);
        }
        if lo - dark >= hyst.max(SHELF_RISE * (peak - dark)) && hi - lo <= 0.75 * hyst {
            let level = sum / SHELF as f32;
            if peak - level >= hyst {
                (light, end) = (level, k + SHELF - 1);
            }
            break;
        }
    }
    let mid = (dark + light) / 2.0;
    let mut best = end as f32 / 2.0;
    let mut best_step = -1.0f32;
    for k in 0..end {
        let (v0, v1) = (v(k), v(k + 1));
        // Noise on a plateau can graze the midpoint; the real edge is the steepest.
        if v0 <= mid && v1 > mid && v1 - v0 > best_step {
            best_step = v1 - v0;
            best = k as f32 + (mid - v0) / (v1 - v0);
        }
    }
    if rising {
        a as f32 + best
    } else {
        b as f32 - best
    }
}

/// Extract the bar/space edges of profile `p` (luminance per sample) into `out`.
///
/// Extrema are confirmed once the profile has moved `hyst` away from them, so ripples
/// smaller than `hyst` (noise) are ignored while any larger swing — however far from a
/// global threshold — yields an edge.
fn find_edges(p: &[f32], hyst: f32, out: &mut Vec<Edge>) {
    out.clear();
    if p.len() < 3 {
        return;
    }
    let (mut hi, mut hi_i, mut lo, mut lo_i) = (p[0], 0usize, p[0], 0usize);
    let mut dir = 0i8; // +1: climbing towards a peak, -1: falling towards a valley
    let mut prev: Option<usize> = None;
    for (i, &v) in p.iter().enumerate().skip(1) {
        if dir >= 0 && v > hi {
            hi = v;
            hi_i = i;
        }
        if dir <= 0 && v < lo {
            lo = v;
            lo_i = i;
        }
        if dir >= 0 && hi - v >= hyst {
            if let Some(pi) = prev {
                out.push(Edge {
                    pos: crossing(p, pi, hi_i, hyst),
                    dark: false,
                });
            }
            prev = Some(hi_i);
            dir = -1;
            lo = v;
            lo_i = i;
        } else if dir <= 0 && v - lo >= hyst {
            if let Some(pi) = prev {
                out.push(Edge {
                    pos: crossing(p, pi, lo_i, hyst),
                    dark: true,
                });
            }
            prev = Some(lo_i);
            dir = 1;
            hi = v;
            hi_i = i;
        }
    }
    // The last swing has no later reversal to confirm it; accept it on its own size.
    if let Some(pi) = prev {
        if dir > 0 && hi - p[pi] >= hyst && hi_i > pi {
            out.push(Edge {
                pos: crossing(p, pi, hi_i, hyst),
                dark: false,
            });
        } else if dir < 0 && p[pi] - lo >= hyst && lo_i > pi {
            out.push(Edge {
                pos: crossing(p, pi, lo_i, hyst),
                dark: true,
            });
        }
    }
}

/// [`match_ratios`] for sub-pixel run widths, with a caller-chosen absolute tolerance
/// floor (in modules) — blur fattens a one-module element at its neighbours' expense.
fn match_ratios_f(widths: &[f32], ratios: &[i32], floor: f32) -> Option<f32> {
    let total: f32 = widths.iter().sum();
    let ratio_total: i32 = ratios.iter().sum();
    let unit = total / ratio_total as f32;
    if unit.is_nan() || unit < 1.0 {
        return None;
    }
    for (&len, &r) in widths.iter().zip(ratios) {
        let expected = r as f32 * unit;
        let allowed = (expected * MATCH_TOLERANCE).max(unit * floor);
        if (len - expected).abs() > allowed {
            return None;
        }
    }
    Some(unit)
}

/// Sensor noise as the edge detector needs it: the per-pixel standard deviation and
/// the correlation between vertically adjacent pixels.
///
/// Noise is taken from *vertical* second differences — the bars of an upright stacked
/// symbol run vertically, so those see the noise and almost none of the pattern — at
/// two spacings. Pixels two apart are all but independent even after a resampling
/// (a derotated crop), so that spacing gives the variance; adjacent pixels then give
/// their correlation, which decides how much averaging neighbouring lines really buys.
#[derive(Debug, Clone, Copy, Default)]
struct Noise {
    sigma: f32,
    rho: f32,
}

impl Noise {
    /// From the same quantile of `|a − 2b + c|` over adjacent (`near`) and
    /// two-apart (`far`) triples; `z` is that quantile of a unit half-normal.
    fn from_quantiles(near: f32, far: f32, z: f32) -> Noise {
        // A second difference of independent samples has variance 6σ²; of neighbours
        // correlated by ρ (and uncorrelated beyond), (6 − 8ρ)σ².
        let var = (far / z).powi(2) / 6.0;
        if var < 1e-3 {
            return Noise::default();
        }
        let rho = ((6.0 - (near / z).powi(2) / var) / 8.0).clamp(0.0, 0.75);
        Noise {
            sigma: var.sqrt(),
            rho,
        }
    }

    /// Standard deviation left after averaging `taps` adjacent lines.
    fn averaged(&self, taps: usize) -> f32 {
        let n = taps as f32;
        self.sigma * (n + 2.0 * (n - 1.0) * self.rho).sqrt() / n
    }

    /// The noisier of two estimates.
    fn max(self, other: Noise) -> Noise {
        if other.averaged(3) > self.averaged(3) {
            other
        } else {
            self
        }
    }
}

/// Median of a unit half-normal, and its upper quartile.
const HALF_NORMAL_MEDIAN: f32 = 0.6745;
const HALF_NORMAL_Q75: f32 = 1.1503;

/// Noise and luminance range (2nd to 98th percentile) of `frame`, from a sparse sample.
fn frame_stats(frame: &GrayFrame<'_>) -> (Noise, f32) {
    let (w, h) = (frame.width(), frame.height());
    let mut values = [0u32; 256];
    let mut near = [0u32; 256];
    let mut far = [0u32; 256];
    let mut n = 0u32;
    for y in (2..h.saturating_sub(2)).step_by(2) {
        let (Some(r0), Some(r1), Some(r2), Some(r3), Some(r4)) = (
            frame.row(y - 2),
            frame.row(y - 1),
            frame.row(y),
            frame.row(y + 1),
            frame.row(y + 2),
        ) else {
            continue;
        };
        for x in (0..w).step_by(2) {
            let c = 2 * i32::from(r2[x]);
            let d1 = (i32::from(r1[x]) - c + i32::from(r3[x])).unsigned_abs();
            let d2 = (i32::from(r0[x]) - c + i32::from(r4[x])).unsigned_abs();
            near[d1.min(255) as usize] += 1;
            far[d2.min(255) as usize] += 1;
            values[r2[x] as usize] += 1;
            n += 1;
        }
    }
    let noise = Noise::from_quantiles(
        quantile(&near, n, 0.5),
        quantile(&far, n, 0.5),
        HALF_NORMAL_MEDIAN,
    );
    let range = quantile(&values, n, 0.98) - quantile(&values, n, 0.02);
    (noise, range)
}

/// The `q`-quantile of a histogram holding `n` samples.
fn quantile(hist: &[u32; 256], n: u32, q: f32) -> f32 {
    if n == 0 {
        return 0.0;
    }
    let target = (n as f32 * q) as u32;
    let mut acc = 0u32;
    for (v, &c) in hist.iter().enumerate() {
        acc += c;
        if acc > target {
            return v as f32;
        }
    }
    255.0
}

/// Edge hysteresis for a profile whose noise has standard deviation `sigma`, in a
/// frame of luminance range `range`.
fn hysteresis(sigma: f32, range: f32) -> f32 {
    (3.0 * sigma).max(0.05 * range).max(5.0)
}

/// One guard column, as found by the grey-level reader.
#[derive(Debug, Clone, Copy)]
struct Guard {
    /// Ends of the pattern's outer edge.
    top: Point,
    bottom: Point,
    /// Module pitch measured along image rows.
    unit: f32,
}

/// Which guard a symbol is read from when only one is in the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Start,
    Stop,
}

/// Fill in the samples of `line` that lie outside the frame, given that
/// `inside` (`first..end`) are those within it.
///
/// What is beyond the frame is taken to continue its border pixel: the quiet zone goes
/// on. But a symbol may be cropped to its very edge, the frame ending on a guard's
/// bar with no quiet zone at all — and a bar that never begins has no edge to be found
/// by. So where the border is dark (nearer the line's darkest sample than its
/// lightest), what lies beyond it is taken to be as light as the line gets.
fn beyond_frame(line: &mut [f32], first: usize, end: usize) {
    if first >= end {
        return;
    }
    let (lo, hi) = line[first..end]
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    let fill = |border: f32| if border < (lo + hi) / 2.0 { hi } else { border };
    let (left, right) = (fill(line[first]), fill(line[end - 1]));
    line[..first].fill(left);
    line[end..].fill(right);
}

/// The two guard columns of a symbol, as found by the grey-level reader.
struct Guards {
    /// Ends of the start pattern's outer (left) edge.
    tl: Point,
    bl: Point,
    /// Ends of the stop pattern's outer (right) edge.
    tr: Point,
    br: Point,
    /// Module pitch measured along image rows at each guard.
    unit_l: f32,
    unit_r: f32,
}

/// Minimum number of anchor rows (taken every other image row) supporting a guard.
const MIN_GREY_ANCHORS: usize = 5;

/// Fit one guard edge through its per-row anchors; returns its two ends and the median
/// module pitch (along image rows) of the rows supporting it.
///
/// `inward` is `+1` for the start guard (the symbol lies to its right) and `-1` for the
/// stop guard; `bar` is how far inside the edge, in modules, the centre of the guard's
/// wide bar lies, and `space` the centre of its three-module space — the light level
/// to judge the bar against where the frame ends at the guard and shows no quiet zone.
fn fit_guard(
    frame: &GrayFrame<'_>,
    anchors: &[(Point, f32)],
    inward: f32,
    bar: f32,
    space: f32,
    rng: &mut Prng,
) -> Option<Guard> {
    if anchors.len() < MIN_GREY_ANCHORS {
        return None;
    }
    let pts: Vec<Point> = anchors.iter().map(|a| a.0).collect();
    let units: Vec<f32> = anchors.iter().map(|a| a.1).collect();
    let thr = (median(&units) * 0.6).clamp(1.5, 6.0);
    let (line, inliers) = ransac_line(&pts, 200, thr, rng)?;
    // The edge must be steep enough to be crossed by image rows.
    if inliers.len() < MIN_GREY_ANCHORS || line.a.abs() < 0.35 {
        return None;
    }
    let unit = median(&inliers.iter().map(|&i| units[i]).collect::<Vec<_>>());
    // Take the densest unbroken stretch of supporting rows as the core of the guard: a
    // stray match elsewhere on the same line (text above or below the symbol) must not
    // stretch it.
    let gap = (4.0 * unit).max(6.0);
    let mut ys: Vec<f32> = inliers.iter().map(|&i| pts[i].y).collect();
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let (mut best, mut start) = ((0usize, 0usize), 0usize);
    for i in 1..=ys.len() {
        if i == ys.len() || ys[i] - ys[i - 1] > gap {
            if i - start > best.1 - best.0 {
                best = (start, i);
            }
            start = i;
        }
    }
    if best.1 - best.0 < MIN_GREY_ANCHORS {
        return None;
    }
    let core = &ys[best.0..best.1];
    let x_at = |y: f32| -(line.b * y + line.c) / line.a;

    // The fine elements of the pattern may be lost to blur on some rows, but its wide
    // bar is not: follow that bar up and down from the core to the guard's true ends.
    // "Dark" is judged against the bar and the quiet zone beside it on the core rows.
    let bar_at = |y: f32| {
        sample_bilinear(
            frame,
            f64::from(x_at(y) + inward * bar * unit),
            f64::from(y),
        ) as f32
    };
    let quiet_at = |y: f32| {
        let outside = x_at(y) - inward * 1.5 * unit;
        let x = if outside < 0.0 || outside > (frame.width() - 1) as f32 {
            x_at(y) + inward * space * unit
        } else {
            outside
        };
        sample_bilinear(frame, f64::from(x), f64::from(y)) as f32
    };
    let dark = median(&core.iter().map(|&y| bar_at(y)).collect::<Vec<_>>());
    let light = median(&core.iter().map(|&y| quiet_at(y)).collect::<Vec<_>>());
    if light - dark < 8.0 {
        return None;
    }
    let threshold = (dark + light) / 2.0;
    let walk = |from: f32, step: f32| {
        let (mut y, mut end, mut misses) = (from, from, 0);
        loop {
            y += step;
            if y < 0.0 || y > (frame.height() - 1) as f32 {
                break;
            }
            if bar_at(y) < threshold {
                end = y;
                misses = 0;
            } else {
                misses += 1;
                if misses > 1 {
                    break;
                }
            }
        }
        end
    };
    let top = walk(core[0], -1.0);
    let bottom = walk(core[core.len() - 1], 1.0);
    Some(Guard {
        top: Point::new(x_at(top), top),
        bottom: Point::new(x_at(bottom), bottom),
        unit,
    })
}

/// Find the start and the stop guard on the grey-level image, each on its own.
fn find_guards(frame: &GrayFrame<'_>, noise: Noise, range: f32) -> (Option<Guard>, Option<Guard>) {
    let (w, h) = (frame.width(), frame.height());
    if w < 40 || h < 9 {
        return (None, None);
    }
    // The profile has a few samples of margin either side of the image row, for a
    // guard that ends at the frame's border (see `beyond_frame`).
    const PAD: usize = 4;
    let mut profile = vec![0f32; w + 2 * PAD];
    let mut near = [0u32; 256];
    let mut far = [0u32; 256];
    let mut edges: Vec<Edge> = Vec::new();
    let mut starts: Vec<(Point, f32)> = Vec::new();
    let mut stops: Vec<(Point, f32)> = Vec::new();
    let mut widths = [0f32; 9];
    for y in (2..h - 2).step_by(2) {
        let (Some(r0), Some(r1), Some(r2), Some(r3), Some(r4)) = (
            frame.row(y - 2),
            frame.row(y - 1),
            frame.row(y),
            frame.row(y + 1),
            frame.row(y + 2),
        ) else {
            continue;
        };
        // Noise is judged per row as well as per frame: a crop derotated onto a blank
        // canvas, or a frame that is mostly flat backdrop, would otherwise hide the
        // grain of the rows that cross the symbol. The upper quartile holds as long as
        // a quarter of the row is on the symbol.
        near.fill(0);
        far.fill(0);
        for (x, p) in profile[PAD..PAD + w].iter_mut().enumerate() {
            *p = (u16::from(r1[x]) + u16::from(r2[x]) + u16::from(r3[x])) as f32 / 3.0;
        }
        beyond_frame(&mut profile, PAD, PAD + w);
        for x in (0..w).step_by(2) {
            let centre = 2 * i32::from(r2[x]);
            let tight = i32::from(r1[x]) - centre + i32::from(r3[x]);
            let wide = i32::from(r0[x]) - centre + i32::from(r4[x]);
            near[tight.unsigned_abs().min(255) as usize] += 1;
            far[wide.unsigned_abs().min(255) as usize] += 1;
        }
        let sampled = w.div_ceil(2) as u32;
        let row_noise = Noise::from_quantiles(
            quantile(&near, sampled, 0.75),
            quantile(&far, sampled, 0.75),
            HALF_NORMAL_Q75,
        );
        let hyst = hysteresis(noise.max(row_noise).averaged(3), range);
        find_edges(&profile, hyst, &mut edges);
        for i in 0..edges.len() {
            if !edges[i].dark {
                continue;
            }
            if i + START_RATIOS.len() < edges.len() {
                for (k, wd) in widths[..START_RATIOS.len()].iter_mut().enumerate() {
                    *wd = edges[i + k + 1].pos - edges[i + k].pos;
                }
                if let Some(unit) =
                    match_ratios_f(&widths[..START_RATIOS.len()], &START_RATIOS, 0.6)
                {
                    starts.push((Point::new(edges[i].pos - PAD as f32, y as f32), unit));
                }
            }
            if i + STOP_RATIOS.len() < edges.len() {
                for (k, wd) in widths.iter_mut().enumerate() {
                    *wd = edges[i + k + 1].pos - edges[i + k].pos;
                }
                if let Some(unit) = match_ratios_f(&widths, &STOP_RATIOS, 0.6) {
                    let end = edges[i + STOP_RATIOS.len()].pos - PAD as f32;
                    stops.push((Point::new(end, y as f32), unit));
                }
            }
        }
    }
    let mut rng = Prng::new(0x9E37_79B9);
    // The start pattern's 8-module bar is centred 4 modules inside the left edge, the
    // stop pattern's 7-module bar 14.5 modules inside the right one.
    // (Their three-module spaces: 15.5 modules inside the left edge, 7.5 the right.)
    let start = fit_guard(frame, &starts, 1.0, 4.0, 15.5, &mut rng);
    let stop = fit_guard(frame, &stops, -1.0, 14.5, 7.5, &mut rng);
    (start, stop)
}

/// The start and stop guards as one symbol's, if they can be.
fn pair_guards(start: &Guard, stop: &Guard) -> Option<Guards> {
    let (tl, bl, unit_l) = (start.top, start.bottom, start.unit);
    let (tr, br, unit_r) = (stop.top, stop.bottom, stop.unit);
    // The pair must describe one plausible symbol: start left of stop, comparable
    // heights and pitches, and a width that holds at least one data column.
    let (hl, hr) = (tl.distance(bl), tr.distance(br));
    let width = midpoint(tl, bl).distance(midpoint(tr, br));
    let unit = (unit_l + unit_r) / 2.0;
    let modules = width / unit;
    let similar = |a: f32, b: f32| a <= 2.5 * b && b <= 2.5 * a;
    if tl.x >= tr.x
        || bl.x >= br.x
        || !similar(hl, hr)
        || !similar(unit_l, unit_r)
        || !(70.0..=640.0).contains(&modules)
    {
        return None;
    }
    Some(Guards {
        tl,
        bl,
        tr,
        br,
        unit_l,
        unit_r,
    })
}

/// Element widths (bar, space, …; eight of them) of a 17-module codeword pattern.
fn pattern_widths(pattern: u32) -> [u8; 8] {
    let mut widths = [0u8; 8];
    let mut k = 0usize;
    let mut prev = true;
    for bit in (0..17).rev() {
        let dark = (pattern >> bit) & 1 != 0;
        if dark != prev {
            k += 1;
            prev = dark;
        }
        if k < 8 {
            widths[k] += 1;
        }
    }
    widths
}

/// Pack six edge-to-edge distances (in modules) into a lookup key.
fn e_key(e: &[u8; 6]) -> u32 {
    e.iter().fold(0u32, |k, &v| (k << 4) | u32::from(v & 15))
}

/// The reference decode table: edge-to-edge key → `cluster · 929 + codeword`, sorted
/// by key. The six distances identify a symbol character uniquely across all three
/// clusters (asserted by a unit test).
fn build_e_table() -> Vec<(u32, u16)> {
    let mut table = Vec::with_capacity(3 * 929);
    for (cluster, patterns) in CODEWORD_PATTERNS.iter().enumerate() {
        for (value, &pattern) in patterns.iter().enumerate() {
            let w = pattern_widths(pattern);
            let mut e = [0u8; 6];
            for i in 0..6 {
                e[i] = w[i] + w[i + 1];
            }
            table.push((e_key(&e), (cluster * 929 + value) as u16));
        }
    }
    table.sort_unstable();
    table
}

/// Decode one symbol character from its nine bounding edges. `drift` is the relative
/// change of the module pitch across the character (0 for a fronto-parallel symbol;
/// negative where perspective makes the modules shrink along the line).
fn decode_cell(edges: &[Edge], drift: f32, table: &[(u32, u16)]) -> Option<Cw> {
    debug_assert_eq!(edges.len(), 9);
    let total = edges[8].pos - edges[0].pos;
    if total.is_nan() || total <= 0.0 {
        return None;
    }
    // Position within the character in modules: with the pitch varying linearly from
    // `u` to `u·(1 + drift)`, the module coordinate is the integral of `1 / pitch`.
    let module_at = |pos: f32| {
        let f = (pos - edges[0].pos) / total;
        17.0 * (f - 0.5 * drift * f * f) / (1.0 - 0.5 * drift)
    };
    // Edge-to-edge distances in modules, each with its nearest integer and — when it
    // sits near a rounding boundary — the integer on the other side.
    let mut exact = [0f32; 6];
    let mut near = [0i32; 6];
    let mut other = [0i32; 6];
    for i in 0..6 {
        exact[i] = module_at(edges[i + 2].pos) - module_at(edges[i].pos);
        near[i] = exact[i].round() as i32;
        let dev = exact[i] - near[i] as f32;
        other[i] = if dev.abs() > 0.25 {
            near[i] + if dev > 0.0 { 1 } else { -1 }
        } else {
            near[i]
        };
    }
    let mut best: Option<(f32, u16, bool)> = None;
    'combo: for mask in 0u32..64 {
        let mut e = [0u8; 6];
        let mut cost = 0f32;
        for i in 0..6 {
            let flipped = mask & (1 << i) != 0;
            if flipped && other[i] == near[i] {
                continue 'combo;
            }
            let v = if flipped { other[i] } else { near[i] };
            if !(2..=12).contains(&v) {
                continue 'combo;
            }
            e[i] = v as u8;
            cost += (exact[i] - v as f32).powi(2);
        }
        if best.is_some_and(|b| b.0 <= cost) {
            continue;
        }
        let key = e_key(&e);
        if let Ok(idx) = table.binary_search_by_key(&key, |t| t.0) {
            best = Some((cost, table[idx].1, mask == 0));
        }
    }
    best.map(|(_, packed, clean)| Cw {
        cluster: (packed / 929) as u8,
        value: packed % 929,
        weight: if clean { 1.0 } else { 0.5 },
    })
}

/// Index of the light→dark edge nearest `target`, if one lies within `reach`.
fn nearest_bar_start(edges: &[Edge], target: f32, reach: f32) -> Option<usize> {
    let first = edges.partition_point(|e| e.pos < target - reach);
    let mut best: Option<(f32, usize)> = None;
    for (i, e) in edges.iter().enumerate().skip(first) {
        if e.pos > target + reach {
            break;
        }
        let d = (e.pos - target).abs();
        if e.dark && best.is_none_or(|b| d < b.0) {
            best = Some((d, i));
        }
    }
    best.map(|b| b.1)
}

/// Most symbol characters a row can hold between the guards: 30 data columns plus the
/// two row indicators.
const MAX_ROW_CELLS: usize = 32;

/// Where the symbol lies on a scan line, in samples along it.
#[derive(Debug, Clone, Copy)]
struct LineSpan {
    /// Positions of the symbol's outer edges (start of the start pattern, end of the
    /// stop pattern).
    x_l: f32,
    x_r: f32,
    /// Module pitch at the two guards.
    unit_l: f32,
    unit_r: f32,
}

impl LineSpan {
    /// Relative change of the module pitch per sample at `pos`. Under perspective the
    /// pitch changes along the row; a projective map makes `√pitch` linear in position,
    /// so the two guard pitches give `d(ln pitch)/dx = 2·d(√pitch)/dx / √pitch`.
    fn growth(&self, pos: f32) -> f32 {
        let (root_l, root_r) = (self.unit_l.sqrt(), self.unit_r.sqrt());
        let span = (self.x_r - self.x_l).max(1.0);
        let root = root_l + (root_r - root_l) * ((pos - self.x_l) / span).clamp(0.0, 1.0);
        2.0 * (root_r - root_l) / (span * root)
    }
}

/// Read the symbol characters of one scan line: left row indicator, data columns, right
/// row indicator.
///
/// The line is walked a character at a time from the start pattern. Every character
/// begins with a bar, so each boundary is snapped to the bar start nearest its expected
/// position, and the pitch follows the characters that read cleanly — a damaged
/// character costs only itself. Where perspective makes the pitch change along the row,
/// [`LineSpan::growth`] predicts each character's width from the last.
///
/// `bounds`, if given, receives the position of every character boundary: the start of
/// each character read and the end of the last.
fn read_line(
    edges: &[Edge],
    span: &LineSpan,
    table: &[(u32, u16)],
    mut bounds: Option<&mut Vec<f32>>,
) -> Vec<Option<Cw>> {
    let mut cells = Vec::new();
    if let Some(b) = bounds.as_deref_mut() {
        b.clear();
    }
    let (x_r, unit_l, unit_r) = (span.x_r, span.unit_l, span.unit_r);
    let Some(first) = nearest_bar_start(edges, span.x_l, 2.0 * unit_l) else {
        return cells;
    };
    let mut pos = edges[first].pos;
    let mut idx = Some(first);
    let mut is_start = true;
    // The guard pitch is the mean over the start pattern, i.e. the pitch at its centre.
    let mut unit = unit_l * (1.0 - span.growth(pos) * 8.5 * unit_l);
    // What remains once fewer than a character plus half a stop pattern is left is the
    // stop pattern itself.
    while x_r - pos > 26.5 * unit_r && cells.len() <= MAX_ROW_CELLS {
        if let Some(b) = bounds.as_deref_mut()
            && !is_start
        {
            b.push(pos);
        }
        let rate = span.growth(pos);
        // Width of 17 modules whose pitch starts at `unit` and changes at `rate`.
        let expected = 17.0 * unit / (1.0 - 8.5 * unit * rate).max(0.5);
        let target = pos + expected;
        let next = nearest_bar_start(edges, target, 1.5 * unit);
        let mut cell = None;
        let mut end_unit = unit * (1.0 + rate * expected);
        if let (Some(i), Some(j)) = (idx, next)
            && j == i + 8
        {
            let width = edges[j].pos - edges[i].pos;
            if (width - expected).abs() <= 1.5 * unit {
                if !is_start {
                    cell = decode_cell(&edges[i..=j], rate * width, table);
                }
                // The measured mean pitch, carried from the centre to the far end.
                let measured = width / 17.0 * (1.0 + rate * width / 2.0);
                end_unit = 0.5 * end_unit + 0.5 * measured;
            }
        }
        unit = end_unit;
        if !is_start {
            cells.push(cell);
        }
        is_start = false;
        match next {
            Some(j) => {
                pos = edges[j].pos;
                idx = Some(j);
            }
            None => {
                pos = target;
                idx = None;
            }
        }
    }
    if let Some(b) = bounds
        && !cells.is_empty()
    {
        b.push(pos);
    }
    cells
}

/// Robust straight-line fit `row ≈ a·line + b` through `(line, row)` samples
/// (Theil–Sen on half-span pairs), tolerant of the occasional misread row indicator.
fn fit_rows(samples: &mut [(f32, f32)]) -> Option<(f32, f32)> {
    let n = samples.len();
    if n < 2 {
        return None;
    }
    samples.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
    let half = n / 2;
    let slopes: Vec<f32> = (0..n - half)
        .filter(|&i| samples[i + half].0 > samples[i].0)
        .map(|i| (samples[i + half].1 - samples[i].1) / (samples[i + half].0 - samples[i].0))
        .collect();
    if slopes.is_empty() {
        return None;
    }
    let a = median(&slopes);
    let b = median(&samples.iter().map(|s| s.1 - a * s.0).collect::<Vec<_>>());
    Some((a, b))
}

/// The codeword with the heaviest vote in `cell`, if any.
fn winner(cell: &[(u16, f32)]) -> Option<(u16, f32)> {
    cell.iter()
        .copied()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(core::cmp::Ordering::Equal))
}

/// Add `weight` to `value`'s tally in `cell`.
fn vote(cell: &mut Vec<(u16, f32)>, value: u16, weight: f32) {
    match cell.iter_mut().find(|v| v.0 == value) {
        Some(v) => v.1 += weight,
        None => cell.push((value, weight)),
    }
}

/// Least vote weight for a symbol parameter read from the row indicators.
const MIN_INDICATOR_WEIGHT: f32 = 1.5;

/// Check codewords a correction that fills in unread characters must leave unused.
///
/// Filling in `f` erasures and correcting `e` errors consumes `f + 2e` of the `k`
/// check codewords, and a grid of unrelated values then still passes with a
/// probability of about `C(n, e) / 929^(k − f − e)`: all the erasures do is take `f`
/// check codewords out of the test. With `f + 2e + 3 <= k` that is at most
/// `C(n, e) / 929^(e + 3)`, below `929^-3` — one in 800 million — for any block
/// length `n` and any `e`. That is stricter than the rule for errors alone has ever
/// been (level 0 accepts a clean block on two check codewords, `929^-2`), so reading
/// around unread characters adds no way for a misread grid to pass.
const ERASURE_MARGIN: usize = 3;

/// A symbol's codewords recovered by the scan-line reader, Reed–Solomon corrected.
struct Codewords {
    rows: usize,
    cols: usize,
    ec_level: usize,
    values: Vec<u32>,
}

/// The codeword grid as voted by the scan lines, with the symbol parameters the row
/// indicators announced.
struct Grid {
    rows: usize,
    cols: usize,
    ec_level: usize,
    /// Scan line `i` runs at row coordinate `slope · i + offset + 0.5` (row `r` spans
    /// `r..r+1`).
    slope: f32,
    offset: f32,
    /// Per data codeword (row-major), the values read and their vote weights.
    votes: Vec<Vec<(u16, f32)>>,
}

/// Row-indicator codewords `(left, right)` of `row` in a symbol of the given size and
/// error-correction level (ISO/IEC 15438 §5.5): the left ones cycle rows / level /
/// columns over the three clusters, the right ones one step behind.
fn row_indicators(row: usize, rows: usize, cols: usize, ec_level: usize) -> (usize, usize) {
    let fields = [(rows - 1) / 3, ec_level * 3 + (rows - 1) % 3, cols - 1];
    let base = 30 * (row / 3);
    (base + fields[row % 3], base + fields[(row % 3 + 2) % 3])
}

impl Grid {
    /// Error-correct one reading of the grid (`None` = unread character).
    ///
    /// The level and size come from the row indicators, so the block must decode at
    /// exactly that level and its length descriptor must match. What is accepted
    /// beyond that is deliberately less than Reed–Solomon can correct, because every
    /// check codeword a correction consumes is one fewer to tell a damaged symbol
    /// from a misread grid that was never one:
    ///
    /// - Unread characters are erasures — their positions are known, so each costs one
    ///   check codeword where a misread one costs two — and a correction that fills
    ///   any in must leave [`ERASURE_MARGIN`] check codewords unused.
    /// - Failing that, an unread character is treated like a misread one, under the
    ///   rule for errors alone: any number the level corrects, except that the weak
    ///   levels keep a margin — with 2 or 4 check codewords a full-capacity correction
    ///   of a badly misread grid would too often land on a wrong codeword.
    fn correct(&self, reading: &[Option<u16>]) -> Option<Codewords> {
        let k = 2usize << self.ec_level;
        let values: Vec<u32> = reading.iter().map(|v| u32::from(v.unwrap_or(0))).collect();
        let unread: Vec<usize> = (0..reading.len())
            .filter(|&i| reading[i].is_none())
            .collect();
        let filled = if unread.is_empty() || unread.len() + ERASURE_MARGIN > k {
            None
        } else {
            super::ec::decode_with_erasures(&values, k, &unread)
                .filter(|(_, errors)| unread.len() + 2 * errors + ERASURE_MARGIN <= k)
                .map(|(corrected, _)| corrected)
        };
        let corrected = filled.or_else(|| {
            let budget = match k {
                2 => 0,
                4 => 1,
                _ => k / 2,
            };
            if unread.len() > budget {
                return None;
            }
            let corrected = super::ec::decode(&values, k)?;
            let changed = reading
                .iter()
                .zip(&corrected)
                .filter(|(read, fixed)| read.map(u32::from) != Some(**fixed))
                .count();
            (changed <= budget).then_some(corrected)
        })?;
        if corrected[0] as usize != values.len() - k {
            return None;
        }
        Some(Codewords {
            rows: self.rows,
            cols: self.cols,
            ec_level: self.ec_level,
            values: corrected,
        })
    }

    /// The heaviest-voted value of every codeword.
    fn voted(&self) -> Vec<Option<u16>> {
        self.votes.iter().map(|c| winner(c).map(|w| w.0)).collect()
    }
}

/// The one row within reach of row coordinate `at` (row `r` spans `r..r+1`) whose
/// cluster is the one a character was read in.
fn row_near(at: f32, cluster: u8) -> Option<usize> {
    let base = at.floor() as i64;
    (base - 1..=base + 1)
        .find(|r| r.rem_euclid(3) == i64::from(cluster))
        .filter(|r| (0..90).contains(r) && (*r as f32 + 0.5 - at).abs() <= 1.0)
        .map(|r| r as usize)
}

/// Place every scan line's readings in the codeword grid and vote, cross-checking the
/// row indicators. `lines[i]` is the [`read_line`] result of line `i`.
fn assemble(lines: &[Vec<Option<Cw>>]) -> Option<Grid> {
    // Column count: the commonest number of characters between the guards.
    let mut tally = [0usize; MAX_ROW_CELLS + 1];
    for line in lines {
        if (3..=MAX_ROW_CELLS).contains(&line.len()) {
            tally[line.len()] += 1;
        }
    }
    let cells = (3..=MAX_ROW_CELLS).max_by_key(|&n| tally[n])?;
    if tally[cells] < 3 {
        return None;
    }
    let cols = cells - 2;

    // A row indicator carries its row number (the cluster and a multiple of 30); the
    // lines carrying one pin the line → row mapping for all the others.
    let row_of = |cw: &Cw| 3 * (cw.value as usize / 30) + cw.cluster as usize;
    let mut samples: Vec<(f32, f32)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.len() != cells {
            continue;
        }
        for cw in [line[0], line[cells - 1]].into_iter().flatten() {
            if cw.value < 900 {
                samples.push((i as f32, row_of(&cw) as f32));
            }
        }
    }
    let (slope, offset) = fit_rows(&mut samples)?;
    if slope.is_nan() || slope <= 0.0 {
        return None;
    }

    let mut votes: Vec<Vec<(u16, f32)>> = vec![Vec::new(); 90 * cols];
    // Row-indicator fields: (rows-1)/3, 3·level + (rows-1)%3, columns-1.
    let mut fields: [Vec<(u16, f32)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (i, line) in lines.iter().enumerate() {
        if line.len() != cells {
            continue;
        }
        // Continuous row coordinate of this line (row `r` spans `r..r+1`).
        let at = slope * i as f32 + offset + 0.5;
        for (c, cw) in line.iter().enumerate() {
            let Some(cw) = cw else { continue };
            let Some(row) = row_near(at, cw.cluster) else {
                continue;
            };
            if c == 0 || c == cells - 1 {
                if cw.value >= 900 || row_of(cw) != row {
                    continue;
                }
                // Left indicators cycle rows/level/columns over the clusters; right
                // ones are one step behind.
                let field = (cw.cluster as usize + if c == 0 { 0 } else { 2 }) % 3;
                vote(&mut fields[field], cw.value % 30, cw.weight);
            } else {
                vote(&mut votes[row * cols + c - 1], cw.value, cw.weight);
            }
        }
    }

    let field = |f: usize| -> Option<usize> {
        winner(&fields[f])
            .filter(|w| w.1 >= MIN_INDICATOR_WEIGHT)
            .map(|w| w.0 as usize)
    };
    let (row_thirds, level_rows, columns) = (field(0)?, field(1)?, field(2)?);
    let rows = 3 * row_thirds + level_rows % 3 + 1;
    let ec_level = level_rows / 3;
    if columns + 1 != cols || !(3..=90).contains(&rows) || ec_level > 8 {
        return None;
    }
    let k = 2usize << ec_level;
    let total = rows * cols;
    if total <= k {
        return None;
    }

    votes.truncate(total);
    Some(Grid {
        rows,
        cols,
        ec_level,
        slope,
        offset,
        votes,
    })
}

/// Render recovered codewords as the canonical module matrix of their symbol.
fn render_codewords(cw: &Codewords) -> BitMatrix {
    let width = 17 * cw.cols + 69;
    let mut matrix = BitMatrix::new(width, cw.rows * ROW_HEIGHT, QUIET_ZONE);
    for row in 0..cw.rows {
        let cluster = row % 3;
        let (left, right) = row_indicators(row, cw.rows, cw.cols, cw.ec_level);
        let mut x = 0usize;
        let mut emit = |pattern: u32, bits: usize| {
            for bit in (0..bits).rev() {
                if (pattern >> bit) & 1 != 0 {
                    for dy in 0..ROW_HEIGHT {
                        matrix.set(x, row * ROW_HEIGHT + dy, true);
                    }
                }
                x += 1;
            }
        };
        emit(START_PATTERN, 17);
        emit(CODEWORD_PATTERNS[cluster][left.min(928)], 17);
        for col in 0..cw.cols {
            let value = cw.values[row * cw.cols + col] as usize;
            emit(CODEWORD_PATTERNS[cluster][value.min(928)], 17);
        }
        emit(CODEWORD_PATTERNS[cluster][right.min(928)], 17);
        emit(STOP_PATTERN, 18);
    }
    matrix
}

/// Most scan lines read across one symbol.
const MAX_SCAN_LINES: usize = 1200;

/// Quiet-zone margin (modules) each scan line is extended by beyond the guards.
const LINE_MARGIN: f32 = 4.0;

/// The grey-level scan-line reader: locate the guards, read along the rows, vote.
///
/// A symbol with both guards in the frame is read between them. Failing that, each
/// guard that was found is tried as the one end of a symbol the frame cuts short.
fn scan_lines(frame: &GrayFrame<'_>) -> Option<Symbol> {
    let (noise, range) = frame_stats(frame);
    if range < 12.0 {
        return None;
    }
    let (start, stop) = find_guards(frame, noise, range);
    if let (Some(start), Some(stop)) = (&start, &stop)
        && let Some(g) = pair_guards(start, stop)
        && let Some(mut sym) = read_between_guards(frame, noise, range, &g)
    {
        // The guards' outer edges are the symbol's outline.
        sym.location.get_or_insert_with(|| Location {
            outline: Quad::new([g.tl, g.tr, g.br, g.bl]),
            rotation: Some((g.tr.y - g.tl.y).atan2(g.tr.x - g.tl.x)),
            module_size: Some((g.unit_l + g.unit_r) / 2.0),
        });
        return Some(sym);
    }
    [(start, Side::Start), (stop, Side::Stop)]
        .into_iter()
        .find_map(|(guard, side)| read_from_guard(frame, noise, range, &guard?, side))
}

/// Read the symbol whose guards are `g`: scan lines along the rows, voted per codeword.
fn read_between_guards(
    frame: &GrayFrame<'_>,
    noise: Noise,
    range: f32,
    g: &Guards,
) -> Option<Symbol> {
    // Module pitch across each guard (perpendicular to its edge), from the pitch along
    // image rows.
    let across = |top: Point, bottom: Point, unit: f32| {
        let len = top.distance(bottom).max(1e-3);
        unit * ((bottom.y - top.y) / len).abs()
    };
    let (pitch_l, pitch_r) = (across(g.tl, g.bl, g.unit_l), across(g.tr, g.br, g.unit_r));
    let (hl, hr) = (g.tl.distance(g.bl), g.tr.distance(g.br));
    let n = ((hl.max(hr).ceil() as usize) + 1).clamp(2, MAX_SCAN_LINES);
    let spacing = hl.min(hr) / (n - 1) as f32;
    // Average each line with its neighbours over a bit under half a row (≥ 3 modules).
    let half = ((0.45 * pitch_l.min(pitch_r) / spacing.max(0.25)).round() as usize).min(3);

    // Lay the lines out: line `i` joins the points a fraction `i/(n-1)` down each guard,
    // extended by a margin so the outer edges have their quiet-zone side.
    let lerp =
        |a: Point, b: Point, t: f32| Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
    let line_at = |i: usize| {
        let t = i as f32 / (n - 1) as f32;
        let (l, r) = (lerp(g.tl, g.bl, t), lerp(g.tr, g.br, t));
        let len = l.distance(r).max(1.0);
        let d = ((r.x - l.x) / len, (r.y - l.y) / len);
        // Pitch along the line: the guards are crossed obliquely.
        let along = |top: Point, bottom: Point, pitch: f32| {
            let e = top.distance(bottom).max(1e-3);
            let normal = ((bottom.y - top.y) / e, -(bottom.x - top.x) / e);
            pitch / (normal.0 * d.0 + normal.1 * d.1).abs().max(0.2)
        };
        let (ul, ur) = (along(g.tl, g.bl, pitch_l), along(g.tr, g.br, pitch_r));
        (l, d, len, ul, ur)
    };
    let longest = [0, n - 1]
        .into_iter()
        .map(|i| {
            let (_, _, len, ul, ur) = line_at(i);
            len + LINE_MARGIN * (ul + ur)
        })
        .fold(0f32, f32::max);
    let samples = longest.ceil() as usize + 1;
    if samples > 8192 {
        return None;
    }

    let mut raw = vec![0f32; n * samples];
    let (frame_w, frame_h) = ((frame.width() - 1) as f32, (frame.height() - 1) as f32);
    // Per line: positions of the outer edges and the left pitch, in samples.
    let mut layout = Vec::with_capacity(n);
    for i in 0..n {
        let (l, d, len, ul, ur) = line_at(i);
        let total = len + LINE_MARGIN * (ul + ur);
        let step = total / (samples - 1) as f32;
        let (x0, y0) = (l.x - LINE_MARGIN * ul * d.0, l.y - LINE_MARGIN * ul * d.1);
        let line = &mut raw[i * samples..(i + 1) * samples];
        // The samples within the frame: `first..end`.
        let (mut first, mut end) = (samples, 0);
        for (s, v) in line.iter_mut().enumerate() {
            let at = s as f32 * step;
            let (x, y) = (x0 + at * d.0, y0 + at * d.1);
            if x >= 0.0 && y >= 0.0 && x <= frame_w && y <= frame_h {
                first = first.min(s);
                end = s + 1;
                *v = sample_bilinear(frame, f64::from(x), f64::from(y)) as f32;
            }
        }
        beyond_frame(line, first, end);
        layout.push(LineSpan {
            x_l: LINE_MARGIN * ul / step,
            x_r: (LINE_MARGIN * ul + len) / step,
            unit_l: ul / step,
            unit_r: ur / step,
        });
    }

    let stack = LineStack {
        raw,
        lines: n,
        samples,
    };
    let noise = noise.max(stack.noise());

    let table = build_e_table();
    let hyst = hysteresis(noise.averaged(2 * half + 1), range);
    let mut profile = vec![0f32; samples];
    let mut edges: Vec<Edge> = Vec::new();
    let mut lines: Vec<Vec<Option<Cw>>> = Vec::with_capacity(n);
    for (i, span) in layout.iter().enumerate() {
        let (lo, hi) = (i.saturating_sub(half), (i + half).min(n - 1));
        stack.average(lo, hi, &mut profile);
        find_edges(&profile, hyst, &mut edges);
        lines.push(read_line(&edges, span, &table, None));
    }

    let grid = assemble(&lines)?;
    let voted = grid.voted();
    // The edge reading usually suffices; where blur has biased it beyond what the error
    // correction absorbs, match the rows against blurred templates instead.
    let codewords = grid.correct(&voted).or_else(|| {
        let matched = match_rows(&stack, &layout, &grid, &voted, noise, range, &table)?;
        grid.correct(&matched)
    })?;
    Pdf417Decoder::new()
        .decode_matrix(&render_codewords(&codewords))
        .ok()
}

// -----------------------------------------------------------------------------------
// One guard (symbols cut short by the frame)
// -----------------------------------------------------------------------------------
//
// A frame may hold only one end of a symbol: the other guard, and with it some of the
// columns, cropped away or never in view. Everything needed to place the rest is next
// to the guard that remains — its row indicators give the row of every scan line and,
// between the three clusters, the symbol's rows, columns and error-correction level.
// So the scan lines are laid at right angles to that guard and read from it as far as
// the frame goes; the characters count columns from the guard, and the columns the
// frame does not hold are erasures for Reed–Solomon to fill in, within the margin of
// `Grid::correct`.

/// Read one scan line leftwards from the stop pattern, whose outer edge is at `x_r`:
/// the right row indicator, then the data columns from the last to the first — as far
/// as `x_min`, where the line begins. The counterpart of [`read_line`], without a
/// second guard to say how the pitch changes along the row.
fn read_line_back(
    edges: &[Edge],
    x_r: f32,
    unit: f32,
    x_min: f32,
    table: &[(u32, u16)],
) -> Vec<Option<Cw>> {
    let mut cells = Vec::new();
    // The stop pattern is 18 modules wide and, like every character, begins with a bar.
    let Some(first) = nearest_bar_start(edges, x_r - 18.0 * unit, 2.0 * unit) else {
        return cells;
    };
    let mut unit = unit;
    let mut pos = edges[first].pos;
    let mut idx = Some(first);
    while pos - 17.0 * unit >= x_min && cells.len() <= MAX_ROW_CELLS {
        let expected = 17.0 * unit;
        let target = pos - expected;
        let prev = nearest_bar_start(edges, target, 1.5 * unit);
        let mut cell = None;
        if let (Some(i), Some(j)) = (prev, idx)
            && j == i + 8
        {
            let width = edges[j].pos - edges[i].pos;
            if (width - expected).abs() <= 1.5 * unit {
                cell = decode_cell(&edges[i..=j], 0.0, table);
                unit = 0.5 * unit + 0.5 * width / 17.0;
            }
        }
        cells.push(cell);
        match prev {
            Some(i) => {
                pos = edges[i].pos;
                idx = Some(i);
            }
            None => {
                pos = target;
                idx = None;
            }
        }
    }
    cells
}

/// [`assemble`] for scan lines read from one guard: `lines[i]` holds the row indicator
/// next to that guard and then the data characters in order of distance from it.
///
/// The one indicator column has to supply what two normally do, so each symbol
/// parameter needs twice the vote; columns the lines do not reach stay without votes.
fn assemble_from_guard(lines: &[Vec<Option<Cw>>], side: Side) -> Option<Grid> {
    let row_of = |cw: &Cw| 3 * (cw.value as usize / 30) + cw.cluster as usize;
    let indicator =
        |line: &Vec<Option<Cw>>| line.first().copied().flatten().filter(|cw| cw.value < 900);
    let mut samples: Vec<(f32, f32)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| Some((i as f32, row_of(&indicator(line)?) as f32)))
        .collect();
    let (slope, offset) = fit_rows(&mut samples)?;
    if slope.is_nan() || slope <= 0.0 {
        return None;
    }
    let row_at = |i: usize, cluster: u8| row_near(slope * i as f32 + offset + 0.5, cluster);

    // Left indicators cycle rows/level/columns over the clusters; right ones are one
    // step behind.
    let shift = if side == Side::Start { 0 } else { 2 };
    let mut fields: [Vec<(u16, f32)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (i, line) in lines.iter().enumerate() {
        if let Some(cw) = indicator(line)
            && row_at(i, cw.cluster) == Some(row_of(&cw))
        {
            vote(
                &mut fields[(cw.cluster as usize + shift) % 3],
                cw.value % 30,
                cw.weight,
            );
        }
    }
    let field = |f: usize| -> Option<usize> {
        winner(&fields[f])
            .filter(|w| w.1 >= 2.0 * MIN_INDICATOR_WEIGHT)
            .map(|w| w.0 as usize)
    };
    let (row_thirds, level_rows, columns) = (field(0)?, field(1)?, field(2)?);
    let rows = 3 * row_thirds + level_rows % 3 + 1;
    let (cols, ec_level) = (columns + 1, level_rows / 3);
    let k = 2usize << ec_level;
    let total = rows * cols;
    if !(3..=90).contains(&rows) || ec_level > 8 || total <= k || total > 928 {
        return None;
    }
    // The guard cannot be taller than the symbol it announces.
    let last = slope * (lines.len() - 1) as f32 + offset + 0.5;
    if offset + 0.5 < -1.5 || last > rows as f32 + 1.5 {
        return None;
    }

    let mut votes: Vec<Vec<(u16, f32)>> = vec![Vec::new(); total];
    for (i, line) in lines.iter().enumerate() {
        for (c, cw) in line.iter().enumerate().skip(1).take(cols) {
            let Some(cw) = cw else { continue };
            let col = if side == Side::Start { c - 1 } else { cols - c };
            if let Some(row) = row_at(i, cw.cluster).filter(|&r| r < rows) {
                vote(&mut votes[row * cols + col], cw.value, cw.weight);
            }
        }
    }
    Some(Grid {
        rows,
        cols,
        ec_level,
        slope,
        offset,
        votes,
    })
}

/// Read a symbol of which only `guard` is in the frame (see the section comment).
fn read_from_guard(
    frame: &GrayFrame<'_>,
    noise: Noise,
    range: f32,
    guard: &Guard,
    side: Side,
) -> Option<Symbol> {
    let height = guard.top.distance(guard.bottom);
    if height < 2.0 {
        return None;
    }
    // Down the guard, and at right angles to it into the symbol.
    let down = (
        (guard.bottom.x - guard.top.x) / height,
        (guard.bottom.y - guard.top.y) / height,
    );
    let inward = match side {
        Side::Start => (down.1, -down.0),
        Side::Stop => (-down.1, down.0),
    };
    // Module pitch across the guard, from the pitch along image rows.
    let pitch = guard.unit * down.1.abs();
    if pitch < 1.0 {
        return None;
    }
    let n = ((height.ceil() as usize) + 1).clamp(2, MAX_SCAN_LINES);
    let spacing = height / (n - 1) as f32;
    let half = ((0.45 * pitch / spacing.max(0.25)).round() as usize).min(3);

    // Each line starts a margin outside the guard and runs a pixel a sample to the
    // frame's border, or to where the widest symbol would end.
    let margin = (LINE_MARGIN * pitch).ceil();
    let (w, h) = ((frame.width() - 1) as f32, (frame.height() - 1) as f32);
    let origin = |i: usize| {
        let t = i as f32 / (n - 1) as f32;
        Point::new(
            guard.top.x + (guard.bottom.x - guard.top.x) * t - inward.0 * margin,
            guard.top.y + (guard.bottom.y - guard.top.y) * t - inward.1 * margin,
        )
    };
    let reach = |i: usize| {
        let o = origin(i);
        let along = |at: f32, d: f32, max: f32| {
            if d > 1e-6 {
                (max - at) / d
            } else if d < -1e-6 {
                -at / d
            } else {
                f32::MAX
            }
        };
        let limit = margin + (17.0 * MAX_ROW_CELLS as f32 + 40.0) * pitch;
        along(o.x, inward.0, w)
            .min(along(o.y, inward.1, h))
            .clamp(0.0, limit.min(8190.0)) as usize
    };
    let reaches: Vec<usize> = (0..n).map(reach).collect();
    let samples = reaches.iter().max()? + 1;
    if (samples as f32) < margin + 40.0 * pitch {
        return None; // not even the guard and its row indicator
    }

    // Past its reach a line repeats its last sample; before the frame begins — the
    // guard may stand at its very border — it is filled in by `beyond_frame`. The
    // lines are stored left to right, so those read from the stop guard are reversed.
    let mut raw = vec![0f32; n * samples];
    for (i, line) in raw.chunks_exact_mut(samples).enumerate() {
        let o = origin(i);
        let mut first = samples;
        for (s, v) in line.iter_mut().enumerate() {
            let at = s.min(reaches[i]) as f32;
            let (x, y) = (o.x + at * inward.0, o.y + at * inward.1);
            if x >= 0.0 && y >= 0.0 && x <= w && y <= h {
                first = first.min(s);
            }
            *v = sample_bilinear(frame, f64::from(x), f64::from(y)) as f32;
        }
        beyond_frame(line, first, samples);
        if side == Side::Stop {
            line.reverse();
        }
    }
    let stack = LineStack {
        raw,
        lines: n,
        samples,
    };
    let noise = noise.max(stack.noise());

    let table = build_e_table();
    let hyst = hysteresis(noise.averaged(2 * half + 1), range);
    let mut profile = vec![0f32; samples];
    let mut edges: Vec<Edge> = Vec::new();
    let mut lines: Vec<Vec<Option<Cw>>> = Vec::with_capacity(n);
    for (i, &reach) in reaches.iter().enumerate() {
        let (lo, hi) = (i.saturating_sub(half), (i + half).min(n - 1));
        stack.average(lo, hi, &mut profile);
        find_edges(&profile, hyst, &mut edges);
        lines.push(match side {
            Side::Start => {
                // `read_line` stops a character and half a stop pattern short of the
                // far edge it is given; here that is wherever no whole character is left.
                let span = LineSpan {
                    x_l: margin,
                    x_r: reach as f32 + 9.5 * pitch,
                    unit_l: pitch,
                    unit_r: pitch,
                };
                read_line(&edges, &span, &table, None)
            }
            Side::Stop => read_line_back(
                &edges,
                (samples - 1) as f32 - margin,
                pitch,
                (samples - 1 - reach) as f32,
                &table,
            ),
        });
    }

    let grid = assemble_from_guard(&lines, side)?;
    let codewords = grid.correct(&grid.voted())?;
    let mut sym = Pdf417Decoder::new()
        .decode_matrix(&render_codewords(&codewords))
        .ok()?;
    // The outline is the part of the symbol the lines cover.
    let width = ((17 * grid.cols + 69) as f32 * pitch).min(samples as f32 - margin);
    let far = |p: Point| Point::new(p.x + inward.0 * width, p.y + inward.1 * width);
    let (near_top, near_bottom) = (guard.top, guard.bottom);
    let outline = match side {
        Side::Start => [near_top, far(near_top), far(near_bottom), near_bottom],
        Side::Stop => [far(near_top), near_top, near_bottom, far(near_bottom)],
    };
    sym.location.get_or_insert_with(|| Location {
        outline: Quad::new(outline),
        rotation: Some(match side {
            Side::Start => inward.1.atan2(inward.0),
            Side::Stop => (-inward.1).atan2(-inward.0),
        }),
        module_size: Some(pitch),
    });
    Some(sym)
}

/// The scan lines of a symbol: `lines × samples` luminance values, row-major.
struct LineStack {
    raw: Vec<f32>,
    lines: usize,
    samples: usize,
}

impl LineStack {
    /// Noise where it matters — on the symbol itself — from the second differences
    /// across neighbouring lines (the pattern is constant down a row, so only the row
    /// boundaries, a minority of the lines, disturb the median).
    fn noise(&self) -> Noise {
        let (n, samples) = (self.lines, self.samples);
        let mut near = [0u32; 256];
        let mut far = [0u32; 256];
        let mut count = 0u32;
        for i in (2..n.saturating_sub(2)).step_by(2) {
            let line = |k: usize| &self.raw[k * samples..(k + 1) * samples];
            let (l0, l1, l2, l3, l4) =
                (line(i - 2), line(i - 1), line(i), line(i + 1), line(i + 2));
            for s in (0..samples).step_by(2) {
                near[((l1[s] - 2.0 * l2[s] + l3[s]).abs() as usize).min(255)] += 1;
                far[((l0[s] - 2.0 * l2[s] + l4[s]).abs() as usize).min(255)] += 1;
                count += 1;
            }
        }
        // (Half a level is added back for the truncation to whole levels.)
        Noise::from_quantiles(
            quantile(&near, count, 0.5) + 0.5,
            quantile(&far, count, 0.5) + 0.5,
            HALF_NORMAL_MEDIAN,
        )
    }

    /// Mean of lines `lo..=hi`, sample by sample, into `out`.
    fn average(&self, lo: usize, hi: usize, out: &mut [f32]) {
        out.fill(0.0);
        for j in lo..=hi {
            for (p, &v) in out
                .iter_mut()
                .zip(&self.raw[j * self.samples..(j + 1) * self.samples])
            {
                *p += v;
            }
        }
        let scale = 1.0 / (hi - lo + 1) as f32;
        for p in out {
            *p *= scale;
        }
    }
}

// -----------------------------------------------------------------------------------
// Template matching (blurred captures)
// -----------------------------------------------------------------------------------
//
// Edge-to-edge distances are immune to ink spread, but not to blur comparable to a
// module: a narrow element between wide ones is fattened, one between narrow ones is
// not, and the edges move accordingly — always the same way for the same character,
// so no amount of voting corrects it. What blur does preserve is the *profile*: the
// luminance across a character is its bar pattern convolved with the blur. So when
// the edge reading does not error-correct, each row is averaged over its core scan
// lines and every character is compared, as a profile, with all 929 characters of its
// row's cluster blurred the same way. The blur itself is measured on the symbol: the
// row indicators are known by then, so the Gaussian width that best reproduces them
// is the one used.

/// Samples per module in a matched profile.
const MATCH_RES: usize = 4;
/// Length of a matched profile: 17 modules and half a module of context either side.
const MATCH_LEN: usize = 18 * MATCH_RES;
/// Alignment offsets (modules) tried when matching: a character boundary found on a
/// blurred edge can be off by a good fraction of a module.
const MATCH_SHIFTS: [f32; 7] = [0.0, -0.25, 0.25, -0.5, 0.5, -0.75, 0.75];
/// Blur widths (Gaussian σ, in modules) tried.
const MATCH_SIGMAS: [f32; 7] = [0.25, 0.4, 0.55, 0.7, 0.85, 1.0, 1.2];
/// Least correlation for a template match to count, and least mean correlation of the
/// known row indicators for the stage to run at all.
const MATCH_MIN: f32 = 0.8;
/// Least lead of the best template over the runner-up.
const MATCH_MARGIN: f32 = 0.02;

/// Remove the mean of `v` and scale it to unit length; `false` if it is flat.
fn normalise(v: &mut [f32]) -> bool {
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    let mut norm = 0f32;
    for x in v.iter_mut() {
        *x -= mean;
        norm += *x * *x;
    }
    if norm < 1e-6 {
        return false;
    }
    let scale = 1.0 / norm.sqrt();
    for x in v.iter_mut() {
        *x *= scale;
    }
    true
}

/// Standard normal CDF, tabulated: the profile of a blurred step edge.
struct StepResponse {
    table: Vec<f32>,
}

impl StepResponse {
    const STEPS_PER_SIGMA: f32 = 32.0;
    const REACH: f32 = 4.0;

    fn new() -> Self {
        let n = (2.0 * Self::REACH * Self::STEPS_PER_SIGMA) as usize + 1;
        let table = (0..n)
            .map(|i| {
                let z = i as f32 / Self::STEPS_PER_SIGMA - Self::REACH;
                // Abramowitz & Stegun 7.1.26 for erf(|z|/√2).
                let x = z.abs() * core::f32::consts::FRAC_1_SQRT_2;
                let t = 1.0 / (1.0 + 0.327_591_1 * x);
                let poly = t
                    * (0.254_829_6
                        + t * (-0.284_496_74
                            + t * (1.421_413_7 + t * (-1.453_152 + t * 1.061_405_4))));
                let erf = 1.0 - poly * (-x * x).exp();
                0.5 * (1.0 + if z < 0.0 { -erf } else { erf })
            })
            .collect();
        StepResponse { table }
    }

    fn at(&self, z: f32) -> f32 {
        let i = (z + Self::REACH) * Self::STEPS_PER_SIGMA;
        if i <= 0.0 {
            0.0
        } else if i >= (self.table.len() - 1) as f32 {
            1.0
        } else {
            self.table[(i + 0.5) as usize]
        }
    }
}

/// The normalised luminance profile of 17-module `pattern` under Gaussian blur `sigma`
/// (modules), sampled like an observed profile. The context is what every symbol
/// character has around it: the space closing the previous one, the bar opening the
/// next, and beyond those a neutral grey.
fn template(pattern: u32, sigma: f32, step: &StepResponse, out: &mut [f32]) {
    // Lightness transitions as (module position, change), starting from grey.
    let mut edges = [(0f32, 0f32); 12];
    let mut count = 0usize;
    let mut level = 0.5f32;
    let mut turn = |at: f32, to: f32, level: &mut f32| {
        if to != *level {
            edges[count] = (at, to - *level);
            count += 1;
            *level = to;
        }
    };
    turn(-1.0, 1.0, &mut level);
    for j in 0..17 {
        let dark = (pattern >> (16 - j)) & 1 != 0;
        turn(j as f32, if dark { 0.0 } else { 1.0 }, &mut level);
    }
    turn(17.0, 0.0, &mut level);
    turn(18.0, 0.5, &mut level);
    let edges = &edges[..count.min(12)];
    for (k, v) in out.iter_mut().enumerate() {
        let m = -0.5 + (k as f32 + 0.5) / MATCH_RES as f32;
        *v = 0.5
            + edges
                .iter()
                .map(|&(at, change)| change * step.at((m - at) / sigma))
                .sum::<f32>();
    }
    normalise(out);
}

/// One row of the symbol, averaged over its core scan lines.
struct RowProfile {
    profile: Vec<f32>,
    /// Character boundaries (see [`read_line`]): `cells + 1` positions.
    bounds: Vec<f32>,
    span: LineSpan,
}

impl RowProfile {
    /// The normalised profile of character `cell`, displaced by `shift` modules;
    /// `false` if it is flat.
    fn observe(&self, cell: usize, shift: f32, out: &mut [f32]) -> bool {
        let (x0, x1) = (self.bounds[cell], self.bounds[cell + 1]);
        let width = x1 - x0;
        // Pitch change across the character, as in `decode_cell`, inverted: module
        // coordinate → position.
        let drift = self.span.growth(x0) * width;
        let last = (self.profile.len() - 1) as f32;
        for (k, v) in out.iter_mut().enumerate() {
            let m = -0.5 + (k as f32 + 0.5) / MATCH_RES as f32 + shift;
            let c = m / 17.0 * (1.0 - 0.5 * drift);
            let f = if drift.abs() < 1e-4 {
                c
            } else {
                (1.0 - (1.0 - 2.0 * drift * c).max(0.0).sqrt()) / drift
            };
            let x = (x0 + f * width).clamp(0.0, last);
            let i = (x as usize).min(self.profile.len() - 2);
            let t = x - i as f32;
            *v = self.profile[i] * (1.0 - t) + self.profile[i + 1] * t;
        }
        normalise(out)
    }
}

/// Dot product.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Re-read the data codewords by template matching (see the section comment). `voted`
/// is the edge reading, kept wherever a row cannot be isolated or no template matches
/// convincingly.
fn match_rows(
    stack: &LineStack,
    layout: &[LineSpan],
    grid: &Grid,
    voted: &[Option<u16>],
    noise: Noise,
    range: f32,
    table: &[(u32, u16)],
) -> Option<Vec<Option<u16>>> {
    let cells = grid.cols + 2;
    // Average each row over the scan lines in its middle half and find the character
    // boundaries on that (much cleaner) profile.
    let mut rows: Vec<Option<RowProfile>> = Vec::with_capacity(grid.rows);
    let mut edges: Vec<Edge> = Vec::new();
    for r in 0..grid.rows {
        let line_at = |row_coord: f32| (row_coord - 0.5 - grid.offset) / grid.slope;
        let (lo, hi) = (
            line_at(r as f32 + 0.25).ceil(),
            line_at(r as f32 + 0.75).floor(),
        );
        let (lo, hi) = if lo <= hi {
            (lo, hi)
        } else {
            let mid = line_at(r as f32 + 0.5).round();
            (mid, mid)
        };
        if lo < 0.0 || hi > (stack.lines - 1) as f32 {
            rows.push(None);
            continue;
        }
        let (lo, hi) = (lo as usize, hi as usize);
        let mut profile = vec![0f32; stack.samples];
        stack.average(lo, hi, &mut profile);
        find_edges(
            &profile,
            hysteresis(noise.averaged(hi - lo + 1), range),
            &mut edges,
        );
        let span = layout[(lo + hi) / 2];
        let mut bounds = Vec::new();
        let read = read_line(&edges, &span, table, Some(&mut bounds));
        rows.push((read.len() == cells).then_some(RowProfile {
            profile,
            bounds,
            span,
        }));
    }

    // Measure the blur on the row indicators, whose values are known.
    let step = StepResponse::new();
    let mut seen = [0f32; MATCH_LEN];
    let mut ideal = [0f32; MATCH_LEN];
    let mut best_sigma = (0f32, 0f32);
    let stride = grid.rows.div_ceil(16);
    for &sigma in &MATCH_SIGMAS {
        let (mut sum, mut count) = (0f32, 0usize);
        for r in (0..grid.rows).step_by(stride) {
            let Some(row) = &rows[r] else { continue };
            let (left, right) = row_indicators(r, grid.rows, grid.cols, grid.ec_level);
            for (cell, value) in [(0, left), (cells - 1, right)] {
                template(
                    CODEWORD_PATTERNS[r % 3][value.min(928)],
                    sigma,
                    &step,
                    &mut ideal,
                );
                let mut best = 0f32;
                for &shift in &MATCH_SHIFTS {
                    if row.observe(cell, shift, &mut seen) {
                        best = best.max(dot(&seen, &ideal));
                    }
                }
                sum += best;
                count += 1;
            }
        }
        if count > 0 && sum / count as f32 > best_sigma.1 {
            best_sigma = (sigma, sum / count as f32);
        }
    }
    let (sigma, fit) = best_sigma;
    if fit < MATCH_MIN {
        return None;
    }

    // All 929 characters of each cluster at that blur.
    let mut bank = vec![0f32; 3 * 929 * MATCH_LEN];
    for (i, t) in bank.as_chunks_mut::<MATCH_LEN>().0.iter_mut().enumerate() {
        template(CODEWORD_PATTERNS[i / 929][i % 929], sigma, &step, t);
    }

    let mut out = voted.to_vec();
    let mut scores = vec![0f32; 929];
    // Coarse alignments tried against the whole cluster; a big symbol gets just one.
    let coarse: &[f32] = if grid.rows * grid.cols > 600 {
        &[0.0]
    } else {
        &[0.0, -0.4, 0.4]
    };
    for (r, row) in rows.iter().enumerate() {
        let Some(row) = row else { continue };
        let templates = &bank[(r % 3) * 929 * MATCH_LEN..(r % 3 + 1) * 929 * MATCH_LEN];
        for c in 0..grid.cols {
            // Rank the cluster at the coarse alignments…
            scores.fill(-1.0);
            for &shift in coarse {
                if !row.observe(c + 1, shift, &mut seen) {
                    continue;
                }
                for (s, t) in scores.iter_mut().zip(templates.as_chunks::<MATCH_LEN>().0) {
                    *s = s.max(dot(&seen, t));
                }
            }
            // …then align the leaders finely.
            let mut leaders = [(-1f32, 0usize); 8];
            for (value, &s) in scores.iter().enumerate() {
                if s > leaders[7].0 {
                    leaders[7] = (s, value);
                    leaders.sort_by(|a, b| {
                        b.0.partial_cmp(&a.0).unwrap_or(core::cmp::Ordering::Equal)
                    });
                }
            }
            for &shift in &MATCH_SHIFTS {
                if !row.observe(c + 1, shift, &mut seen) {
                    continue;
                }
                for l in &mut leaders {
                    let t = &templates[l.1 * MATCH_LEN..(l.1 + 1) * MATCH_LEN];
                    l.0 = l.0.max(dot(&seen, t));
                }
            }
            leaders.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(core::cmp::Ordering::Equal));
            if leaders[0].0 >= MATCH_MIN && leaders[0].0 - leaders[1].0 >= MATCH_MARGIN {
                out[r * grid.cols + c] = Some(leaders[0].1 as u16);
            }
        }
    }
    Some(out)
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    #[test]
    fn start_ratio_matches_ideal_run() {
        // Ideal start pattern at 4 px/module.
        let lens: Vec<i32> = START_RATIOS.iter().map(|&r| r * 4).collect();
        let unit = match_ratios(&lens, &START_RATIOS).unwrap();
        assert!((unit - 4.0).abs() < 0.01);
    }

    #[test]
    fn stop_ratio_matches_ideal_run() {
        let lens: Vec<i32> = STOP_RATIOS.iter().map(|&r| r * 5).collect();
        let unit = match_ratios(&lens, &STOP_RATIOS).unwrap();
        assert!((unit - 5.0).abs() < 0.01);
    }

    #[test]
    fn ratio_rejects_wrong_shape() {
        // A uniform run set does not match the start's wide leading bar.
        let lens = [4, 4, 4, 4, 4, 4, 4, 4];
        assert!(match_ratios(&lens, &START_RATIOS).is_none());
    }

    #[test]
    fn median_odd_and_even() {
        assert!((median(&[3.0, 1.0, 2.0]) - 2.0).abs() < 1e-6);
        assert!((median(&[1.0, 2.0, 3.0, 4.0]) - 2.5).abs() < 1e-6);
    }

    // ---- grey-level scan-line reader ------------------------------------------------

    /// Edges of `widths` (bar first) laid out from `x0` at `unit` samples per module.
    fn ideal_edges(widths: &[u8], x0: f32, unit: f32) -> Vec<Edge> {
        let mut edges = Vec::new();
        let mut pos = x0;
        for (i, &w) in widths.iter().enumerate() {
            edges.push(Edge {
                pos,
                dark: i % 2 == 0,
            });
            pos += f32::from(w) * unit;
        }
        edges.push(Edge {
            pos,
            dark: widths.len().is_multiple_of(2),
        });
        edges
    }

    #[test]
    fn edge_to_edge_distances_identify_every_codeword() {
        // The reference decode rests on this: no two symbol characters, in any
        // cluster, share their six edge-to-edge distances.
        let table = build_e_table();
        assert_eq!(table.len(), 3 * 929);
        assert!(table.windows(2).all(|w| w[0].0 != w[1].0));
    }

    #[test]
    fn every_codeword_decodes_from_its_edges() {
        let table = build_e_table();
        for (cluster, patterns) in CODEWORD_PATTERNS.iter().enumerate() {
            for (value, &pattern) in patterns.iter().enumerate() {
                let edges = ideal_edges(&pattern_widths(pattern), 12.3, 3.1);
                let cw = decode_cell(&edges, 0.0, &table).expect("decodes");
                assert_eq!((cw.cluster as usize, cw.value as usize), (cluster, value));
                assert_eq!(cw.weight, 1.0);
            }
        }
    }

    #[test]
    fn row_indicators_agree_with_the_encoder() {
        for (rows, cols, level) in [(3, 1, 0), (17, 1, 2), (30, 12, 5), (90, 30, 8)] {
            for row in 0..rows {
                assert_eq!(
                    row_indicators(row, rows, cols, level),
                    super::super::encode::row_indicators(row, rows, cols, level)
                );
            }
        }
    }

    #[test]
    fn edges_follow_a_lighting_gradient_and_blurred_bars() {
        // Bars of 6, 2 and 2 samples on a background that doubles in brightness
        // across the line, box-blurred: no single threshold separates them.
        let mut ideal = vec![1.0f32; 80];
        for range in [10..16, 30..32, 50..52] {
            ideal[range].fill(0.1);
        }
        let profile: Vec<f32> = (0..80usize)
            .map(|i| {
                let lo = i.saturating_sub(1);
                let hi = (i + 1).min(79);
                let mean = ideal[lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32;
                mean * (100.0 + 1.5 * i as f32)
            })
            .collect();
        let mut edges = Vec::new();
        find_edges(&profile, 8.0, &mut edges);
        // (The brightening before the first bar may itself be reported as a rise from
        // the first sample; bars begin at a fall.)
        let got: Vec<(f32, bool)> = edges
            .iter()
            .skip_while(|e| !e.dark)
            .map(|e| (e.pos, e.dark))
            .collect();
        let want = [9.5, 15.5, 29.5, 31.5, 49.5, 51.5];
        assert_eq!(got.len(), want.len(), "{got:?}");
        for (i, (&(pos, dark), &w)) in got.iter().zip(&want).enumerate() {
            assert_eq!(dark, i % 2 == 0);
            assert!((pos - w).abs() < 0.6, "edge {i} at {pos}, want {w}");
        }
    }

    #[test]
    fn a_bar_edge_is_the_step_next_to_the_bar() {
        // Blank canvas (255), dim page (90), bar (30), page, canvas: the fall into the
        // bar is a staircase, and the bar's edges are its lower steps.
        let mut profile = vec![255.0f32; 60];
        profile[10..50].fill(90.0);
        profile[25..35].fill(30.0);
        let mut edges = Vec::new();
        find_edges(&profile, 10.0, &mut edges);
        assert_eq!(edges.len(), 2);
        assert!(edges[0].dark && (edges[0].pos - 24.5).abs() < 0.6);
        assert!(!edges[1].dark && (edges[1].pos - 34.5).abs() < 0.6);
    }

    #[test]
    fn noise_estimate_sees_through_resampling() {
        // Smoothing white noise over neighbouring rows (as a derotation does) lowers
        // adjacent differences far more than the noise a profile still carries; the
        // estimate must report the correlation rather than a small sigma.
        let (w, h) = (96usize, 96usize);
        let mut rng = crate::transform::Rng::new(7);
        let white: Vec<f32> = (0..w * h).map(|_| 20.0 * rng.next_gaussian()).collect();
        let smooth: Vec<u8> = (0..w * h)
            .map(|i| {
                let below = if i + w < w * h {
                    white[i + w]
                } else {
                    white[i]
                };
                (128.0 + (white[i] + below) / 2.0).clamp(0.0, 255.0) as u8
            })
            .collect();
        let frame = GrayFrame::new(&smooth, w, h).unwrap();
        let (noise, _) = frame_stats(&frame);
        // Each pixel now has σ = 20/√2 ≈ 14.1 and correlates 0.5 with the next row.
        assert!((noise.sigma - 14.1).abs() < 2.5, "{noise:?}");
        assert!((noise.rho - 0.5).abs() < 0.15, "{noise:?}");
        // Three such rows average to σ·√(3 + 4ρ)/3, not σ/√3.
        assert!(noise.averaged(3) > 1.2 * noise.sigma / 3f32.sqrt());
    }

    #[test]
    fn weak_error_correction_keeps_a_detection_margin() {
        // Level 0 has two check codewords: enough to correct one error, but a grid
        // that needs correcting is as likely misread beyond that, so it is refused.
        let data = [7u32, 1, 2, 3, 4, 5, 6];
        let mut block = data.to_vec();
        block.extend(crate::codes::pdf417::ec::encode(&data, 2));
        let grid = Grid {
            rows: 3,
            cols: 3,
            ec_level: 0,
            slope: 1.0,
            offset: 0.0,
            votes: Vec::new(),
        };
        let clean: Vec<Option<u16>> = block.iter().map(|&v| Some(v as u16)).collect();
        assert_eq!(grid.correct(&clean).expect("clean block").values, block);
        let mut damaged = clean.clone();
        damaged[4] = Some(500);
        assert!(grid.correct(&damaged).is_none());
        let mut unread = clean;
        unread[4] = None;
        assert!(grid.correct(&unread).is_none());
    }

    #[test]
    fn unread_characters_are_filled_in_within_a_margin() {
        // Level 2 has eight check codewords. Reed–Solomon could fill in eight
        // erasures, but then nothing would be left to check the result against.
        let data = [12u32, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
        let mut block = data.to_vec();
        block.extend(crate::codes::pdf417::ec::encode(&data, 8));
        let grid = |ec_level: usize| Grid {
            rows: 5,
            cols: 4,
            ec_level,
            slope: 1.0,
            offset: 0.0,
            votes: Vec::new(),
        };
        let clean: Vec<Option<u16>> = block.iter().map(|&v| Some(v as u16)).collect();
        let damaged = |unread: &[usize], misread: &[usize]| {
            let mut reading = clean.clone();
            for &i in unread {
                reading[i] = None;
            }
            for &i in misread {
                reading[i] = Some((block[i] as u16 + 1) % 929);
            }
            grid(2).correct(&reading).map(|cw| cw.values)
        };
        // Five erasures leave the margin of three; one error costs two more.
        assert_eq!(damaged(&[1, 5, 9, 13, 17], &[]), Some(block.clone()));
        assert_eq!(damaged(&[1, 5, 9], &[14]), Some(block.clone()));
        assert_eq!(damaged(&[0, 19], &[14]), Some(block.clone()));
        assert_eq!(damaged(&[1, 5, 9, 13, 17, 19], &[]), None);
        assert_eq!(damaged(&[1, 5, 9, 13], &[14]), None);
        // Without erasures the level's own capacity still applies, as it does when a
        // lone unread character is no better placed than a misread one.
        assert_eq!(damaged(&[], &[2, 6, 10, 14]), Some(block.clone()));
        assert_eq!(damaged(&[3], &[6, 10, 14]), Some(block.clone()));
        assert_eq!(damaged(&[3], &[2, 6, 10, 14]), None);

        // Level 1 (four check codewords) fills in one unread character, not two.
        let data = [16u32, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let mut block = data.to_vec();
        block.extend(crate::codes::pdf417::ec::encode(&data, 4));
        let mut reading: Vec<Option<u16>> = block.iter().map(|&v| Some(v as u16)).collect();
        reading[7] = None;
        assert_eq!(grid(1).correct(&reading).expect("one").values, block);
        reading[11] = None;
        assert!(grid(1).correct(&reading).is_none());
    }

    #[test]
    fn a_wide_bar_of_uneven_ink_keeps_its_edge() {
        // The start pattern's bar as a photograph shows it (ZXing's pdf417-2/01): the
        // ink is a few levels lighter along most of the bar than at its darkest spot.
        // That stretch is not a shelf of page beside a narrower bar.
        let profile: [f32; 48] = [
            141., 142., 142., 142., 142., 141., 140., 141., 142., 142., 141., 139., 135., 119.,
            72., 28., 21., 19., 20., 19., 18., 18., 18., 22., 22., 20., 20., 20., 22., 23., 21.,
            19., 18., 19., 20., 18., 15., 13., 14., 19., 40., 82., 121., 129., 112., 69., 37., 49.,
        ];
        let mut edges = Vec::new();
        find_edges(&profile, 6.75, &mut edges);
        assert!(edges.len() >= 2, "{edges:?}");
        assert!(
            edges[0].dark && (edges[0].pos - 14.0).abs() < 0.7,
            "{edges:?}"
        );
        assert!(
            !edges[1].dark && (edges[1].pos - 40.7).abs() < 0.7,
            "{edges:?}"
        );
    }

    #[test]
    fn lines_read_back_from_the_stop_pattern() {
        // One row of a symbol, as ideal edges: start, left indicator, three data
        // characters, right indicator, stop. Reading back from the stop pattern gives
        // the right indicator and then the data, last column first.
        let table = build_e_table();
        let values = [(1usize, 33usize), (1, 500), (1, 77), (1, 901), (1, 32)];
        let unit = 3.0f32;
        let mut edges: Vec<Edge> = Vec::new();
        let mut x = 20.0f32;
        let mut push = |pattern: u32, bits: usize, edges: &mut Vec<Edge>| {
            let mut prev = false;
            for bit in (0..bits).rev() {
                let dark = (pattern >> bit) & 1 != 0;
                if dark != prev {
                    edges.push(Edge { pos: x, dark });
                    prev = dark;
                }
                x += unit;
            }
            if prev {
                edges.push(Edge {
                    pos: x,
                    dark: false,
                });
            }
        };
        push(START_PATTERN, 17, &mut edges);
        for &(cluster, value) in &values {
            push(CODEWORD_PATTERNS[cluster][value], 17, &mut edges);
        }
        push(STOP_PATTERN, 18, &mut edges);
        let x_r = edges.last().unwrap().pos;
        let read = |x_min: f32| -> Vec<Option<(u8, u16)>> {
            read_line_back(&edges, x_r, unit * 1.04, x_min, &table)
                .iter()
                .map(|c| c.map(|c| (c.cluster, c.value)))
                .collect()
        };
        let want: Vec<Option<(u8, u16)>> = values
            .iter()
            .rev()
            .map(|&(c, v)| Some((c as u8, v as u16)))
            .collect();
        // With the whole row in view, what lies beyond the left indicator is the start
        // pattern: no character.
        let all = read(0.0);
        assert_eq!(all[..5], want[..]);
        assert!(all[5..].iter().all(|c| c.is_none()), "{all:?}");
        // Cut inside the first data column: only whole characters are read.
        let cut = 20.0 + unit * (17.0 * 2.0 + 5.0);
        assert_eq!(read(cut), want[..3]);
    }

    #[test]
    fn guards_flush_with_the_frame_are_found() {
        // A crop to the symbol's very edge: the frame ends on the guards' outer bars,
        // with no quiet zone beside them for their first edge to stand against.
        use crate::codes::pdf417::{EcLevel, Pdf417Encoder};
        use crate::traits::Encode;
        let encoder = Pdf417Encoder::new();
        let symbol = encoder
            .build_text("FLUSH WITH THE FRAME 0123456789", EcLevel::new(4).unwrap())
            .unwrap();
        let encoding = encoder.encode(&symbol).unwrap();
        let (scale, quiet) = (3usize, QUIET_ZONE);
        // (Sharp, as software writes a symbol: blur would leave half an edge.)
        let full = crate::render::render(&encoding, scale);
        let crop = |x0: usize, x1: usize| {
            let mut out = crate::image::GrayImage::filled(x1 - x0, full.height(), 255);
            for y in 0..full.height() {
                for x in x0..x1 {
                    out.set(x - x0, y, full.get(x, y));
                }
            }
            out
        };
        let w = full.width();
        // Both guards at the border; then each alone, the far side cut inside the
        // row indicator.
        for (x0, x1) in [
            (quiet * scale, w - quiet * scale),
            (quiet * scale, w - (quiet + 18 + 8) * scale),
            ((quiet + 17 + 8) * scale, w - quiet * scale),
        ] {
            let image = crop(x0, x1);
            let read = scan_lines(&image.as_frame())
                .unwrap_or_else(|| panic!("columns {x0}..{x1}: not read"));
            assert_eq!(read.segments, symbol.segments);
            assert_eq!(read.meta, symbol.meta);
            assert_eq!(encoder.encode(&read).unwrap(), encoding);
        }
    }

    #[test]
    fn blurred_symbol_round_trips_losslessly() {
        // Blur wide enough to bias the edge reading: the template stage must recover
        // the exact codewords, and with them the exact symbol (geometry and level).
        use crate::codes::pdf417::{EcLevel, Pdf417Encoder};
        use crate::traits::Encode;
        let encoder = Pdf417Encoder::new();
        let symbol = encoder
            .build_text("PDF417 LIVE", EcLevel::new(2).unwrap())
            .unwrap();
        let encoding = encoder.encode(&symbol).unwrap();
        let image = crate::transform::gaussian_blur(&crate::render::render(&encoding, 3), 1.6);
        let read = scan_lines(&image.as_frame()).expect("reads");
        assert_eq!(read.segments, symbol.segments);
        assert_eq!(read.meta, symbol.meta);
        assert_eq!(encoder.encode(&read).unwrap(), encoding);
    }
}
