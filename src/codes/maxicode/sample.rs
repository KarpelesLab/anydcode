//! MaxiCode image sampling: pixels → [`BitMatrix`] → [`Symbol`].
//!
//! A MaxiCode has one finder — the **bullseye** of three dark rings at its centre —
//! and a fixed size, so the whole symbol hangs off that one pattern. The sampler:
//!
//! 1. scans rows for the bullseye's cross-section — eleven runs, dark and light
//!    rings of one width either side of a wider light centre — first against the
//!    global Otsu threshold, then against a sliding local mean for uneven light
//!    ([`find_bullseyes`]). Each hit is confirmed down its column and re-centred.
//!    Nothing is binarized: both passes classify the grey rows as they go, and a
//!    frame without the pattern costs no more than that;
//! 2. casts rays from the candidate centre over the **grey** image and takes the
//!    ring edges on each. An ellipse fitted to the centreline of the outer light ring
//!    (the midpoint of its two edges, which blur and threshold do not move) gives
//!    the centre and the affine shape of the symbol plane; anything that is not
//!    three concentric rings stops here ([`fit_bullseye`]);
//! 3. finds the module lattice. The bullseye fixes neither the lattice's rotation
//!    nor, exactly, its pitch, so both are searched for the pose under which the
//!    image is *constant within a module and changes between modules*
//!    ([`alignment`]) — a score that works on hexagons, dots and blur alike;
//! 4. resolves which of the six 60° turns is upright from the **orientation
//!    patterns**, the six fixed module triplets around the bullseye;
//! 5. extends the fit over the whole symbol: patches of modules are slid to their
//!    best alignment and a homography is solved by least squares through all of
//!    them, working outwards so the map is never extrapolated far ([`refine`]);
//! 6. reads every module as a small-area mean against the bullseye's own
//!    dark/light midpoint, then against a threshold that follows the light across
//!    the symbol, and hands the grid to [`MaxiCodeDecoder`], whose Reed–Solomon
//!    check validates the read.
//!
//! Printers disagree about the bullseye — its centre is anything from one ring
//! width across to three, and heavy ink runs the rim into the modules beside it —
//! so the finder asks only for evenly spaced rings around a light centre, mirrored
//! left and right, and measures the pitch on the modules themselves.
//!
//! Cost is bounded per frame: the two row passes (about a millisecond each on a
//! 1080p frame, every other row; a capped number of column walks on a texture
//! that looks like a cross-section on every row), at most [`MAX_EYES`] ring fits
//! per pass, and at most [`MAX_FITS`] lattice searches — a few milliseconds each
//! — which only something with three evenly spaced concentric rings reaches.
//!
//! ## Working envelope
//!
//! Validated on [`render_hex`](super::render_hex) renders (see
//! `tests/maxicode_image.rs`): any rotation, from 4 px per module up, Gaussian blur
//! to σ = 2 px and sensor noise to σ = 30 at 8 px per module, a quarter of the
//! contrast, a shadow across the symbol, ink spread that fuses modules with each
//! other and with the bullseye's rim, and perspective tilt to a 20 % foreshortening
//! of one side. A mirrored symbol is not read.

use super::tables::{HEIGHT, MAXI_GRID, ORIENTATION, WIDTH};
use super::{CENTER_COL, CENTER_ROW, MaxiCodeDecoder, RING_EDGES, ROW_PITCH};
use crate::error::{Error, Result};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::imgproc::homography::Homography;
use crate::imgproc::sample::sample_bilinear;
use crate::output::BitMatrix;
use crate::symbol::Symbol;
use alloc::vec::Vec;

/// Bullseye candidates examined per pass, most-confirmed first.
const MAX_EYES: usize = 8;
/// Bullseyes per frame that may go on from the ring fit to the lattice search, the
/// one costly step.
const MAX_FITS: usize = 4;
/// Rays cast from a candidate centre to fit its rings.
const RAYS: usize = 32;
/// Rays that must cross the ring edges, evenly spaced, for the candidate to stand.
const MIN_RAYS: usize = 26;
/// Radius (module pitches) of the centreline of the outer light ring: the circle the
/// bullseye is fitted on. Its two edges move opposite ways under blur, ink spread
/// and a biased threshold, so their midpoint stays put; and unlike the outer dark
/// ring it never touches a module.
const RING_MID: f64 = (RING_EDGES[3] + RING_EDGES[4]) / 2.0;
/// The three directions of the module lattice (unit vectors, one module pitch).
const DIRS: [(f64, f64); 3] = [(1.0, 0.0), (0.5, ROW_PITCH), (-0.5, ROW_PITCH)];
/// Modules closer to the centre than this are under the bullseye.
const EYE_CLEAR: f64 = 5.3;

/// A bullseye candidate from the row scan.
#[derive(Debug, Clone, Copy)]
struct Eye {
    cx: f64,
    cy: f64,
    /// Half the cross-section's length along the row and the column, in pixels.
    rx: f64,
    ry: f64,
    /// The dark/light midpoint of the cross-section that found it.
    thr: f64,
    /// Rows that hit it.
    count: u32,
}

/// A bullseye fitted on the grey image.
#[derive(Debug, Clone, Copy)]
struct Core {
    cx: f64,
    cy: f64,
    /// Module pitches → pixels, up to a rotation about the centre (row-major 2×2).
    shape: [f64; 4],
    /// Mean luminance of the dark and the light rings.
    dark: f64,
    light: f64,
}

impl Core {
    fn thr(&self) -> f64 {
        (self.dark + self.light) / 2.0
    }

    fn contrast(&self) -> f64 {
        self.light - self.dark
    }

