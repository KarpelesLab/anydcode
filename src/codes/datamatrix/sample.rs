//! Data Matrix image sampling: pixels → [`BitMatrix`] → [`Symbol`].
//!
//! The structural [`DataMatrixDecoder`](super::DataMatrixDecoder) consumes a clean
//! module grid. This module is the front-end that recovers such a grid from a real
//! image. Unlike QR, a Data Matrix carries no repeated finder rings to cross-correlate;
//! its fiducials are the two **solid** edges of the L-shaped finder and the two
//! opposite **timing** borders (alternating modules). The sampler:
//!
//! 1. binarizes the frame (global Otsu) and isolates the symbol blob (the L finder is
//!    solid, so it forms the largest connected component and its bounding box covers
//!    the whole symbol);
//! 2. recovers the symbol's four corners from the dark-pixel cloud (a rotation-robust
//!    farthest-point method, so no axis-aligned assumption);
//! 3. identifies the solid L by measuring per-edge darkness — the two adjacent edges
//!    that read ~100 % dark are the finder; their shared vertex is the L corner
//!    (bottom-left in the symbol's own frame);
//! 4. estimates the module count per axis by counting transitions along the timing
//!    edges, snapping to the nearest valid square size;
//! 5. builds a [`Homography`] from the four corners and samples every module centre
//!    into a [`BitMatrix`], which the structural decoder then error-corrects.
//!
//! All four corners are then refined to sub-pixel accuracy by fitting straight lines
//! to the symbol's dark boundary along each edge and intersecting adjacent lines. The
//! fourth (top-right) corner — where the two timing borders meet and which is not a
//! locateable pattern — falls out as the intersection of the two fitted timing lines,
//! so it is perspective-correct rather than an affine parallelogram guess. That is what
//! gives the sampler its (size-graded) tilt tolerance.
//!
//! Size and chirality are confirmed by decoding: the sampler snaps to a handful of
//! candidate sizes and tries both grid handednesses, returning the first that passes
//! Reed–Solomon. Square symbol sizes only (10×10 … 144×144), matching the decoder.
//!
//! ## Robustness envelope
//!
//! Built and validated for the transforms in [`crate::transform`] on synthetic renders
//! at 6 px/module with the 1-module quiet zone (see `tests/datamatrix_image.rs`). Within
//! that it handles the symbol upright (pixel-exact), rotated by any angle, scaled
//! (~4.5 px/module up to 2×), blurred (σ ≤ ~1.5 px), noisy (σ ≤ ~25 luminance),
//! brightness/contrast shifted, and under perspective tilt whose tolerance shrinks with
//! symbol size (≈ 0.20 at dim 10–12, ≈ 0.10 at dim 22, ≈ 0.05 at the multi-region
//! sizes). Because the background is assumed to contain a single symbol on a clean quiet
//! zone, every dark pixel is taken to belong to the code.
//!
//! Binarization is tried global-Otsu first, then — only if that fails to decode — an
//! adaptive (Bradley local-mean) fallback, so a symbol under a lighting gradient or glare
//! that a single global threshold cannot separate from its background still decodes.

use super::DataMatrixDecoder;
use super::placement::QUIET_ZONE;
use super::tables::all_squares;
use crate::error::{Error, Result};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::imgproc::binary::BinaryImage;
use crate::imgproc::components::{Connectivity, connected_components};
use crate::imgproc::homography::Homography;
use crate::imgproc::line::{Line, fit_line_least_squares};
use crate::imgproc::sample::sample_bilinear;
use crate::imgproc::threshold::{adaptive_binarize_bradley, otsu_binarize, otsu_threshold};
use crate::output::BitMatrix;
use crate::pipeline::{Candidate, Hints};
use crate::symbol::Symbol;
use crate::symbology::Symbology;
use crate::traits::{Analyze, Detect};

/// Image-based Data Matrix scanner: finds, samples and decodes a Data Matrix symbol
/// in a [`GrayFrame`].
#[derive(Debug, Default, Clone, Copy)]
pub struct DataMatrixScanner;

impl DataMatrixScanner {
    /// A new scanner.
    pub fn new() -> Self {
        DataMatrixScanner
    }
}

