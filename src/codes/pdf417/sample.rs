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
//!
//! Out of scope, and deliberately not asserted: **perspective tilt** (the affine model
//! does not correct keystoning, and a finder-less stacked code offers no interior anchor
//! to recover it robustly), steep rotation, multiple symbols per frame, and symbols
//! whose guards are clipped by the frame edge.

use super::Pdf417Decoder;
use super::encode::ROW_HEIGHT;
use crate::error::{Error, Result};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::imgproc::homography::Homography;
use crate::imgproc::line::{Line, ransac_line};
use crate::imgproc::rng::Prng;
use crate::imgproc::sample::sample_bilinear;
use crate::imgproc::threshold::otsu_threshold;
use crate::output::{BitMatrix, Encoding};
use crate::pipeline::{Candidate, Hints};
use crate::symbol::Symbol;
use crate::symbology::Symbology;
use crate::traits::{Analyze, Decode, Detect};

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
        match locate(frame) {
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
/// Returns `None` if no symbol is found or the sampled grid does not decode.
pub fn scan(frame: &GrayFrame<'_>) -> Option<Symbol> {
    let matrix = sample_grid(frame)?;
    Pdf417Decoder::new().decode(&Encoding::Matrix(matrix)).ok()
}

/// Locate the PDF417 symbol in `frame` and sample it to a clean [`BitMatrix`].
///
/// This is the geometry-recovery half of [`scan`], exposed so tests can assert the
/// sampled grid directly (e.g. pixel-exact recovery of an upright render).
pub fn sample_grid(frame: &GrayFrame<'_>) -> Option<BitMatrix> {
    Some(locate(frame)?.sample(frame))
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
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    if v.len().is_multiple_of(2) {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    }
}

/// Minimum number of rows that must carry both guards for a confident location.
const MIN_ANCHOR_ROWS: usize = 6;

/// Full location pipeline for `frame`.
fn locate(frame: &GrayFrame<'_>) -> Option<Located> {
    if frame.width() < 40 || frame.height() < 9 {
        return None;
    }
    let bin = Binary::from_frame(frame);

    let mut left_pts = Vec::new();
    let mut right_pts = Vec::new();
    let mut start_units = Vec::new();
    for y in 0..bin.height {
        let runs = encode_row(&bin, y);
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
    let (tl0, bl0) = guard_corners(&bin, &left_line, &left_in, interior, unit_along, 3.5);
    let (tr0, br0) = guard_corners(&bin, &right_line, &right_in, interior, unit_along, 14.5);

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

#[cfg(test)]
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
}
