//! Camera detection of printed App Clip Codes on a luminance frame.
//!
//! A faithful grayscale port of the reference raster scanner (rs/appclipcode):
//!
//! 1. **Edge field** — Sobel gradients over the frame; strong-edge pixels keep their
//!    unit normal.
//! 2. **Ring Hough** — every edge pixel votes along ±its normal at each expected
//!    ring-edge distance, over a sweep of outer radii; accumulator peaks are circle
//!    candidates. The search runs on a ≤320 px copy, candidates are mapped back and
//!    hill-climbed at full resolution on a circle score (edge energy on the expected
//!    ring radii minus energy in the quiet zones).
//! 3. **Affine estimate** — the outer-circle edge cloud's second moments give the
//!    ellipse axes of a tilted code; a coordinate-descent refine tunes center, angle
//!    and both scales.
//! 4. **Rotation search** — the code carries no fiducial, so every 1° rotation is
//!    scored by how separably the 128 positions split into arcs vs gaps (Otsu on a
//!    per-position boundary-contrast measure), the best scores refined to 0.1°.
//! 5. **Bit recovery** — per position: gap/arc from thresholded contrast (Otsu plus a
//!    sweep of plausible visible counts), then 2-means over the visible arcs'
//!    luminance for the foreground/third-color bit, both polarities tried.
//!    Reed–Solomon (and the fixed template byte) arbitrate every attempt, so a wrong
//!    hypothesis cannot false-accept; the first URL that survives wins.
//!
//! The three code colors are distinguished by *luminance* alone: in every Apple
//! palette the derived third color is a visibly lighter tint of the foreground
//! (≥ 50/255 luma apart), so the color-blind port loses nothing on real codes.

use super::codec::decode_payload;
use super::svg::RINGS;
use super::tables::GAPS_BITS_ORDER_LUT;
use super::url::decompress_url;
use crate::error::{Error, Result};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;

const MAX_SEARCH_DIM: usize = 320;
const MIN_VISIBLE_POSITIONS: usize = 56;
const MAX_CANDIDATES: usize = 4;
const MAX_ROTATION_CANDIDATES: usize = 8;
const BG_RADIUS: f64 = 400.0;
const STROKE: f64 = 23.5;
const TEMPLATE_BITS: [bool; 8] = [false, true, false, true, false, true, false, false];

/// Scan `frame` for an App Clip Code and decode its URL.
pub fn scan(frame: &GrayFrame<'_>) -> Result<Symbol> {
    let gray = Gray::from_frame(frame);
    let full_field = EdgeField::new(&gray);
    let (search, search_scale) = gray.shrink_to(MAX_SEARCH_DIM);
    let search_field = EdgeField::new(&search);

    let mut candidates = locate_candidates(&search_field);
    if candidates.is_empty() {
        return Err(Error::undecodable("no App Clip ring pattern found"));
    }
    for c in &mut candidates {
        c.cx /= search_scale;
        c.cy /= search_scale;
        c.scale /= search_scale;
        *c = refine_candidate(&full_field, *c);
    }

    if std::env::var("ANYD_APPCLIP_DEBUG").is_ok() {
        for c in &candidates {
            eprintln!(
                "appclip cand cx={:.1} cy={:.1} scale={:.4} (outer r {:.1}px) score={:.4}",
                c.cx,
                c.cy,
                c.scale,
                BG_RADIUS * c.scale,
                c.score
            );
        }
    }
    for cand in &candidates {
        if let Some(url) = decode_candidate(&gray, &full_field, *cand) {
            let r = BG_RADIUS * cand.scale;
            let (x0, y0) = ((cand.cx - r) as f32, (cand.cy - r) as f32);
            let (x1, y1) = ((cand.cx + r) as f32, (cand.cy + r) as f32);
            let mut sym = Symbol::new(
                Symbology::AppClipCode,
                vec![Segment::byte(url.into_bytes())],
                SymbolMeta::Generic,
            );
            sym.location = Some(Location {
                outline: Quad::new([
                    Point::new(x0, y0),
                    Point::new(x1, y0),
                    Point::new(x1, y1),
                    Point::new(x0, y1),
                ]),
                rotation: None,
                module_size: Some((STROKE * cand.scale) as f32),
            });
            return Ok(sym);
        }
    }
    Err(Error::undecodable("App Clip ring pattern did not decode"))
}

