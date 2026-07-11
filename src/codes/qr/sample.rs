//! QR image sampling: pixels → [`BitMatrix`] → [`Symbol`].
//!
//! The structural [`QrDecoder`](super::QrDecoder) consumes a clean module grid. This
//! module is the front-end that recovers such a grid from a real image: it binarizes
//! the frame, locates the three finder patterns by their 1:1:3:1:1 signature, orders
//! them into `top-left / top-right / bottom-left`, estimates the symbol version,
//! recovers the perspective mapping from module grid to pixels, samples every module
//! center, and hands the resulting [`BitMatrix`] to the structural decoder.
//!
//! ## Robustness envelope
//!
//! The sampler is built for the transforms in [`crate::transform`]. It handles the
//! symbol upright, rotated by any angle, uniformly scaled, mildly blurred/noisy, and
//! under mild perspective tilt. Module size is measured *along* each finder-to-finder
//! span (not from an axis-aligned run), so version estimation stays correct under
//! arbitrary rotation.
//!
//! The fourth (bottom-right) grid corner is recovered two ways. For version 2 and up
//! the real bottom-right alignment pattern is located and used, giving a true 4-point
//! perspective homography that corrects tilt. Version 1 has no alignment pattern, so
//! its fourth corner falls back to the parallelogram rule — exact under any affine
//! transform (rotation, scale, shear) but not perspective. Strong perspective, and any
//! perspective on version 1, are out of scope.
//!
//! ## Real-world captures
//!
//! Camera photos rarely binarize cleanly under one global threshold, so several extra
//! layers make the front-end robust to them (see `tests/real_world.rs` for the asserted
//! envelope — uneven lighting, glare, blur, mild curvature):
//!
//! - **Binarization ladder** — global Otsu first (exact and cheap on clean renders),
//!   then adaptive Bradley/Sauvola local thresholds that survive lighting gradients and
//!   glare. Each pass is tried in turn until a grid decodes.
//! - **Dual-axis finder scan** — the frame is run-length swept both by rows *and* by
//!   columns, recovering a finder whose signature is spoiled along one axis by blur.
//! - **Synthesized third finder** — when only two strong finders survive (heavy blur can
//!   destroy the top-right pattern's rings entirely), the missing corner is reconstructed
//!   geometrically and locked onto the real (blurred) finder by template correlation.
//! - **Local module sampling** — each module centre is classified against a local-mean
//!   window (from an integral image), not one global cutoff, so a gradient across the
//!   symbol does not flip modules.
//! - **Interior fourth-corner sweep** — a decode-validated fallback that anchors the
//!   homography on the interior bottom-right alignment position and sweeps it, bowing the
//!   grid to follow the curvature of a label on a bottle or can. Reached only when every
//!   direct hypothesis has failed, so clean renders never pay for it.

use super::QrDecoder;
use super::matrix::QUIET_ZONE;
use crate::error::{Error, Result};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::imgproc::binary::BinaryImage;
use crate::imgproc::integral::IntegralImage;
use crate::imgproc::sample::sample_bilinear;
use crate::imgproc::threshold::{
    adaptive_binarize_bradley, adaptive_binarize_sauvola, otsu_threshold,
};
use crate::output::BitMatrix;
use crate::pipeline::{Candidate, Hints};
use crate::symbol::Symbol;
use crate::traits::{Analyze, Detect};
use crate::transform::Projection;

/// Image-based QR scanner: finds, samples and decodes a QR symbol in a [`GrayFrame`].
#[derive(Debug, Default, Clone, Copy)]
pub struct QrScanner;

impl QrScanner {
    /// A new scanner.
    pub fn new() -> Self {
        QrScanner
    }
}

impl Detect for QrScanner {
    fn detect(&self, frame: &GrayFrame<'_>, _hints: &Hints) -> Vec<Candidate> {
        match locate_any(frame) {
            Some(loc) => vec![Candidate {
                location: loc.as_location(),
                symbology: Some(crate::symbology::Symbology::QrCode),
                fingerprint: None,
                known: None,
            }],
            None => Vec::new(),
        }
    }
}

impl Analyze for QrScanner {
    fn analyze(&self, frame: &GrayFrame<'_>, candidate: &Candidate) -> Result<Symbol> {
        if let Some(known) = &candidate.known {
            return Ok(known.clone());
        }
        scan(frame)
    }
}

/// Locate, sample and structurally decode the QR symbol in `frame`.
///
/// Real camera photos rarely binarize cleanly under one global threshold (glare,
/// uneven lighting, blur, curved surfaces), so this walks a small ladder of
/// binarizations — global Otsu first, then adaptive local ones — and, within each,
/// several finder-triple hypotheses. The first grid that survives Reed–Solomon wins;
/// clean synthetic renders still decode on the very first (global) pass.
pub fn scan(frame: &GrayFrame<'_>) -> Result<Symbol> {
    let integral = IntegralImage::from_frame(frame);
    let decoder = QrDecoder::new();
    let mut last = Error::undecodable("no QR symbol found");
    // Located hypotheses that decoded neither directly, kept for the fourth-corner
    // refinement fallback below.
    let mut pending: Vec<Located> = Vec::new();
    for pass in 0.. {
        let Some(bin) = binarize(frame, pass) else {
            break;
        };
        for located in candidates(frame, &bin) {
            let thr = located.threshold(&integral);
            let matrix = located.sample(frame, &thr);
            match decoder.decode_matrix(&matrix) {
                Ok(sym) => return Ok(sym),
                Err(e) => {
                    last = e;
                    if located.dimension > 21 && pending.len() < REFINE_HYPOTHESES {
                        pending.push(located);
                    }
                }
            }
        }
    }
    // Fallback for curved/tilted captures whose bottom-right corner the affine
    // parallelogram places wrongly and whose alignment pattern is too degraded to
    // detect: sweep that free corner and let Reed–Solomon accept the geometry that
    // reads. Reached only when every direct hypothesis failed, so clean renders never
    // pay for it.
    for located in &pending {
        if let Some(sym) = refine_fourth_corner(frame, located, &integral, &decoder) {
            return Ok(sym);
        }
    }
    Err(last)
}

