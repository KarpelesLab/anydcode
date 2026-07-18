//! Aztec image sampling: pixels → [`BitMatrix`] → [`Symbol`].
//!
//! Aztec is built to be found: every symbol centres on a **bullseye** of concentric
//! square rings (radius 0/2/4 modules for compact symbols, plus radius 6 for
//! full-range), so a row scan for a run of equal-width alternating dark/light modules
//! finds candidate centres cheaply — and the ring *count* along the cross-section
//! immediately says compact vs full-range. The sampler then:
//!
//! 1. binarizes the frame (global Otsu, then an adaptive fallback);
//! 2. scans rows for the 9-run (compact) / 13-run (full) equal-width bullseye
//!    cross-section, cross-checked vertically, and clusters the hits;
//! 3. floods the **outermost dark ring** (a square annulus, its own connected
//!    region) and extracts its four corners by the rotation-robust farthest-point
//!    method — these anchor a 4-point homography of the core;
//! 4. reads the **mode message** ring through that homography (RS over GF(16)
//!    arbitrates the four possible rotations) to learn the layer count, and with it
//!    the exact symbol size;
//! 5. samples the full grid and hands it to [`AztecDecoder`], whose Reed–Solomon
//!    check validates the read. An 11×11 rune attempt covers Aztec Runes.
//!
//! The core homography is exact under rotation/scale/affine capture; perspective
//! tolerance shrinks with symbol size since the four anchors span only the core.

use super::decode::read_mode_message;
use super::layout::Layout;
use super::{AztecDecoder, QUIET_ZONE};
use crate::error::{Error, Result};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::imgproc::binary::BinaryImage;
use crate::imgproc::components::{extreme_quad, flood_region};
use crate::imgproc::homography::Homography;
use crate::imgproc::sample::{sample_bilinear, sample_grid};
use crate::imgproc::threshold::{adaptive_binarize_bradley, otsu_binarize, otsu_threshold};
use crate::symbol::Symbol;

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