// ===================================================================================
// Grayscale bitmap
// ===================================================================================

/// Owned luminance raster in `[0, 1]` with bilinear sampling.
struct Gray {
    w: usize,
    h: usize,
    px: Vec<f64>,
}

impl Gray {
    fn from_frame(frame: &GrayFrame<'_>) -> Gray {
        let (w, h) = (frame.width(), frame.height());
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                px.push(f64::from(frame.get_unchecked(x, y)) / 255.0);
            }
        }
        Gray { w, h, px }
    }

    #[inline]
    fn at(&self, x: usize, y: usize) -> f64 {
        self.px[y * self.w + x]
    }

    /// Bilinear sample; `None` outside the frame.
    fn sample(&self, x: f64, y: f64) -> Option<f64> {
        if x < 0.0 || y < 0.0 || x > (self.w - 1) as f64 || y > (self.h - 1) as f64 {
            return None;
        }
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.w - 1);
        let y1 = (y0 + 1).min(self.h - 1);
        let (tx, ty) = (x - x0 as f64, y - y0 as f64);
        let top = self.at(x0, y0) * (1.0 - tx) + self.at(x1, y0) * tx;
        let bot = self.at(x0, y1) * (1.0 - tx) + self.at(x1, y1) * tx;
        Some(top * (1.0 - ty) + bot * ty)
    }

    /// Box-downscale so the longest side is ≤ `max_dim`; returns the copy and the
    /// applied scale factor (1.0 when already small enough).
    fn shrink_to(&self, max_dim: usize) -> (Gray, f64) {
        let side = self.w.max(self.h);
        if side <= max_dim {
            return (
                Gray {
                    w: self.w,
                    h: self.h,
                    px: self.px.clone(),
                },
                1.0,
            );
        }
        let scale = max_dim as f64 / side as f64;
        let dw = ((self.w as f64 * scale).round() as usize).max(1);
        let dh = ((self.h as f64 * scale).round() as usize).max(1);
        let mut px = Vec::with_capacity(dw * dh);
        let inv = 1.0 / scale;
        for y in 0..dh {
            let sy0 = (y as f64 * inv) as usize;
            let sy1 = (((y + 1) as f64 * inv).ceil() as usize)
                .min(self.h)
                .max(sy0 + 1);
            for x in 0..dw {
                let sx0 = (x as f64 * inv) as usize;
                let sx1 = (((x + 1) as f64 * inv).ceil() as usize)
                    .min(self.w)
                    .max(sx0 + 1);
                let mut sum = 0.0;
                for sy in sy0..sy1 {
                    for sx in sx0..sx1 {
                        sum += self.at(sx, sy);
                    }
                }
                px.push(sum / ((sy1 - sy0) * (sx1 - sx0)) as f64);
            }
        }
        (Gray { w: dw, h: dh, px }, scale)
    }
}

// ===================================================================================
// Edge field
// ===================================================================================

struct EdgePixel {
    x: f64,
    y: f64,
    nx: f64,
    ny: f64,
    mag: f64,
}

struct EdgeField {
    w: usize,
    h: usize,
    edge: Vec<f64>,
    edges: Vec<EdgePixel>,
}