/// Located hypotheses (dim > 21) retained for the fourth-corner refinement fallback.
const REFINE_HYPOTHESES: usize = 4;

/// Re-decode `located` while searching for the position of its bottom-right **alignment
/// pattern** — the interior fiducial at grid `(dim-6.5, dim-6.5)`.
///
/// The three finder corners are trusted; anchoring the homography on an *interior* point
/// (rather than the outer corner) lets it bow to follow the curvature of a label on a
/// bottle or can, which an affine/parallelogram fit cannot. The pattern is often too
/// blurred to detect by its cross-section, so instead of trusting a single detection we
/// sweep a neighbourhood of the affine-predicted position and let Reed–Solomon accept
/// the placement that reads. Reached only when every direct hypothesis failed.
fn refine_fourth_corner(
    frame: &GrayFrame<'_>,
    located: &Located,
    integral: &IntegralImage,
    decoder: &QrDecoder,
) -> Option<Symbol> {
    let [tl, tr, _, bl] = located.corners;
    let dim = located.dimension;
    let d = dim as f64;
    let ms = located.module_size.max(1.0);
    // Homography src uses the alignment centre (grid dim-6.5) as its third correspondence.
    let src = [
        (3.5, 3.5),
        (d - 3.5, 3.5),
        (d - 6.5, d - 6.5),
        (3.5, d - 3.5),
    ];
    // Affine prediction of that alignment centre, then a sub-module sweep around it.
    let u = (d - 10.0) as f32 / (d - 7.0) as f32;
    let ex = tl.x + u * (tr.x - tl.x) + u * (bl.x - tl.x);
    let ey = tl.y + u * (tr.y - tl.y) + u * (bl.y - tl.y);
    // Try a couple of module-classification thresholds: the local-mean window and a
    // global Otsu cutoff. Blur and curvature can make either the better discriminator.
    let radius = (ms * 2.0).round().clamp(2.0, 64.0) as usize;
    let thresholds = [
        ModuleThreshold::Local { integral, radius },
        ModuleThreshold::Global(located.threshold),
    ];
    let reach = ms * 3.0;
    let step = (ms * 0.4).clamp(0.75, 3.0);
    let n = (reach / step) as i32;
    for thr in &thresholds {
        for gy in -n..=n {
            for gx in -n..=n {
                let ax = ex + gx as f32 * step;
                let ay = ey + gy as f32 * step;
                let dst = [
                    (tl.x as f64, tl.y as f64),
                    (tr.x as f64, tr.y as f64),
                    (ax as f64, ay as f64),
                    (bl.x as f64, bl.y as f64),
                ];
                let projection = Projection::quad_to_quad(src, dst);
                let trial = Located {
                    projection,
                    dimension: dim,
                    threshold: located.threshold,
                    local: located.local,
                    corners: located.corners,
                    module_size: located.module_size,
                };
                let matrix = trial.sample(frame, thr);
                if let Ok(sym) = decoder.decode_matrix(&matrix) {
                    return Some(sym);
                }
            }
        }
    }
    None
}

/// Locate the QR symbol in `frame` and sample it to a clean [`BitMatrix`].
///
/// Tries the same binarization/finder ladder as [`scan`] and returns the grid of the
/// first hypothesis that decodes; if none decode it returns the first that merely
/// located (still useful for inspection). Errors only when no finders are found at all.
pub fn sample_grid(frame: &GrayFrame<'_>) -> Result<BitMatrix> {
    let integral = IntegralImage::from_frame(frame);
    let decoder = QrDecoder::new();
    let mut first: Option<BitMatrix> = None;
    for pass in 0.. {
        let Some(bin) = binarize(frame, pass) else {
            break;
        };
        for located in candidates(frame, &bin) {
            let thr = located.threshold(&integral);
            let matrix = located.sample(frame, &thr);
            if decoder.decode_matrix(&matrix).is_ok() {
                return Ok(matrix);
            }
            if first.is_none() {
                first = Some(matrix);
            }
        }
    }
    first.ok_or_else(|| Error::undecodable("no QR finder patterns found"))
}

/// Geometry-only location for the cheap detection pass: the first hypothesis that
/// locates under any binarization in the ladder.
fn locate_any(frame: &GrayFrame<'_>) -> Option<Located> {
    for pass in 0.. {
        let bin = binarize(frame, pass)?;
        if let Some(located) = candidates(frame, &bin).into_iter().next() {
            return Some(located);
        }
    }
    None
}

// ===================================================================================
// Binarization
// ===================================================================================

