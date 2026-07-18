//! Micro QR image sampling: pixels → [`BitMatrix`] → [`Symbol`].
//!
//! A Micro QR symbol carries a single QR-style finder pattern (the 1:1:3:1:1
//! concentric square) in one corner, with timing patterns along the two adjacent
//! edges. The sampler:
//!
//! 1. binarizes the frame (global Otsu, then an adaptive fallback);
//! 2. sweeps rows for the finder ratio (reusing the QR run-scan helpers) with a
//!    vertical cross-check, clustering confirmed centres;
//! 3. floods the finder's **outer 7×7 dark ring** — isolated from the data by the
//!    light separator, so the flood recovers exactly it — and takes its four
//!    corners (rotation-robust farthest-point method) as a 4-point homography of
//!    the finder;
//! 4. extends that homography over the whole grid and tries the four rotations ×
//!    the four symbol sizes (M1 11×11 … M4 17×17): the wrong hypotheses read quiet
//!    zone or garbage and fail the format/error-correction checks inside
//!    [`MicroQrDecoder`], which arbitrates the true one.
//!
//! The finder-anchored homography is exact under rotation/scale/affine capture;
//! perspective tolerance is limited since all four anchors span one corner.

use super::{MicroQrDecoder, matrix::QUIET_ZONE};
use crate::codes::qr::sample::{found_pattern_cross, run_center, scan_line_runs, walk_run};
use crate::error::{Error, Result};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::imgproc::binary::BinaryImage;
use crate::imgproc::components::{extreme_quad, flood_region};
use crate::imgproc::homography::Homography;
use crate::imgproc::sample::sample_grid;
use crate::imgproc::threshold::{adaptive_binarize_bradley, otsu_binarize, otsu_threshold};
use crate::symbol::Symbol;

/// The four Micro QR grid sizes (M1–M4).
const SIZES: [usize; 4] = [11, 13, 15, 17];

/// A clustered finder-pattern centre.
#[derive(Debug, Clone, Copy)]
struct Finder {
    x: f32,
    y: f32,
    module: f32,
    count: u32,
}