impl Detect for DataMatrixScanner {
    fn detect(&self, frame: &GrayFrame<'_>, _hints: &Hints) -> Vec<Candidate> {
        match locate_geometry(frame) {
            Ok(loc) => vec![Candidate {
                location: loc.as_location(),
                symbology: Some(Symbology::DataMatrix),
                fingerprint: None,
                known: None,
            }],
            Err(_) => Vec::new(),
        }
    }
}

impl Analyze for DataMatrixScanner {
    fn analyze(&self, frame: &GrayFrame<'_>, candidate: &Candidate) -> Result<Symbol> {
        if let Some(known) = &candidate.known {
            return Ok(known.clone());
        }
        scan(frame)
    }
}

/// Locate, sample and structurally decode the Data Matrix symbol in `frame`.
pub fn scan(frame: &GrayFrame<'_>) -> Result<Symbol> {
    solve(frame).map(|(_, symbol)| symbol)
}

/// Locate the Data Matrix symbol in `frame` and sample it to a clean [`BitMatrix`].
///
/// The returned grid is the one that decoded successfully (correct size and
/// chirality), ready to hand to [`DataMatrixDecoder`](super::DataMatrixDecoder).
pub fn sample_grid(frame: &GrayFrame<'_>) -> Result<BitMatrix> {
    solve(frame).map(|(located, _)| located.matrix)
}

// ===================================================================================
// Located symbol
// ===================================================================================

/// A fully located + sampled symbol: the winning grid plus geometry for reporting.
struct Located {
    matrix: BitMatrix,
    corners: [Point; 4],
    dim: usize,
}

impl Located {
    fn as_location(&self) -> Location {
        let [tl, tr, _, bl] = self.corners;
        let rotation = (tr.y - tl.y).atan2(tr.x - tl.x);
        let side = (tl.distance(tr) + tl.distance(bl)) / 2.0;
        Location {
            outline: Quad::new(self.corners),
            rotation: Some(rotation),
            module_size: Some(side / self.dim as f32),
        }
    }
}

// ===================================================================================
// Geometry extraction
// ===================================================================================

/// The recovered symbol geometry, before size/chirality are confirmed by decoding.
///
/// `bl` is the L corner (bottom-left in the symbol frame); `tl`/`br` are the far ends
/// of the two solid edges (their top-left / bottom-right labelling is the ambiguous
/// chirality, resolved by decoding); `tr` is the refined fourth corner (intersection
/// of the two fitted timing-border lines).
struct Geometry {
    bl: Point,
    tl: Point,
    br: Point,
    tr: Point,
    threshold: u8,
    n_est: usize,
}

/// Minimum frame side (pixels) worth attempting.
const MIN_FRAME: usize = 10;

/// The binarizations tried, in order: global Otsu first (exact on clean renders), then
/// an adaptive local one as a fallback for unevenly-lit real captures. The sampling
/// threshold stays the global Otsu value; only the blob-isolation bitmap changes.
fn binarizations(frame: &GrayFrame<'_>) -> [BinaryImage; 2] {
    let radius = (frame.width().min(frame.height()) / 8).clamp(8, 50);
    [
        otsu_binarize(frame),
        adaptive_binarize_bradley(frame, radius, 0.10),
    ]
}

/// Full detect + geometry + size/chirality search + structural decode.
fn solve(frame: &GrayFrame<'_>) -> Result<(Located, Symbol)> {
    let threshold = otsu_threshold(frame);
    for bin in binarizations(frame) {
        if let Ok(result) = solve_with(frame, &bin, threshold) {
            return Ok(result);
        }
    }
    Err(Error::undecodable("no candidate Data Matrix grid decoded"))
}