/// A binarized frame used for finder detection and geometry: `true` = dark module.
///
/// `local` records whether the binarization was adaptive (local). Adaptive passes also
/// drive *local* module sampling; the global pass keeps the plain global threshold, so
/// clean synthetic renders sample identically to before.
struct Binary {
    img: BinaryImage,
    /// Global Otsu threshold of the frame, kept for reporting and the global-sample path.
    threshold: u8,
    /// `true` when produced by an adaptive (local) method.
    local: bool,
}

impl Binary {
    #[inline]
    fn dark(&self, x: usize, y: usize) -> bool {
        self.img.get(x, y)
    }

    #[inline]
    fn width(&self) -> usize {
        self.img.width()
    }

    #[inline]
    fn height(&self) -> usize {
        self.img.height()
    }
}

/// Build the `pass`-th binarization in the robustness ladder, or `None` once exhausted.
///
/// Pass 0 is plain global Otsu — cheap and exact on evenly-lit renders, so the whole
/// synthetic test-suite decodes here unchanged. Passes 1+ are adaptive/local methods
/// (Bradley local-mean at two window sizes, then Sauvola) which survive glare, lighting
/// gradients and the mild contrast loss of blur on real photos.
fn binarize(frame: &GrayFrame<'_>, pass: usize) -> Option<Binary> {
    let threshold = otsu_threshold(frame);
    let small = frame.width().min(frame.height());
    // Adaptive window ≈ a handful of modules wide; two sizes cover a range of symbol
    // scales within the frame.
    let r_small = (small / 12).clamp(6, 40);
    let r_large = (small / 6).clamp(10, 80);
    let img = match pass {
        0 => {
            let w = frame.width();
            let h = frame.height();
            let mut bin = BinaryImage::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    if frame.get_unchecked(x, y) <= threshold {
                        bin.set(x, y, true);
                    }
                }
            }
            return Some(Binary {
                img: bin,
                threshold,
                local: false,
            });
        }
        1 => adaptive_binarize_bradley(frame, r_small, 0.08),
        2 => adaptive_binarize_bradley(frame, r_large, 0.08),
        3 => adaptive_binarize_sauvola(frame, r_small, 0.2, 128.0),
        _ => return None,
    };
    Some(Binary {
        img,
        threshold,
        local: true,
    })
}

// ===================================================================================
// Finder-pattern detection
// ===================================================================================

/// A detected finder-pattern center with its estimated module size and hit count.
#[derive(Debug, Clone, Copy)]
struct Finder {
    x: f32,
    y: f32,
    module_size: f32,
    count: u32,
}

/// Check that five run lengths match the finder 1:1:3:1:1 ratio; return the module
/// size (average narrow-run width) on success.
fn found_pattern_cross(counts: [i32; 5]) -> Option<f32> {
    let total: i32 = counts.iter().sum();
    if total < 7 {
        return None;
    }
    let module = total as f32 / 7.0;
    // Blur softens edges and adjacent dark data modules can bleed into the outer rings,
    // so the ratios are matched with generous slack: ~0.7 module on each narrow ring and
    // ~2.1 modules on the wide centre run. Reed–Solomon plus the geometric triple check
    // reject the extra false positives this admits.
    let max_var = module * 0.7;
    let ok = (counts[0] as f32 - module).abs() < max_var
        && (counts[1] as f32 - module).abs() < max_var
        && (counts[2] as f32 - 3.0 * module).abs() < 3.0 * max_var
        && (counts[3] as f32 - module).abs() < max_var
        && (counts[4] as f32 - module).abs() < max_var;
    ok.then_some(module)
}

/// Walk a dark-light-dark-light-dark run centered at `start` along an axis, returning
/// the five run lengths and the end index just past the final dark run. `sample(k)`
/// returns whether the pixel at axis position `k` is dark; the axis spans `0..len`.
fn walk_run(len: i32, start: i32, sample: impl Fn(i32) -> bool) -> Option<([i32; 5], i32)> {
    let mut counts = [0i32; 5];
    let mut i = start;
    while i >= 0 && sample(i) {
        counts[2] += 1;
        i -= 1;
    }
    if i < 0 {
        return None;
    }
    while i >= 0 && !sample(i) {
        counts[1] += 1;
        i -= 1;
    }
    if i < 0 || counts[1] == 0 {
        return None;
    }
    while i >= 0 && sample(i) {
        counts[0] += 1;
        i -= 1;
    }
    if counts[0] == 0 {
        return None;
    }
    let mut j = start + 1;
    while j < len && sample(j) {
        counts[2] += 1;
        j += 1;
    }
    if j >= len {
        return None;
    }
    while j < len && !sample(j) {
        counts[3] += 1;
        j += 1;
    }
    if j >= len || counts[3] == 0 {
        return None;
    }
    while j < len && sample(j) {
        counts[4] += 1;
        j += 1;
    }
    if counts[4] == 0 {
        return None;
    }
    Some((counts, j))
}

/// Refined center from run lengths and the walk's end index.
fn run_center(counts: [i32; 5], end: i32) -> f32 {
    end as f32 - counts[4] as f32 - counts[3] as f32 - counts[2] as f32 / 2.0
}

/// Vertical finder cross-check through column `cx`, returning the refined center-`y`.
fn cross_check_vertical(bin: &Binary, cx: usize, start: usize) -> Option<f32> {
    let (counts, end) = walk_run(bin.height() as i32, start as i32, |k| {
        bin.dark(cx, k as usize)
    })?;
    found_pattern_cross(counts)?;
    Some(run_center(counts, end))
}