/// Locate, sample and structurally decode the Micro QR symbol in `frame`.
pub fn scan(frame: &GrayFrame<'_>) -> Result<Symbol> {
    let threshold = otsu_threshold(frame);
    let mut last = Error::undecodable("no Micro QR finder pattern found");
    for bin in binarizations(frame) {
        match scan_with(frame, &bin, threshold) {
            Ok(sym) => return Ok(sym),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn binarizations(frame: &GrayFrame<'_>) -> [BinaryImage; 2] {
    let radius = (frame.width().min(frame.height()) / 8).clamp(8, 50);
    [
        otsu_binarize(frame),
        adaptive_binarize_bradley(frame, radius, 0.10),
    ]
}

fn scan_with(frame: &GrayFrame<'_>, bin: &BinaryImage, threshold: u8) -> Result<Symbol> {
    let finders = find_finders(bin);
    if finders.is_empty() {
        return Err(Error::undecodable("no Micro QR finder pattern found"));
    }
    let decoder = MicroQrDecoder::new();
    let mut last = Error::undecodable("Micro QR finder did not decode");

    for finder in finders.iter().take(4) {
        let Some(mut corners) = finder_ring_corners(bin, finder) else {
            continue;
        };
        // Clockwise order in image coordinates, else the sample mirrors.
        if shoelace(&corners) < 0.0 {
            corners.swap(1, 3);
        }

        // Four rotations (which ring corner is the symbol's top-left) × four sizes;
        // format info + ECC inside the decoder arbitrate.
        for rot in 0..4 {
            let mut dst = [Point::new(0.0, 0.0); 4];
            for (i, d) in dst.iter_mut().enumerate() {
                let c = corners[(i + rot) % 4];
                *d = Point::new(c.0, c.1);
            }
            // The finder's outer ring spans grid (0,0)–(7,7).
            let src = [
                Point::new(0.0, 0.0),
                Point::new(7.0, 0.0),
                Point::new(7.0, 7.0),
                Point::new(0.0, 7.0),
            ];
            let Ok(h) = Homography::from_correspondences(src, dst) else {
                continue;
            };
            for &dim in &SIZES {
                let matrix = sample_grid(frame, &h, dim, threshold, QUIET_ZONE);
                match decoder.decode_matrix(&matrix) {
                    Ok(mut sym) => {
                        let d = dim as f64;
                        let quad = [
                            h.map_f64(0.0, 0.0),
                            h.map_f64(d, 0.0),
                            h.map_f64(d, d),
                            h.map_f64(0.0, d),
                        ];
                        sym.location = Some(Location {
                            outline: Quad::new(quad.map(|(x, y)| Point::new(x as f32, y as f32))),
                            rotation: None,
                            module_size: Some(finder.module),
                        });
                        return Ok(sym);
                    }
                    Err(e) => last = e,
                }
            }
        }
    }
    Err(last)
}

fn shoelace(q: &[(f32, f32); 4]) -> f32 {
    let mut sum = 0.0;
    for i in 0..4 {
        let (x0, y0) = q[i];
        let (x1, y1) = q[(i + 1) % 4];
        sum += x0 * y1 - x1 * y0;
    }
    sum
}

/// Row sweep for the 1:1:3:1:1 finder ratio with a vertical cross-check.
fn find_finders(bin: &BinaryImage) -> Vec<Finder> {
    let (w, h) = (bin.width(), bin.height());
    let mut out: Vec<Finder> = Vec::new();
    for y in 0..h {
        scan_line_runs(
            w,
            |x| bin.get(x, y),
            |mid, counts| {
                let Some(module) = found_pattern_cross(counts) else {
                    return;
                };
                let Some((vc, vend)) = walk_run(h as i32, y as i32, |k| bin.get(mid, k as usize))
                else {
                    return;
                };
                if found_pattern_cross(vc).is_none() {
                    return;
                }
                let cy = run_center(vc, vend);
                merge(&mut out, mid as f32, cy, module);
            },
        );
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.count));
    out
}

fn merge(finders: &mut Vec<Finder>, x: f32, y: f32, module: f32) {
    for f in finders.iter_mut() {
        if (f.x - x).abs() <= f.module && (f.y - y).abs() <= f.module {
            let c = f.count as f32;
            f.x = (f.x * c + x) / (c + 1.0);
            f.y = (f.y * c + y) / (c + 1.0);
            f.module = (f.module * c + module) / (c + 1.0);
            f.count += 1;
            return;
        }
    }
    finders.push(Finder {
        x,
        y,
        module,
        count: 1,
    });
}

/// Corners of the finder's outer 7×7 dark ring: march from the centre out of the
/// solid 3×3 core, across the light ring, into the outer ring, and flood it. The
/// light separator row/column isolates the ring from the data region, so the flood
/// cannot leak.
fn finder_ring_corners(bin: &BinaryImage, finder: &Finder) -> Option<[(f32, f32); 4]> {
    let w = bin.width();
    let mut x = finder.x as usize;
    let y = finder.y as usize;
    let mut seen_light = false;
    let mut seed = None;
    let limit = ((finder.module * 4.5) as usize).max(4);
    for _ in 0..limit {
        if x + 1 >= w {
            break;
        }
        x += 1;
        let dark = bin.get(x, y);
        if !dark {
            seen_light = true;
        } else if seen_light {
            seed = Some((x, y));
            break;
        }
    }
    let pixels = flood_region(bin, seed?, true);
    if pixels.is_empty() {
        return None;
    }
    // Sanity: the ring must span ~7 modules.
    let span = 7.0 * finder.module;
    let (min_x, max_x) = pixels.iter().fold((usize::MAX, 0), |(lo, hi), &(px, _)| {
        (lo.min(px), hi.max(px))
    });
    let width = (max_x - min_x) as f32;
    if width < span * 0.7 || width > span * 1.5 {
        return None;
    }
    extreme_quad(&pixels)
}