    /// The module map with the lattice turned by `phi` and its pitch scaled by `s`.
    fn pose(&self, phi: f64, s: f64) -> Homography {
        let (sin, cos) = phi.sin_cos();
        let b = &self.shape;
        Homography::from_matrix([
            s * (b[0] * cos + b[1] * sin),
            s * (-b[0] * sin + b[1] * cos),
            self.cx,
            s * (b[2] * cos + b[3] * sin),
            s * (-b[2] * sin + b[3] * cos),
            self.cy,
            0.0,
            0.0,
            1.0,
        ])
    }
}

/// Locate, sample and structurally decode the MaxiCode symbol in `frame`.
///
/// The returned symbol carries its [`Symbol::location`]: the outline of the module
/// area, the rotation of its rows and the module pitch.
///
/// # Errors
/// [`Error::Undecodable`] when no bullseye is found, or
/// [`Error::ErrorCorrectionFailed`] (or another decode error) when one is found but
/// the modules around it do not form a valid symbol.
pub fn scan(frame: &GrayFrame<'_>) -> Result<Symbol> {
    let mut last = Error::undecodable("no MaxiCode bullseye found");
    if frame.width() < 32 || frame.height() < 32 {
        return Err(last);
    }
    let decoder = MaxiCodeDecoder::new();
    // Bullseyes already fitted and tried: the second pass finding one again would
    // only repeat the same grey-level work.
    let mut tried: Vec<(f64, f64, f64)> = Vec::new();
    for pass in 0..2 {
        let eyes = find_bullseyes(frame, pass == 1);
        for eye in eyes.iter().take(MAX_EYES) {
            let near = |&(x, y, reach): &(f64, f64, f64)| {
                (x - eye.cx).abs() <= reach && (y - eye.cy).abs() <= reach
            };
            if tried.iter().any(near) {
                continue;
            }
            let Some(core) = fit_bullseye(frame, eye) else {
                continue;
            };
            tried.push((core.cx, core.cy, eye.rx.min(eye.ry) / 2.0));
            match decode_core(frame, &core, &decoder) {
                Ok(sym) => return Ok(sym),
                Err(e) => last = e,
            }
            if tried.len() == MAX_FITS {
                return Err(last);
            }
        }
    }
    Err(last)
}

// ---------------------------------------------------------------------------
// Bullseye candidates
// ---------------------------------------------------------------------------

/// Whether eleven consecutive run lengths are a bullseye cross-section: five rings
/// of one width, a light centre, the same five rings mirrored. Printers differ on
/// the centre — from a little over one ring width to three, and ink spread eats
/// into it — so it is the mirror symmetry that tells the bullseye from a window two
/// runs off it.
///
/// The outermost dark ring may have run into a dark module beside it, so its runs
/// only have a lower bound. Returns the cross-section's length with those two runs
/// taken at the ring width.
fn cross_section_length(lens: &[f64; 11]) -> Option<f64> {
    let inner: f64 = lens[1..10].iter().sum();
    let ring = (inner - lens[5]) / 8.0;
    if inner < 16.0 || lens[5] < 0.8 * ring || lens[5] > 3.8 * ring {
        return None;
    }
    let rings = (1..5).all(|i| {
        let (a, b) = (lens[i], lens[10 - i]);
        a.min(b) >= 0.5 * ring && a.max(b) <= 1.7 * ring && (a - b).abs() <= 0.4 * ring + 1.0
    });
    (rings && lens[0].min(lens[10]) >= 0.5 * ring).then_some(inner + 2.0 * ring)
}

/// Otsu's threshold over the pixels the row scan will visit (every `stride`-th row,
/// every other pixel): values `<= t` are dark.
fn sparse_otsu(frame: &GrayFrame<'_>, stride: usize) -> u8 {
    let mut hist = [0u32; 256];
    for y in (0..frame.height()).step_by(stride) {
        if let Some(row) = frame.row(y) {
            for &v in row.iter().step_by(2) {
                hist[usize::from(v)] += 1;
            }
        }
    }
    let total: u64 = hist.iter().map(|&c| u64::from(c)).sum();
    let sum_all: u64 = (0u64..).zip(hist).map(|(i, c)| i * u64::from(c)).sum();
    let (mut below, mut sum_below) = (0u64, 0u64);
    let (mut best, mut best_t) = (-1.0f64, 127u8);
    for (t, &count) in hist.iter().enumerate() {
        below += u64::from(count);
        sum_below += t as u64 * u64::from(count);
        let above = total - below;
        if below == 0 || above == 0 {
            continue;
        }
        let diff = sum_below as f64 / below as f64 - (sum_all - sum_below) as f64 / above as f64;
        let var = below as f64 * above as f64 * diff * diff;
        if var > best {
            best = var;
            best_t = t as u8;
        }
    }
    best_t
}

/// Classify a row against the mean of the `2·radius + 1` pixels around each pixel
/// (clipped at the ends): dark is an eighth or more below it. `prefix` is scratch.
fn classify_local(row: &[u8], radius: usize, prefix: &mut [u32], darks: &mut [u8]) {
    let w = row.len();
    for (x, &v) in row.iter().enumerate() {
        prefix[x + 1] = prefix[x] + u32::from(v);
    }
    let dark = |x: usize, x0: usize, x1: usize| {
        u8::from(u32::from(row[x]) * (x1 - x0) as u32 * 8 < (prefix[x1] - prefix[x0]) * 7)
    };
    let side = 2 * radius + 1;
    if w < side {
        for (x, d) in darks.iter_mut().enumerate() {
            *d = dark(x, x.saturating_sub(radius), (x + radius + 1).min(w));
        }
        return;
    }
    for x in (0..radius).chain(w - radius..w) {
        darks[x] = dark(x, x.saturating_sub(radius), (x + radius + 1).min(w));
    }
    // The interior, where the window is whole: three slices in step, no branches.
    let n = side as u32 * 8;
    let (lo, hi) = (&prefix[..w - side + 1], &prefix[side..]);
    let inner = darks[radius..w - radius].iter_mut().zip(&row[radius..]);
    for ((d, &v), (&a, &b)) in inner.zip(lo.iter().zip(hi)) {
        *d = u8::from(u32::from(v) * n < (b - a) * 7);
    }
}

