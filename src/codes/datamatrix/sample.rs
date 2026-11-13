//! Data Matrix image sampling: pixels → [`BitMatrix`] → [`Symbol`].
//!
//! The structural [`DataMatrixDecoder`](super::DataMatrixDecoder) consumes a clean
//! module grid. This module is the front-end that recovers such a grid from a real
//! image. Unlike QR, a Data Matrix carries no repeated finder rings to cross-correlate;
//! its fiducials are the two **solid** edges of the L-shaped finder and the two
//! opposite **timing** borders (alternating modules). The sampler:
//!
//! 1. binarizes the frame (global Otsu) and isolates the symbol blob (the L finder is
//!    solid, so it is one connected component reaching three of the symbol's corners);
//! 2. recovers the symbol's corners from the dark-pixel cloud (a rotation-robust
//!    farthest-point method, so no axis-aligned assumption);
//! 3. identifies the solid L by measuring per-edge darkness — the two adjacent edges
//!    that read ~100 % dark are the finder; their shared vertex is the L corner
//!    (bottom-left in the symbol's own frame);
//! 4. seeds the fourth corner — where the two timing borders meet, which the L does
//!    not reach and which is no locatable pattern — and refines all four to sub-pixel
//!    accuracy by fitting straight lines to the symbol's dark boundary along each edge
//!    and intersecting adjacent lines;
//! 5. estimates the module count per axis by counting transitions along the timing
//!    edges, snapping to the nearest valid sizes — square where the two counts
//!    agree, rectangular where they do not;
//! 6. builds a [`Homography`] from the four corners and samples every module centre
//!    into a [`BitMatrix`], which the structural decoder then error-corrects.
//!
//! The boundary fit only looks a module or so either side of where it is seeded, so
//! the seed for the fourth corner decides how much perspective the sampler takes. It is
//! made twice, and each seed followed through to a grid of its own. First the
//! parallelogram completion of the L's three corners: exact for a symbol seen square-on
//! at any rotation. Then the meeting point of the tangents from the L's two ends to the
//! symbol's ink ([`far_corner_seed`]): the timing borders themselves, wherever
//! perspective has put them — a third of a side away from the completion under a 30 %
//! foreshortening.
//!
//! Size and chirality are confirmed by decoding: the sampler snaps to a handful of
//! candidate sizes and tries both grid handednesses (a rectangle has one: its long
//! timing border is its top row), returning the first that wears a finder (solid L,
//! timing borders not badly out of step) and passes Reed–Solomon.
//!
//! The rectangular sizes are held to the same envelope as the squares, with one
//! exception: the 8-row DMRE sizes of 64 columns and more are read square-on at any
//! rotation but not under perspective, where four corners no longer pin down the
//! foreshortening along such a long row.
//!
//! ## Robustness envelope
//!
//! Built and validated for the transforms in [`crate::transform`] on synthetic renders
//! at 6 px/module with the 1-module quiet zone (see `tests/datamatrix_image.rs`). Within
//! that it handles the symbol upright (pixel-exact), rotated by any angle, scaled
//! (~4.5 px/module up to 2×), blurred (σ ≤ ~1.5 px), noisy (σ ≤ ~25 luminance),
//! brightness/contrast shifted, and under perspective tilt up to a 30 % foreshortening
//! of either side at any rotation. The symbol is taken to be one of the largest dark
//! connected components; each of the largest few plausibly shaped ones is tried in
//! turn — a solid block, dark on all four sides, is not one — so a dark background
//! around the label or print beside the code does not displace it, and print a quiet
//! zone away from the timing borders is not taken for them. Dark clutter *inside* the
//! symbol's quad, or touching its L, still skews the corners.
//!
//! Binarization is tried global-Otsu first, then — only if that fails to decode — an
//! adaptive (Bradley local-mean) fallback, so a symbol under a lighting gradient or glare
//! that a single global threshold cannot separate from its background is still found.
//! Its modules are then read against the frame-wide threshold or, where that one is
//! busy telling the lit half of the scene from the shaded half, the Otsu threshold of
//! the symbol's own box ([`local_threshold`]).

use super::DataMatrixDecoder;
use super::placement::QUIET_ZONE;
use super::tables::{all_specs, all_squares};
use crate::error::{Error, Result};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::imgproc::binary::BinaryImage;
use crate::imgproc::components::{Connectivity, connected_components, flood_region};
use crate::imgproc::homography::Homography;
use crate::imgproc::line::{Line, fit_line_least_squares};
use crate::imgproc::sample::{sample_bilinear, sample_grid_rect};
use crate::imgproc::threshold::{adaptive_binarize_bradley, otsu_binarize, otsu_threshold};
use crate::output::BitMatrix;
use crate::pipeline::{Candidate, Hints};
use crate::symbol::Symbol;
use crate::symbology::Symbology;
use crate::traits::{Analyze, Detect};
use alloc::{vec, vec::Vec};

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
    solve(frame).map(|(located, mut symbol)| {
        // Every read says where it is: a live front-end has nothing to draw otherwise.
        symbol.location.get_or_insert_with(|| located.as_location());
        symbol
    })
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
}