/// Full solve under one already-built binarization.
fn solve_with(
    frame: &GrayFrame<'_>,
    bin: &BinaryImage,
    threshold: u8,
) -> Result<(Located, Symbol)> {
    let geom = extract_geometry(frame, bin, threshold)?;
    let decoder = DataMatrixDecoder::new();

    // Try the nearest valid square sizes, and both grid handednesses, returning the
    // first that survives Reed–Solomon. RS makes a false accept astronomically
    // unlikely, so the first success is the true reading.
    for &dim in &candidate_sizes(geom.n_est) {
        for swap in [false, true] {
            let dst = corner_targets(&geom, swap);
            let src = grid_corners(dim);
            let Ok(h) = Homography::from_correspondences(src, dst) else {
                continue;
            };
            let matrix =
                crate::imgproc::sample::sample_grid(frame, &h, dim, geom.threshold, QUIET_ZONE);
            if let Ok(symbol) = decoder.decode_matrix(&matrix) {
                let located = Located {
                    matrix,
                    corners: dst,
                    dim,
                };
                return Ok((located, symbol));
            }
        }
    }
    Err(Error::undecodable("no candidate Data Matrix grid decoded"))
}

/// Geometry-only location (no decode) for the cheap detection pass. Uses the snapped
/// size estimate and the un-swapped chirality; the reported outline is orientation
/// independent.
fn locate_geometry(frame: &GrayFrame<'_>) -> Result<Located> {
    let threshold = otsu_threshold(frame);
    let mut last = Error::undecodable("no Data Matrix geometry recovered");
    for bin in binarizations(frame) {
        match locate_geometry_with(frame, &bin, threshold) {
            Ok(loc) => return Ok(loc),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Geometry-only location under one already-built binarization.
fn locate_geometry_with(
    frame: &GrayFrame<'_>,
    bin: &BinaryImage,
    threshold: u8,
) -> Result<Located> {
    let geom = extract_geometry(frame, bin, threshold)?;
    let dim = candidate_sizes(geom.n_est)[0];
    let dst = corner_targets(&geom, false);
    let src = grid_corners(dim);
    let h = Homography::from_correspondences(src, dst)
        .map_err(|_| Error::undecodable("degenerate Data Matrix corners"))?;
    let matrix = crate::imgproc::sample::sample_grid(frame, &h, dim, geom.threshold, QUIET_ZONE);
    Ok(Located {
        matrix,
        corners: dst,
        dim,
    })
}

/// The four grid-space source corners for a `dim × dim` symbol, in TL, TR, BR, BL
/// order (matching [`corner_targets`] and the module-centre convention of
/// [`crate::imgproc::sample::sample_grid`]).
fn grid_corners(dim: usize) -> [Point; 4] {
    let d = dim as f32;
    [
        Point::new(0.0, 0.0),
        Point::new(d, 0.0),
        Point::new(d, d),
        Point::new(0.0, d),
    ]
}

/// The four image-space target corners in TL, TR, BR, BL order. `swap` flips the two
/// solid-edge endpoints to try the opposite grid handedness.
fn corner_targets(geom: &Geometry, swap: bool) -> [Point; 4] {
    if swap {
        [geom.br, geom.tr, geom.tl, geom.bl]
    } else {
        [geom.tl, geom.tr, geom.br, geom.bl]
    }
}

/// Extract corners, the L orientation, a size estimate and the binarization threshold
/// from a supplied binarization `bin` (and its global sampling `threshold`).
fn extract_geometry(frame: &GrayFrame<'_>, bin: &BinaryImage, threshold: u8) -> Result<Geometry> {
    if frame.width() < MIN_FRAME || frame.height() < MIN_FRAME {
        return Err(Error::undecodable(
            "frame too small for a Data Matrix symbol",
        ));
    }

    // The solid L finder forms the largest dark component; its bounding box covers the
    // whole symbol (the L spans a full column and a full row), so restrict the corner
    // search to it and drop any stray noise elsewhere.
    let comps = connected_components(bin, Connectivity::Eight);
    let symbol = comps
        .iter()
        .max_by_key(|c| c.area)
        .ok_or_else(|| Error::undecodable("no dark region found"))?;
    if symbol.area < 16 {
        return Err(Error::undecodable("dark region too small for a symbol"));
    }
    let b = symbol.bounds;
    let pad = 2usize;
    let x0 = b.min_x.saturating_sub(pad);
    let y0 = b.min_y.saturating_sub(pad);
    let x1 = (b.max_x + pad).min(bin.width() - 1);
    let y1 = (b.max_y + pad).min(bin.height() - 1);

    let corners = find_corners(bin, x0, y0, x1, y1)?;
    let centroid = centroid_of(&corners);

    // Identify the solid L: the vertex whose two incident edges are the darkest.
    let dark: [f32; 4] = [
        edge_darkness(bin, corners[0], corners[1], centroid),
        edge_darkness(bin, corners[1], corners[2], centroid),
        edge_darkness(bin, corners[2], corners[3], centroid),
        edge_darkness(bin, corners[3], corners[0], centroid),
    ];
    // Vertex i is shared by edge (i-1) and edge i.
    let mut l_vertex = 0usize;
    let mut best = -1.0f32;
    for i in 0..4 {
        let s = dark[(i + 3) % 4] + dark[i];
        if s > best {
            best = s;
            l_vertex = i;
        }
    }

    // Rough module size from a first timing count (the parallelogram fourth corner is
    // accurate enough for *counting* even if not for sampling), then refine all four
    // corners as intersections of edge lines fit to the symbol's dark boundary. That
    // recovers a perspective-correct fourth corner (the two timing borders' meeting
    // point), which the parallelogram rule cannot.
    let tl0 = corners[(l_vertex + 3) % 4];
    let br0 = corners[(l_vertex + 1) % 4];
    let bl0 = corners[l_vertex];
    let tr0 = Point::new(tl0.x + br0.x - bl0.x, tl0.y + br0.y - bl0.y);
    let side = (bl0.distance(tl0) + bl0.distance(br0)) / 2.0;
    let n_rough = (count_modules(frame, tl0, tr0, centroid, threshold)
        + count_modules(frame, br0, tr0, centroid, threshold))
    .div_ceil(2)
    .max(4);
    let module_px = (side / n_rough as f32).max(1.0);

    // Seed the fourth (timing-meeting) corner with the parallelogram estimate before
    // refining: the raw farthest-point detection recesses it inward by up to a module,
    // which would skew the two timing edges' boundary search.
    let mut init = corners;
    init[(l_vertex + 2) % 4] = tr0;
    let refined = refine_corners(bin, &init, centroid, module_px);

    let bl = refined[l_vertex];
    let tl = refined[(l_vertex + 3) % 4];
    let br = refined[(l_vertex + 1) % 4];
    let tr = refined[(l_vertex + 2) % 4];

    // Final module count from the refined timing edges.
    let m1 = count_modules(frame, tl, tr, centroid, threshold);
    let m2 = count_modules(frame, br, tr, centroid, threshold);
    let n_est = (m1 + m2).div_ceil(2); // rounded average

    Ok(Geometry {
        bl,
        tl,
        br,
        tr,
        threshold,
        n_est,
    })
}

/// Centroid of four points.
fn centroid_of(pts: &[Point; 4]) -> Point {
    let (sx, sy) = pts
        .iter()
        .fold((0.0f32, 0.0f32), |(sx, sy), p| (sx + p.x, sy + p.y));
    Point::new(sx / 4.0, sy / 4.0)
}

/// Recover the four corners of the (convex) symbol quad from the dark-pixel cloud in
/// the bounding window, using a rotation-robust farthest-point construction.
fn find_corners(
    bin: &BinaryImage,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
) -> Result<[Point; 4]> {
    // Pass 1: centroid of dark pixels.
    let mut sx = 0.0f64;
    let mut sy = 0.0f64;
    let mut n = 0u64;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if bin.get(x, y) {
                sx += x as f64;
                sy += y as f64;
                n += 1;
            }
        }
    }
    if n < 16 {
        return Err(Error::undecodable("too few dark pixels for a symbol"));
    }
    let centroid = Point::new((sx / n as f64) as f32, (sy / n as f64) as f32);

    let p0 = farthest_from(bin, x0, y0, x1, y1, centroid);
    let p2 = farthest_from(bin, x0, y0, x1, y1, p0);
    // The other two corners maximise |perpendicular distance| to the P0–P2 diagonal,
    // one on each side.
    let ax = p2.x - p0.x;
    let ay = p2.y - p0.y;
    let mut best_pos = 0.0f32;
    let mut best_neg = 0.0f32;
    let mut p1 = p0;
    let mut p3 = p0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if !bin.get(x, y) {
                continue;
            }
            let cross = ax * (y as f32 - p0.y) - ay * (x as f32 - p0.x);
            if cross > best_pos {
                best_pos = cross;
                p1 = Point::new(x as f32, y as f32);
            } else if cross < best_neg {
                best_neg = cross;
                p3 = Point::new(x as f32, y as f32);
            }
        }
    }

    let mut pts = [p0, p1, p2, p3];
    order_clockwise(&mut pts, centroid);
    // Reject a degenerate (collinear / coincident) result.
    if quad_area(&pts) < 4.0 {
        return Err(Error::undecodable("degenerate symbol quad"));
    }
    Ok(pts)
}