/// The positions where a classified row changes, starting from light: `edges[j]..
/// edges[j + 1]` is a dark run for even `j`, a light one for odd.
fn transitions(darks: &[u8], edges: &mut Vec<u32>) {
    edges.clear();
    let mut prev = 0u32;
    for (c, chunk) in darks.chunks(32).enumerate() {
        // The chunk as a bit mask, then its changes as the set bits of the mask
        // against itself shifted by one: no branch per pixel, and most of a frame —
        // flat — costs nothing more.
        let mask = (0u32..)
            .zip(chunk)
            .fold(0, |m, (i, &d)| m | u32::from(d) << i);
        let mut changes = mask ^ (mask << 1 | prev);
        while changes != 0 {
            edges.push(32 * c as u32 + changes.trailing_zeros());
            changes &= changes - 1;
        }
        prev = mask >> (chunk.len() - 1) & 1;
    }
    if prev == 1 {
        edges.push(darks.len() as u32);
    }
}

/// Scan rows for the bullseye cross-section; confirm and centre each hit on the grey
/// image. `local` classifies pixels against a sliding mean rather than the global
/// Otsu threshold.
fn find_bullseyes(frame: &GrayFrame<'_>, local: bool) -> Vec<Eye> {
    let (w, h) = (frame.width(), frame.height());
    let mut eyes: Vec<Eye> = Vec::new();
    // The centre disc of a readable bullseye is several rows tall: on a large frame
    // every other row is enough.
    let stride = if h >= 900 { 2 } else { 1 };
    let global = if local { 0 } else { sparse_otsu(frame, stride) };
    let radius = (w.min(h) / 10).max(16);
    let mut darks = alloc::vec![0u8; w];
    let mut edges: Vec<u32> = Vec::with_capacity(w + 2);
    let mut prefix = alloc::vec![0u32; if local { w + 1 } else { 0 }];
    // Cross-sections that may fail to confirm before the frame is written off as a
    // texture: stripes and halftones look like one on every row.
    let mut budget = (w * h / 128).max(4000);
    'rows: for y in (0..h).step_by(stride) {
        let Some(row) = frame.row(y) else {
            break;
        };
        if local {
            classify_local(row, radius, &mut prefix, &mut darks);
        } else {
            for (d, &v) in darks.iter_mut().zip(row) {
                *d = u8::from(v <= global);
            }
        }
        transitions(&darks, &mut edges);
        if edges.len() < 12 {
            continue;
        }
        for j in (0..edges.len() - 11).step_by(2) {
            let e = &edges[j..j + 12];
            // The cheap half of the test first: the two rings beside the centre
            // are alike, and the whole is not tiny.
            let (a, b) = (e[5] - e[4], e[7] - e[6]);
            if e[10] - e[1] < 16 || a > 2 * b + 2 || b > 2 * a + 2 {
                continue;
            }
            let mut lens = [0.0; 11];
            for (i, l) in lens.iter_mut().enumerate() {
                *l = f64::from(e[i + 1] - e[i]);
            }
            let Some(length) = cross_section_length(&lens) else {
                continue;
            };
            let cx = f64::from(e[5] + e[6]) / 2.0;
            // The dark/light midpoint of this very cross-section (its rim runs, which
            // may be part of something else, left out).
            let (mut sums, mut ns) = ([0u32; 2], [0u32; 2]);
            for i in 1..10 {
                for &v in &row[e[i] as usize..e[i + 1] as usize] {
                    sums[i % 2] += u32::from(v);
                    ns[i % 2] += 1;
                }
            }
            let thr = (f64::from(sums[0]) / f64::from(ns[0])
                + f64::from(sums[1]) / f64::from(ns[1]))
                / 2.0;
            let Some(eye) = confirm(frame, cx as usize, y, thr, length / 2.0) else {
                budget -= 1;
                if budget == 0 {
                    break 'rows;
                }
                continue;
            };
            // Every row through the centre disc lands on the same centre.
            match eyes
                .iter_mut()
                .find(|k| (k.cx - eye.cx).abs() < 0.1 * k.rx && (k.cy - eye.cy).abs() < 0.1 * k.ry)
            {
                Some(known) => known.count += 1,
                None => eyes.push(eye),
            }
        }
    }
    eyes.sort_by_key(|e| core::cmp::Reverse(e.count));
    eyes
}

/// Walk from light pixel `(x, y)` along `(dx, dy)` through the bullseye: the rest of
/// the centre run, then five ring runs. `None` if the frame ends first, or a run
/// grows past what a bullseye `limit` pixels across could hold — which is how most
/// false hits are dismissed, a few pixels in.
fn walk(
    frame: &GrayFrame<'_>,
    (x, y): (usize, usize),
    (dx, dy): (isize, isize),
    thr: f64,
    limit: usize,
) -> Option<[usize; 6]> {
    let mut runs = [0usize; 6];
    let mut idx = 0;
    let (mut x, mut y) = (x as isize, y as isize);
    // The centre is under two fifths of the whole and a ring under a fifth; the rim
    // may have a module or two against it.
    let caps = [
        limit / 4,
        limit / 5,
        limit / 5,
        limit / 5,
        limit / 5,
        limit / 2,
    ];
    loop {
        x += dx;
        y += dy;
        if x < 0 || y < 0 {
            return None;
        }
        let v = frame.get(x as usize, y as usize)?;
        // Odd runs are dark rings.
        if (f64::from(v) < thr) != (idx % 2 == 1) {
            idx += 1;
            if idx == 6 {
                return Some(runs);
            }
        }
        runs[idx] += 1;
        if runs[idx] > caps[idx] {
            return None;
        }
    }
}