/// Horizontal finder cross-check through row `cy`, returning the refined center-`x`.
fn cross_check_horizontal(bin: &Binary, cy: usize, start: usize) -> Option<f32> {
    let (counts, end) = walk_run(bin.width() as i32, start as i32, |k| {
        bin.dark(k as usize, cy)
    })?;
    found_pattern_cross(counts)?;
    Some(run_center(counts, end))
}

/// Merge a new candidate center into the cluster list, averaging coincident hits.
fn add_center(centers: &mut Vec<Finder>, x: f32, y: f32, module_size: f32) {
    for f in centers.iter_mut() {
        if (f.x - x).abs() <= f.module_size && (f.y - y).abs() <= f.module_size {
            let c = f.count as f32;
            f.x = (f.x * c + x) / (c + 1.0);
            f.y = (f.y * c + y) / (c + 1.0);
            f.module_size = (f.module_size * c + module_size) / (c + 1.0);
            f.count += 1;
            return;
        }
    }
    centers.push(Finder {
        x,
        y,
        module_size,
        count: 1,
    });
}

/// Run-length encode one line, invoking `emit(run_start, [c0..c4])` for every window of
/// five consecutive runs that begins on a dark run — `start` is the pixel index where
/// the middle (centre) run begins.
fn scan_line_runs(len: usize, dark: impl Fn(usize) -> bool, mut emit: impl FnMut(usize, [i32; 5])) {
    let mut runs: Vec<(bool, usize, i32)> = Vec::new();
    let mut cur = dark(0);
    let mut start = 0usize;
    for p in 1..len {
        let d = dark(p);
        if d != cur {
            runs.push((cur, start, (p - start) as i32));
            cur = d;
            start = p;
        }
    }
    runs.push((cur, start, (len - start) as i32));
    if runs.len() < 5 {
        return;
    }
    for i in 0..=runs.len() - 5 {
        if !runs[i].0 {
            continue; // pattern must start on a dark run
        }
        let counts = [
            runs[i].2,
            runs[i + 1].2,
            runs[i + 2].2,
            runs[i + 3].2,
            runs[i + 4].2,
        ];
        let center = &runs[i + 2];
        emit(center.1 + (center.2 as usize) / 2, counts);
    }
}

/// Scan the binarized frame for finder patterns and return their clustered centers.
///
/// The frame is swept **both** row-by-row and column-by-column. A blurred finder whose
/// horizontal profile is spoiled by neighbouring dark data (common for the top-right
/// pattern, whose inner side abuts the payload) is often still clean along the vertical,
/// and vice-versa, so scanning both axes recovers finders a single-axis sweep misses.
/// Hits from either axis are merged by [`add_center`].
fn find_finders(bin: &Binary) -> Vec<Finder> {
    let mut centers: Vec<Finder> = Vec::new();
    let (w, h) = (bin.width(), bin.height());

    // Row sweep: candidate centre pixel is (mid, y); refine vertically then horizontally.
    for y in 0..h {
        scan_line_runs(
            w,
            |x| bin.dark(x, y),
            |mid, counts| {
                if found_pattern_cross(counts).is_none() {
                    return;
                }
                let Some(cy) = cross_check_vertical(bin, mid, y) else {
                    return;
                };
                let cy_row = cy.round().clamp(0.0, (h - 1) as f32) as usize;
                let Some(cx) = cross_check_horizontal(bin, cy_row, mid) else {
                    return;
                };
                let module = found_pattern_cross(counts).unwrap();
                add_center(&mut centers, cx, cy, module);
            },
        );
    }

    // Column sweep: candidate centre pixel is (x, mid); refine horizontally then vertically.
    for x in 0..w {
        scan_line_runs(
            h,
            |y| bin.dark(x, y),
            |mid, counts| {
                if found_pattern_cross(counts).is_none() {
                    return;
                }
                let Some(cx) = cross_check_horizontal(bin, mid, x) else {
                    return;
                };
                let cx_col = cx.round().clamp(0.0, (w - 1) as f32) as usize;
                let Some(cy) = cross_check_vertical(bin, cx_col, mid) else {
                    return;
                };
                let module = found_pattern_cross(counts).unwrap();
                add_center(&mut centers, cx, cy, module);
            },
        );
    }
    centers
}

// ===================================================================================
// Geometry: order finders, estimate version, build the sampling projection
// ===================================================================================

/// A fully located symbol: the grid→pixel projection plus derived metadata.
struct Located {
    projection: Projection,
    dimension: usize,
    threshold: u8,
    local: bool,
    corners: [Point; 4],
    module_size: f32,
}

/// How a module centre is classified dark/light during sampling.
enum ModuleThreshold<'a> {
    /// One global cutoff — used for the clean/global-Otsu pass so evenly-lit renders
    /// sample exactly as before.
    Global(u8),
    /// Compare the sampled luminance against the local mean of a window of `radius`
    /// pixels (Bradley-style), read in O(1) from `integral`. Robust to lighting
    /// gradients and glare across the symbol.
    Local {
        integral: &'a IntegralImage,
        radius: usize,
    },
}

impl Located {
    fn as_location(&self) -> Location {
        let rotation = {
            let dx = self.corners[1].x - self.corners[0].x;
            let dy = self.corners[1].y - self.corners[0].y;
            dy.atan2(dx)
        };
        Location {
            outline: Quad::new(self.corners),
            rotation: Some(rotation),
            module_size: Some(self.module_size),
        }
    }

