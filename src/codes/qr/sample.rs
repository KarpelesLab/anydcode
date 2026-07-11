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

use super::QrDecoder;
use super::matrix::QUIET_ZONE;
use crate::error::{Error, Result};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::output::BitMatrix;
use crate::pipeline::{Candidate, Hints};
use crate::symbol::Symbol;
use crate::traits::{Analyze, Decode, Detect};
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
        match locate(frame) {
            Ok(loc) => vec![Candidate {
                location: loc.as_location(),
                symbology: Some(crate::symbology::Symbology::QrCode),
                fingerprint: None,
                known: None,
            }],
            Err(_) => Vec::new(),
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
pub fn scan(frame: &GrayFrame<'_>) -> Result<Symbol> {
    let matrix = sample_grid(frame)?;
    QrDecoder::new().decode(&crate::output::Encoding::Matrix(matrix))
}

/// Locate the QR symbol in `frame` and sample it to a clean [`BitMatrix`].
pub fn sample_grid(frame: &GrayFrame<'_>) -> Result<BitMatrix> {
    let located = locate(frame)?;
    Ok(located.sample(frame))
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

/// Otsu's method: the global luminance threshold maximizing between-class variance.
fn otsu_threshold(frame: &GrayFrame<'_>) -> u8 {
    let mut hist = [0u64; 256];
    for y in 0..frame.height() {
        for x in 0..frame.width() {
            hist[frame.get_unchecked(x, y) as usize] += 1;
        }
    }
    let total: u64 = (frame.width() * frame.height()) as u64;
    let sum: f64 = (0..256).map(|i| i as f64 * hist[i] as f64).sum();
    let mut sum_b = 0.0;
    let mut w_b = 0u64;
    let mut max_var = -1.0;
    let mut threshold = 127u8;
    for (t, &count) in hist.iter().enumerate() {
        w_b += count;
        if w_b == 0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0 {
            break;
        }
        sum_b += t as f64 * count as f64;
        let m_b = sum_b / w_b as f64;
        let m_f = (sum - sum_b) / w_f as f64;
        let var = w_b as f64 * w_f as f64 * (m_b - m_f) * (m_b - m_f);
        if var > max_var {
            max_var = var;
            threshold = t as u8;
        }
    }
    threshold
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
    let max_var = module / 2.0;
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
    let (counts, end) = walk_run(bin.height as i32, start as i32, |k| {
        bin.dark(cx, k as usize)
    })?;
    found_pattern_cross(counts)?;
    Some(run_center(counts, end))
}

/// Horizontal finder cross-check through row `cy`, returning the refined center-`x`.
fn cross_check_horizontal(bin: &Binary, cy: usize, start: usize) -> Option<f32> {
    let (counts, end) = walk_run(bin.width as i32, start as i32, |k| bin.dark(k as usize, cy))?;
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

/// Scan the binarized frame for finder patterns and return their clustered centers.
fn find_finders(bin: &Binary) -> Vec<Finder> {
    let mut centers: Vec<Finder> = Vec::new();
    let w = bin.width;
    for y in 0..bin.height {
        // Run-length encode the row as (dark, start, len).
        let mut runs: Vec<(bool, usize, i32)> = Vec::new();
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

        if runs.len() < 5 {
            continue;
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
            let Some(module) = found_pattern_cross(counts) else {
                continue;
            };
            let center = &runs[i + 2];
            let cx = center.1 + (center.2 as usize) / 2;
            let Some(cy) = cross_check_vertical(bin, cx, y) else {
                continue;
            };
            let cy_row = cy.round().clamp(0.0, (bin.height - 1) as f32) as usize;
            let Some(cx_ref) = cross_check_horizontal(bin, cy_row, cx) else {
                continue;
            };
            add_center(&mut centers, cx_ref, cy, module);
        }
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
    corners: [Point; 4],
    module_size: f32,
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

    /// Sample every module center into a clean [`BitMatrix`].
    fn sample(&self, frame: &GrayFrame<'_>) -> BitMatrix {
        let dim = self.dimension;
        let mut matrix = BitMatrix::new(dim, dim, QUIET_ZONE);
        for my in 0..dim {
            for mx in 0..dim {
                let (px, py) = self.projection.map(mx as f64 + 0.5, my as f64 + 0.5);
                if sample_dark(frame, px, py, self.threshold) {
                    matrix.set(mx, my, true);
                }
            }
        }
        matrix
    }
}

/// Sample a 3×3 neighborhood around `(px, py)` and decide dark vs light against the
/// binarization threshold — averaging suppresses single-pixel noise.
fn sample_dark(frame: &GrayFrame<'_>, px: f64, py: f64, threshold: u8) -> bool {
    let cx = px.round() as i32;
    let cy = py.round() as i32;
    let mut sum = 0u32;
    let mut n = 0u32;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let x = cx + dx;
            let y = cy + dy;
            if x >= 0
                && y >= 0
                && let Some(v) = frame.get(x as usize, y as usize)
            {
                sum += v as u32;
                n += 1;
            }
        }
    }
    if n == 0 {
        return false;
    }
    (sum / n) <= threshold as u32
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
    let x1 = ((expected.x as i32 + radius) as usize).min(bin.width - 1);
    let y0 = (expected.y as i32 - radius).max(0) as usize;
    let y1 = ((expected.y as i32 + radius) as usize).min(bin.height - 1);
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
                walk_run(bin.height as i32, y as i32, |k| bin.dark(cxi, k as usize))
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
        if ix < 0 || iy < 0 || ix >= bin.width as i32 || iy >= bin.height as i32 {
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

/// Full location pipeline for `frame`.
fn locate(frame: &GrayFrame<'_>) -> Result<Located> {
    if frame.width() < 21 || frame.height() < 21 {
        return Err(Error::undecodable("frame too small for a QR symbol"));
    }
    let bin = Binary::from_frame(frame);
    let mut centers = find_finders(&bin);
    if centers.len() < 3 {
        return Err(Error::undecodable(
            "fewer than three QR finder patterns found",
        ));
    }
    // Prefer the highest-confidence clusters.
    centers.sort_by_key(|f| std::cmp::Reverse(f.count));
    let (tl, tr, bl, module_size) = pick_three(&centers)?;

    // Estimate the symbol dimension from the finder-center spacing. The centers sit
    // 3.5 modules in from each edge, so the TL→TR / TL→BL spans are (dim - 7) modules.
    // Measuring module size *along* each span (not from an axis-aligned run) keeps the
    // estimate correct under arbitrary rotation, since spacing and module size then
    // scale together.
    let ms_h = directional_module_size(&bin, tl, tr).unwrap_or(module_size);
    let ms_v = directional_module_size(&bin, tl, bl).unwrap_or(module_size);
    let dist_tr = tl.distance(tr);
    let dist_bl = tl.distance(bl);
    let est_h = dist_tr / ms_h + 7.0;
    let est_v = dist_bl / ms_v + 7.0;
    let module_size = (ms_h + ms_v) / 2.0;
    let dimension =
        snap_dimension((est_h + est_v) / 2.0).ok_or_else(|| Error::undecodable("bad QR size"))?;

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
        if let Some(align) = find_alignment(&bin, Point::new(ex, ey), module_size) {
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

    Ok(Located {
        projection,
        dimension,
        threshold: bin.threshold,
        corners: [tl, tr, br_corner, bl],
        module_size,
    })
}

/// Choose the three best finder clusters, ordered as `[top-left, top-right,
/// bottom-left]`, from the count-sorted candidate list.
fn pick_three(centers: &[Finder]) -> Result<(Point, Point, Point, f32)> {
    // Take up to the strongest clusters, preferring those seen more than once.
    let strong: Vec<Finder> = centers.iter().filter(|f| f.count >= 2).copied().collect();
    let pool: &[Finder] = if strong.len() >= 3 { &strong } else { centers };
    if pool.len() < 3 {
        return Err(Error::undecodable("insufficient finder patterns"));
    }
    let (pts, module_size) = order_finders(pool[0], pool[1], pool[2]);
    Ok((pts[0], pts[1], pts[2], module_size))
}
