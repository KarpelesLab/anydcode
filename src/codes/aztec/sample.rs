//! Aztec image sampling: pixels → [`BitMatrix`] → [`Symbol`].
//!
//! Aztec is built to be found: every symbol centres on a **bullseye** of concentric
//! square rings (radius 0/2/4 modules for compact symbols, plus radius 6 for
//! full-range), so a row scan for a run of equal-width alternating dark/light modules
//! finds candidate centres cheaply — and the ring *count* along the cross-section
//! immediately says compact vs full-range. The sampler then:
//!
//! 1. binarizes the frame — global Otsu, a wide adaptive window, then a window only
//!    a few modules wide for small or blurred symbols ([`binarize`]);
//! 2. scans rows for the 9-run (compact) / 13-run (full) equal-width bullseye
//!    cross-section, cross-checked vertically, and clusters the hits. The binary
//!    image is used for **nothing else**: it only proposes centres;
//! 3. fits the bullseye on the **grey** image: rays from the candidate centre walk
//!    the ring profile for its extrema and take each ring boundary where the profile
//!    crosses the midpoint of its two neighbours — a position blur and uneven light
//!    do not move — giving a first quad ([`ring_quad`]); then every ring boundary is
//!    probed along its whole length and a homography is solved by least squares
//!    over those hundreds of sub-pixel edge points ([`refine_core`]);
//! 4. confirms the bullseye module by module (a texture stops here) and reads the
//!    ring count off the fitted map, not off the binary cross-section;
//! 5. picks the rotation by the **orientation marks**, then reads the **mode
//!    message** ring (RS over GF(16)) to learn the layer count and so the exact
//!    symbol size;
//! 6. extends the fit over the whole symbol from the edges between its own modules
//!    ([`grow`]) — the bullseye alone pins the map to a fraction of a pixel at
//!    radius 4, which is a module or more by the rim of a large or tilted symbol;
//! 7. samples each module as a small-area mean, against the bullseye's own
//!    dark/light midpoint and then a threshold that follows the light across the
//!    symbol, and hands the grid to [`AztecDecoder`], whose Reed–Solomon check
//!    validates the read. An 11×11 rune attempt covers Aztec Runes.
//!
//! Cost is bounded per frame: three binarizations (the third skipped on a frame the
//! row scan flags as a periodic texture), at most [`MAX_EYES`] candidates given
//! eight rays each, and at most [`MAX_FITS`] of those a full fit.

use super::decode::read_mode_message;
use super::layout::Layout;
use super::rune::{RUNE_SIZE, render_rune};
use super::{AztecDecoder, QUIET_ZONE};
use crate::error::{Error, Result};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::imgproc::binary::BinaryImage;
use crate::imgproc::cluster::CenterIndex;
use crate::imgproc::homography::Homography;
use crate::imgproc::sample::sample_bilinear;
use crate::imgproc::threshold::otsu_binarize;
use crate::output::BitMatrix;
use crate::symbol::Symbol;
use crate::symbology::Symbology;
use alloc::vec::Vec;

/// Bullseye candidates looked at per binarization, best-confirmed first. Looking is
/// cheap — eight rays tell a bullseye from a texture ([`ring_quad`]).
const MAX_EYES: usize = 32;
/// Candidates per binarization that may go on to the full grey-level fit.
const MAX_FITS: usize = 8;

/// A detected bullseye: centre, module pitch along the scan axes, ring count.
#[derive(Debug, Clone, Copy)]
struct Bullseye {
    cx: f32,
    cy: f32,
    module: f32,
    /// `true` when the cross-section showed the full-range ring at radius 6.
    full: bool,
    count: u32,
}

/// A bullseye confirmed and fitted on the grey image.
struct Core {
    /// Centre-relative module coordinates (module centres on integers, the bullseye
    /// centre at the origin) → image pixels.
    h: Homography,
    /// Full-range (`true`) or compact bullseye.
    full: bool,
    /// Mean luminance of the bullseye's dark and light modules.
    dark: f64,
    light: f64,
}

/// Locate, sample and structurally decode the Aztec symbol in `frame`.
pub fn scan(frame: &GrayFrame<'_>) -> Result<Symbol> {
    let decoder = AztecDecoder::new();
    let mut last = Error::undecodable("no Aztec bullseye found");
    // Centres (and reach) of bullseyes already fitted and tried: a later binarization
    // finding the same one again would only repeat the same grey-level work.
    let mut tried: Vec<(f64, f64, f64)> = Vec::new();
    // A frame where the row scan fires all over is a periodic texture, not a small
    // soft symbol: the extra fine-grained pass would only redo that work a third time.
    let mut textured = false;
    for rung in 0..RUNGS {
        if rung == FINE_RUNG && textured {
            break;
        }
        let bin = binarize(frame, rung);
        let (eyes, hits) = find_bullseyes(&bin);
        textured |= hits > frame.width() * frame.height() / 16;
        let mut fits = 0;
        for eye in eyes.iter().take(MAX_EYES) {
            if fits == MAX_FITS {
                break;
            }
            let (ex, ey) = (f64::from(eye.cx), f64::from(eye.cy));
            if tried
                .iter()
                .any(|&(x, y, reach)| (x - ex).abs() <= reach && (y - ey).abs() <= reach)
            {
                continue;
            }
            let Some(h0) = ring_quad(frame, eye) else {
                continue;
            };
            fits += 1;
            let Some(core) = fit_core(frame, &h0) else {
                continue;
            };
            let (cx, cy) = core.h.map_f64(0.0, 0.0);
            tried.push((cx, cy, 2.0 * module_px(&core.h)));
            match decode_core(frame, &core, &decoder) {
                Ok(sym) => return Ok(sym),
                Err(e) => last = e,
            }
        }
    }
    Err(last)
}

/// Binarizations tried, in order.
const RUNGS: usize = 3;
/// The rung of [`fine_binarize`].
const FINE_RUNG: usize = 2;

/// The `rung`-th binarization of the ladder: global Otsu, a wide adaptive window for
/// uneven light, then a module-sized window for small or blurred symbols.
fn binarize(frame: &GrayFrame<'_>, rung: usize) -> BinaryImage {
    match rung {
        0 => otsu_binarize(frame),
        1 => {
            let radius = (frame.width().min(frame.height()) / 8).clamp(8, 50);
            wide_binarize(frame, radius, 100)
        }
        _ => fine_binarize(frame),
    }
}