/// Locate, sample and structurally decode the Aztec symbol in `frame`.
pub fn scan(frame: &GrayFrame<'_>) -> Result<Symbol> {
    let threshold = otsu_threshold(frame);
    let mut last = Error::undecodable("no Aztec bullseye found");
    for bin in binarizations(frame) {
        match scan_with(frame, &bin, threshold) {
            Ok(sym) => return Ok(sym),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// The binarizations tried, in order (the sampling threshold stays global Otsu).
fn binarizations(frame: &GrayFrame<'_>) -> [BinaryImage; 2] {
    let radius = (frame.width().min(frame.height()) / 8).clamp(8, 50);
    [
        otsu_binarize(frame),
        adaptive_binarize_bradley(frame, radius, 0.10),
    ]
}

fn scan_with(frame: &GrayFrame<'_>, bin: &BinaryImage, threshold: u8) -> Result<Symbol> {
    let eyes = find_bullseyes(bin);
    if eyes.is_empty() {
        return Err(Error::undecodable("no Aztec bullseye found"));
    }
    let decoder = AztecDecoder::new();
    let mut last = Error::undecodable("Aztec bullseye did not decode");

    for eye in eyes.iter().take(4) {
        // The light annulus just inside the outer dark ring anchors the core
        // homography: unlike the dark rings — whose corners touch data-dependent
        // mode-message modules — it is a *closed* fixed pattern, sealed between two
        // solid rings, so a flood fill recovers exactly it. Its outer corners sit at
        // Chebyshev radius 3.5 (compact) / 5.5 (full) module units.
        let ring_half = if eye.full { 5.5f64 } else { 3.5 };
        let Some(mut corners) = ring_corners(bin, eye) else {
            continue;
        };
        // The homography sources run clockwise in image coordinates; a
        // counter-clockwise corner walk would sample the symbol mirrored.
        if shoelace(&corners) < 0.0 {
            corners.swap(1, 3);
        }

        // Four rotations: the mode message's RS code arbitrates which is right.
        for rot in 0..4 {
            let mut dst = [Point::new(0.0, 0.0); 4];
            for (i, d) in dst.iter_mut().enumerate() {
                let c = corners[(i + rot) % 4];
                *d = Point::new(c.0, c.1);
            }
            // Centre-relative core coordinates, clockwise from top-left.
            let rh = ring_half as f32;
            let src = [
                Point::new(-rh, -rh),
                Point::new(rh, -rh),
                Point::new(rh, rh),
                Point::new(-rh, rh),
            ];
            let Ok(h) = Homography::from_correspondences(src, dst) else {
                continue;
            };

            if std::env::var("ANYD_AZTEC_DEBUG").is_ok() {
                let q = quiet_beyond_core(frame, &h, threshold);
                eprintln!(
                    "aztec eye ({:.1},{:.1}) m={:.2} full={} rot={rot} corners={:?} quiet={q}",
                    eye.cx, eye.cy, eye.module, eye.full, corners
                );
            }
            match decode_via_mode(frame, &h, eye.full, threshold, &decoder) {
                Ok(mut sym) => {
                    sym.location = Some(location_from(&h, eye));
                    return Ok(sym);
                }
                Err(e) => last = e,
            }

            // Aztec Runes are 11×11 compact shells whose mode ring carries a data
            // byte instead of a layer count, so the mode read above rejects them.
            // Only attempt the rune grid when nothing surrounds the core — the mode
            // RS can mis-"correct" a genuine data symbol's ring into a rune value,
            // and a real rune is alone inside its quiet zone.
            if !eye.full
                && quiet_beyond_core(frame, &h, threshold)
                && let Ok(mut sym) = try_grid(frame, &h, 11, threshold, &decoder)
            {
                sym.location = Some(location_from(&h, eye));
                return Ok(sym);
            }
        }
    }
    Err(last)
}

/// Signed shoelace area of the corner quad (positive = clockwise in image coords).
fn shoelace(q: &[(f32, f32); 4]) -> f32 {
    let mut sum = 0.0;
    for i in 0..4 {
        let (x0, y0) = q[i];
        let (x1, y1) = q[(i + 1) % 4];
        sum += x0 * y1 - x1 * y0;
    }
    sum
}

/// Dark fraction just outside an 11×11 core, in the core frame. A rune's quiet zone
/// is light there; a data-bearing symbol is ~half dark.
fn quiet_beyond_core(frame: &GrayFrame<'_>, core: &Homography, threshold: u8) -> bool {
    let thr = f64::from(threshold);
    let mut dark = 0usize;
    let mut total = 0usize;
    for r in [6.5f64, 7.5] {
        for i in 0..24 {
            let a = i as f64 / 24.0 * std::f64::consts::TAU;
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
    threshold: u8,
    decoder: &AztecDecoder,
) -> Result<Symbol> {
    // Mode positions in centre-relative module coordinates: use any layout of the
    // right family — the ring only depends on compact/full and the centre.
    let probe = Layout::new(!full, 1);
    let c = probe.center as f64;
    let thr = f64::from(threshold);
    let bits: Vec<bool> = probe
        .mode_positions()
        .iter()
        .map(|&(x, y)| {
            let (px, py) = core.map_f64(x as f64 - c, y as f64 - c);
            sample_bilinear(frame, px, py) <= thr
        })
        .collect();
    let (layers, _) = read_mode_message(&bits, !full).inspect_err(|e| {
        if std::env::var("ANYD_AZTEC_DEBUG").is_ok() {
            eprintln!("aztec mode read failed: {e:?}");
        }
    })?;
    if std::env::var("ANYD_AZTEC_DEBUG").is_ok() {
        eprintln!("aztec mode ok: layers={layers}");
    }

    let layout = Layout::new(!full, layers);
    try_grid(frame, core, layout.size, threshold, decoder)
}

/// Sample a `size`×`size` grid through the centre-relative core homography.
fn try_grid(
    frame: &GrayFrame<'_>,
    core: &Homography,
    size: usize,
    threshold: u8,
    decoder: &AztecDecoder,
) -> Result<Symbol> {
    // sample_grid maps absolute continuous grid coordinates (module (i,j) centred at
    // (i+0.5, j+0.5)) while the core homography is centre-relative with *module
    // centres* on integer coordinates — the bullseye centre module (c,c) sits at
    // centre-relative (0,0), i.e. absolute (c+0.5, c+0.5). Rebuild an absolute
    // mapping by pushing four absolute corners through that shift.
    let shift = (size / 2) as f64 + 0.5;
    let s = size as f32;
    let abs_src = [
        Point::new(0.0, 0.0),
        Point::new(s, 0.0),
        Point::new(s, s),
        Point::new(0.0, s),
    ];
    let dst = abs_src.map(|p| {
        let (x, y) = core.map_f64(f64::from(p.x) - shift, f64::from(p.y) - shift);
        Point::new(x as f32, y as f32)
    });
    let h = Homography::from_correspondences(abs_src, dst)
        .map_err(|_| Error::undecodable("degenerate Aztec homography"))?;
    let matrix = sample_grid(frame, &h, size, threshold, QUIET_ZONE);
    decoder.decode_matrix(&matrix)
}

/// A reported [`Location`] for the decoded symbol: the core corners scaled out make
/// a fair outline; module size comes from the core span.
fn location_from(core: &Homography, eye: &Bullseye) -> Location {
    let r = 8.0f64;
    let quad = [
        core.map_f64(-r, -r),
        core.map_f64(r, -r),
        core.map_f64(r, r),
        core.map_f64(-r, r),
    ];
    Location {
        outline: Quad::new(quad.map(|(x, y)| Point::new(x as f32, y as f32))),
        rotation: None,
        module_size: Some(eye.module),
    }
}

/// Scan rows for the equal-run bullseye cross-section and cluster confirmed centres.
fn find_bullseyes(bin: &BinaryImage) -> Vec<Bullseye> {
    let (w, h) = (bin.width(), bin.height());
    let mut eyes: Vec<Bullseye> = Vec::new();

    for y in 0..h {
        // Run-length encode the row.
        let mut runs: Vec<(bool, usize, usize)> = Vec::new();
        let mut cur = bin.get(0, y);
        let mut start = 0usize;
        for x in 1..w {
            let d = bin.get(x, y);
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
                if !runs[i].0 {
                    continue; // must start dark
                }
                let window = &runs[i..i + need];
                let Some(module) = equal_runs_module(window) else {
                    continue;
                };
                let mid = &window[need / 2];
                let cx = mid.1 + mid.2 / 2;
                // Vertical cross-check: the same equal-run pattern down the centre
                // column, and its centre must land back on this row.
                let Some((cy, vmodule)) = vertical_check(bin, cx, y, need) else {
                    continue;
                };
                merge(
                    &mut eyes,
                    Bullseye {
                        cx: cx as f32,
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
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    dedup_overlapping(eyes)
}

/// Bullseye cross-section check. The *inner* runs must share one module width; the
/// first and last dark runs may run longer — the modules just outside the outer ring
/// are mode-message bits, and a dark one merges with the ring's run.
fn equal_runs_module(window: &[(bool, usize, usize)]) -> Option<f32> {
    let inner = &window[1..window.len() - 1];
    let total: usize = inner.iter().map(|r| r.2).sum();
    let module = total as f32 / inner.len() as f32;
    if module < 1.0 {
        return None;
    }
    let tol = (module * 0.4).max(1.0);
    if !inner.iter().all(|r| (r.2 as f32 - module).abs() <= tol) {
        return None;
    }
    let ok_outer = |len: usize| (len as f32) >= module * 0.6;
    (ok_outer(window[0].2) && ok_outer(window[window.len() - 1].2)).then_some(module)
}

/// Walk the equal-run pattern vertically through `(cx, y)`; returns the refined
/// centre-y and vertical module width.
fn vertical_check(bin: &BinaryImage, cx: usize, y: usize, need: usize) -> Option<(f32, f32)> {
    let h = bin.height();
    let half = need / 2;
    // March up and down collecting run lengths from the centre run outward.
    let mut lengths = vec![0usize; need];
    let dark_at = |yy: i64| yy >= 0 && (yy as usize) < h && bin.get(cx, yy as usize);

    let mut up = y as i64;
    let mut down = y as i64 + 1;
    // Centre run.
    if !dark_at(up) {
        return None;
    }
    let mut run = 0usize;
    while dark_at(up) {
        run += 1;
        up -= 1;
    }
    while dark_at(down) {
        run += 1;
        down += 1;
    }
    lengths[half] = run;
    // Alternate outward on both sides.
    let mut expect = false;
    for k in 1..=half {
        let mut run_up = 0usize;
        while up >= 0 && dark_at(up) == expect && run_up < h {
            run_up += 1;
            up -= 1;
            if up < 0 {
                break;
            }
        }
        let mut run_down = 0usize;
        while (down as usize) < h && dark_at(down) == expect && run_down < h {
            run_down += 1;
            down += 1;
        }
        if run_up == 0 || run_down == 0 {
            return None;
        }
        lengths[half - k] = run_up;
        lengths[half + k] = run_down;
        expect = !expect;
    }
    // As in the row check: inner runs share the module width, the outermost dark
    // runs may merge with adjacent dark mode-message modules.
    let inner = &lengths[1..need - 1];
    let module = inner.iter().sum::<usize>() as f32 / inner.len() as f32;
    let tol = (module * 0.4).max(1.0);
    if !inner.iter().all(|&l| (l as f32 - module).abs() <= tol) {
        return None;
    }
    if (lengths[0] as f32) < module * 0.6 || (lengths[need - 1] as f32) < module * 0.6 {
        return None;
    }
    // Refined centre: midpoint of the centre run (its span was walked first).
    Some((y as f32, module))
}

fn merge(eyes: &mut Vec<Bullseye>, eye: Bullseye) {
    for e in eyes.iter_mut() {
        if e.full == eye.full
            && (e.cx - eye.cx).abs() <= e.module * 2.0
            && (e.cy - eye.cy).abs() <= e.module * 2.0
        {
            let c = e.count as f32;
            e.cx = (e.cx * c + eye.cx) / (c + 1.0);
            e.cy = (e.cy * c + eye.cy) / (c + 1.0);
            e.module = (e.module * c + eye.module) / (c + 1.0);
            e.count += 1;
            return;
        }
    }
    eyes.push(eye);
}

/// Drop compact interpretations that sit on an already-kept full-range centre.
fn dedup_overlapping(eyes: Vec<Bullseye>) -> Vec<Bullseye> {
    let mut out: Vec<Bullseye> = Vec::new();
    for e in eyes {
        if out
            .iter()
            .any(|k| (k.cx - e.cx).abs() <= k.module * 3.0 && (k.cy - e.cy).abs() <= k.module * 3.0)
        {
            continue;
        }
        out.push(e);
    }
    out
}

/// Corners of the enclosed light annulus inside the outermost dark ring.
fn ring_corners(bin: &BinaryImage, eye: &Bullseye) -> Option<[(f32, f32); 4]> {
    // Ray-march from the centre along +x, counting dark runs: dot, ring@2 (and
    // ring@4 when full). The first light pixel after that run sits inside the
    // enclosed light annulus (Chebyshev 3 compact / 5 full).
    let want_run = if eye.full { 3 } else { 2 };
    let w = bin.width();
    let mut x = eye.cx as usize;
    let y = eye.cy as usize;
    let mut runs = 0usize;
    let mut in_dark = false;
    let mut seed = None;
    let limit = ((eye.module * if eye.full { 7.0 } else { 5.0 }) as usize).max(4);
    for _ in 0..limit {
        if x + 1 >= w {
            break;
        }
        x += 1;
        let dark = bin.get(x, y);
        if dark && !in_dark {
            runs += 1;
            in_dark = true;
        } else if !dark {
            if in_dark && runs == want_run {
                seed = Some((x, y));
                break;
            }
            in_dark = false;
        }
    }
    let pixels = flood_region(bin, seed?, false);
    // Sanity: the annulus must span roughly the expected width — a leak through a
    // broken ring floods the page background and fails this immediately.
    let span = if eye.full { 11.0 } else { 7.0 } * eye.module;
    let (min_x, max_x) = pixels.iter().fold((usize::MAX, 0), |(lo, hi), &(px, _)| {
        (lo.min(px), hi.max(px))
    });
    if pixels.is_empty() {
        return None;
    }
    let width = (max_x - min_x) as f32;
    if width < span * 0.7 || width > span * 1.5 {
        return None;
    }
    extreme_quad(&pixels)
}