impl EdgeField {
    fn new(gray: &Gray) -> EdgeField {
        let (w, h) = (gray.w, gray.h);
        let mut gx_v = vec![0.0f64; w * h];
        let mut gy_v = vec![0.0f64; w * h];
        let mut edge = vec![0.0f64; w * h];
        let mut mags: Vec<f64> = Vec::new();
        for y in 1..h.saturating_sub(1) {
            for x in 1..w - 1 {
                let idx = y * w + x;
                let gx = -gray.at(x - 1, y - 1) - 2.0 * gray.at(x - 1, y) - gray.at(x - 1, y + 1)
                    + gray.at(x + 1, y - 1)
                    + 2.0 * gray.at(x + 1, y)
                    + gray.at(x + 1, y + 1);
                let gy = -gray.at(x - 1, y - 1) - 2.0 * gray.at(x, y - 1) - gray.at(x + 1, y - 1)
                    + gray.at(x - 1, y + 1)
                    + 2.0 * gray.at(x, y + 1)
                    + gray.at(x + 1, y + 1);
                let mag = gx.hypot(gy);
                gx_v[idx] = gx;
                gy_v[idx] = gy;
                edge[idx] = mag;
                if mag > 0.0 {
                    mags.push(mag);
                }
            }
        }

        let mut edges = Vec::new();
        if !mags.is_empty() {
            mags.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p90 = mags[(0.90 * (mags.len() - 1) as f64) as usize];
            let threshold = p90.max(mags[mags.len() - 1] * 0.18);
            for y in 1..h.saturating_sub(1) {
                for x in 1..w - 1 {
                    let idx = y * w + x;
                    let mag = edge[idx];
                    if mag < threshold {
                        continue;
                    }
                    edges.push(EdgePixel {
                        x: x as f64 + 0.5,
                        y: y as f64 + 0.5,
                        nx: gx_v[idx] / mag,
                        ny: gy_v[idx] / mag,
                        mag,
                    });
                }
            }
        }
        EdgeField { w, h, edge, edges }
    }

    fn sample(&self, x: f64, y: f64) -> f64 {
        if x < 0.0 || y < 0.0 || x > (self.w - 1) as f64 || y > (self.h - 1) as f64 {
            return 0.0;
        }
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.w - 1);
        let y1 = (y0 + 1).min(self.h - 1);
        let (tx, ty) = (x - x0 as f64, y - y0 as f64);
        let top = self.edge[y0 * self.w + x0] * (1.0 - tx) + self.edge[y0 * self.w + x1] * tx;
        let bot = self.edge[y1 * self.w + x0] * (1.0 - tx) + self.edge[y1 * self.w + x1] * tx;
        top * (1.0 - ty) + bot * ty
    }
}

// ===================================================================================
// Circle candidates
// ===================================================================================

#[derive(Clone, Copy)]
struct Candidate {
    cx: f64,
    cy: f64,
    /// Image pixels per canonical unit (canonical outer radius is 400).
    scale: f64,
    score: f64,
}

/// Ring-edge radii (both stroke edges of each ring plus the outer circle) in image
/// pixels at `scale`.
fn edge_radii(scale: f64) -> [f64; 11] {
    let hs = STROKE / 2.0;
    let mut out = [0.0f64; 11];
    for (i, &(r, _, _, _)) in RINGS.iter().enumerate() {
        out[i * 2] = (r - hs) * scale;
        out[i * 2 + 1] = (r + hs) * scale;
    }
    out[10] = BG_RADIUS * scale;
    out
}

/// Radii that must be *quiet* (between rings, inside ring 1, outside ring 5).
fn quiet_radii(scale: f64) -> [f64; 6] {
    let hs = STROKE / 2.0;
    let r = |i: usize| RINGS[i].0;
    [
        (r(0) - STROKE) * scale,
        ((r(0) + hs) + (r(1) - hs)) * 0.5 * scale,
        ((r(1) + hs) + (r(2) - hs)) * 0.5 * scale,
        ((r(2) + hs) + (r(3) - hs)) * 0.5 * scale,
        ((r(3) + hs) + (r(4) - hs)) * 0.5 * scale,
        ((r(4) + hs) + BG_RADIUS) * 0.5 * scale,
    ]
}