/// Bradley–Roth adaptive binarization: a pixel is dark when it is more than
/// `per_mille`/1000 below the mean of the `(2·radius+1)²` window around it (clipped at the borders).
/// The window sums slide — a column sum per x, a prefix sum per row — so the cost
/// per pixel is a handful of integer operations whatever the radius.
fn wide_binarize(frame: &GrayFrame<'_>, radius: usize, per_mille: u64) -> BinaryImage {
    let (w, h) = (frame.width(), frame.height());
    let mut bits = alloc::vec![false; w * h];
    let mut col = alloc::vec![0u32; w];
    let mut prefix = alloc::vec![0u64; w + 1];
    let keep = 1000 - per_mille.min(1000);
    let add = |col: &mut [u32], y: usize, sign: bool| {
        if let Some(row) = frame.row(y) {
            for (c, &v) in col.iter_mut().zip(row) {
                if sign {
                    *c += u32::from(v);
                } else {
                    *c -= u32::from(v);
                }
            }
        }
    };
    for y in 0..radius.min(h) {
        add(&mut col, y, true);
    }
    for y in 0..h {
        if y + radius < h {
            add(&mut col, y + radius, true);
        }
        if y > radius {
            add(&mut col, y - radius - 1, false);
        }
        let rows = ((y + radius + 1).min(h) - y.saturating_sub(radius)) as u64;
        for (x, &c) in col.iter().enumerate() {
            prefix[x + 1] = prefix[x] + u64::from(c);
        }
        let Some(row) = frame.row(y) else {
            break;
        };
        let out = &mut bits[y * w..(y + 1) * w];
        let dark = |x: usize, x0: usize, x1: usize| {
            let sum = prefix[x1] - prefix[x0];
            u64::from(row[x]) * rows * (x1 - x0) as u64 * 1000 < sum * keep
        };
        // Interior: the window is whole, its bounds two slices in step with the row.
        let inner = radius.min(w)..w.saturating_sub(radius).max(radius.min(w));
        for (x, o) in out.iter_mut().enumerate() {
            if !inner.contains(&x) {
                *o = dark(x, x.saturating_sub(radius), (x + radius + 1).min(w));
            }
        }
        if !inner.is_empty() {
            let full = rows * (2 * radius + 1) as u64 * 1000;
            let lo = &prefix[..inner.len()];
            let hi = &prefix[2 * radius + 1..2 * radius + 1 + inner.len()];
            let px = &row[inner.clone()];
            for (((o, &v), &a), &b) in out[inner].iter_mut().zip(px).zip(lo).zip(hi) {
                *o = u64::from(v) * full < (b - a) * keep;
            }
        }
    }
    BinaryImage::from_bits(w, h, bits).unwrap_or_else(|_| BinaryImage::new(w, h))
}

/// Half-width of the [`fine_binarize`] window (at most 4: its sums are `u32`).
const FINE_RADIUS: usize = 4;

/// Binarize against the mean of a window only a few modules wide.
///
/// A blurred bullseye of 2–3 pixel rings never returns to the page's white between
/// its dark rings, so any threshold tied to a wide neighbourhood merges them; the
/// mean of a window the size of the pattern itself sits halfway between the rings
/// whatever the blur. The price of so local a threshold is that a flat area becomes
/// a coin flip per pixel — so a pixel is only ever dark where the window has real
/// contrast, measured against the noise floor of the frame's quietest regions.
fn fine_binarize(frame: &GrayFrame<'_>) -> BinaryImage {
    let radius = FINE_RADIUS;
    let (w, h) = (frame.width(), frame.height());
    let side = 2 * radius + 1;
    let img = BinaryImage::new(w, h);
    if w < side || h < side {
        return img;
    }
    let n = (side * side) as u64;

    // Noise floor: the lower quartile of the window variance over a sparse lattice.
    let mut vars: Vec<u64> = Vec::new();
    for y in (0..=h - side).step_by(16) {
        for x in (0..=w - side).step_by(16) {
            let (mut s, mut q) = (0u64, 0u64);
            for yy in y..y + side {
                for xx in x..x + side {
                    let v = u64::from(frame.get_unchecked(xx, yy));
                    s += v;
                    q += v * v;
                }
            }
            // n²·variance, exactly.
            vars.push(n * q - s * s);
        }
    }
    vars.sort_unstable();
    let floor = (vars[vars.len() / 4] as f64).sqrt() / n as f64;
    let gate = (2.5 * floor).clamp(4.0, 24.0);
    let gate = (gate * gate * (n * n) as f64) as u64;

    // Sliding window: per-column sums over the `side` rows, then a running sum along
    // the row. Pixels closer than `radius` to the border stay light. With `side ≤ 9`
    // every quantity fits `u32` (n·Σv² ≤ 81²·255²).
    let gate = gate.min(u64::from(u32::MAX)) as u32;
    let n = n as u32;
    let mut bits = alloc::vec![false; w * h];
    let mut col_s = alloc::vec![0u32; w];
    let mut col_q = alloc::vec![0u32; w];
    let mut col_3 = alloc::vec![0u32; w];
    for y in 0..h {
        let Some(row) = frame.row(y) else {
            return img;
        };
        for ((cs, cq), &v) in col_s.iter_mut().zip(col_q.iter_mut()).zip(row) {
            let v = u32::from(v);
            *cs += v;
            *cq += v * v;
        }
        if y + 1 < side {
            continue;
        }
        let cy = y - radius;
        // The pixel itself is taken as its 3×3 binomial mean: against so local a
        // threshold, sensor noise would otherwise shred every ring into speckle.
        let (Some(above), Some(mid), Some(below)) =
            (frame.row(cy - 1), frame.row(cy), frame.row(cy + 1))
        else {
            return img;
        };
        for (((c, &a), &b), &d) in col_3.iter_mut().zip(above).zip(mid).zip(below) {
            *c = u32::from(a) + 2 * u32::from(b) + u32::from(d);
        }
        let out = &mut bits[cy * w + radius..cy * w + w - radius];
        let (mut s, mut q) = (0u32, 0u32);
        for x in 0..side - 1 {
            s += col_s[x];
            q += col_q[x];
        }
        for (i, o) in out.iter_mut().enumerate() {
            s += col_s[i + side - 1];
            q += col_q[i + side - 1];
            let v = col_3[i + radius - 1] + 2 * col_3[i + radius] + col_3[i + radius + 1];
            *o = v * n < s * 16 && n * q - s * s > gate;
            s -= col_s[i];
            q -= col_q[i];
        }
        let Some(top) = frame.row(y + 1 - side) else {
            return img;
        };
        for ((cs, cq), &v) in col_s.iter_mut().zip(col_q.iter_mut()).zip(top) {
            let v = u32::from(v);
            *cs -= v;
            *cq -= v * v;
        }
    }
    BinaryImage::from_bits(w, h, bits).unwrap_or(img)
}

/// Mean pixel pitch of one module under `h`, measured at the bullseye centre.
fn module_px(h: &Homography) -> f64 {
    let (x0, y0) = h.map_f64(0.0, 0.0);
    let (x1, y1) = h.map_f64(1.0, 0.0);
    let (x2, y2) = h.map_f64(0.0, 1.0);
    ((x1 - x0).hypot(y1 - y0) + (x2 - x0).hypot(y2 - y0)) / 2.0
}

/// `h` with the module frame turned by `k` quarter turns.
fn rotated(h: &Homography, k: usize) -> Homography {
    let m = h.matrix();
    // Columns of the module frame: (u, v) → (u, v), (−v, u), (−u, −v), (v, −u).
    let (a, b): ([f64; 2], [f64; 2]) = match k % 4 {
        0 => ([1.0, 0.0], [0.0, 1.0]),
        1 => ([0.0, 1.0], [-1.0, 0.0]),
        2 => ([-1.0, 0.0], [0.0, -1.0]),
        _ => ([0.0, -1.0], [1.0, 0.0]),
    };
    let mut out = m;
    for row in 0..3 {
        let (c0, c1) = (m[row * 3], m[row * 3 + 1]);
        out[row * 3] = c0 * a[0] + c1 * a[1];
        out[row * 3 + 1] = c0 * b[0] + c1 * b[1];
    }
    Homography::from_matrix(out)
}