impl Located {
    fn as_location(&self) -> Location {
        let [tl, tr, _, bl] = self.corners;
        let rotation = (tr.y - tl.y).atan2(tr.x - tl.x);
        let across = tl.distance(tr) / self.matrix.width() as f32;
        let down = tl.distance(bl) / self.matrix.height() as f32;
        Location {
            outline: Quad::new(self.corners),
            rotation: Some(rotation),
            module_size: Some((across + down) / 2.0),
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
    /// A point inside the symbol, toward which the timing borders are probed.
    centroid: Point,
    /// Sampling thresholds to try, in order: the one the geometry was fit under, then
    /// the other of the frame's Otsu threshold and the symbol's own (see
    /// [`local_threshold`]).
    thresholds: [u8; 2],
}

impl Geometry {
    /// Modules along the two timing borders under `threshold`: the one from `tl`,
    /// then the one from `br`.
    fn module_counts(&self, frame: &GrayFrame<'_>, threshold: u8) -> (usize, usize) {
        (
            count_modules(frame, self.tl, self.tr, self.centroid, threshold),
            count_modules(frame, self.br, self.tr, self.centroid, threshold),
        )
    }
}

/// Minimum frame side (pixels) worth attempting.
const MIN_FRAME: usize = 10;

/// The binarizations tried, in order: global Otsu first (exact on clean renders), then
/// an adaptive local one as a fallback for unevenly-lit real captures. Only the
/// blob-isolation bitmap changes; the sampling thresholds are the same under both.
fn binarizations(frame: &GrayFrame<'_>) -> [BinaryImage; 2] {
    let radius = (frame.width().min(frame.height()) / 8).clamp(8, 50);
    [
        otsu_binarize(frame),
        adaptive_binarize_bradley(frame, radius, 0.10),
    ]
}

/// Full detect + geometry + size/chirality search + structural decode.
///
/// A small frame that yields nothing is read once more at twice the size: the
/// geometry probes work a couple of pixels in from each edge, which at two pixels a
/// module — a symbol as software writes it to a file — is already the next module.
fn solve(frame: &GrayFrame<'_>) -> Result<(Located, Symbol)> {
    if let Ok(result) = solve_at_scale(frame) {
        return Ok(result);
    }
    let (w, h) = (frame.width(), frame.height());
    if w * h <= UPSCALE_MAX_PIXELS && w >= MIN_FRAME && h >= MIN_FRAME {
        let mut big = vec![0u8; 4 * w * h];
        for y in 0..2 * h {
            for x in 0..2 * w {
                big[y * 2 * w + x] = frame.get_unchecked(x / 2, y / 2);
            }
        }
        if let Ok(doubled) = GrayFrame::new(&big, 2 * w, 2 * h)
            && let Ok((mut located, symbol)) = solve_at_scale(&doubled)
        {
            // Pixel centres sit on whole coordinates, in both frames.
            located.corners = located
                .corners
                .map(|p| Point::new((p.x - 0.5) / 2.0, (p.y - 0.5) / 2.0));
            return Ok((located, symbol));
        }
    }
    Err(Error::undecodable("no candidate Data Matrix grid decoded"))
}

/// The largest frame, in pixels, that [`solve`] reads a second time at twice the
/// size. Above it a symbol of two pixels a module is a small part of the picture,
/// and the second pass a cost every frame without a symbol would pay.
const UPSCALE_MAX_PIXELS: usize = 320 * 200;

/// [`solve`] on the frame as given.
fn solve_at_scale(frame: &GrayFrame<'_>) -> Result<(Located, Symbol)> {
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
    let decoder = DataMatrixDecoder::new();
    for geom in extract_geometries(frame, bin, threshold)? {
        for (i, &threshold) in geom.thresholds.iter().enumerate() {
            // The second threshold is worth a try only where it reads differently.
            if i == 1 && threshold.abs_diff(geom.thresholds[0]) < 4 {
                continue;
            }
            // Try the nearest valid sizes, and for a square both grid handednesses,
            // returning the first that wears a finder and survives Reed–Solomon.
            let (m1, m2) = geom.module_counts(frame, threshold);
            for (width, height, swap) in candidate_grids(m1, m2) {
                let dst = corner_targets(&geom, swap);
                let src = grid_corners(width, height);
                let Ok(h) = Homography::from_correspondences(src, dst) else {
                    continue;
                };
                let matrix = sample_grid_rect(frame, &h, width, height, threshold, QUIET_ZONE);
                // Reed–Solomon alone will, once in a great while, find a codeword
                // in the texture of something else; a symbol also wears its finder.
                // The bounds are loose — a grainy L misreads up to a third of its
                // modules, and a grid a hair short samples its timing borders in
                // the quiet zone (every dark module of theirs "wrong", half the
                // border) while the data inside still corrects.
                let (l_bad, t_bad) = finder_errors(&matrix);
                let around = width + height;
                if 5 * l_bad > 2 * (around - 1) || 5 * t_bad > 3 * (around - 3) {
                    continue;
                }
                if let Ok(symbol) = decoder.decode_matrix(&matrix) {
                    let located = Located {
                        matrix,
                        corners: dst,
                    };
                    return Ok((located, symbol));
                }
            }
        }
    }
    Err(Error::undecodable("no candidate Data Matrix grid decoded"))
}

/// How far a sampled grid is from wearing a Data Matrix finder: the light modules
/// along its solid L (left column, bottom row) and the modules out of step along its
/// timing borders (top row, right column).
fn finder_errors(matrix: &BitMatrix) -> (usize, usize) {
    let (w, h) = (matrix.width(), matrix.height());
    let mut l_bad = 0;
    let mut t_bad = 0;
    for y in 0..h {
        l_bad += !matrix.get(0, y) as usize;
        if y + 1 < h && y > 0 {
            t_bad += (matrix.get(w - 1, y) != (y % 2 == 1)) as usize;
        }
    }
    for x in 0..w {
        t_bad += (matrix.get(x, 0) != (x % 2 == 0)) as usize;
        if x > 0 {
            l_bad += !matrix.get(x, h - 1) as usize;
        }
    }
    (l_bad, t_bad)
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
    let geom = extract_geometries(frame, bin, threshold)?
        .into_iter()
        .next()
        .ok_or_else(|| Error::undecodable("no Data Matrix geometry recovered"))?;
    let threshold = geom.thresholds[0];
    let (m1, m2) = geom.module_counts(frame, threshold);
    let (width, height, swap) = candidate_grids(m1, m2)[0];
    let dst = corner_targets(&geom, swap);
    let src = grid_corners(width, height);
    let h = Homography::from_correspondences(src, dst)
        .map_err(|_| Error::undecodable("degenerate Data Matrix corners"))?;
    let matrix = sample_grid_rect(frame, &h, width, height, threshold, QUIET_ZONE);
    Ok(Located {
        matrix,
        corners: dst,
    })
}

/// The four grid-space source corners for a `width × height` symbol, in TL, TR, BR,
/// BL order (matching [`corner_targets`] and the module-centre convention of
/// [`crate::imgproc::sample::sample_grid`]).
fn grid_corners(width: usize, height: usize) -> [Point; 4] {
    let (w, h) = (width as f32, height as f32);
    [
        Point::new(0.0, 0.0),
        Point::new(w, 0.0),
        Point::new(w, h),
        Point::new(0.0, h),
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

/// Dark components tried as the symbol, largest first.
const MAX_COMPONENTS: usize = 6;

/// How elongated a component's box may be: the widest rectangular symbol is 8×144
/// modules, which a rotation only makes squarer.
const MAX_ASPECT: usize = 20;

/// Symbol geometries for the largest plausible dark components, in that order.
fn extract_geometries(
    frame: &GrayFrame<'_>,
    bin: &BinaryImage,
    threshold: u8,
) -> Result<Vec<Geometry>> {
    if frame.width() < MIN_FRAME || frame.height() < MIN_FRAME {
        return Err(Error::undecodable(
            "frame too small for a Data Matrix symbol",
        ));
    }

    // The solid L finder is one dark component reaching three of the symbol's corners.
    // On a clean crop it is the largest component; in a scene the dark surface behind
    // the label or a block of print beside it can be larger, so the largest few that
    // could be a symbol — a box at least ten modules of a pixel each way, not a thin
    // rule — are all tried.
    let mut comps = connected_components(bin, Connectivity::Eight);
    comps.sort_by_key(|c| core::cmp::Reverse(c.area));
    let geoms: Vec<Geometry> = comps
        .iter()
        .filter(|c| {
            let (w, h) = (c.bounds.width(), c.bounds.height());
            c.area >= 16
                && w >= MIN_FRAME
                && h >= MIN_FRAME
                && w <= MAX_ASPECT * h
                && h <= MAX_ASPECT * w
        })
        .take(MAX_COMPONENTS)
        .flat_map(|c| extract_geometry(frame, bin, threshold, &c.bounds, c.area))
        .collect();
    if geoms.is_empty() {
        return Err(Error::undecodable("no dark region shaped like a symbol"));
    }
    Ok(geoms)
}

/// Whether `(x, y)` lies in the convex, consistently wound `quad` (edges included).
fn in_quad(quad: &[Point; 4], x: f32, y: f32) -> bool {
    // Same side of all four edges.
    let mut sign = 0.0f32;
    (0..4).all(|i| {
        let (a, c) = (quad[i], quad[(i + 1) % 4]);
        let cross = (c.x - a.x) * (y - a.y) - (c.y - a.y) * (x - a.x);
        if cross == 0.0 {
            return true;
        }
        if sign == 0.0 {
            sign = cross.signum();
        }
        cross.signum() == sign
    })
}

/// `quad` grown about `centre` by the factor `k`, plus two pixels.
fn grown(quad: &[Point; 4], centre: Point, k: f32) -> [Point; 4] {
    quad.map(|p| {
        let (dx, dy) = (p.x - centre.x, p.y - centre.y);
        let k = k + 2.0 / dx.hypot(dy).max(1.0);
        Point::new(centre.x + dx * k, centre.y + dy * k)
    })
}

/// The pixel box `(x0, y0, x1, y1)` of `quad`, clamped to a `width × height` image.
fn quad_box(quad: &[Point; 4], width: usize, height: usize) -> (usize, usize, usize, usize) {
    let span = |f: fn(&Point) -> f32, limit: usize| {
        let lo = quad.iter().map(f).fold(f32::MAX, f32::min);
        let hi = quad.iter().map(f).fold(f32::MIN, f32::max);
        (
            lo.floor().clamp(0.0, limit as f32) as usize,
            hi.ceil().clamp(0.0, limit as f32) as usize,
        )
    };
    let (x0, x1) = span(|p| p.x, width - 1);
    let (y0, y1) = span(|p| p.y, height - 1);
    (x0, y0, x1, y1)
}

/// Symbol geometries for the dark component bounded by `b`: one per estimate of its
/// far corner.
fn extract_geometry(
    frame: &GrayFrame<'_>,
    bin: &BinaryImage,
    threshold: u8,
    b: &crate::imgproc::components::BoundingBox,
    area: usize,
) -> Vec<Geometry> {
    let pad = 2usize;
    let x0 = b.min_x.saturating_sub(pad);
    let y0 = b.min_y.saturating_sub(pad);
    let x1 = (b.max_x + pad).min(bin.width() - 1);
    let y1 = (b.max_y + pad).min(bin.height() - 1);

    // Everything below reasons about "the symbol's dark boundary", so ink that is not
    // the symbol's — print beside the code, the dark surface behind a label — must not
    // be allowed to attract the corner search or the edge-line fits, which otherwise
    // lock onto it and skew the grid. Two stages: a first quad from the L component's
    // *own* pixels (the box of a rotated symbol has other ink in its corners), then the
    // working mask is the binarization clipped to that quad, slightly grown — which
    // brings back the symbol's modules that are not connected to the L (timing dots,
    // interior islands) and nothing else.
    // The component's pixels: flood from a dark pixel on its top row. Another component
    // can share that row, so floods are tried until one has this component's area.
    let mut component = Vec::new();
    let mut x = b.min_x;
    while x <= b.max_x {
        if bin.get(x, b.min_y) {
            let pixels = flood_region(bin, (x, b.min_y), true);
            if pixels.len() == area {
                component = pixels;
                break;
            }
            // Skip the rest of this dark run: it floods to the same region.
            while x <= b.max_x && bin.get(x, b.min_y) {
                x += 1;
            }
        } else {
            x += 1;
        }
    }
    let mut own = BinaryImage::new(bin.width(), bin.height());
    for &(x, y) in &component {
        own.set(x, y, true);
    }
    let Ok(rough) = find_corners(&own, x0, y0, x1, y1) else {
        return Vec::new();
    };
    let centre = centroid_of(&rough);
    // The L reaches three of the symbol's corners; the fourth — where the two timing
    // borders meet — may not be connected to it at all, leaving that corner of the
    // rough quad recessed (or somewhere inside the symbol). The L's vertex is the
    // corner whose two edges are solid; the corner opposite it is replaced by the
    // parallelogram completion of the other three when that lies farther out.
    let solid: [f32; 4] =
        core::array::from_fn(|i| edge_darkness(&own, rough[i], rough[(i + 1) % 4], centre));
    let vertex = (0..4)
        .max_by(|&i, &j| {
            (solid[(i + 3) % 4] + solid[i]).total_cmp(&(solid[(j + 3) % 4] + solid[j]))
        })
        .unwrap_or(0);
    let far = (vertex + 2) % 4;
    // A solid block — the dark surface a label sits on, a logo — is dark along all
    // four sides; a finder L only along two. (And the block is the costly component:
    // most of the frame's ink.)
    if solid[(vertex + 1) % 4].min(solid[(vertex + 2) % 4]) > 0.9 {
        return Vec::new();
    }
    let mut hull = rough;
    let completed = Point::new(
        rough[(far + 3) % 4].x + rough[(far + 1) % 4].x - rough[vertex].x,
        rough[(far + 3) % 4].y + rough[(far + 1) % 4].y - rough[vertex].y,
    );
    if completed.distance(centre) > rough[far].distance(centre) {
        hull[far] = completed;
    }

    // The working mask: the binarization clipped to the quad, grown a little about its
    // centroid for blur and the rough fit.
    let clip = grown(&hull, centre, 1.08);
    let mut window = BinaryImage::new(bin.width(), bin.height());
    for y in y0..=y1 {
        for x in x0..=x1 {
            if bin.get(x, y) && in_quad(&clip, x as f32, y as f32) {
                window.set(x, y, true);
            }
        }
    }
    let Ok(corners) = find_corners(&window, x0, y0, x1, y1) else {
        return Vec::new();
    };
    let centroid = centroid_of(&corners);

    // Identify the solid L: the vertex whose two incident edges are the darkest.
    let dark: [f32; 4] = [
        edge_darkness(&window, corners[0], corners[1], centroid),
        edge_darkness(&window, corners[1], corners[2], centroid),
        edge_darkness(&window, corners[2], corners[3], centroid),
        edge_darkness(&window, corners[3], corners[0], centroid),
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
    let tl0 = corners[(l_vertex + 3) % 4];
    let br0 = corners[(l_vertex + 1) % 4];
    let bl0 = corners[l_vertex];

    // What is still missing is the far corner, where the two timing borders meet: the
    // one corner the L does not reach, and no locatable pattern itself. It is seeded
    // twice over, each seed then refined by the boundary fit into a geometry of its
    // own. Each leads with the threshold of its kind — the frame's for the plain
    // estimate, the symbol's own for the one built for hard captures — and keeps the
    // other in reserve.
    let completed = Point::new(tl0.x + br0.x - bl0.x, tl0.y + br0.y - bl0.y);
    let local = local_threshold(frame, &[bl0, tl0, completed, br0]).unwrap_or(threshold);

    // First the parallelogram completion of the L's three corners: exact for a symbol
    // seen square-on at any rotation, and it asks nothing of the timing borders.
    let mut geoms = vec![fit_geometry(
        frame,
        &window,
        corners,
        l_vertex,
        completed,
        [threshold, local],
    )];

    // Then the meeting point of the tangents from the L's two ends to the symbol's ink
    // (see [`far_corner_seed`]): right under perspective, where the completion lands
    // modules away from the real corner and out of the boundary fit's reach.
    let thickness =
        arm_thickness(&window, bl0, tl0, centroid).min(arm_thickness(&window, bl0, br0, centroid));
    if let Some(seed) = far_corner_seed(bin, bl0, tl0, br0, thickness) {
        // With the far corner known the mask is drawn tight around the whole symbol —
        // wherever that corner lies, and whatever the L's box (turned, a symbol pokes
        // its far corner out of it). Tight enough to keep the print beside the symbol
        // out of the fit; binarization speckle (a grainy capture) is dropped too.
        let mut quad = corners;
        quad[(l_vertex + 2) % 4] = seed;
        let clip = grown(&quad, centroid_of(&quad), 1.04);
        let (bx0, by0, bx1, by1) = quad_box(&clip, bin.width(), bin.height());
        let mut tight = BinaryImage::new(bin.width(), bin.height());
        for y in by0..=by1 {
            for x in bx0..=bx1 {
                if bin.get(x, y) && in_quad(&clip, x as f32, y as f32) && !is_speckle(bin, x, y) {
                    tight.set(x, y, true);
                }
            }
        }
        geoms.push(fit_geometry(
            frame,
            &tight,
            corners,
            l_vertex,
            seed,
            [local, threshold],
        ));
    }
    geoms
}

/// Whether the dark pixel `(x, y)` has fewer than two dark neighbours: binarization
/// noise rather than ink.
fn is_speckle(bin: &BinaryImage, x: usize, y: usize) -> bool {
    let mut company = 0;
    for ny in y.saturating_sub(1)..=(y + 1).min(bin.height() - 1) {
        for nx in x.saturating_sub(1)..=(x + 1).min(bin.width() - 1) {
            company += bin.get(nx, ny) as u32;
        }
    }
    company < 3
}

/// Symbol geometry from the working mask `bin` (the symbol's ink and as little else as
/// could be arranged), its rough `corners` — `l_vertex` indexing the L's — and the
/// seed `tr0` for the far corner.
fn fit_geometry(
    frame: &GrayFrame<'_>,
    bin: &BinaryImage,
    corners: [Point; 4],
    l_vertex: usize,
    tr0: Point,
    thresholds: [u8; 2],
) -> Geometry {
    let centroid = centroid_of(&corners);
    let tl0 = corners[(l_vertex + 3) % 4];
    let br0 = corners[(l_vertex + 1) % 4];
    let bl0 = corners[l_vertex];

    // Rough module size from a first timing count (the seeded fourth corner is
    // accurate enough for *counting* even if not for sampling), then refine all four
    // corners as intersections of edge lines fit to the symbol's dark boundary. That
    // makes every corner sub-pixel, the fourth included.
    let side = (bl0.distance(tl0) + bl0.distance(br0)) / 2.0;
    let m1 = count_modules(frame, tl0, tr0, centroid, thresholds[0]);
    let m2 = count_modules(frame, br0, tr0, centroid, thresholds[0]);
    let module_px = if 10 * m1.max(m2) >= 13 * m1.min(m2) {
        // A rectangle: each timing border against the solid edge it runs along.
        (bl0.distance(br0) / m1.max(4) as f32 + bl0.distance(tl0) / m2.max(4) as f32) / 2.0
    } else {
        side / (m1 + m2).div_ceil(2).max(4) as f32
    }
    .max(1.0);

    // The raw farthest-point detection recesses the fourth corner inward by up to a
    // module, which would skew the two timing edges' boundary search: the seed
    // replaces it.
    let mut init = corners;
    init[(l_vertex + 2) % 4] = tr0;
    let refined = refine_corners(bin, &init, centroid, module_px);

    Geometry {
        bl: refined[l_vertex],
        tl: refined[(l_vertex + 3) % 4],
        br: refined[(l_vertex + 1) % 4],
        tr: refined[(l_vertex + 2) % 4],
        centroid,
        thresholds,
    }
}

/// Otsu threshold of the frame over the box of `quad` alone: what separates the
/// symbol's ink from its paper, where the frame-wide threshold may be busy separating
/// the lit half of the scene from the shaded one.
fn local_threshold(frame: &GrayFrame<'_>, quad: &[Point; 4]) -> Option<u8> {
    let (x0, y0, x1, y1) = quad_box(quad, frame.width(), frame.height());
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut pixels = Vec::with_capacity(w * h);
    for y in y0..=y1 {
        pixels.extend((x0..=x1).map(|x| frame.get_unchecked(x, y)));
    }
    GrayFrame::new(&pixels, w, h)
        .ok()
        .map(|f| otsu_threshold(&f))
}

/// Thickness in pixels of the solid finder arm `a → b`: a module, give or take the
/// data modules that happen to back onto it (hence a low quartile of several probes).
fn arm_thickness(bin: &BinaryImage, a: Point, b: Point, centroid: Point) -> f32 {
    let len = a.distance(b);
    if len < 4.0 {
        return 0.0;
    }
    // Inward unit normal.
    let (mut nx, mut ny) = (-(b.y - a.y) / len, (b.x - a.x) / len);
    let mid = Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
    if nx * (centroid.x - mid.x) + ny * (centroid.y - mid.y) < 0.0 {
        (nx, ny) = (-nx, -ny);
    }
    let dark =
        |x: f32, y: f32| x >= 0.0 && y >= 0.0 && bin.get(x.round() as usize, y.round() as usize);
    let cap = (len / 4.0) as usize;
    let mut runs = [0usize; 12];
    for (i, run) in runs.iter_mut().enumerate() {
        let t = 0.25 + 0.5 * (i as f32 + 0.5) / 12.0;
        let (px, py) = (a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
        // From just outside the arm, inward: skip the light, count the dark.
        let mut d = -3.0f32;
        while d < 3.0 && !dark(px + nx * d, py + ny * d) {
            d += 1.0;
        }
        while *run < cap && dark(px + nx * d, py + ny * d) {
            *run += 1;
            d += 1.0;
        }
    }
    runs.sort_unstable();
    runs[3] as f32
}

/// A first estimate of the symbol's far corner — where its two timing borders meet —
/// from the three corners the solid L reaches and the ink in `bin`.
///
/// The symbol is a convex quad and every dark timing module touches its border, so the
/// border through `tl` is the line through `tl` that leaves all the symbol's ink on one
/// side: the tangent from `tl` to the ink over by the opposite border (and likewise
/// from `br`). The two meet at the far corner under any perspective.
///
/// Ink that is not the symbol's lies beyond its quiet zone, so each tangent is found
/// by sweeping a ray about its pivot outward from deep inside the symbol and stopping
/// at the first angular gap of about half a finder `thickness`: rays inside the symbol
/// cross many modules and always hit ink, the first clear ray grazes the border.
/// `None` when there is no ink to sweep or the borders do not meet ahead of the L.
fn far_corner_seed(
    bin: &BinaryImage,
    bl: Point,
    tl: Point,
    br: Point,
    thickness: f32,
) -> Option<Point> {
    // Oblique frame: `u` along the arm bl → tl, `v` along bl → br.
    let (ux, uy) = (tl.x - bl.x, tl.y - bl.y);
    let (vx, vy) = (br.x - bl.x, br.y - bl.y);
    let (lu, lv) = (ux.hypot(uy), vx.hypot(vy));
    let det = vx * uy - vy * ux;
    if lu < 4.0 || lv < 4.0 || det.abs() < 0.2 * lu * lv {
        return None;
    }
    // Every pixel of ink a border could pass, in arm lengths from bl: (along u, along
    // v). Binarization speckle in the quiet zone must not bridge the gap: it is left
    // out.
    let reach = |k: f32| Point::new(bl.x + k * (ux + vx), bl.y + k * (uy + vy));
    let span = [
        reach(-0.05),
        Point::new(bl.x + SWEEP_REACH * ux, bl.y + SWEEP_REACH * uy),
        reach(SWEEP_REACH),
        Point::new(bl.x + SWEEP_REACH * vx, bl.y + SWEEP_REACH * vy),
    ];
    let (x0, y0, x1, y1) = quad_box(&span, bin.width(), bin.height());
    let mut ink: Vec<(f32, f32)> = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            if !bin.get(x, y) {
                continue;
            }
            let (dx, dy) = (x as f32 - bl.x, y as f32 - bl.y);
            let pu = (vx * dy - vy * dx) / det;
            let pv = (dx * uy - dy * ux) / det;
            let ahead = (-0.05..=SWEEP_REACH).contains(&pu) && (-0.05..=SWEEP_REACH).contains(&pv);
            if ahead && (pu >= SWEEP_FROM || pv >= SWEEP_FROM) && !is_speckle(bin, x, y) {
                ink.push((pu, pv));
            }
        }
    }
    let gap = ((0.4 * thickness) as usize).clamp(2, 8);
    // A border's slope is how far it climbs along one arm per unit run along the
    // other, both in arm lengths: 0 for a parallelogram. The border through tl runs
    // along v and climbs along u; the one through br the other way round.
    let slopes = |reach: [f32; 2]| -> Option<[f32; 2]> {
        Some([
            border_slope(
                ink.iter().map(|&(pu, pv)| (pv, pu - 1.0)),
                lu,
                reach[0],
                gap,
            )?,
            border_slope(
                ink.iter().map(|&(pu, pv)| (pu, pv - 1.0)),
                lv,
                reach[1],
                gap,
            )?,
        ])
    };
    // tl + a·(v + su·u) = br + b·(u + sv·v), in arm lengths from bl:
    // along u: 1 + a·su = b; along v: a = 1 + b·sv.
    let meet = |[su, sv]: [f32; 2]| -> Option<[f32; 2]> {
        let a = (1.0 + sv) / (1.0 - su * sv);
        let b = 1.0 + a * su;
        (a.is_finite() && (0.4..=SWEEP_REACH).contains(&a) && (0.4..=SWEEP_REACH).contains(&b))
            .then_some([a, b])
    };
    // Each border ends where the other crosses it, and what lies beyond is not the
    // symbol's: print past the *other* border would carry the sweep straight over the
    // quiet zone. So a first pass reads only the stretch of each border that is the
    // symbol's under any plausible perspective, and a second the whole of it as the
    // first located it.
    let first = meet(slopes([SWEEP_SURE; 2])?)?;
    let [su, sv] = slopes(first)?;
    let [a, _] = meet([su, sv])?;
    Some(Point::new(
        tl.x + a * (vx + su * ux),
        tl.y + a * (vy + su * uy),
    ))
}

/// Border sweeps ignore ink nearer their pivot than this fraction of the arm: it
/// says little about the border's direction.
const SWEEP_FROM: f32 = 0.15;

/// The longest a timing border is taken to run, as a multiple of the arm beside it:
/// well past what perspective does to a symbol that still resolves.
const SWEEP_REACH: f32 = 1.7;

/// The stretch of a timing border (as a fraction of the arm it runs along) taken to be
/// the symbol's before the far corner is known.
const SWEEP_SURE: f32 = 0.75;

/// Slopes beyond this — the border leaning ~35° off the arm — are no symbol's.
const MAX_SLOPE: f32 = 0.7;

/// Slope of the timing border leaving a pivot: `ink` is every dark pixel as `(run,
/// rise)` from that pivot in arm lengths, `px` the pixels per unit rise, `reach` how
/// far along the run to look, `gap` the pixels of clear ray that end the sweep (see
/// [`far_corner_seed`]).
fn border_slope(
    ink: impl Iterator<Item = (f32, f32)>,
    px: f32,
    reach: f32,
    gap: usize,
) -> Option<f32> {
    // One bin is a pixel of rise at the far end of the arm.
    let mut bins = vec![false; (2.0 * MAX_SLOPE * px) as usize + 2];
    let bin_of = |s: f32| ((s + MAX_SLOPE) * px).floor();
    for (run, rise) in ink {
        if !(SWEEP_FROM..=reach).contains(&run) {
            continue;
        }
        // The pixel's angular footprint, so that pixels near the pivot leave no false
        // gaps — taken inward of its centre, so that they do not overshoot the border.
        let slope = rise / run;
        let half = 0.5 / (run * px);
        let (lo, hi) = (bin_of(slope - 2.0 * half), bin_of(slope));
        if hi < 0.0 {
            // Far inside the symbol: still the start of the sweep.
            bins[0] = true;
        } else if lo < bins.len() as f32 {
            let (lo, hi) = (lo.max(0.0) as usize, (hi as usize).min(bins.len() - 1));
            bins[lo..=hi].fill(true);
        }
    }
    let first = bins.iter().position(|&b| b)?;
    let mut last = first;
    for (i, _) in bins.iter().enumerate().skip(first).filter(|(_, b)| **b) {
        if i - last > gap {
            break;
        }
        last = i;
    }
    (last + 1 < bins.len()).then(|| (last as f32 + 0.5) / px - MAX_SLOPE)
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
    // The construction takes the cloud's longest chord for a diagonal. In a long
    // rectangle seen in perspective it can be a side instead — the quad then covers
    // a fraction of the ink — and the corners are read off the convex hull.
    if let Some((hull, hull_area)) = convex_hull(bin, x0, y0, x1, y1)
        && quad_area(&pts) < HULL_COVER * hull_area
        && let Some(mut quad) = largest_quad(&hull)
    {
        order_clockwise(&mut quad, centroid);
        pts = quad;
    }
    // Reject a degenerate (collinear / coincident) result.
    if quad_area(&pts) < 4.0 {
        return Err(Error::undecodable("degenerate symbol quad"));
    }
    Ok(pts)
}

/// The least of the dark cloud's convex hull a corner quad covers when the
/// farthest-point construction has found the corners: it loses no more than the
/// light module at the timing borders' corner and the hull's rounding.
const HULL_COVER: f32 = 0.85;

/// Convex hull of the dark pixels in the window, counter-clockwise in image
/// coordinates, and its area. `None` for fewer than three hull points.
fn convex_hull(
    bin: &BinaryImage,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
) -> Option<(Vec<Point>, f32)> {
    // Only the first and last dark pixel of each row can be on the hull; rows come
    // sorted, which is the order the monotone chain wants (by y, then x).
    let mut pts: Vec<Point> = Vec::new();
    for y in y0..=y1 {
        let Some(first) = (x0..=x1).find(|&x| bin.get(x, y)) else {
            continue;
        };
        let last = (x0..=x1).rev().find(|&x| bin.get(x, y)).unwrap_or(first);
        pts.push(Point::new(first as f32, y as f32));
        if last != first {
            pts.push(Point::new(last as f32, y as f32));
        }
    }
    if pts.len() < 3 {
        return None;
    }
    let cross =
        |o: Point, a: Point, b: Point| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut hull: Vec<Point> = Vec::with_capacity(pts.len() + 1);
    for pass in 0..2 {
        let start = hull.len();
        let feed = |p: Point, hull: &mut Vec<Point>| {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0
            {
                hull.pop();
            }
            hull.push(p);
        };
        if pass == 0 {
            pts.iter().for_each(|&p| feed(p, &mut hull));
        } else {
            pts.iter().rev().for_each(|&p| feed(p, &mut hull));
        }
        hull.pop();
    }
    if hull.len() < 3 {
        return None;
    }
    let twice: f32 = (0..hull.len())
        .map(|i| {
            let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    Some((hull, twice.abs() / 2.0))
}

/// The four vertices of the convex polygon `hull` that span the largest quad, in
/// hull order. `None` for fewer than four vertices.
fn largest_quad(hull: &[Point]) -> Option<[Point; 4]> {
    let n = hull.len();
    if n < 4 {
        return None;
    }
    // Twice the area of the triangle over hull vertices (indices taken modulo n).
    let tri = |a: usize, b: usize, c: usize| {
        let (a, b, c) = (hull[a % n], hull[b % n], hull[c % n]);
        ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs()
    };
    let mut best = (0.0f32, [0usize; 4]);
    for i in 0..n - 2 {
        // For the diagonal i–k the best vertex on each side is the farthest from it,
        // and both move one way round the hull as k does.
        let (mut j, mut l) = (i + 1, i + 3);
        for k in i + 2..n {
            while j + 1 < k && tri(i, j + 1, k) >= tri(i, j, k) {
                j += 1;
            }
            l = l.max(k + 1);
            while l + 1 < n + i && tri(k, l + 1, i) >= tri(k, l, i) {
                l += 1;
            }
            if l >= n + i {
                continue;
            }
            let area = tri(i, j, k) + tri(k, l, i);
            if area > best.0 {
                best = (area, [i, j, k, l % n]);
            }
        }
    }
    (best.0 > 0.0).then(|| best.1.map(|v| hull[v]))
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
    let lines: [Line; 4] = core::array::from_fn(|i| {
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
    core::array::from_fn(|i| {
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
        aa.partial_cmp(&bb).unwrap_or(core::cmp::Ordering::Equal)
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
        let (ix, iy) = inset_toward(px, py, (a, b), centroid, INSET);
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
        let (fx, fy) = inset_toward_f(px, py, (a, b), centroid, INSET);
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

/// Move point `(px, py)` of `edge` `dist` pixels into the symbol, returning rounded
/// pixel indices (clamped at 0).
fn inset_toward(
    px: f32,
    py: f32,
    edge: (Point, Point),
    centroid: Point,
    dist: f32,
) -> (usize, usize) {
    let (fx, fy) = inset_toward_f(px, py, edge, centroid, dist);
    (fx.max(0.0) as usize, fy.max(0.0) as usize)
}

/// Move `(px, py)`, a point of `edge`, `dist` pixels into the symbol, returning
/// fractional coordinates: toward `centroid`, or — where that runs more along the
/// edge than away from it, towards the ends of a rectangle's long sides — straight
/// in from the edge.
fn inset_toward_f(
    px: f32,
    py: f32,
    (a, b): (Point, Point),
    centroid: Point,
    dist: f32,
) -> (f64, f64) {
    let dx = centroid.x - px;
    let dy = centroid.y - py;
    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
    let (mut ux, mut uy) = (dx / len, dy / len);
    let (ex, ey) = (b.x - a.x, b.y - a.y);
    let edge_len = ex.hypot(ey).max(1e-3);
    // The edge's unit normal on the centroid's side.
    let (mut nx, mut ny) = (-ey / edge_len, ex / edge_len);
    if nx * ux + ny * uy < 0.0 {
        (nx, ny) = (-nx, -ny);
    }
    // At the end of a square's side the centroid is 45° off the normal.
    if nx * ux + ny * uy < 0.7 {
        (ux, uy) = (nx, ny);
    }
    ((px + ux * dist) as f64, (py + uy * dist) as f64)
}

/// How many sizes are tried around a module count.
const NEAREST_SIZES: usize = 3;

/// The grids to try for a symbol whose timing borders count `m1` modules (the one
/// from `tl`) and `m2` (the one from `br`), likeliest first: `(width, height, swap)`,
/// `swap` being the handedness for [`corner_targets`].
///
/// Counts within 30 % of each other are a square's: the nearest square sizes, each
/// in both handednesses, since nothing tells its two solid edges apart. Counts
/// further apart are a rectangle's, whose longer timing border is its top row: the
/// nearest rectangular sizes in that one handedness — and then the squares all the
/// same, for a small square whose borders were miscounted.
fn candidate_grids(m1: usize, m2: usize) -> Vec<(usize, usize, bool)> {
    let mut grids = Vec::new();
    let (long, short) = (m1.max(m2), m1.min(m2));
    if 10 * long >= 13 * short {
        let swap = m2 > m1;
        let mut rects: Vec<(usize, usize)> = all_specs()
            .iter()
            .filter(|s| !s.is_square())
            .map(|s| (s.width, s.height))
            .collect();
        // A miscount is a module or two whatever the side's length, so the short
        // side, with the fewest sizes to choose from, weighs the same as the long.
        rects.sort_by_key(|&(w, h)| w.abs_diff(long) + h.abs_diff(short));
        grids.extend(
            rects
                .iter()
                .take(NEAREST_SIZES + 1)
                .map(|&(w, h)| (w, h, swap)),
        );
    }
    let n_est = (m1 + m2).div_ceil(2); // rounded average
    let mut sizes: Vec<usize> = all_squares().iter().map(|s| s.width).collect();
    sizes.sort_by_key(|&s| s.abs_diff(n_est));
    for &dim in sizes.iter().take(NEAREST_SIZES) {
        grids.extend([false, true].map(|swap| (dim, dim, swap)));
    }
    grids
}