/// Refine the four quad corners as intersections of straight lines fit to the
/// symbol's dark boundary along each edge. Corner order is preserved. Recovering true
/// edge lines makes the fourth corner perspective-correct and every corner sub-pixel.
fn refine_corners(
    bin: &BinaryImage,
    corners: &[Point; 4],
    centroid: Point,
    module_px: f32,
) -> [Point; 4] {
    let lines: [Line; 4] = std::array::from_fn(|i| {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        edge_line(bin, a, b, centroid, module_px).unwrap_or_else(|| {
            Line::from_points(a, b).unwrap_or(Line {
                a: 1.0,
                b: 0.0,
                c: -a.x,
            })
        })
    });
    // New corner i is where edge (i-1) meets edge i.
    std::array::from_fn(|i| {
        let prev = lines[(i + 3) % 4];
        let cur = lines[i];
        intersect(prev, cur).unwrap_or(corners[i])
    })
}

/// Fit a line to the outer dark boundary of the symbol along edge `a → b`.
///
/// At sample positions across the central span of the edge, march along the outward
/// normal and keep the outermost dark pixel — for a solid edge that is the finder
/// boundary, for a timing edge it is the outer face of each dark timing module (light
/// positions simply contribute no point). A least-squares fit over those boundary
/// points recovers the true edge line.
fn edge_line(
    bin: &BinaryImage,
    a: Point,
    b: Point,
    centroid: Point,
    module_px: f32,
) -> Option<Line> {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 2.0 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    // Outward unit normal (pointing away from the centroid).
    let mid = Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
    let mut nx = -uy;
    let mut ny = ux;
    if nx * (mid.x - centroid.x) + ny * (mid.y - centroid.y) < 0.0 {
        nx = -nx;
        ny = -ny;
    }
    let steps = (len as usize).clamp(16, 2048);
    let out_range = 1.5 * module_px;
    let in_range = 0.4 * module_px;
    let mut pts: Vec<Point> = Vec::new();
    for s in 0..steps {
        // Central 84 % of the edge to stay clear of the corners.
        let t = 0.08 + 0.84 * (s as f32 + 0.5) / steps as f32;
        let bx = a.x + ux * (len * t);
        let by = a.y + uy * (len * t);
        // Find the outermost dark pixel along the normal.
        let mut found: Option<Point> = None;
        let mut d = out_range;
        while d >= -in_range {
            let px = bx + nx * d;
            let py = by + ny * d;
            if px >= 0.0 && py >= 0.0 && bin.get(px.round() as usize, py.round() as usize) {
                found = Some(Point::new(px, py));
                break;
            }
            d -= 0.5;
        }
        if let Some(p) = found {
            pts.push(p);
        }
    }
    if pts.len() < 4 {
        return None;
    }
    fit_line_least_squares(&pts)
}