/// Luminance of the module at centre-relative `(u, v)`: the mean over its middle,
/// which rides out sensor noise far better than the single centre pixel.
fn module_value(frame: &GrayFrame<'_>, h: &Homography, u: f64, v: f64) -> f64 {
    const TAPS: [(f64, f64); 5] = [
        (0.0, 0.0),
        (-0.22, -0.22),
        (0.22, -0.22),
        (0.22, 0.22),
        (-0.22, 0.22),
    ];
    let mut acc = 0.0;
    for (du, dv) in TAPS {
        let (x, y) = h.map_f64(u + du, v + dv);
        acc += sample_bilinear(frame, x, y);
    }
    acc / TAPS.len() as f64
}

/// Whether `(x, y)` lies inside the frame.
fn inside(frame: &GrayFrame<'_>, x: f64, y: f64) -> bool {
    x >= 0.0 && y >= 0.0 && x <= (frame.width() - 1) as f64 && y <= (frame.height() - 1) as f64
}

// ---------------------------------------------------------------------------
// Grey-level bullseye fit
// ---------------------------------------------------------------------------

/// Rays cast from a candidate centre (a multiple of four).
const RAYS: usize = 48;

/// Fit the bullseye at `eye` on the grey image and confirm it is one.
fn fit_core(frame: &GrayFrame<'_>, h0: &Homography) -> Option<Core> {
    let mut h = refine_core(frame, h0, 3)?;
    h = refine_core(frame, &h, 3)?;
    let (mut dark, mut light, bad) = bullseye_levels(frame, &h, 0, 4)?;
    if bad > MAX_CORE_MISMATCH {
        return None;
    }
    // A full-range bullseye carries two more rings: light at radius 5, dark at 6. On a
    // compact symbol those are the mode ring and the first data layer — coin flips.
    let full = bullseye_levels_with(frame, &h, 5, 6, (dark + light) / 2.0)
        .is_some_and(|bad| bad <= MAX_FULL_MISMATCH);
    if full {
        // Fit and confirm again over all seven rings.
        if let Some(wider) = refine_core(frame, &h, 5) {
            h = wider;
        }
        let bad;
        (dark, light, bad) = bullseye_levels(frame, &h, 0, 6)?;
        if bad > 2 * MAX_CORE_MISMATCH {
            return None;
        }
    }
    Some(Core {
        h,
        full,
        dark,
        light,
    })
}

/// Bullseye modules (of the 81 within radius 4) allowed on the wrong side of the
/// dark/light midpoint.
const MAX_CORE_MISMATCH: usize = 4;
/// As [`MAX_CORE_MISMATCH`] for the 88 modules of rings 5 and 6 of a full bullseye.
const MAX_FULL_MISMATCH: usize = 12;
/// Least dark/light separation of a believable bullseye, in grey levels.
const MIN_CONTRAST: f64 = 14.0;

/// Sample the bullseye rings `lo..=hi` through `h`: mean dark level, mean light level
/// and the number of modules on the wrong side of their midpoint.
fn bullseye_levels(
    frame: &GrayFrame<'_>,
    h: &Homography,
    lo: i32,
    hi: i32,
) -> Option<(f64, f64, usize)> {
    let mut vals: Vec<(bool, f64)> = Vec::with_capacity(((2 * hi + 1) * (2 * hi + 1)) as usize);
    let (mut ds, mut dn, mut ls, mut ln) = (0.0, 0.0, 0.0, 0.0);
    for v in -hi..=hi {
        for u in -hi..=hi {
            let ring = u.abs().max(v.abs());
            if ring < lo {
                continue;
            }
            let (x, y) = h.map_f64(f64::from(u), f64::from(v));
            if !inside(frame, x, y) {
                return None;
            }
            let val = module_value(frame, h, f64::from(u), f64::from(v));
            let dark = ring % 2 == 0;
            if dark {
                ds += val;
                dn += 1.0;
            } else {
                ls += val;
                ln += 1.0;
            }
            vals.push((dark, val));
        }
    }
    if dn == 0.0 || ln == 0.0 {
        return None;
    }
    let (dark, light) = (ds / dn, ls / ln);
    if light - dark < MIN_CONTRAST {
        return None;
    }
    let thr = (dark + light) / 2.0;
    let bad = vals.iter().filter(|&&(d, v)| (v <= thr) != d).count();
    Some((dark, light, bad))
}

/// Modules of rings `lo..=hi` on the wrong side of `thr`.
fn bullseye_levels_with(
    frame: &GrayFrame<'_>,
    h: &Homography,
    lo: i32,
    hi: i32,
    thr: f64,
) -> Option<usize> {
    let mut bad = 0;
    for v in -hi..=hi {
        for u in -hi..=hi {
            let ring = u.abs().max(v.abs());
            if ring < lo {
                continue;
            }
            let (x, y) = h.map_f64(f64::from(u), f64::from(v));
            if !inside(frame, x, y) {
                return None;
            }
            let val = module_value(frame, h, f64::from(u), f64::from(v));
            if (val <= thr) != (ring % 2 == 0) {
                bad += 1;
            }
        }
    }
    Some(bad)
}

/// First four ring boundaries (Chebyshev radius 0.5, 1.5, 2.5, 3.5 modules) along the
/// ray from `(ox, oy)` towards `(dx, dy)`, as distances in pixels.
///
/// The grey profile is walked for its alternating extrema — the centre dot, light
/// ring, dark ring, light ring, dark ring — and each boundary is where the profile
/// crosses the midpoint of the two extrema it separates. Blur attenuates neighbouring
/// rings alike, so that crossing stays on the true edge where a fixed threshold has
/// long since merged the rings.
fn ray_edges(
    frame: &GrayFrame<'_>,
    (ox, oy): (f64, f64),
    (dx, dy): (f64, f64),
    step: f64,
    n: usize,
) -> Option<[f64; 4]> {
    let side = step * 0.6;
    let mut raw: Vec<f64> = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 * step;
        let (x, y) = (ox + dx * t, oy + dy * t);
        if !inside(frame, x, y) {
            break;
        }
        raw.push(
            (sample_bilinear(frame, x, y)
                + sample_bilinear(frame, x - dy * side, y + dx * side)
                + sample_bilinear(frame, x + dy * side, y - dx * side))
                / 3.0,
        );
    }
    let len = raw.len();
    if len < 8 {
        return None;
    }
    let v: Vec<f64> = (0..len)
        .map(|i| (raw[i.saturating_sub(1)] + 2.0 * raw[i] + raw[(i + 1).min(len - 1)]) / 4.0)
        .collect();
    let (lo, hi) = v
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    let delta = ((hi - lo) * 0.2).max(3.0);

    // Extrema by hysteresis: dot (min), ring 1 (max), ring 2 (min), ring 3 (max).
    let mut ext = [(0usize, 0.0f64); 5];
    let mut found = 0;
    let mut look_max = false;
    let (mut mn, mut mx) = ((0usize, v[0]), (0usize, v[0]));
    for (i, &x) in v.iter().enumerate() {
        if x > mx.1 {
            mx = (i, x);
        }
        if x < mn.1 {
            mn = (i, x);
        }
        if look_max {
            if x < mx.1 - delta {
                ext[found] = mx;
                found += 1;
                mn = (i, x);
                look_max = false;
            }
        } else if x > mn.1 + delta {
            ext[found] = mn;
            found += 1;
            mx = (i, x);
            look_max = true;
        }
        if found == 4 {
            break;
        }
    }
    if found < 4 {
        return None;
    }
    // Ring 4's far side borders the mode ring, which may be dark too, so its extremum
    // cannot wait for a rise: take the minimum over where the ring must lie.
    let ring = (ext[3].0 - ext[1].0) as f64 / 2.0;
    let end = ((ext[3].0 as f64 + 1.5 * ring).ceil() as usize).min(len - 1);
    if end <= ext[3].0 {
        return None;
    }
    let mut last = (ext[3].0 + 1, v[ext[3].0 + 1]);
    for (i, &x) in v.iter().enumerate().take(end + 1).skip(ext[3].0 + 1) {
        if x < last.1 {
            last = (i, x);
        }
    }
    if last.1 > ext[3].1 - delta {
        return None;
    }
    ext[4] = last;

    let mut out = [0.0f64; 4];
    for k in 0..4 {
        let (a, b) = (ext[k], ext[k + 1]);
        let level = (a.1 + b.1) / 2.0;
        let mut at = None;
        for i in a.0..b.0 {
            let (p, q) = (v[i] - level, v[i + 1] - level);
            if p == 0.0 {
                at = Some(i as f64);
                break;
            }
            if p * q < 0.0 {
                at = Some(i as f64 + p / (p - q));
                break;
            }
        }
        out[k] = at? * step;
    }
    // Rings are evenly spaced along any ray; boundaries that are not are something
    // else's edges.
    let gaps = [out[1] - out[0], out[2] - out[1], out[3] - out[2]];
    let (lo, hi) = gaps
        .iter()
        .fold((f64::MAX, 0.0f64), |(lo, hi), &g| (lo.min(g), hi.max(g)));
    if hi > 1.8 * lo {
        return None;
    }
    Some(out)
}