/// The cross-section through `(x, y)` along the x axis (`horizontal`) or the y axis:
/// its length and the offset of the centre run's middle from the start.
fn cross_section(
    frame: &GrayFrame<'_>,
    (x, y): (usize, usize),
    horizontal: bool,
    thr: f64,
    limit: usize,
) -> Option<(f64, f64)> {
    if f64::from(frame.get(x, y)?) < thr {
        return None;
    }
    let (dx, dy) = if horizontal { (1, 0) } else { (0, 1) };
    let back = walk(frame, (x, y), (-dx, -dy), thr, limit)?;
    let fwd = walk(frame, (x, y), (dx, dy), thr, limit)?;
    let mut lens = [0.0; 11];
    for i in 1..6 {
        lens[5 - i] = back[i] as f64;
        lens[5 + i] = fwd[i] as f64;
    }
    lens[5] = (back[0] + fwd[0] + 1) as f64;
    Some((
        cross_section_length(&lens)?,
        (fwd[0] as f64 - back[0] as f64) / 2.0,
    ))
}

/// Confirm a row hit at `(x, y)` down its column and centre it: column, row, column.
fn confirm(frame: &GrayFrame<'_>, x: usize, y: usize, thr: f64, half: f64) -> Option<Eye> {
    // Tilt may stretch the column to twice the row.
    let limit = (4.0 * half) as usize + 10;
    let (_, dy) = cross_section(frame, (x, y), false, thr, limit)?;
    let y = (y as f64 + dy).round() as usize;
    let (row, dx) = cross_section(frame, (x, y), true, thr, limit)?;
    let cx = x as f64 + dx;
    let x = cx.round() as usize;
    let (col, dy) = cross_section(frame, (x, y), false, thr, limit)?;
    let (rx, ry) = (row / 2.0, col / 2.0);
    (rx < 2.5 * ry && ry < 2.5 * rx).then_some(Eye {
        cx,
        cy: y as f64 + dy,
        rx,
        ry,
        thr,
        count: 1,
    })
}

// ---------------------------------------------------------------------------
// Bullseye fit
// ---------------------------------------------------------------------------

/// Solve the `N×N` system `a·x = b` by Gaussian elimination with partial pivoting.
fn solve<const N: usize>(mut a: [[f64; N]; N], mut b: [f64; N]) -> Option<[f64; N]> {
    for col in 0..N {
        let pivot = (col..N).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let (top, rest) = a.split_at_mut(col + 1);
        let top = &top[col];
        for (row, r) in rest.iter_mut().zip(col + 1..) {
            let f = row[col] / top[col];
            if f != 0.0 {
                for (v, t) in row.iter_mut().zip(top).skip(col) {
                    *v -= f * t;
                }
                b[r] -= f * b[col];
            }
        }
    }
    let mut x = [0.0; N];
    for r in (0..N).rev() {
        let mut s = b[r];
        for c in r + 1..N {
            s -= a[r][c] * x[c];
        }
        x[r] = s / a[r][r];
    }
    Some(x)
}

/// Least squares over rows `(coefficients, target)`: solves the normal equations.
fn least_squares<const N: usize>(rows: &[([f64; N], f64)]) -> Option<[f64; N]> {
    let mut a = [[0.0; N]; N];
    let mut b = [0.0; N];
    for (c, t) in rows {
        for i in 0..N {
            for j in 0..N {
                a[i][j] += c[i] * c[j];
            }
            b[i] += c[i] * t;
        }
    }
    solve(a, b)
}

/// The radii at which the ray from `(cx, cy)` along `(dx, dy)` crosses `thr`: the
/// first five ring edges, light-to-dark first. (The sixth, the bullseye's rim, is
/// not looked for: a dark module may sit against it.)
fn ray_edges(
    frame: &GrayFrame<'_>,
    (cx, cy): (f64, f64),
    (dx, dy): (f64, f64),
    thr: f64,
    step: f64,
    reach: f64,
) -> Option<[f64; 5]> {
    let (w, h) = (frame.width() as f64, frame.height() as f64);
    let mut edges = [0.0; 5];
    let mut found = 0;
    let mut prev = sample_bilinear(frame, cx, cy);
    if prev < thr {
        return None;
    }
    let mut r = step;
    while r <= reach {
        let (x, y) = (cx + r * dx, cy + r * dy);
        if x < 0.0 || y < 0.0 || x > w - 1.0 || y > h - 1.0 {
            return None;
        }
        let v = sample_bilinear(frame, x, y);
        // Even edges go light → dark.
        if (v < thr) == (found % 2 == 0) {
            let t = if (v - prev).abs() > 1e-9 {
                (thr - prev) / (v - prev)
            } else {
                0.5
            };
            edges[found] = r - step + t.clamp(0.0, 1.0) * step;
            found += 1;
            if found == 5 {
                return Some(edges);
            }
        }
        prev = v;
        r += step;
    }
    None
}