    /// Choose how to threshold module centres for this hypothesis. Global for the
    /// global-Otsu pass; a local-mean window (sized to the detected module) otherwise.
    fn threshold<'a>(&self, integral: &'a IntegralImage) -> ModuleThreshold<'a> {
        if self.local {
            // Window a couple of modules across, so it always spans both dark and light
            // modules and its mean lands between them.
            let radius = (self.module_size * 2.0).round().clamp(2.0, 64.0) as usize;
            ModuleThreshold::Local { integral, radius }
        } else {
            ModuleThreshold::Global(self.threshold)
        }
    }

    /// Sample every module center into a clean [`BitMatrix`].
    fn sample(&self, frame: &GrayFrame<'_>, thr: &ModuleThreshold<'_>) -> BitMatrix {
        let dim = self.dimension;
        // Average a few taps within the module footprint so blur and single-pixel
        // noise do not flip a module.
        let tap = (self.module_size * 0.25).clamp(0.5, 8.0) as f64;
        let mut matrix = BitMatrix::new(dim, dim, QUIET_ZONE);
        for my in 0..dim {
            for mx in 0..dim {
                let (px, py) = self.projection.map(mx as f64 + 0.5, my as f64 + 0.5);
                if sample_dark(frame, px, py, tap, thr) {
                    matrix.set(mx, my, true);
                }
            }
        }
        matrix
    }
}

/// Decide dark vs light at module centre `(px, py)`.
///
/// Luminance is the mean of five bilinear taps (centre plus four offset by `tap`),
/// which survives blur and noise. It is then compared either to a fixed global cutoff
/// or to the local-neighbourhood mean (Bradley), which tracks lighting gradients.
fn sample_dark(
    frame: &GrayFrame<'_>,
    px: f64,
    py: f64,
    tap: f64,
    thr: &ModuleThreshold<'_>,
) -> bool {
    let lum = (sample_bilinear(frame, px, py)
        + sample_bilinear(frame, px - tap, py)
        + sample_bilinear(frame, px + tap, py)
        + sample_bilinear(frame, px, py - tap)
        + sample_bilinear(frame, px, py + tap))
        / 5.0;
    match *thr {
        ModuleThreshold::Global(t) => lum <= f64::from(t),
        ModuleThreshold::Local { integral, radius } => {
            let cx = (px.round().max(0.0) as usize).min(integral.width().saturating_sub(1));
            let cy = (py.round().max(0.0) as usize).min(integral.height().saturating_sub(1));
            let (sum, count) = integral.window_sum_count(cx, cy, radius);
            let mean = sum as f64 / count.max(1) as f64;
            // Dark when meaningfully below the local mean.
            lum < mean - 4.0 && lum < mean * 0.98
        }
    }
}

/// Order three finder centers into `[top-left, top-right, bottom-left]` using the
/// hypotenuse rule and a cross-product handedness check (ZXing's `orderBestPatterns`).
fn order_finders(a: Finder, b: Finder, c: Finder) -> ([Point; 3], f32) {
    let pa = Point::new(a.x, a.y);
    let pb = Point::new(b.x, b.y);
    let pc = Point::new(c.x, c.y);
    let d_ab = pa.distance(pb);
    let d_bc = pb.distance(pc);
    let d_ac = pa.distance(pc);

    // `corner` is the finder opposite the longest edge (the right-angle vertex);
    // `p`/`q` are the two hypotenuse endpoints.
    let (corner, mut p, mut q) = if d_bc >= d_ab && d_bc >= d_ac {
        (pa, pb, pc)
    } else if d_ac >= d_ab && d_ac >= d_bc {
        (pb, pa, pc)
    } else {
        (pc, pa, pb)
    };

    // Ensure (p = bottom-left, q = top-right) handedness for a non-mirrored symbol.
    let cross = (q.x - corner.x) * (p.y - corner.y) - (q.y - corner.y) * (p.x - corner.x);
    if cross < 0.0 {
        std::mem::swap(&mut p, &mut q);
    }
    // top-left = corner, top-right = q, bottom-left = p
    let module_size = (a.module_size + b.module_size + c.module_size) / 3.0;
    ([corner, q, p], module_size)
}

/// Four ordered `(x, y)` correspondence points (grid or image space).
type Quad4 = [(f64, f64); 4];

/// Grid/image correspondences using the parallelogram rule for the fourth corner
/// (a would-be bottom-right finder center at grid `(dim-3.5, dim-3.5)`).
fn parallelogram(tl: Point, tr: Point, bl: Point, d: f64) -> (Quad4, Quad4) {
    let br = Point::new(tr.x + bl.x - tl.x, tr.y + bl.y - tl.y);
    let src = [
        (3.5, 3.5),
        (d - 3.5, 3.5),
        (d - 3.5, d - 3.5),
        (3.5, d - 3.5),
    ];
    let dst = [
        (tl.x as f64, tl.y as f64),
        (tr.x as f64, tr.y as f64),
        (br.x as f64, br.y as f64),
        (bl.x as f64, bl.y as f64),
    ];
    (src, dst)
}