/// Intersection of two lines in implicit form, or `None` if near-parallel.
fn intersect(l0: Line, l1: Line) -> Option<Point> {
    let det = l0.a * l1.b - l1.a * l0.b;
    if det.abs() < 1e-6 {
        return None;
    }
    let x = (-l0.c * l1.b + l1.c * l0.b) / det;
    let y = (-l0.a * l1.c + l1.a * l0.c) / det;
    Some(Point::new(x, y))
}

/// The dark pixel in the window farthest from `p`.
fn farthest_from(bin: &BinaryImage, x0: usize, y0: usize, x1: usize, y1: usize, p: Point) -> Point {
    let mut best = p;
    let mut best_d = -1.0f32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if !bin.get(x, y) {
                continue;
            }
            let dx = x as f32 - p.x;
            let dy = y as f32 - p.y;
            let d = dx * dx + dy * dy;
            if d > best_d {
                best_d = d;
                best = Point::new(x as f32, y as f32);
            }
        }
    }
    best
}

/// Order four points clockwise around `centroid` (image coordinates, y down).
fn order_clockwise(pts: &mut [Point; 4], centroid: Point) {
    pts.sort_by(|a, b| {
        let aa = (a.y - centroid.y).atan2(a.x - centroid.x);
        let bb = (b.y - centroid.y).atan2(b.x - centroid.x);
        aa.partial_cmp(&bb).unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Shoelace area of a quad.
fn quad_area(p: &[Point; 4]) -> f32 {
    let mut s = 0.0f32;
    for i in 0..4 {
        let a = p[i];
        let b = p[(i + 1) % 4];
        s += a.x * b.y - b.x * a.y;
    }
    (s / 2.0).abs()
}

/// Inset distance (pixels) used when probing just inside an edge.
const INSET: f32 = 2.0;

/// Fraction of samples along edge `a → b` that read dark, probed a small distance
/// inside the symbol. Solid finder edges read ~1.0; timing edges read ~0.5.
fn edge_darkness(bin: &BinaryImage, a: Point, b: Point, centroid: Point) -> f32 {
    let n = 24usize;
    let mut dark = 0usize;
    for k in 0..n {
        // Avoid the corners themselves (probe the central 70 % of the edge).
        let s = 0.15 + 0.7 * (k as f32 + 0.5) / n as f32;
        let px = a.x + (b.x - a.x) * s;
        let py = a.y + (b.y - a.y) * s;
        let (ix, iy) = inset_toward(px, py, centroid, INSET);
        if bin.get(ix, iy) {
            dark += 1;
        }
    }
    dark as f32 / n as f32
}

/// Count the modules along a timing edge `a → b` by counting dark/light transitions of
/// a finely super-sampled luminance profile probed just inside the edge.
fn count_modules(
    frame: &GrayFrame<'_>,
    a: Point,
    b: Point,
    centroid: Point,
    threshold: u8,
) -> usize {
    let len = a.distance(b);
    let k = ((len * 2.0) as usize).clamp(32, 4096);
    let t = threshold as f64;
    let mut prev: Option<bool> = None;
    let mut transitions = 0usize;
    for i in 0..k {
        let s = (i as f32 + 0.5) / k as f32;
        let px = a.x + (b.x - a.x) * s;
        let py = a.y + (b.y - a.y) * s;
        let (fx, fy) = inset_toward_f(px, py, centroid, INSET);
        let dark = sample_bilinear(frame, fx, fy) <= t;
        if let Some(p) = prev
            && p != dark
        {
            transitions += 1;
        }
        prev = Some(dark);
    }
    transitions + 1
}

/// Move point `(px, py)` `dist` pixels toward `centroid`, returning rounded pixel
/// indices (clamped at 0).
fn inset_toward(px: f32, py: f32, centroid: Point, dist: f32) -> (usize, usize) {
    let (fx, fy) = inset_toward_f(px, py, centroid, dist);
    (fx.max(0.0) as usize, fy.max(0.0) as usize)
}

/// Move `(px, py)` `dist` pixels toward `centroid`, returning fractional coordinates.
fn inset_toward_f(px: f32, py: f32, centroid: Point, dist: f32) -> (f64, f64) {
    let dx = centroid.x - px;
    let dy = centroid.y - py;
    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
    ((px + dx / len * dist) as f64, (py + dy / len * dist) as f64)
}

/// Valid square sizes nearest to `n_est`, closest first (up to three).
fn candidate_sizes(n_est: usize) -> Vec<usize> {
    let mut sizes: Vec<usize> = all_squares().iter().map(|s| s.symbol_size).collect();
    sizes.sort_by_key(|&s| (s as isize - n_est as isize).abs());
    sizes.truncate(3);
    sizes
}