/// Fit the bullseye on the grey image: the ring edges along rays from the
/// candidate centre, an ellipse through the centreline of the outer light ring.
fn fit_bullseye(frame: &GrayFrame<'_>, eye: &Eye) -> Option<Core> {
    let reach = 1.5 * eye.rx.max(eye.ry);
    let step = (eye.rx.min(eye.ry) / 40.0).clamp(0.25, 1.0);
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(RAYS);
    let mut mean = 0.0;
    // The ring edges' radii, as fractions of the outer centreline's, summed over rays.
    let mut profile = [0.0; 6];
    for i in 0..RAYS {
        let a = core::f64::consts::TAU * i as f64 / RAYS as f64;
        let (dy, dx) = a.sin_cos();
        let Some(e) = ray_edges(frame, (eye.cx, eye.cy), (dx, dy), eye.thr, step, reach) else {
            continue;
        };
        // The rings are evenly spaced — dark centrelines as far apart as light
        // ones — around a centre no smaller than a fraction of a ring. (How large
        // the centre is varies from printer to printer.)
        let dark = (e[2] + e[3]) / 2.0 - (e[0] + e[1]) / 2.0;
        let light = (e[3] + e[4]) / 2.0 - (e[1] + e[2]) / 2.0;
        if (dark - light).abs() > 0.3 * light || e[0] < 0.1 * light {
            continue;
        }
        let mid = (e[3] + e[4]) / 2.0;
        for (p, edge) in profile.iter_mut().zip(e) {
            *p += edge / mid;
        }
        // The rim, a ring's width beyond the last edge found.
        profile[5] += (e[4] + light / 2.0) / mid;
        mean += mid;
        pts.push((mid * dx, mid * dy));
    }
    if pts.len() < MIN_RAYS {
        return None;
    }
    let r0 = mean / pts.len() as f64;

    // The conic A·x² + B·xy + C·y² + D·x + E·y = 1 through the centreline points,
    // in units of the mean radius.
    let rows: Vec<([f64; 5], f64)> = pts
        .iter()
        .map(|&(x, y)| {
            let (x, y) = (x / r0, y / r0);
            ([x * x, x * y, y * y, x, y], 1.0)
        })
        .collect();
    let [a, b, c, d, e] = least_squares(&rows)?;
    let [x0, y0] = solve([[2.0 * a, b], [b, 2.0 * c]], [-d, -e])?;
    // About its centre: A·x² + B·xy + C·y² = k.
    let k = 1.0 + a * x0 * x0 + b * x0 * y0 + c * y0 * y0;
    if k.is_nan() || k <= 0.0 {
        return None;
    }
    let (a, b, c) = (a / k, b / (2.0 * k), c / k);
    // Eigen-decomposition of [[a, b], [b, c]]; the ellipse's semi-axes are 1/√λ.
    let theta = 0.5 * (2.0 * b).atan2(a - c);
    let (sin, cos) = theta.sin_cos();
    let l1 = a * cos * cos + 2.0 * b * sin * cos + c * sin * sin;
    let l2 = a * sin * sin - 2.0 * b * sin * cos + c * cos * cos;
    if l1.is_nan() || l2.is_nan() || l1 <= 0.0 || l2 <= 0.0 {
        return None;
    }
    let (s1, s2) = (r0 / l1.sqrt(), r0 / l2.sqrt());
    // Not a circle seen at any sane tilt, or too small to hold modules. (Written so
    // that a NaN from a degenerate fit fails it.)
    let sane = s1 <= 3.0 * s2 && s2 <= 3.0 * s1 && s1.min(s2) >= 6.0 && x0.hypot(y0) <= 0.3;
    if !sane {
        return None;
    }
    // The symmetric map taking the unit circle to the ellipse, per module pitch.
    let (m1, m2) = (s1 / RING_MID, s2 / RING_MID);
    let shape = [
        m1 * cos * cos + m2 * sin * sin,
        (m1 - m2) * sin * cos,
        (m1 - m2) * sin * cos,
        m1 * sin * sin + m2 * cos * cos,
    ];
    let mut core = Core {
        cx: eye.cx + x0 * r0,
        cy: eye.cy + y0 * r0,
        shape,
        dark: 0.0,
        light: 0.0,
    };

    // Ring by ring on the fitted map: three dark, three light, each clear of the
    // midpoint all the way round.
    let h = core.pose(0.0, 1.0);
    let profile = profile.map(|p| RING_MID * p / pts.len() as f64);
    let level = |lo: f64, hi: f64| {
        let r = (lo + hi) / 2.0;
        let mut vals = [0.0; 16];
        for (i, v) in vals.iter_mut().enumerate() {
            let (sin, cos) = (core::f64::consts::TAU * i as f64 / 16.0).sin_cos();
            let (x, y) = h.map_f64(r * cos, r * sin);
            *v = sample_bilinear(frame, x, y);
        }
        vals
    };
    let rings = [
        level(0.0, 0.0),
        level(profile[0], profile[1]),
        level(profile[1], profile[2]),
        level(profile[2], profile[3]),
        level(profile[3], profile[4]),
        level(profile[4], profile[5]),
    ];
    let mean_of = |parity: usize| {
        let vals = rings.iter().skip(parity).step_by(2).flatten();
        vals.sum::<f64>() / 48.0
    };
    core.light = mean_of(0);
    core.dark = mean_of(1);
    if core.contrast() < 16.0 {
        return None;
    }
    let thr = core.thr();
    let wrong = rings
        .iter()
        .enumerate()
        .flat_map(|(i, ring)| ring.iter().map(move |&v| (v < thr) != (i % 2 == 1)))
        .filter(|&w| w)
        .count();
    (wrong <= 6).then_some(core)
}

// ---------------------------------------------------------------------------
// Lattice
// ---------------------------------------------------------------------------

/// Module-pitch coordinates of module `(row, col)` relative to the bullseye centre.
fn module_center(row: usize, col: usize) -> (f64, f64) {
    (
        col as f64 - CENTER_COL as f64 + if row % 2 == 1 { 0.5 } else { 0.0 },
        (row as f64 - CENTER_ROW as f64) * ROW_PITCH,
    )
}