fn locate_candidates(field: &EdgeField) -> Vec<Candidate> {
    if field.edges.is_empty() {
        return Vec::new();
    }
    let min_dim = field.w.min(field.h) as f64;
    let min_outer = 20.0f64.max(min_dim * 0.10);
    let max_outer = min_dim * 0.45;
    let step = 3.0f64.max(min_dim / 96.0);

    let mut raw: Vec<Candidate> = Vec::new();
    let mut acc = vec![0.0f64; field.w * field.h];
    let mut outer = min_outer;
    while outer <= max_outer {
        let scale = outer / RINGS[4].0;
        acc.fill(0.0);
        let radii = edge_radii(scale);
        for e in &field.edges {
            for &r in &radii {
                for sign in [1.0, -1.0] {
                    let px = (e.x + sign * e.nx * r).round();
                    let py = (e.y + sign * e.ny * r).round();
                    if px >= 0.0 && (px as usize) < field.w && py >= 0.0 && (py as usize) < field.h
                    {
                        acc[py as usize * field.w + px as usize] += e.mag;
                    }
                }
            }
        }
        for peak in top_local_maxima(&acc, field.w, field.h, 2, 10.0) {
            raw.push(Candidate {
                cx: peak.0 as f64,
                cy: peak.1 as f64,
                scale,
                score: peak.2,
            });
        }
        outer += step;
    }

    raw.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    let mut out: Vec<Candidate> = Vec::new();
    for cand in raw {
        let cand = refine_candidate(field, cand);
        if cand.score <= 0.0 {
            continue;
        }
        if out.iter().any(|e| {
            (cand.cx - e.cx).hypot(cand.cy - e.cy) < 18.0
                && ((cand.scale - e.scale) / e.scale).abs() < 0.15
        }) {
            continue;
        }
        out.push(cand);
        if out.len() == MAX_CANDIDATES {
            break;
        }
    }
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    out
}

fn top_local_maxima(
    acc: &[f64],
    w: usize,
    h: usize,
    count: usize,
    radius: f64,
) -> Vec<(usize, usize, f64)> {
    let mut peaks: Vec<(usize, usize, f64)> = Vec::new();
    for y in 1..h.saturating_sub(1) {
        for x in 1..w - 1 {
            let s = acc[y * w + x];
            if s == 0.0
                || s < acc[y * w + x - 1]
                || s < acc[y * w + x + 1]
                || s < acc[(y - 1) * w + x]
                || s < acc[(y + 1) * w + x]
            {
                continue;
            }
            peaks.push((x, y, s));
        }
    }
    peaks.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    let mut out: Vec<(usize, usize, f64)> = Vec::new();
    for p in peaks {
        if out
            .iter()
            .any(|e| ((p.0 as f64 - e.0 as f64).hypot(p.1 as f64 - e.1 as f64)) < radius)
        {
            continue;
        }
        out.push(p);
        if out.len() == count {
            break;
        }
    }
    out
}

fn circle_score(field: &EdgeField, cx: f64, cy: f64, scale: f64) -> f64 {
    let er = edge_radii(scale);
    let qr = quiet_radii(scale);
    let samples = 72usize;
    let mut edge_energy = 0.0;
    let mut quiet_energy = 0.0;
    for i in 0..samples {
        let a = i as f64 / samples as f64 * std::f64::consts::TAU;
        let (s, c) = a.sin_cos();
        for &r in &er {
            edge_energy += field.sample(cx + c * r, cy + s * r);
        }
        for &r in &qr {
            quiet_energy += field.sample(cx + c * r, cy + s * r);
        }
    }
    edge_energy / (er.len() * samples) as f64 - quiet_energy / (qr.len() * samples) as f64 * 0.75
}

fn refine_candidate(field: &EdgeField, cand: Candidate) -> Candidate {
    let mut best = cand;
    best.score = circle_score(field, best.cx, best.cy, best.scale);
    for center_step in [6.0, 3.0, 1.5, 0.5] {
        let scale_step = best.scale * 0.04;
        let mut improved = true;
        while improved {
            improved = false;
            for dy in [-center_step, 0.0, center_step] {
                for dx in [-center_step, 0.0, center_step] {
                    for ds in [-scale_step, 0.0, scale_step] {
                        if dx == 0.0 && dy == 0.0 && ds == 0.0 {
                            continue;
                        }
                        let (cx, cy, scale) = (best.cx + dx, best.cy + dy, best.scale + ds);
                        if scale <= 0.0 {
                            continue;
                        }
                        let score = circle_score(field, cx, cy, scale);
                        if score > best.score {
                            best = Candidate {
                                cx,
                                cy,
                                scale,
                                score,
                            };
                            improved = true;
                        }
                    }
                }
            }
        }
    }
    best
}

// ===================================================================================
// Affine transform
// ===================================================================================

#[derive(Clone, Copy)]
struct Xform {
    cx: f64,
    cy: f64,
    /// Rotation of the x axis, radians.
    angle: f64,
    sx: f64,
    sy: f64,
}