/// A line `n·p = d` with unit normal `n`.
#[derive(Clone, Copy)]
struct FitLine {
    nx: f64,
    ny: f64,
    d: f64,
}

impl FitLine {
    /// Total-least-squares line through `pts`.
    fn fit(pts: &[(f64, f64)]) -> Option<FitLine> {
        if pts.len() < 2 {
            return None;
        }
        let n = pts.len() as f64;
        let (mx, my) = pts
            .iter()
            .fold((0.0, 0.0), |(ax, ay), &(x, y)| (ax + x, ay + y));
        let (mx, my) = (mx / n, my / n);
        let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
        for &(x, y) in pts {
            sxx += (x - mx) * (x - mx);
            sxy += (x - mx) * (y - my);
            syy += (y - my) * (y - my);
        }
        let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        let (nx, ny) = (-theta.sin(), theta.cos());
        Some(FitLine {
            nx,
            ny,
            d: nx * mx + ny * my,
        })
    }

    /// Line through the best-agreeing 70% of `pts` (a ray that miscounted a ring, or
    /// clipped a blur-rounded corner, must not drag the side), with the mean squared
    /// residual of those points.
    fn fit_trimmed(pts: &[(f64, f64)]) -> Option<(FitLine, f64)> {
        if pts.len() < 4 {
            return None;
        }
        let first = FitLine::fit(pts)?;
        let mut by_dist: Vec<(f64, (f64, f64))> =
            pts.iter().map(|&p| (first.dist(p).abs(), p)).collect();
        by_dist.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
        let keep: Vec<(f64, f64)> = by_dist
            .iter()
            .take((pts.len() * 7).div_ceil(10).max(3))
            .map(|&(_, p)| p)
            .collect();
        let line = FitLine::fit(&keep)?;
        let cost = keep.iter().map(|&p| line.dist(p).powi(2)).sum::<f64>() / keep.len() as f64;
        Some((line, cost))
    }

    fn dist(&self, (x, y): (f64, f64)) -> f64 {
        self.nx * x + self.ny * y - self.d
    }

    fn intersect(&self, o: &FitLine) -> Option<(f64, f64)> {
        let det = self.nx * o.ny - self.ny * o.nx;
        if det.abs() < 1e-6 {
            return None;
        }
        Some((
            (self.d * o.ny - self.ny * o.d) / det,
            (self.nx * o.d - self.d * o.nx) / det,
        ))
    }
}

/// First estimate of the core homography: rays from the candidate centre find the
/// outer boundary of the third ring (the square of half-side 3.5 modules that both
/// bullseye families share), and four lines fitted to those points give its corners.
fn ring_quad(frame: &GrayFrame<'_>, eye: &Bullseye) -> Option<Homography> {
    let m = f64::from(eye.module).max(1.0);
    let step = (m / 5.0).clamp(0.5, 2.0);
    // The row scan measured the module along the image axes — up to √2 of its true
    // width — and the far corner of ring 4 is 4.5·√2 modules out.
    let n = (6.6 * m / step).ceil() as usize + 2;
    let origin = (f64::from(eye.cx) + 0.5, f64::from(eye.cy) + 0.5);
    let mut pts: [Option<(f64, f64)>; RAYS] = [None; RAYS];
    let mut hits = 0;
    let mut cast = |i: usize, hits: &mut usize| {
        let a = i as f64 / RAYS as f64 * core::f64::consts::TAU;
        let dir = (a.cos(), a.sin());
        if let Some(e) = ray_edges(frame, origin, dir, step, n) {
            pts[i] = Some((origin.0 + dir.0 * e[3], origin.1 + dir.1 * e[3]));
            *hits += 1;
        }
    };
    // Eight rays round the compass first: a texture that merely fooled the row scan
    // shows four evenly spaced rings to few of them.
    let probe = RAYS / 8;
    for i in (0..RAYS).step_by(probe) {
        cast(i, &mut hits);
    }
    if hits < 6 {
        return None;
    }
    for i in (0..RAYS).filter(|i| i % probe != 0) {
        cast(i, &mut hits);
    }
    if hits < RAYS * 5 / 8 {
        return None;
    }

    // Which rays point at corners? Try every quarter-turn-periodic split; the right
    // one puts each side's points on a line.
    let q = RAYS / 4;
    let side_pts = |o: usize, j: usize| -> Vec<(f64, f64)> {
        (2..=q - 2)
            .filter_map(|i| pts[(o + j * q + i) % RAYS])
            .collect()
    };
    let mut best: Option<(f64, [FitLine; 4])> = None;
    for o in 0..q {
        let mut cost = 0.0;
        let mut lines = [FitLine {
            nx: 0.0,
            ny: 0.0,
            d: 0.0,
        }; 4];
        let mut ok = true;
        for (j, l) in lines.iter_mut().enumerate() {
            let Some((line, c)) = FitLine::fit_trimmed(&side_pts(o, j)) else {
                ok = false;
                break;
            };
            *l = line;
            cost += c;
        }
        if ok && best.as_ref().is_none_or(|(c, _)| cost < *c) {
            best = Some((cost, lines));
        }
    }
    let (_, lines) = best?;
    // Corner j sits between side j−1 and side j, in ray (clockwise) order.
    let mut dst = [Point::new(0.0, 0.0); 4];
    for (j, d) in dst.iter_mut().enumerate() {
        let (x, y) = lines[(j + 3) % 4].intersect(&lines[j])?;
        if !inside(frame, x, y) {
            return None;
        }
        *d = Point::new(x as f32, y as f32);
    }
    let src = [
        Point::new(-3.5, -3.5),
        Point::new(3.5, -3.5),
        Point::new(3.5, 3.5),
        Point::new(-3.5, 3.5),
    ];
    let h = Homography::from_correspondences(src, dst).ok()?;
    let px = module_px(&h);
    (px.is_finite() && px >= 1.0 && px <= 1.6 * m && px >= 0.5 * m).then_some(h)
}