/// How well the module lattice under `h`, shifted by `(du, dv)` modules, lines up
/// with the image around the lattice points `cells`.
///
/// Along each lattice direction, two samples a quarter-module either side of a
/// module centre lie in the same module; the next pair, half a module on, straddles
/// the border to its neighbour. The score is the mean difference across borders
/// less the mean difference within modules, in luminance: zero for a lattice that
/// is out of step or on a blank area, and up to half the symbol's contrast when
/// every module is where the map says.
fn alignment(
    frame: &GrayFrame<'_>,
    h: &Homography,
    cells: &[(f64, f64)],
    (du, dv): (f64, f64),
) -> f64 {
    if cells.is_empty() {
        return 0.0;
    }
    let at = |u: f64, v: f64| {
        let (x, y) = h.map_f64(u, v);
        sample_bilinear(frame, x, y)
    };
    let mut score = 0.0;
    for &(u, v) in cells {
        let (u, v) = (u + du, v + dv);
        for (a, b) in DIRS {
            let p0 = at(u - 0.25 * a, v - 0.25 * b);
            let p1 = at(u + 0.25 * a, v + 0.25 * b);
            let p2 = at(u + 0.75 * a, v + 0.75 * b);
            score += (p1 - p2).abs() - (p0 - p1).abs();
        }
    }
    score / (3 * cells.len()) as f64
}

/// Lattice points in the ring `lo..hi` module pitches from the centre. The ring and
/// the lattice are both unchanged by a 60° turn, so the set serves before the
/// symbol's orientation is known.
fn ring_cells(lo: f64, hi: f64) -> Vec<(f64, f64)> {
    let mut cells = Vec::new();
    let n = hi as i32 + 2;
    for j in -n..=n {
        for i in -2 * n..=2 * n {
            let (u, v) = (f64::from(i) + 0.5 * f64::from(j), f64::from(j) * ROW_PITCH);
            let r = u.hypot(v);
            if r >= lo && r < hi {
                cells.push((u, v));
            }
        }
    }
    cells
}

/// The symbol's own modules clear of the bullseye.
fn symbol_cells() -> Vec<(f64, f64)> {
    let mut cells = Vec::with_capacity(WIDTH * HEIGHT);
    for row in 0..HEIGHT {
        for col in 0..WIDTH - row % 2 {
            let (u, v) = module_center(row, col);
            if u.hypot(v) >= EYE_CLEAR {
                cells.push((u, v));
            }
        }
    }
    cells
}

/// Search the lattice's rotation (within its 60° period) and pitch for the pose that
/// lines up best with the modules just outside the bullseye.
fn find_lattice(frame: &GrayFrame<'_>, core: &Core) -> Option<Homography> {
    let cells = ring_cells(EYE_CLEAR + 0.3, 8.8);
    let score = |phi: f64, s: f64| alignment(frame, &core.pose(phi, s), &cells, (0.0, 0.0));
    const PHI_STEP: f64 = 1.5 * core::f64::consts::PI / 180.0;
    const S_STEP: f64 = 0.03;
    let mut best = (f64::MIN, 0.0, 1.0);
    for i in 0..40 {
        for j in -5..=5 {
            let (phi, s) = (f64::from(i) * PHI_STEP, 1.0 + f64::from(j) * S_STEP);
            let v = score(phi, s);
            if v > best.0 {
                best = (v, phi, s);
            }
        }
    }
    let (_, phi0, s0) = best;
    for i in -4..=4 {
        for j in -4..=4 {
            let (phi, s) = (
                phi0 + f64::from(i) * PHI_STEP / 4.0,
                s0 + f64::from(j) * S_STEP / 4.0,
            );
            let v = score(phi, s);
            if v > best.0 {
                best = (v, phi, s);
            }
        }
    }
    // A lattice is there at all: a fair share of borders show a module edge.
    (best.0 > 0.06 * core.contrast()).then(|| core.pose(best.1, best.2))
}

/// `h` with the module frame turned by `k` sixths of a turn.
fn turned(h: &Homography, k: usize) -> Homography {
    let (sin, cos) = (core::f64::consts::FRAC_PI_3 * k as f64).sin_cos();
    let mut m = h.matrix();
    for row in 0..3 {
        let (a, b) = (m[3 * row], m[3 * row + 1]);
        m[3 * row] = a * cos + b * sin;
        m[3 * row + 1] = -a * sin + b * cos;
    }
    Homography::from_matrix(m)
}

/// Mean luminance of the middle of the module at `(u, v)`.
fn module_value(frame: &GrayFrame<'_>, h: &Homography, (u, v): (f64, f64)) -> f64 {
    let at = |u: f64, v: f64| {
        let (x, y) = h.map_f64(u, v);
        sample_bilinear(frame, x, y)
    };
    let mut sum = 2.0 * at(u, v);
    for (a, b) in DIRS {
        sum += at(u + 0.2 * a, v + 0.2 * b) + at(u - 0.2 * a, v - 0.2 * b);
    }
    sum / 8.0
}

/// The fixed modules around the bullseye — the six orientation triplets — as
/// `(u, v, dark)`.
fn orientation_cells() -> Vec<(f64, f64, bool)> {
    let mut cells = Vec::new();
    for (row, grid) in MAXI_GRID.iter().enumerate() {
        for (col, &n) in grid.iter().enumerate().take(WIDTH - row % 2) {
            let (u, v) = module_center(row, col);
            let r = u.hypot(v);
            if n == 0 && (EYE_CLEAR..10.0).contains(&r) {
                cells.push((u, v, ORIENTATION.contains(&(row, col))));
            }
        }
    }
    cells
}

/// A correspondence: module coordinates, and the pixel they belong at.
type Pair = ((f64, f64), (f64, f64));