impl Xform {
    fn point(&self, radius: f64, theta: f64) -> (f64, f64) {
        let (st, ct) = theta.sin_cos();
        let (sa, ca) = self.angle.sin_cos();
        let lx = self.sx * radius * ct;
        let ly = self.sy * radius * st;
        (self.cx + ca * lx - sa * ly, self.cy + sa * lx + ca * ly)
    }
}

fn xform_score(field: &EdgeField, x: &Xform) -> f64 {
    let er = edge_radii(1.0);
    let qr = quiet_radii(1.0);
    let samples = 96usize;
    let mut edge_energy = 0.0;
    let mut quiet_energy = 0.0;
    for i in 0..samples {
        let a = i as f64 / samples as f64 * std::f64::consts::TAU;
        for &r in &er {
            let (px, py) = x.point(r, a);
            edge_energy += field.sample(px, py);
        }
        for &r in &qr {
            let (px, py) = x.point(r, a);
            quiet_energy += field.sample(px, py);
        }
    }
    edge_energy / (er.len() * samples) as f64 - quiet_energy / (qr.len() * samples) as f64 * 0.7
}

/// Estimate tilt from the outer-circle edge cloud's second moments and refine by
/// coordinate descent over (center, angle, both scales).
fn estimate_xform(field: &EdgeField, cand: Candidate) -> Xform {
    let base = Xform {
        cx: cand.cx,
        cy: cand.cy,
        angle: 0.0,
        sx: cand.scale,
        sy: cand.scale,
    };
    let mut seeds = vec![base];

    let tol = cand.scale * (STROKE * 1.2);
    let expected = BG_RADIUS * cand.scale;
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut weight = 0.0;
    let mut selected: Vec<&EdgePixel> = Vec::new();
    for e in &field.edges {
        let r = (e.x - cand.cx).hypot(e.y - cand.cy);
        let d = (r - expected).abs();
        if d > tol {
            continue;
        }
        let w = e.mag * (1.0 - d / tol);
        selected.push(e);
        sum_x += e.x * w;
        sum_y += e.y * w;
        weight += w;
    }
    if weight > 0.0 {
        let (cx, cy) = (sum_x / weight, sum_y / weight);
        let mut xx = 0.0;
        let mut xy = 0.0;
        let mut yy = 0.0;
        let mut w2 = 0.0;
        for e in &selected {
            let r = (e.x - cx).hypot(e.y - cy);
            let d = (r - expected).abs();
            if d > tol {
                continue;
            }
            let w = e.mag * (1.0 - d / tol);
            let (dx, dy) = (e.x - cx, e.y - cy);
            xx += dx * dx * w;
            xy += dx * dy * w;
            yy += dy * dy * w;
            w2 += w;
        }
        seeds.push(Xform {
            cx,
            cy,
            angle: 0.0,
            sx: cand.scale,
            sy: cand.scale,
        });
        if w2 > 0.0 {
            let (xx, xy, yy) = (xx / w2, xy / w2, yy / w2);
            let trace = xx + yy;
            let delta = ((xx - yy) * (xx - yy) + 4.0 * xy * xy).max(0.0).sqrt();
            let l1 = (trace + delta) * 0.5;
            let l2 = (trace - delta) * 0.5;
            if l1 > 0.0 && l2 > 0.0 {
                // Major-axis direction of the ellipse: the eigenvector of the moment
                // matrix for λ₁ is (λ₁ − yy, xy).
                let major_angle = if xy.abs() > 1e-9 {
                    f64::atan2(xy, l1 - yy)
                } else if xx >= yy {
                    0.0
                } else {
                    std::f64::consts::FRAC_PI_2
                };
                let ratio = (l1 / l2).sqrt().clamp(1.0, 1.35);
                if ratio >= 1.03 {
                    seeds.push(Xform {
                        cx,
                        cy,
                        angle: major_angle,
                        sx: ((2.0 * l1).sqrt() / BG_RADIUS)
                            .clamp(cand.scale * 0.7, cand.scale * 1.4),
                        sy: ((2.0 * l2).sqrt() / BG_RADIUS)
                            .clamp(cand.scale * 0.7, cand.scale * 1.4),
                    });
                }
            }
        }
    }

    // Pick the best seed, then coordinate-descent.
    let mut best = seeds[0];
    let mut best_score = xform_score(field, &best);
    for s in &seeds[1..] {
        let sc = xform_score(field, s);
        if sc > best_score {
            best = *s;
            best_score = sc;
        }
    }

    for center_step in [18.0, 8.0, 3.0, 1.0] {
        let mut angle_step = 8.0f64.to_radians();
        let mut scale_step = best.sx.max(best.sy) * 0.10;
        let mut improved = true;
        while improved {
            improved = false;
            let cur = xform_score(field, &best);
            let trial = |x: Xform| -> Option<Xform> {
                if x.sx <= 0.0 || x.sy <= 0.0 {
                    return None;
                }
                (xform_score(field, &x) > cur).then_some(x)
            };
            let cands = [
                Xform {
                    cx: best.cx - center_step,
                    ..best
                },
                Xform {
                    cx: best.cx + center_step,
                    ..best
                },
                Xform {
                    cy: best.cy - center_step,
                    ..best
                },
                Xform {
                    cy: best.cy + center_step,
                    ..best
                },
                Xform {
                    angle: best.angle - angle_step,
                    ..best
                },
                Xform {
                    angle: best.angle + angle_step,
                    ..best
                },
                Xform {
                    sx: best.sx - scale_step,
                    ..best
                },
                Xform {
                    sx: best.sx + scale_step,
                    ..best
                },
                Xform {
                    sy: best.sy - scale_step,
                    ..best
                },
                Xform {
                    sy: best.sy + scale_step,
                    ..best
                },
            ];
            let mut cur_best = cur;
            for c in cands {
                if let Some(x) = trial(c) {
                    let sc = xform_score(field, &x);
                    if sc > cur_best {
                        best = x;
                        cur_best = sc;
                        improved = true;
                    }
                }
            }
            angle_step *= 0.7;
            scale_step *= 0.7;
        }
    }
    best
}