/// Check run lengths against the alignment pattern's cross-section (dark-light-**dark
/// center**-light-dark). Only the inner light-dark-light trio (`counts[1..=3]`) is
/// required to hold the 1:1:1 ratio, because the outer dark rings can merge with
/// adjacent dark data modules; the outer runs need only be present. Returns the
/// measured module size on success.
fn found_alignment_cross(counts: [i32; 5], module: f32) -> Option<f32> {
    let inner = counts[1] + counts[2] + counts[3];
    let m = inner as f32 / 3.0;
    if (m - module).abs() > module * 0.5 {
        return None;
    }
    let max_var = m * 0.5;
    let inner_ok = (counts[1] as f32 - m).abs() < max_var
        && (counts[2] as f32 - m).abs() < max_var
        && (counts[3] as f32 - m).abs() < max_var;
    let outer_ok = counts[0] > 0 && counts[4] > 0;
    (inner_ok && outer_ok).then_some(m)
}

/// Search a window around `expected` for the bottom-right alignment pattern, returning
/// the detected center closest to the prediction.
fn find_alignment(bin: &Binary, expected: Point, module_size: f32) -> Option<Point> {
    let radius = (module_size * 5.0).ceil() as i32;
    let x0 = (expected.x as i32 - radius).max(0) as usize;
    let x1 = ((expected.x as i32 + radius) as usize).min(bin.width() - 1);
    let y0 = (expected.y as i32 - radius).max(0) as usize;
    let y1 = ((expected.y as i32 + radius) as usize).min(bin.height() - 1);
    if x1 <= x0 + 4 {
        return None;
    }
    let mut best: Option<(Point, f32)> = None;
    for y in y0..=y1 {
        // Run-length encode only the search window of this row.
        let mut runs: Vec<(bool, usize, i32)> = Vec::new();
        let mut cur = bin.dark(x0, y);
        let mut start = x0;
        for x in (x0 + 1)..=x1 {
            let dk = bin.dark(x, y);
            if dk != cur {
                runs.push((cur, start, (x - start) as i32));
                cur = dk;
                start = x;
            }
        }
        runs.push((cur, start, (x1 + 1 - start) as i32));
        if runs.len() < 5 {
            continue;
        }
        for i in 0..=runs.len() - 5 {
            if !runs[i].0 {
                continue;
            }
            let counts = [
                runs[i].2,
                runs[i + 1].2,
                runs[i + 2].2,
                runs[i + 3].2,
                runs[i + 4].2,
            ];
            if found_alignment_cross(counts, module_size).is_none() {
                continue;
            }
            let center = &runs[i + 2];
            let cxi = center.1 + (center.2 as usize) / 2;
            // Confirm the same 1:1:1:1:1 signature vertically through the center.
            let Some((vc, vend)) =
                walk_run(bin.height() as i32, y as i32, |k| bin.dark(cxi, k as usize))
            else {
                continue;
            };
            if found_alignment_cross(vc, module_size).is_none() {
                continue;
            }
            let cx = cxi as f32;
            let cy = run_center(vc, vend);
            let dist = (cx - expected.x).abs() + (cy - expected.y).abs();
            if best.is_none_or(|(_, d)| dist < d) {
                best = Some((Point::new(cx, cy), dist));
            }
        }
    }
    best.map(|(p, _)| p)
}

/// Estimate the module size along the direction from finder center `from` toward
/// `to`, by walking outward from `from` across its own finder pattern. The center of a
/// finder sits 3.5 modules from the pattern's outer edge, which is reached after three
/// colour transitions (dark→light→dark→light); the distance walked divided by 3.5 is
/// the module size along that exact direction (so it is rotation-invariant).
fn directional_module_size(bin: &Binary, from: Point, to: Point) -> Option<f32> {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    let step = 0.25;
    let mut dist = 0.0f32;
    let mut prev = true; // finder center is dark
    let mut transitions = 0;
    // Cap the walk at a generous multiple of the inter-finder distance.
    while dist <= len {
        dist += step;
        let px = from.x + ux * dist;
        let py = from.y + uy * dist;
        let (ix, iy) = (px.round() as i32, py.round() as i32);
        if ix < 0 || iy < 0 || ix >= bin.width() as i32 || iy >= bin.height() as i32 {
            return None;
        }
        let cur = bin.dark(ix as usize, iy as usize);
        if cur != prev {
            transitions += 1;
            prev = cur;
            if transitions == 3 {
                // Reached the far (light) edge of the finder ring.
                return Some(dist / 3.5);
            }
        }
    }
    None
}

/// Snap an estimated side length to the nearest valid QR dimension (`21..=177`).
fn snap_dimension(estimate: f32) -> Option<usize> {
    let version = (((estimate - 17.0) / 4.0).round() as i32).clamp(1, 40);
    let dim = 17 + 4 * version as usize;
    (dim >= 21).then_some(dim)
}

/// Whether module `(i, j)` of a 7×7 finder pattern is dark: the outer ring plus the
/// central 3×3 block are dark; the ring between them is light.
fn finder_module_dark(i: usize, j: usize) -> bool {
    let ring = i == 0 || i == 6 || j == 0 || j == 6;
    let center = (2..=4).contains(&i) && (2..=4).contains(&j);
    ring || center
}