/// One patch of the fit: the modules within `reach` of `anchor` are slid over
/// `±span` modules to their best alignment. Returns the anchor's correspondence —
/// its module coordinates and where in the image it really is — or `None` where
/// the patch shows no lattice.
fn slide(
    frame: &GrayFrame<'_>,
    h: &Homography,
    cells: &[(f64, f64)],
    anchor: (f64, f64),
    span: f64,
    floor: f64,
) -> Option<Pair> {
    const N: usize = 7;
    let patch: Vec<(f64, f64)> = cells
        .iter()
        .copied()
        .filter(|&(u, v)| (u - anchor.0).hypot(v - anchor.1) <= 2.6)
        .collect();
    if patch.len() < 8 {
        return None;
    }
    let step = 2.0 * span / (N - 1) as f64;
    let offset = |i: usize| -span + i as f64 * step;
    let mut scores = [[0.0; N]; N];
    let mut best = (0, 0);
    for j in 0..N {
        for i in 0..N {
            scores[j][i] = alignment(frame, h, &patch, (offset(i), offset(j)));
            if scores[j][i] > scores[best.1][best.0] {
                best = (i, j);
            }
        }
    }
    let (i, j) = best;
    if scores[j][i] < floor {
        return None;
    }
    // Parabolic interpolation of the peak along each axis.
    let peak = |lo: f64, mid: f64, hi: f64| {
        let curve = lo - 2.0 * mid + hi;
        if curve < -1e-9 {
            (0.5 * (lo - hi) / curve).clamp(-0.5, 0.5)
        } else {
            0.0
        }
    };
    let mut du = offset(i);
    let mut dv = offset(j);
    if i > 0 && i + 1 < N {
        du += step * peak(scores[j][i - 1], scores[j][i], scores[j][i + 1]);
    }
    if j > 0 && j + 1 < N {
        dv += step * peak(scores[j - 1][i], scores[j][i], scores[j + 1][i]);
    }
    Some((anchor, h.map_f64(anchor.0 + du, anchor.1 + dv)))
}

/// Solve the map through correspondences `(module, pixel)` by least squares: an
/// affine one, or a full homography when `projective`.
fn fit_map(pairs: &[Pair], (cx, cy): (f64, f64), projective: bool) -> Option<Homography> {
    // Pixels are taken relative to the centre so the system is well conditioned.
    if projective {
        if pairs.len() < 8 {
            return None;
        }
        let mut rows: Vec<([f64; 8], f64)> = Vec::with_capacity(2 * pairs.len());
        for &((u, v), (x, y)) in pairs {
            let (x, y) = (x - cx, y - cy);
            rows.push(([u, v, 1.0, 0.0, 0.0, 0.0, -u * x, -v * x], x));
            rows.push(([0.0, 0.0, 0.0, u, v, 1.0, -u * y, -v * y], y));
        }
        let p = least_squares(&rows)?;
        Some(Homography::from_matrix([
            p[0] + cx * p[6],
            p[1] + cx * p[7],
            p[2] + cx,
            p[3] + cy * p[6],
            p[4] + cy * p[7],
            p[5] + cy,
            p[6],
            p[7],
            1.0,
        ]))
    } else {
        if pairs.len() < 4 {
            return None;
        }
        let xs: Vec<([f64; 3], f64)> = pairs
            .iter()
            .map(|&((u, v), (x, _))| ([u, v, 1.0], x - cx))
            .collect();
        let ys: Vec<([f64; 3], f64)> = pairs
            .iter()
            .map(|&((u, v), (_, y))| ([u, v, 1.0], y - cy))
            .collect();
        let (p, q) = (least_squares(&xs)?, least_squares(&ys)?);
        Some(Homography::from_matrix([
            p[0],
            p[1],
            p[2] + cx,
            q[0],
            q[1],
            q[2] + cy,
            0.0,
            0.0,
            1.0,
        ]))
    }
}

/// One round of the fit: slide a patch at every anchor and solve the map through
/// the results (and the bullseye centre, which the rings pinned).
fn refine(
    frame: &GrayFrame<'_>,
    core: &Core,
    h: &Homography,
    cells: &[(f64, f64)],
    anchors: &[(f64, f64)],
    span: f64,
    projective: bool,
) -> Option<Homography> {
    let floor = 0.05 * core.contrast();
    let mut pairs: Vec<Pair> = anchors
        .iter()
        .filter_map(|&a| slide(frame, h, cells, a, span, floor))
        .collect();
    if pairs.len() < anchors.len() / 3 {
        return None;
    }
    for _ in 0..4 {
        pairs.push(((0.0, 0.0), (core.cx, core.cy)));
    }
    fit_map(&pairs, (core.cx, core.cy), projective)
}

/// `n` anchors evenly round the circle of radius `r` modules.
fn circle_anchors(r: f64, n: usize) -> impl Iterator<Item = (f64, f64)> {
    (0..n).map(move |i| {
        let (sin, cos) = (core::f64::consts::TAU * i as f64 / n as f64).sin_cos();
        (r * cos, r * sin)
    })
}