// ===================================================================================
// Rotation search + sampling
// ===================================================================================

struct PositionSample {
    /// Mean luminance at the arc-start patch of this position.
    color: f64,
    /// Squared luminance distance between the position-boundary patch and the local
    /// background — low over a gap (background at the boundary), high over an arc.
    contrast: f64,
}

fn sample_positions(gray: &Gray, x: &Xform, theta_deg: f64) -> Option<Vec<PositionSample>> {
    let mut out = Vec::with_capacity(128);
    for &(radius, ring_rot, n, gap_deg) in RINGS.iter() {
        let bit_angle = 360.0 / n as f64;
        let bit_rad = bit_angle.to_radians();
        let gap_rad = gap_deg.to_radians();
        let jitter = gap_rad * 0.35;
        for pos in 0..n {
            let angle = (ring_rot + theta_deg + pos as f64 * bit_angle).to_radians();

            let boundary = patch(
                gray,
                x,
                radius,
                angle,
                jitter,
                &[-STROKE * 0.22, 0.0, STROKE * 0.22],
            )?;
            let bg_off = STROKE * 0.85;
            let inner = patch(gray, x, radius - bg_off, angle, jitter * 0.6, &[0.0])?;
            let outer = patch(gray, x, radius + bg_off, angle, jitter * 0.6, &[0.0])?;
            let local_bg = (inner + outer) * 0.5;

            let start_off = gap_rad + 0.35 * (bit_rad - 2.0 * gap_rad);
            let arc = patch(
                gray,
                x,
                radius,
                angle + start_off,
                jitter,
                &[-STROKE * 0.18, 0.0, STROKE * 0.18],
            )?;

            let d = boundary - local_bg;
            out.push(PositionSample {
                color: arc,
                contrast: d * d,
            });
        }
    }
    Some(out)
}

fn patch(
    gray: &Gray,
    x: &Xform,
    radius: f64,
    angle: f64,
    jitter: f64,
    radial: &[f64],
) -> Option<f64> {
    let mut sum = 0.0;
    let mut count = 0.0;
    for &ro in radial {
        for da in [-jitter, 0.0, jitter] {
            let (px, py) = x.point(radius + ro, angle + da);
            sum += gray.sample(px, py)?;
            count += 1.0;
        }
    }
    Some(sum / count)
}