/// Correlation of the 7×7 finder template centred at pixel `(cx, cy)` with the frame,
/// given per-module step vectors `ax` (columns) and `ay` (rows). Returns the mean
/// luminance of the template's light modules minus that of its dark modules — larger is
/// a better match. Blur leaves this low-frequency signal intact, so it localizes a
/// finder whose run-length signature is gone.
fn finder_template_score(
    frame: &GrayFrame<'_>,
    cx: f32,
    cy: f32,
    ax: (f32, f32),
    ay: (f32, f32),
) -> f32 {
    let (mut light, mut dark) = (0.0f32, 0.0f32);
    let (mut nl, mut nd) = (0.0f32, 0.0f32);
    for j in 0..7usize {
        for i in 0..7usize {
            let fi = i as f32 - 3.0;
            let fj = j as f32 - 3.0;
            let px = cx + fi * ax.0 + fj * ay.0;
            let py = cy + fi * ax.1 + fj * ay.1;
            let lum = sample_bilinear(frame, px as f64, py as f64) as f32;
            if finder_module_dark(i, j) {
                dark += lum;
                nd += 1.0;
            } else {
                light += lum;
                nl += 1.0;
            }
        }
    }
    light / nl.max(1.0) - dark / nd.max(1.0)
}

/// Refine a synthesized finder centre `guess` by template correlation against the frame.
///
/// `vertex` is the right-angle finder and `arm` the finder at the far end of one side,
/// so the symbol's module axes near `guess` are the unit directions `vertex→arm` (rows)
/// and `vertex→guess` (columns), each one module long. A small sub-pixel search around
/// `guess` maximizes [`finder_template_score`], snapping the guessed corner onto the
/// real (blurred) finder and thereby correcting the module pitch.
fn refine_finder(frame: &GrayFrame<'_>, vertex: Finder, arm: Finder, guess: Finder) -> Finder {
    let ms = vertex.module_size.max(1.0);
    let unit = |dx: f32, dy: f32| {
        let l = (dx * dx + dy * dy).sqrt().max(1e-3);
        (dx / l, dy / l)
    };
    let (uyx, uyy) = unit(arm.x - vertex.x, arm.y - vertex.y);
    let (uxx, uxy) = unit(guess.x - vertex.x, guess.y - vertex.y);
    let ax = (uxx * ms, uxy * ms);
    let ay = (uyx * ms, uyy * ms);

    let reach = 2.0 * ms;
    let step = 0.5f32;
    let steps = (reach / step) as i32;
    let mut best = guess;
    let mut best_score = f32::MIN;
    for oy in -steps..=steps {
        for ox in -steps..=steps {
            let cx = guess.x + ox as f32 * step;
            let cy = guess.y + oy as f32 * step;
            let s = finder_template_score(frame, cx, cy, ax, ay);
            if s > best_score {
                best_score = s;
                best = Finder {
                    x: cx,
                    y: cy,
                    module_size: ms,
                    count: guess.count,
                };
            }
        }
    }
    best
}

/// Strongest finder clusters retained per binarization (caps the triple combinatorics).
const MAX_FINDERS: usize = 8;
/// Located hypotheses returned per binarization, best-scoring first.
const MAX_TRIPLES: usize = 24;
/// Minimum cluster hit-count for a finder used to *synthesize* a missing third pattern.
const SYNTH_MIN_COUNT: u32 = 3;
/// Score penalty applied to synthesized triples so genuine triples are decoded first.
const SYNTH_PENALTY: f32 = 10.0;

/// Enumerate located hypotheses for one binarization, most finder-like first.
///
/// Real photos throw off spurious finder-like blobs (data clusters, glare edges) and,
/// worse, can *destroy* one finder's 1:1:3:1:1 signature entirely: heavy blur on a small
/// symbol merges the top-right pattern's white ring into the adjacent payload, so it is
/// never detected however the frame is binarized. We therefore (1) score every plausible
/// triple among the detected finders and, (2) when two strong finders survive, synthesize
/// the geometrically-implied third corner (a right isosceles construction, both
/// chiralities) so a symbol with one ruined finder still gets a grid. The caller decodes
/// the ranked list until one passes Reed–Solomon; clean renders decode on the first
/// (genuine) triple, so synthesized hypotheses cost nothing there.
fn candidates(frame: &GrayFrame<'_>, bin: &Binary) -> Vec<Located> {
    if bin.width() < 21 || bin.height() < 21 {
        return Vec::new();
    }
    let mut centers = find_finders(bin);
    centers.sort_by_key(|f| std::cmp::Reverse(f.count));
    centers.truncate(MAX_FINDERS);

    // Scored candidate triples (real detections first, then synthesized ones).
    let mut triples: Vec<(f32, [Finder; 3])> = Vec::new();

    let n = centers.len();
    for i in 0..n {
        for j in (i + 1)..n {
            for k in (j + 1)..n {
                if let Some(score) = triple_score(centers[i], centers[j], centers[k]) {
                    triples.push((score, [centers[i], centers[j], centers[k]]));
                }
            }
        }
    }

    // Synthesize a third finder from each reliable pair. `a` is treated as the
    // right-angle vertex; the missing arm end is `a + rot±90°(b - a)`.
    let strong: Vec<Finder> = centers
        .iter()
        .filter(|f| f.count >= SYNTH_MIN_COUNT)
        .take(5)
        .copied()
        .collect();
    for a in &strong {
        for b in &strong {
            if std::ptr::eq(a, b) {
                continue;
            }
            for &sign in &[1.0f32, -1.0] {
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let c = Finder {
                    x: a.x - sign * dy,
                    y: a.y + sign * dx,
                    module_size: a.module_size,
                    count: 1,
                };
                // Skip if a real finder already sits where we would synthesize one — the
                // genuine triple already covers that layout.
                if centers.iter().any(|f| {
                    (f.x - c.x).abs() <= f.module_size && (f.y - c.y).abs() <= f.module_size
                }) {
                    continue;
                }
                // The square construction is only a first guess: on a tilted or curved
                // capture the true corner is not exactly square. Lock it onto the real
                // (possibly blurred) finder by template correlation, which fixes the
                // module pitch a wrong corner would otherwise skew.
                let c = refine_finder(frame, *a, *b, c);
                if let Some(score) = triple_score(*a, *b, c) {
                    triples.push((score + SYNTH_PENALTY, [*a, *b, c]));
                }
            }
        }
    }

    triples.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut out = Vec::new();
    for (_, tri) in triples.into_iter().take(MAX_TRIPLES) {
        if let Some(located) = build_located(bin, tri[0], tri[1], tri[2]) {
            out.push(located);
        }
    }
    out
}