/// From a fitted bullseye to a decoded symbol.
fn decode_core(frame: &GrayFrame<'_>, core: &Core, decoder: &MaxiCodeDecoder) -> Result<Symbol> {
    let none = || Error::undecodable("no MaxiCode module lattice around the bullseye");
    let h = find_lattice(frame, core).ok_or_else(none)?;
    // Tighten the pose on the modules nearest the bullseye, still as an affine map.
    let near = ring_cells(EYE_CLEAR, 10.6);
    let anchors: Vec<(f64, f64)> = circle_anchors(7.6, 12).collect();
    let h = refine(frame, core, &h, &near, &anchors, 0.3, false).ok_or_else(none)?;

    // Which sixth of a turn is upright: the orientation triplets say.
    let marks = orientation_cells();
    let thr = core.thr();
    let mut turns: Vec<(usize, usize)> = (0..6)
        .map(|k| {
            let hk = turned(&h, k);
            let wrong = marks
                .iter()
                .filter(|&&(u, v, dark)| (module_value(frame, &hk, (u, v)) < thr) != dark)
                .count();
            (wrong, k)
        })
        .collect();
    turns.sort_unstable();
    let mut last = Error::undecodable("MaxiCode orientation patterns not found");
    let cells = symbol_cells();
    for &(wrong, k) in turns.iter().take(2) {
        if wrong > marks.len() / 6 {
            break;
        }
        match decode_turned(frame, core, &turned(&h, k), &cells, decoder) {
            Ok(sym) => return Ok(sym),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Extend the upright near-bullseye map `h` over the whole symbol, read and decode.
fn decode_turned(
    frame: &GrayFrame<'_>,
    core: &Core,
    h: &Homography,
    cells: &[(f64, f64)],
    decoder: &MaxiCodeDecoder,
) -> Result<Symbol> {
    // Outwards a couple of modules at a time — under perspective the map drifts
    // with the square of the distance from where it was fitted — then patches all
    // over the symbol, twice.
    let mut h = Homography::from_matrix(h.matrix());
    for outer in [9.6, 11.8, 14.0] {
        let rings: Vec<(f64, f64)> = circle_anchors(7.6, 12)
            .chain(circle_anchors(outer, 18))
            .collect();
        if let Some(next) = refine(frame, core, &h, cells, &rings, 0.3, true) {
            h = next;
        }
    }
    let mut all: Vec<(f64, f64)> = Vec::new();
    for j in 0..7 {
        for i in 0..7 {
            let (u, v) = (-12.25 + 4.25 * f64::from(i), -12.0 + 4.0 * f64::from(j));
            if u.hypot(v) >= 6.5 {
                all.push((u, v));
            }
        }
    }
    for span in [0.36, 0.18] {
        if let Some(next) = refine(frame, core, &h, cells, &all, span, true) {
            h = next;
        }
    }

    let mut values = [[0.0f64; WIDTH]; HEIGHT];
    for (row, line) in values.iter_mut().enumerate() {
        for (col, v) in line.iter_mut().enumerate() {
            *v = module_value(frame, &h, module_center(row, col));
        }
    }
    let thr = core.thr();
    let flat = to_matrix(|_, _| thr, &values);
    let mut result = decoder.decode_matrix(&flat);
    if result.is_err() {
        // The light may change across the symbol: follow it.
        let contrast = core.contrast();
        let local = |row: usize, col: usize| {
            let (mut lo, mut hi) = (f64::MAX, f64::MIN);
            for line in &values[row.saturating_sub(3)..(row + 4).min(HEIGHT)] {
                for &v in &line[col.saturating_sub(3)..(col + 4).min(WIDTH)] {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            if hi - lo > 0.5 * contrast {
                (lo + hi) / 2.0
            } else {
                thr
            }
        };
        result = decoder.decode_matrix(&to_matrix(local, &values));
    }
    let mut sym = result?;
    sym.location = Some(location(&h));
    Ok(sym)
}

/// Threshold the module values into the grid the structural decoder reads: data
/// modules as sampled, the fixed modules as the encoder draws them.
fn to_matrix(thr: impl Fn(usize, usize) -> f64, values: &[[f64; WIDTH]; HEIGHT]) -> BitMatrix {
    let mut m = BitMatrix::new(WIDTH, HEIGHT, 1);
    for (row, line) in values.iter().enumerate() {
        for (col, &v) in line.iter().enumerate() {
            if MAXI_GRID[row][col] != 0 && v < thr(row, col) {
                m.set(col, row, true);
            }
        }
    }
    for &(row, col) in &ORIENTATION {
        m.set(col, row, true);
    }
    m
}

/// Where the symbol is: the outline of its module area, the direction of its rows.
fn location(h: &Homography) -> Location {
    let (left, _) = module_center(0, 0);
    let (right, _) = module_center(0, WIDTH - 1);
    let (left, right) = (left - 0.5, right + 0.5);
    let reach = 0.5 / ROW_PITCH;
    let (_, top) = module_center(0, 0);
    let (_, bottom) = module_center(HEIGHT - 1, 0);
    let (top, bottom) = (top - reach, bottom + reach);
    let corner = |u: f64, v: f64| {
        let (x, y) = h.map_f64(u, v);
        Point::new(x as f32, y as f32)
    };
    let (x0, y0) = h.map_f64(0.0, 0.0);
    let (x1, y1) = h.map_f64(1.0, 0.0);
    let (x2, y2) = h.map_f64(0.5, ROW_PITCH);
    Location {
        outline: Quad::new([
            corner(left, top),
            corner(right, top),
            corner(right, bottom),
            corner(left, bottom),
        ]),
        rotation: Some((y1 - y0).atan2(x1 - x0) as f32),
        module_size: Some((((x1 - x0).hypot(y1 - y0) + (x2 - x0).hypot(y2 - y0)) / 2.0) as f32),
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::codes::maxicode::{MaxiCodeEncoder, render_hex};
    use crate::output::Encoding;
    use crate::traits::Encode;

    /// The six orientation triplets: eighteen fixed modules, eleven of them dark.
    #[test]
    fn orientation_cells_are_the_six_triplets() {
        let cells = orientation_cells();
        assert_eq!(cells.len(), 18);
        assert_eq!(cells.iter().filter(|c| c.2).count(), 11);
    }

    /// A render reads back as the very grid that was drawn.
    #[test]
    fn render_samples_back_to_the_same_matrix() {
        let enc = MaxiCodeEncoder::new();
        let sym = enc.build_text("MaxiCode sampler: identical grid").unwrap();
        let Encoding::Matrix(m) = enc.encode(&sym).unwrap() else {
            panic!("expected a matrix");
        };
        let img = render_hex(&m, 10.0);
        let read = scan(&img.as_frame()).expect("render must scan");
        assert_eq!(enc.encode(&read), Ok(Encoding::Matrix(m)));
        assert_eq!(read.payload_bytes(), sym.payload_bytes());
    }
}