/// Otsu split of scalar values: `(threshold, separability score)`.
fn otsu_split(values: &[f64]) -> (f64, f64) {
    if values.len() < 2 {
        return (values.first().copied().unwrap_or(0.0), 0.0);
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = sorted.len();
    let mut prefix = vec![0.0f64; n + 1];
    for (i, &v) in sorted.iter().enumerate() {
        prefix[i + 1] = prefix[i] + v;
    }
    let total_mean = prefix[n] / n as f64;
    let mut best_t = sorted[n / 2];
    let mut best_score = -1.0;
    for i in 1..n {
        if sorted[i] == sorted[i - 1] {
            continue;
        }
        let w0 = i as f64 / n as f64;
        let w1 = 1.0 - w0;
        let m0 = prefix[i] / i as f64;
        let m1 = (prefix[n] - prefix[i]) / (n - i) as f64;
        let between = w0 * w1 * (m0 - m1) * (m0 - m1);
        let score = between / (total_mean * total_mean + 1e-12);
        if score > best_score {
            best_score = score;
            best_t = (sorted[i - 1] + sorted[i]) * 0.5;
        }
    }
    (best_t, best_score)
}

fn rotation_candidates(gray: &Gray, x: &Xform) -> Vec<f64> {
    let mut coarse: Vec<(f64, f64)> = Vec::new();
    let mut theta = 0.0;
    while theta < 360.0 {
        if let Some(samples) = sample_positions(gray, x, theta) {
            let contrasts: Vec<f64> = samples.iter().map(|s| s.contrast).collect();
            let (_, score) = otsu_split(&contrasts);
            coarse.push((theta, score));
        }
        theta += 1.0;
    }
    coarse.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    let mut refined: Vec<(f64, f64)> = Vec::new();
    let mut seen: Vec<i64> = Vec::new();
    for &(base, _) in &coarse {
        let key = base.round() as i64;
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let mut t = base - 0.5;
        while t <= base + 0.5 {
            if let Some(samples) = sample_positions(gray, x, t) {
                let contrasts: Vec<f64> = samples.iter().map(|s| s.contrast).collect();
                let (_, score) = otsu_split(&contrasts);
                refined.push((t.rem_euclid(360.0), score));
            }
            t += 0.1;
        }
        if seen.len() == MAX_ROTATION_CANDIDATES {
            break;
        }
    }
    refined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    let mut out: Vec<f64> = Vec::new();
    for &(t, _) in &refined {
        if out.iter().any(|&e| {
            let d = (e - t).rem_euclid(360.0);
            d.min(360.0 - d) < 0.15
        }) {
            continue;
        }
        out.push(t);
        if out.len() == MAX_ROTATION_CANDIDATES {
            break;
        }
    }
    out
}

// ===================================================================================
// Bit recovery
// ===================================================================================

fn decode_candidate(gray: &Gray, field: &EdgeField, cand: Candidate) -> Option<String> {
    let base = estimate_xform(field, cand);
    let avg = (base.sx + base.sy) * 0.5;
    let variants = [
        base,
        Xform {
            cx: base.cx,
            cy: base.cy,
            angle: 0.0,
            sx: avg,
            sy: avg,
        },
        Xform {
            cx: cand.cx,
            cy: cand.cy,
            angle: 0.0,
            sx: cand.scale,
            sy: cand.scale,
        },
    ];
    let debug = std::env::var("ANYD_APPCLIP_DEBUG").is_ok();
    for (vi, x) in variants.iter().enumerate() {
        let thetas = rotation_candidates(gray, x);
        if debug {
            eprintln!(
                "appclip variant {vi}: cx={:.1} cy={:.1} angle={:.2} sx={:.4} sy={:.4} thetas={:?}",
                x.cx,
                x.cy,
                x.angle.to_degrees(),
                x.sx,
                x.sy,
                thetas
                    .iter()
                    .map(|t| (t * 10.0).round() / 10.0)
                    .collect::<Vec<_>>()
            );
        }
        for theta in thetas {
            let Some(samples) = sample_positions(gray, x, theta) else {
                continue;
            };
            for bits in decode_attempts(&samples) {
                if let Ok(payload) = decode_payload(&bits)
                    && let Ok(url) = decompress_url(&payload)
                {
                    return Some(url);
                }
            }
        }
    }
    None
}

/// Candidate bit vectors for one sampled position set, cheapest mistakes first.
fn decode_attempts(samples: &[PositionSample]) -> Vec<Vec<bool>> {
    let contrasts: Vec<f64> = samples.iter().map(|s| s.contrast).collect();
    let thresholds = threshold_candidates(&contrasts);

    let mut attempts: Vec<(f64, Vec<bool>)> = Vec::new();
    for threshold in thresholds {
        let visible: Vec<bool> = contrasts.iter().map(|&c| c < threshold).collect();
        let visible_count = visible.iter().filter(|&&v| v).count();
        if visible_count < MIN_VISIBLE_POSITIONS || visible_count > samples.len() - 8 {
            continue;
        }
        let colors: Vec<f64> = samples
            .iter()
            .zip(&visible)
            .filter(|&(_, &v)| v)
            .map(|(s, _)| s.color)
            .collect();

        for mapping in classify_two(&colors) {
            // Gap bits (visible = arc = 0) then the per-arc color bits. No separator
            // is inserted: Apple's generator *renders* the separator bit as the first
            // visible arc's color (always foreground), so the first sampled color IS
            // bit 128. (The reference Go scanner inserts a synthetic separator here —
            // consistent only with its own test renderer, which skips bit 128; real
            // codes printed from Apple's SVG output follow this convention.)
            let mut bits = Vec::with_capacity(128 + visible_count);
            for &v in &visible {
                bits.push(!v);
            }
            let mut mi = 0;
            for &v in &visible {
                if v {
                    bits.push(mapping[mi]);
                    mi += 1;
                }
            }
            let penalty =
                f64::from(template_mismatch(&bits)) * 10.0 + (visible_count as f64 - 64.0).abs();
            attempts.push((penalty, bits));
        }
    }
    attempts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    attempts.into_iter().map(|(_, b)| b).collect()
}

fn threshold_candidates(contrasts: &[f64]) -> Vec<f64> {
    let (otsu, _) = otsu_split(contrasts);
    let mut sorted = contrasts.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut out = vec![otsu];
    for visible in [56usize, 60, 64, 68, 72, 76, 80, 84, 88, 92] {
        if visible == 0 || visible >= sorted.len() {
            continue;
        }
        let t = (sorted[visible - 1] + sorted[visible]) * 0.5;
        if out.iter().all(|&e| (e - t).abs() >= 1e-12) {
            out.push(t);
        }
    }
    out
}

/// 2-means over arc luminances; returns one or both polarity assignments
/// (`true` = third color).
fn classify_two(colors: &[f64]) -> Vec<Vec<bool>> {
    if colors.is_empty() {
        return Vec::new();
    }
    let mut a = colors[0];
    let mut b = colors[0];
    let mut max_d = -1.0;
    for &c in &colors[1..] {
        let d = (c - a) * (c - a);
        if d > max_d {
            max_d = d;
            b = c;
        }
    }
    let mut labels = vec![false; colors.len()];
    for _ in 0..12 {
        let mut sum_a = 0.0;
        let mut na = 0.0;
        let mut sum_b = 0.0;
        let mut nb = 0.0;
        for (i, &c) in colors.iter().enumerate() {
            if (c - a).abs() <= (c - b).abs() {
                labels[i] = false;
                sum_a += c;
                na += 1.0;
            } else {
                labels[i] = true;
                sum_b += c;
                nb += 1.0;
            }
        }
        if na > 0.0 {
            a = sum_a / na;
        }
        if nb > 0.0 {
            b = sum_b / nb;
        }
    }
    if max_d <= 1e-8 {
        return vec![labels];
    }
    let flipped: Vec<bool> = labels.iter().map(|&l| !l).collect();
    vec![labels, flipped]
}

/// Hamming distance of the assembled bit vector's template byte from `0x2A`.
fn template_mismatch(bits: &[bool]) -> u8 {
    if bits.len() < 128 {
        return TEMPLATE_BITS.len() as u8;
    }
    let mut mismatch = 0u8;
    for (i, &want) in TEMPLATE_BITS.iter().enumerate() {
        // pre-permutation bit 120+i lives at LUT[120+i] in ring order.
        if bits[GAPS_BITS_ORDER_LUT[120 + i]] != want {
            mismatch += 1;
        }
    }
    mismatch
}