/// Score a finder triple by how much it resembles a QR's three corner patterns — the
/// two arms from the right-angle vertex should be near-perpendicular, of similar
/// length, and the three finders should agree on module size. Returns `None` for
/// implausible triples; lower scores are more finder-like.
fn triple_score(a: Finder, b: Finder, c: Finder) -> Option<f32> {
    let ([tl, tr, bl], ms) = order_finders(a, b, c);
    if ms <= 0.0 {
        return None;
    }
    let (v1x, v1y) = (tr.x - tl.x, tr.y - tl.y);
    let (v2x, v2y) = (bl.x - tl.x, bl.y - tl.y);
    let l1 = (v1x * v1x + v1y * v1y).sqrt();
    let l2 = (v2x * v2x + v2y * v2y).sqrt();
    // Both arms must be several modules long (a QR spans dim-7 ≥ 14 modules; be lenient).
    if l1 < ms * 6.0 || l2 < ms * 6.0 {
        return None;
    }
    // Similar arm lengths (square symbol; allow perspective/curvature slack).
    let ratio = (l1 / l2).max(l2 / l1);
    if ratio > 2.0 {
        return None;
    }
    // Near-perpendicular arms.
    let cos = (v1x * v2x + v1y * v2y) / (l1 * l2);
    if cos.abs() > 0.45 {
        return None;
    }
    // Module sizes should roughly agree across the three finders.
    let (mn, mx) = [a.module_size, b.module_size, c.module_size]
        .iter()
        .fold((f32::MAX, 0.0f32), |(mn, mx), &m| (mn.min(m), mx.max(m)));
    if mn <= 0.0 || mx / mn > 3.0 {
        return None;
    }
    // Prefer square, perpendicular, high-confidence triples.
    let count = (a.count + b.count + c.count) as f32;
    Some(cos.abs() * 2.0 + ratio.ln() - 0.02 * count)
}

/// Build the full grid→pixel hypothesis for a chosen finder triple, or `None` if the
/// implied dimension is invalid.
fn build_located(bin: &Binary, a: Finder, b: Finder, c: Finder) -> Option<Located> {
    let ([tl, tr, bl], module_size) = order_finders(a, b, c);

    // Estimate the symbol dimension from the finder-center spacing. The centers sit
    // 3.5 modules in from each edge, so the TL→TR / TL→BL spans are (dim - 7) modules.
    // Measuring module size *along* each span (not from an axis-aligned run) keeps the
    // estimate correct under arbitrary rotation, since spacing and module size then
    // scale together.
    let ms_h = directional_module_size(bin, tl, tr).unwrap_or(module_size);
    let ms_v = directional_module_size(bin, tl, bl).unwrap_or(module_size);
    let dist_tr = tl.distance(tr);
    let dist_bl = tl.distance(bl);
    let est_h = dist_tr / ms_h + 7.0;
    let est_v = dist_bl / ms_v + 7.0;
    let module_size = (ms_h + ms_v) / 2.0;
    let dimension = snap_dimension((est_h + est_v) / 2.0)?;

    let d = dimension as f64;

    // Fourth corner. For versions >= 2 there is a bottom-right alignment pattern whose
    // true image position recovers real perspective; find it near where the affine
    // (parallelogram) estimate predicts. Version 1 has none, so we fall back to the
    // parallelogram rule (exact only for affine transforms).
    let (src, dst) = if dimension > 21 {
        // Affine prediction of the alignment center at grid (dim-6.5, dim-6.5).
        let u = (d - 10.0) / (d - 7.0);
        let ex = tl.x + u as f32 * (tr.x - tl.x) + u as f32 * (bl.x - tl.x);
        let ey = tl.y + u as f32 * (tr.y - tl.y) + u as f32 * (bl.y - tl.y);
        if let Some(align) = find_alignment(bin, Point::new(ex, ey), module_size) {
            let src = [
                (3.5, 3.5),
                (d - 3.5, 3.5),
                (d - 6.5, d - 6.5),
                (3.5, d - 3.5),
            ];
            let dst = [
                (tl.x as f64, tl.y as f64),
                (tr.x as f64, tr.y as f64),
                (align.x as f64, align.y as f64),
                (bl.x as f64, bl.y as f64),
            ];
            (src, dst)
        } else {
            parallelogram(tl, tr, bl, d)
        }
    } else {
        parallelogram(tl, tr, bl, d)
    };
    let projection = Projection::quad_to_quad(src, dst);

    // Outline corners for reporting use the geometric bottom-right (parallelogram).
    let br_corner = Point::new(tr.x + bl.x - tl.x, tr.y + bl.y - tl.y);

    Some(Located {
        projection,
        dimension,
        threshold: bin.threshold,
        local: bin.local,
        corners: [tl, tr, br_corner, bl],
        module_size,
    })
}