/// Solve the `N×N` system `a·x = b` by Gaussian elimination with partial pivoting.
fn solve<const N: usize>(mut a: [[f64; N]; N], mut b: [f64; N]) -> Option<[f64; N]> {
    for col in 0..N {
        let pivot = (col..N).max_by(|&i, &j| {
            a[i][col]
                .abs()
                .partial_cmp(&a[j][col].abs())
                .unwrap_or(core::cmp::Ordering::Equal)
        })?;
        if a[pivot][col].abs().partial_cmp(&1e-12) != Some(core::cmp::Ordering::Greater) {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..N {
            let f = a[row][col] / a[col][col];
            if f != 0.0 {
                let pivot_row = a[col];
                for (v, p) in a[row].iter_mut().zip(pivot_row).skip(col) {
                    *v -= f * p;
                }
                b[row] -= f * b[col];
            }
        }
    }
    let mut x = [0.0f64; N];
    for row in (0..N).rev() {
        let mut acc = b[row];
        for k in row + 1..N {
            acc -= a[row][k] * x[k];
        }
        x[row] = acc / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Least-squares fit of the image→module map from points known to lie on a module
/// coordinate line (`u = c` or `v = c`). With the inverse homography as the unknown,
/// every such point is one *linear* equation, so the whole bullseye — hundreds of
/// edge points — pins the eight coefficients at once.
struct EdgeFit {
    ata: [[f64; 8]; 8],
    atb: [f64; 8],
    /// Points on `u = c` lines, points on `v = c` lines.
    count: [usize; 2],
    /// Normalisation of image coordinates: centre and scale.
    cx: f64,
    cy: f64,
    scale: f64,
}

impl EdgeFit {
    fn new(cx: f64, cy: f64, scale: f64) -> Self {
        EdgeFit {
            ata: [[0.0; 8]; 8],
            atb: [0.0; 8],
            count: [0; 2],
            cx,
            cy,
            scale,
        }
    }

    /// Image point `(x, y)` lies on module line `u = c` (`axis` 0) or `v = c` (1).
    fn add(&mut self, axis: usize, c: f64, x: f64, y: f64) {
        let (x, y) = ((x - self.cx) / self.scale, (y - self.cy) / self.scale);
        let mut row = [0.0f64; 8];
        row[axis * 3] = x;
        row[axis * 3 + 1] = y;
        row[axis * 3 + 2] = 1.0;
        row[6] = -c * x;
        row[7] = -c * y;
        for i in 0..8 {
            if row[i] == 0.0 {
                continue;
            }
            for j in 0..8 {
                self.ata[i][j] += row[i] * row[j];
            }
            self.atb[i] += row[i] * c;
        }
        self.count[axis] += 1;
    }

    /// The fitted module→image homography.
    fn solve(&self) -> Option<Homography> {
        if self.count[0] < 8 || self.count[1] < 8 {
            return None;
        }
        let g = solve(self.ata, self.atb)?;
        // Undo the normalisation: G·N with N = [1/s, 0, −cx/s; 0, 1/s, −cy/s; 0, 0, 1].
        let s = self.scale;
        let mut m = [0.0f64; 9];
        for r in 0..3 {
            let (a, b, c) = if r < 2 {
                (g[r * 3], g[r * 3 + 1], g[r * 3 + 2])
            } else {
                (g[6], g[7], 1.0)
            };
            m[r * 3] = a / s;
            m[r * 3 + 1] = b / s;
            m[r * 3 + 2] = c - (a * self.cx + b * self.cy) / s;
        }
        Homography::from_matrix(m).inverse().ok()
    }
}

/// Re-fit the core homography from the ring boundaries at radius `0.5..=rings+0.5`:
/// every boundary is probed across at half-module intervals along each of its sides
/// for the sub-pixel edge, and all of them together solve for the map.
fn refine_core(frame: &GrayFrame<'_>, h: &Homography, rings: usize) -> Option<Homography> {
    let px = module_px(h);
    let (cx, cy) = h.map_f64(0.0, 0.0);
    let mut fit = EdgeFit::new(cx, cy, px * (rings as f64 + 0.5));
    for j in 0..=rings {
        let r = j as f64 + 0.5;
        let inner_dark = j % 2 == 0;
        let tmax = (r - 0.5).max(0.2);
        let nt = (4.0 * tmax).ceil() as usize + 1;
        for s in 0..4 {
            let at = |rho: f64, t: f64| -> (f64, f64) {
                let (u, v) = match s {
                    0 => (rho, t),
                    1 => (t, rho),
                    2 => (-rho, t),
                    _ => (t, -rho),
                };
                h.map_f64(u, v)
            };
            for it in 0..nt {
                let t = -tmax + 2.0 * tmax * it as f64 / (nt - 1) as f64;
                if let Some(rho) = edge_across(frame, &at, r, t, inner_dark) {
                    let (x, y) = at(rho, t);
                    let c = if s < 2 { r } else { -r };
                    fit.add(s % 2, c, x, y);
                }
            }
        }
    }
    let out = fit.solve()?;
    let (nx, ny) = out.map_f64(0.0, 0.0);
    let npx = module_px(&out);
    let sane = npx.is_finite()
        && (nx - cx).hypot(ny - cy) <= 1.5 * px
        && npx >= 0.7 * px
        && npx <= 1.4 * px;
    sane.then_some(out)
}

/// Sub-module position of the dark/light edge expected at `r` along the probe
/// `at(rho, t)`, searched half a module either side.
fn edge_across(
    frame: &GrayFrame<'_>,
    at: &dyn Fn(f64, f64) -> (f64, f64),
    r: f64,
    t: f64,
    inner_dark: bool,
) -> Option<f64> {
    const N: usize = 8;
    let mut v = [0.0f64; N + 1];
    for (i, slot) in v.iter_mut().enumerate() {
        let rho = r - 0.5 + i as f64 / N as f64;
        let mut acc = 0.0;
        for dt in [-0.15, 0.0, 0.15] {
            let (x, y) = at(rho, t + dt);
            if !inside(frame, x, y) {
                return None;
            }
            acc += sample_bilinear(frame, x, y);
        }
        *slot = acc / 3.0;
    }
    let diff = v[N] - v[0];
    if diff.abs() < 6.0 || (diff > 0.0) != inner_dark {
        return None;
    }
    let level = (v[0] + v[N]) / 2.0;
    let mut best: Option<f64> = None;
    for i in 0..N {
        let (p, q) = (v[i] - level, v[i + 1] - level);
        if p * q <= 0.0 && p != q {
            let pos = i as f64 + p / (p - q);
            if best.is_none_or(|b| (pos - 4.0).abs() < (b - 4.0).abs()) {
                best = Some(pos);
            }
        }
    }
    Some(r - 0.5 + best? / N as f64)
}

/// Re-fit the map from every dark/light module boundary within Chebyshev radius
/// `reach` of the centre — bullseye rings, mode ring and data alike. Wherever two
/// neighbouring modules read differently there is an edge on a known grid line, and
/// its sub-pixel position is one more equation for [`EdgeFit`]. This is what carries
/// the fit beyond the bullseye: the core alone pins the map to a fraction of a pixel
/// at radius 4, which is a module or more by the rim of a large or tilted symbol.
fn refine_grid(
    frame: &GrayFrame<'_>,
    h: &Homography,
    reach: i32,
    thr: f64,
    contrast: f64,
) -> Option<Homography> {
    let px = module_px(h);
    let (cx, cy) = h.map_f64(0.0, 0.0);
    let mut fit = EdgeFit::new(cx, cy, px * f64::from(reach));
    let side = (2 * reach + 1) as usize;
    // Large grids are probed on every `stride`-th row and column: thousands of edges
    // either way, at a bounded cost.
    let stride = 1 + side / 40;
    let mut line: Vec<f64> = alloc::vec![0.0; side];
    for axis in 0..2 {
        let at = |rho: f64, t: f64| -> (f64, f64) {
            if axis == 0 {
                h.map_f64(rho, t)
            } else {
                h.map_f64(t, rho)
            }
        };
        for row in (0..side).step_by(stride) {
            let t = f64::from(row as i32 - reach);
            for (i, slot) in line.iter_mut().enumerate() {
                let (x, y) = at(f64::from(i as i32 - reach), t);
                *slot = if inside(frame, x, y) {
                    sample_bilinear(frame, x, y)
                } else {
                    f64::NAN
                };
            }
            for i in 0..side - 1 {
                let (a, b) = (line[i], line[i + 1]);
                // NaN (outside the frame) is no edge.
                let step = (a - b).abs();
                if step.is_nan() || step < 0.4 * contrast || (a <= thr) == (b <= thr) {
                    continue;
                }
                let r = f64::from(i as i32 - reach) + 0.5;
                if let Some(rho) = edge_across(frame, &at, r, t, a < b) {
                    let (x, y) = at(rho, t);
                    fit.add(axis, r, x, y);
                }
            }
        }
    }
    let out = fit.solve()?;
    let (nx, ny) = out.map_f64(0.0, 0.0);
    let npx = module_px(&out);
    let sane = npx.is_finite()
        && (nx - cx).hypot(ny - cy) <= 1.5 * px
        && npx >= 0.7 * px
        && npx <= 1.4 * px;
    sane.then_some(out)
}

/// Carry a map fitted out to radius `from` to the rim of a `size`-module symbol,
/// re-fitting on a reach that grows by half each round so the extrapolation never
/// outruns what the previous fit can vouch for.
fn grow(
    frame: &GrayFrame<'_>,
    h: &Homography,
    from: i32,
    size: usize,
    thr: f64,
    contrast: f64,
) -> Homography {
    let half = (size / 2) as i32;
    let mut h = *h;
    let mut reach = from;
    while reach < half {
        reach = (reach + reach / 2).min(half);
        match refine_grid(frame, &h, reach, thr, contrast) {
            Some(next) => h = next,
            None => break,
        }
    }
    h
}

// ---------------------------------------------------------------------------
// Reading the symbol off a fitted core
// ---------------------------------------------------------------------------

/// Orientation modules (of 12) that must read as one rotation's marks for that
/// rotation to be tried.
const MIN_ORIENT: usize = 9;

/// How many of the twelve orientation modules (the corners of the mode ring and their
/// neighbours) read as the marks of an upright symbol under `h`.
fn orientation_score(frame: &GrayFrame<'_>, h: &Homography, full: bool, thr: f64) -> usize {
    let r = if full { 7.0 } else { 5.0 };
    let q = r - 1.0;
    // (u, v, dark): three dark at top-left, two at top-right, one at bottom-right.
    let marks = [
        (-r, -r, true),
        (-q, -r, true),
        (-r, -q, true),
        (r, -r, true),
        (r, -q, true),
        (q, -r, false),
        (r, q, true),
        (r, r, false),
        (q, r, false),
        (-r, r, false),
        (-q, r, false),
        (-r, q, false),
    ];
    marks
        .iter()
        .filter(|&&(u, v, dark)| (module_value(frame, h, u, v) <= thr) == dark)
        .count()
}

/// Read the symbol whose bullseye is `core`: orientation, mode message, data grid.
fn decode_core(frame: &GrayFrame<'_>, core: &Core, decoder: &AztecDecoder) -> Result<Symbol> {
    let thr = (core.dark + core.light) / 2.0;
    let contrast = core.light - core.dark;
    // Take in the mode ring and orientation marks before reading them.
    let reach = if core.full { 7 } else { 5 };
    let base = refine_grid(frame, &core.h, reach, thr, contrast).unwrap_or(core.h);
    let mut rots: Vec<(usize, Homography)> = (0..4)
        .map(|k| {
            let h = rotated(&base, k);
            (orientation_score(frame, &h, core.full, thr), h)
        })
        .collect();
    rots.sort_by_key(|r| core::cmp::Reverse(r.0));
    let mut last = Error::undecodable("Aztec bullseye did not decode");
    for (score, h) in &rots {
        if *score < MIN_ORIENT {
            break;
        }
        match decode_via_mode(frame, h, core.full, thr, contrast, decoder) {
            Ok(sym) => return Ok(sym),
            Err(e) => last = e,
        }
        // Aztec Runes are 11×11 compact shells whose mode ring carries a data
        // byte instead of a layer count, so the mode read above rejects them.
        // Only attempt the rune grid when nothing surrounds the core — the mode
        // RS can mis-"correct" a genuine data symbol's ring into a rune value,
        // and a real rune is alone inside its quiet zone.
        if !core.full
            && quiet_beyond_core(frame, h, thr)
            && let Ok(sym) = try_rune(frame, h, thr, decoder)
        {
            return Ok(sym);
        }
    }
    Err(last)
}

/// Dark fraction just outside an 11×11 core, in the core frame. A rune's quiet zone
/// is light there; a data-bearing symbol is ~half dark.
fn quiet_beyond_core(frame: &GrayFrame<'_>, core: &Homography, thr: f64) -> bool {
    let mut dark = 0usize;
    let mut total = 0usize;
    for r in [6.5f64, 7.5] {
        for i in 0..24 {
            let a = i as f64 / 24.0 * core::f64::consts::TAU;
            let (px, py) = core.map_f64(r * a.cos(), r * a.sin());
            total += 1;
            if sample_bilinear(frame, px, py) <= thr {
                dark += 1;
            }
        }
    }
    (dark as f32) < total as f32 * 0.2
}

/// Read the mode-message ring through the core homography, derive the symbol size,
/// sample the full grid and decode it.
fn decode_via_mode(
    frame: &GrayFrame<'_>,
    core: &Homography,
    full: bool,
    thr: f64,
    contrast: f64,
    decoder: &AztecDecoder,
) -> Result<Symbol> {
    // Mode positions in centre-relative module coordinates: use any layout of the
    // right family — the ring only depends on compact/full and the centre.
    let probe = Layout::new(!full, 1);
    let c = probe.center as f64;
    let bits: Vec<bool> = probe
        .mode_positions()
        .iter()
        .map(|&(x, y)| module_value(frame, core, x as f64 - c, y as f64 - c) <= thr)
        .collect();
    let (layers, _) = read_mode_message(&bits, !full)?;
    let layout = Layout::new(!full, layers);
    // The whole symbol's edges first; then the core's own map, which stays the truer
    // one near the centre when the surface is not flat.
    let whole = grow(
        frame,
        core,
        if full { 7 } else { 5 },
        layout.size,
        thr,
        contrast,
    );
    let mut last = Error::undecodable("Aztec grid did not decode");
    for h in [&whole, core] {
        let values = module_values(frame, h, layout.size);
        // The bullseye's own threshold first; then one that follows the light across
        // the symbol, for a large code lit unevenly.
        for local in [false, true] {
            let matrix = if local {
                threshold_local(&values, layout.size, thr, contrast)
            } else {
                threshold_flat(&values, layout.size, thr)
            };
            match decoder.decode_matrix(&matrix) {
                Ok(mut sym) => {
                    sym.location = Some(location_from(h, layout.size));
                    return Ok(sym);
                }
                Err(e) => last = e,
            }
        }
        if whole == *core {
            break;
        }
    }
    Err(last)
}

/// Luminance of every module of a `size`×`size` grid (odd `size`, centred on the
/// bullseye) through the centre-relative map, row-major.
fn module_values(frame: &GrayFrame<'_>, core: &Homography, size: usize) -> Vec<f64> {
    let c = (size / 2) as f64;
    let mut out = Vec::with_capacity(size * size);
    for y in 0..size {
        for x in 0..size {
            out.push(module_value(frame, core, x as f64 - c, y as f64 - c));
        }
    }
    out
}

/// Modules at or below `thr` are dark.
fn threshold_flat(values: &[f64], size: usize, thr: f64) -> BitMatrix {
    let mut matrix = BitMatrix::new(size, size, QUIET_ZONE);
    for (i, &v) in values.iter().enumerate() {
        matrix.set(i % size, i / size, v <= thr);
    }
    matrix
}

/// Each module against the midpoint of the dark and light levels of its own
/// neighbourhood (an 11×11-module window split in two about its mean). Where the
/// window shows no real contrast — nothing to split — the bullseye threshold stands.
fn threshold_local(values: &[f64], size: usize, thr: f64, contrast: f64) -> BitMatrix {
    const REACH: usize = 5;
    let mut matrix = BitMatrix::new(size, size, QUIET_ZONE);
    for y in 0..size {
        let (y0, y1) = (y.saturating_sub(REACH), (y + REACH + 1).min(size));
        for x in 0..size {
            let (x0, x1) = (x.saturating_sub(REACH), (x + REACH + 1).min(size));
            let window =
                || (y0..y1).flat_map(move |yy| values[yy * size + x0..yy * size + x1].iter());
            let n = ((y1 - y0) * (x1 - x0)) as f64;
            let mean = window().sum::<f64>() / n;
            let (mut lo, mut lo_n, mut hi, mut hi_n) = (0.0, 0.0, 0.0, 0.0);
            for &v in window() {
                if v <= mean {
                    lo += v;
                    lo_n += 1.0;
                } else {
                    hi += v;
                    hi_n += 1.0;
                }
            }
            let local = if lo_n > 0.0 && hi_n > 0.0 && hi / hi_n - lo / lo_n >= 0.15 * contrast {
                (lo / lo_n + hi / hi_n) / 2.0
            } else {
                thr
            };
            matrix.set(x, y, values[y * size + x] <= local);
        }
    }
    matrix
}

/// Modules of the 11×11 rune grid that may disagree with the rune it decodes to.
const MAX_RUNE_MISMATCH: usize = 6;

/// Sample and decode the 11×11 Aztec Rune grid.
///
/// A rune's only protection is its ring's RS(7,2) over GF(16), and correcting two of
/// seven words makes that accept one random ring in ~200 — with four rotations, several
/// eyes and several binarizations per frame, a texture that merely passes the bullseye
/// check would "decode" as a rune within seconds of video. But a rune has no
/// free modules at all: its value fixes every one of the 121. So the read is only
/// believed when the whole sampled grid — all the rings, the orientation marks, the
/// mode ring — is the rune it claims to be, give or take a few modules of damage.
fn try_rune(
    frame: &GrayFrame<'_>,
    core: &Homography,
    thr: f64,
    decoder: &AztecDecoder,
) -> Result<Symbol> {
    let matrix = threshold_flat(&module_values(frame, core, RUNE_SIZE), RUNE_SIZE, thr);
    let mut sym = decoder.decode_matrix(&matrix)?;
    let value = match sym.payload_bytes().as_slice() {
        [value] if sym.symbology == Symbology::AztecRunes => *value,
        _ => return Err(Error::undecodable("11×11 Aztec grid is not a rune")),
    };
    let ideal = render_rune(value);
    let mismatches = (0..RUNE_SIZE * RUNE_SIZE)
        .filter(|i| {
            let (x, y) = (i % RUNE_SIZE, i / RUNE_SIZE);
            matrix.get(x, y) != ideal.get(x, y)
        })
        .count();
    if mismatches > MAX_RUNE_MISMATCH {
        return Err(Error::undecodable(
            "Aztec Rune grid does not match its value",
        ));
    }
    sym.location = Some(location_from(core, RUNE_SIZE));
    Ok(sym)
}

/// The reported [`Location`]: the symbol's outline through the fitted map.
fn location_from(core: &Homography, size: usize) -> Location {
    let r = size as f64 / 2.0;
    let quad = [
        core.map_f64(-r, -r),
        core.map_f64(r, -r),
        core.map_f64(r, r),
        core.map_f64(-r, r),
    ];
    Location {
        outline: Quad::new(quad.map(|(x, y)| Point::new(x as f32, y as f32))),
        rotation: None,
        module_size: Some(module_px(core) as f32),
    }
}

/// Scan rows for the equal-run bullseye cross-section and cluster confirmed centres.
fn find_bullseyes(bin: &BinaryImage) -> (Vec<Bullseye>, usize) {
    let (w, h) = (bin.width(), bin.height());
    let mut eyes: Vec<Bullseye> = Vec::new();
    let mut index = CenterIndex::new(w, h);
    let mut hits = 0usize;

    if w == 0 {
        return (eyes, 0);
    }
    let mut runs: Vec<(bool, usize, usize)> = Vec::new();
    for (y, row) in bin.bits().chunks_exact(w).enumerate() {
        // Run-length encode the row.
        runs.clear();
        let mut cur = row[0];
        let mut start = 0usize;
        for (x, &d) in row.iter().enumerate().skip(1) {
            if d != cur {
                runs.push((cur, start, x - start));
                cur = d;
                start = x;
            }
        }
        runs.push((cur, start, w - start));

        for full in [true, false] {
            let need = if full { 13 } else { 9 };
            if runs.len() < need {
                continue;
            }
            for i in 0..=runs.len() - need {
                if !runs[i].0 || !plausible_rings(&runs[i + 1..i + need - 1]) {
                    continue; // must start dark, on runs that could be rings
                }
                let window = &runs[i..i + need];
                let mut lens = [0usize; 13];
                for (l, r) in lens.iter_mut().zip(window) {
                    *l = r.2;
                }
                let Some(module) = runs_module(&lens[..need]) else {
                    continue;
                };
                hits += 1;
                let mid = &window[need / 2];
                let cx = mid.1 + mid.2 / 2;
                // Vertical cross-check: the same equal-run pattern down the centre
                // column, and its centre must land back on this row.
                let Some((cy, vmodule)) = vertical_check(bin, cx, y, need, module) else {
                    continue;
                };
                merge(
                    &mut eyes,
                    &mut index,
                    Bullseye {
                        cx: mid.1 as f32 + (mid.2 as f32 - 1.0) / 2.0,
                        cy,
                        module: (module + vmodule) / 2.0,
                        full,
                        count: 1,
                    },
                );
            }
        }
    }

    // Prefer full-range interpretations and well-confirmed centres. A full bullseye
    // also matches the compact pattern (its inner 9 runs), so when both exist at one
    // spot the full one must win.
    eyes.sort_by(|a, b| {
        b.count.cmp(&a.count).then(b.full.cmp(&a.full)).then(
            b.module
                .partial_cmp(&a.module)
                .unwrap_or(core::cmp::Ordering::Equal),
        )
    });
    (dedup_overlapping(eyes), hits)
}

/// Cheap necessary condition of [`runs_module`] on the inner runs, bailing at the
/// first pair of ring runs that cannot share a module width: they all lie within 40%
/// (or one pixel) of their mean, so no two differ by more than 7:3 or two pixels.
/// Nearly every window of a text or noise row fails within its first two or three.
fn plausible_rings(inner: &[(bool, usize, usize)]) -> bool {
    let centre = inner.len() / 2;
    let (mut lo, mut hi) = (usize::MAX, 0usize);
    for (i, r) in inner.iter().enumerate() {
        if i == centre {
            continue;
        }
        lo = lo.min(r.2);
        hi = hi.max(r.2);
        if hi - lo > 2 && 3 * hi > 7 * lo {
            return false;
        }
    }
    true
}

/// Bullseye cross-section check on the run lengths of a window, centre dot in the
/// middle. The ring runs must share one module width. The first and last dark runs
/// may run longer — the modules just outside the outer ring are mode-message bits,
/// and a dark one merges with the ring's run. The centre dot may run shorter: blur
/// eats an isolated dot from all sides where it only trades width between
/// neighbouring rings, so on a small soft symbol the dot is the thinnest run by far.
fn runs_module(lens: &[usize]) -> Option<f32> {
    let n = lens.len();
    let centre = n / 2;
    let rings = || {
        lens[1..n - 1]
            .iter()
            .enumerate()
            .filter(move |&(i, _)| i + 1 != centre)
            .map(|(_, &l)| l as f32)
    };
    let module = rings().sum::<f32>() / (n - 3) as f32;
    if module < 1.0 {
        return None;
    }
    let tol = (module * 0.4).max(1.0);
    if !rings().all(|l| (l - module).abs() <= tol) {
        return None;
    }
    let dot = lens[centre] as f32;
    if dot > module + tol || dot < module * 0.3 {
        return None;
    }
    let ok_outer = |len: usize| (len as f32) >= module * 0.6;
    (ok_outer(lens[0]) && ok_outer(lens[n - 1])).then_some(module)
}

/// Walk the equal-run pattern vertically through `(cx, y)`; returns the refined
/// centre-y and vertical module width.
fn vertical_check(
    bin: &BinaryImage,
    cx: usize,
    y: usize,
    need: usize,
    module: f32,
) -> Option<(f32, f32)> {
    let h = bin.height();
    let half = need / 2;
    // No run of a bullseye is anywhere near four times the module width its row showed.
    // Without the cap, every row of a striped texture (equal runs along the row, solid
    // down the column) walks the full image height for each of its hits.
    let cap = (module * 4.0) as usize + 2;
    // March up and down collecting run lengths from the centre run outward.
    let mut lengths = [0usize; 13];
    let lengths = &mut lengths[..need];
    let dark_at = |yy: i64| yy >= 0 && (yy as usize) < h && bin.get(cx, yy as usize);

    let mut up = y as i64;
    let mut down = y as i64 + 1;
    // Centre run.
    if !dark_at(up) {
        return None;
    }
    let mut run = 0usize;
    while dark_at(up) && run <= cap {
        run += 1;
        up -= 1;
    }
    while dark_at(down) && run <= cap {
        run += 1;
        down += 1;
    }
    if run > cap {
        return None;
    }
    // The centre run spans the rows strictly between `up` and `down`.
    let centre = (up + down) as f32 / 2.0;
    lengths[half] = run;
    // Alternate outward on both sides.
    let mut expect = false;
    for k in 1..=half {
        let mut run_up = 0usize;
        while up >= 0 && dark_at(up) == expect && run_up < cap {
            run_up += 1;
            up -= 1;
            if up < 0 {
                break;
            }
        }
        let mut run_down = 0usize;
        while (down as usize) < h && dark_at(down) == expect && run_down < cap {
            run_down += 1;
            down += 1;
        }
        if run_up == 0 || run_down == 0 {
            return None;
        }
        // Only the outermost runs may be cut short by the cap (they are only checked
        // for a minimum length below); an inner run that long is no ring.
        if k < half && (run_up == cap || run_down == cap) {
            return None;
        }
        lengths[half - k] = run_up;
        lengths[half + k] = run_down;
        expect = !expect;
    }
    // As in the row check: inner runs share the module width, the outermost dark
    // runs may merge with adjacent dark mode-message modules.
    let module = runs_module(lengths)?;
    // Refined centre: midpoint of the centre run, so that every row crossing the
    // centre dot reports the same point.
    Some((centre, module))
}

fn merge(eyes: &mut Vec<Bullseye>, index: &mut CenterIndex, eye: Bullseye) {
    // The first eye (in creation order) of the same kind within a module of the hit
    // absorbs it — hits on one bullseye agree to a pixel, while a window that matched
    // two rings off-centre (a full bullseye holds several compact cross-sections)
    // lands two modules away and must not drag the centre off the dot. The index narrows the search to the eyes near the hit: a periodic
    // texture is one equal-run hit every few pixels, tens of thousands of eyes.
    let near = index.candidates(eye.cx, eye.cy).iter().copied().find(|&i| {
        let e = &eyes[i as usize];
        e.full == eye.full && (e.cx - eye.cx).abs() <= e.module && (e.cy - eye.cy).abs() <= e.module
    });
    if let Some(i) = near {
        let e = &mut eyes[i as usize];
        let c = e.count as f32;
        e.cx = (e.cx * c + eye.cx) / (c + 1.0);
        e.cy = (e.cy * c + eye.cy) / (c + 1.0);
        e.module = (e.module * c + eye.module) / (c + 1.0);
        e.count += 1;
        index.cover(i, e.cx, e.cy, e.module);
        return;
    }
    index.cover(eyes.len() as u32, eye.cx, eye.cy, eye.module);
    eyes.push(eye);
}

/// Keep the best-confirmed eye of each spot (a full bullseye also matches as a
/// compact one at the same centre), up to [`MAX_EYES`].
fn dedup_overlapping(eyes: Vec<Bullseye>) -> Vec<Bullseye> {
    let mut out: Vec<Bullseye> = Vec::new();
    for e in eyes {
        // Only the first few are ever tried; on a texture with thousands of eyes,
        // filtering them all against each other is quadratic for nothing.
        if out.len() == MAX_EYES {
            break;
        }
        if out
            .iter()
            .any(|k| (k.cx - e.cx).abs() <= k.module && (k.cy - e.cy).abs() <= k.module)
        {
            continue;
        }
        out.push(e);
    }
    out
}
