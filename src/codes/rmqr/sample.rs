//! rMQR image sampling: pixels → [`BitMatrix`] → [`Symbol`].
//!
//! An rMQR symbol carries a full QR-style finder (7×7, the 1:1:3:1:1 signature) in
//! its top-left corner, and — crucially for a *rectangular* symbol whose width can
//! reach 139 modules — its 18-bit format information sits **directly beside that
//! finder** (columns 8–11), BCH-protected and encoding the exact symbol size. So the
//! sampler never guesses dimensions:
//!
//! 1. binarize (global Otsu, then an adaptive fallback);
//! 2. find the finder with the shared QR ratio scan; flood its separator-isolated
//!    outer ring and anchor a 4-point homography on its corners (four rotation
//!    hypotheses, shoelace-normalized);
//! 3. read format copy A through that homography — the BCH nearest-codeword check
//!    arbitrates the rotation *and* yields `(width, height, EC level)`;
//! 4. for wide symbols the finder-local homography drifts across the symbol, so the
//!    **finder-sub centre dot** (an isolated single dark module at the far corner)
//!    is searched around its predicted position and, when found, replaces one
//!    anchor — pinning the geometry across the full width;
//! 5. sample the rectangular grid and hand it to [`RmqrDecoder`], whose format
//!    cross-check and Reed–Solomon validate the read.

use super::matrix::{FMT_MASK_A, QUIET_ZONE, decode_format};
use super::{RmqrDecoder, RmqrSize};
use crate::codes::microqr::sample::{find_finders, finder_ring_corners, shoelace};
use crate::error::{Error, Result};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::imgproc::binary::BinaryImage;
use crate::imgproc::components::flood_region;
use crate::imgproc::homography::Homography;
use crate::imgproc::sample::sample_bilinear;
use crate::imgproc::threshold::{adaptive_binarize_bradley, otsu_binarize, otsu_threshold};
use crate::output::BitMatrix;
use crate::symbol::Symbol;

/// Locate, sample and structurally decode the rMQR symbol in `frame`.
pub fn scan(frame: &GrayFrame<'_>) -> Result<Symbol> {
    let threshold = otsu_threshold(frame);
    let mut last = Error::undecodable("no rMQR finder pattern found");
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
        return Err(Error::undecodable("no rMQR finder pattern found"));
    }
    let decoder = RmqrDecoder::new();
    let mut last = Error::undecodable("rMQR finder did not decode");

    for finder in finders.iter().take(4) {
        let Some(mut corners) = finder_ring_corners(bin, finder) else {
            continue;
        };
        if shoelace(&corners) < 0.0 {
            corners.swap(1, 3);
        }

        for rot in 0..4 {
            let mut dst = [Point::new(0.0, 0.0); 4];
            for (i, d) in dst.iter_mut().enumerate() {
                let c = corners[(i + rot) % 4];
                *d = Point::new(c.0, c.1);
            }
            let src = [
                Point::new(0.0, 0.0),
                Point::new(7.0, 0.0),
                Point::new(7.0, 7.0),
                Point::new(0.0, 7.0),
            ];
            let Ok(h) = Homography::from_correspondences(src, dst) else {
                continue;
            };

            // Format copy A (columns 8–11, rows 1–5) names the exact size — and its
            // BCH check rejects the three wrong rotations.
            let thr = f64::from(threshold);
            let mut raw = 0u32;
            for n in 0..18usize {
                let (gx, gy) = ((8 + n / 5) as f64 + 0.5, (1 + n % 5) as f64 + 0.5);
                let (px, py) = h.map_f64(gx, gy);
                if sample_bilinear(frame, px, py) <= thr {
                    raw |= 1 << n;
                }
            }
            let Some((size, _ec)) = decode_format(raw ^ FMT_MASK_A) else {
                continue;
            };

            // Wide symbols need a far anchor: swap the finder's bottom-right ring
            // corner for the finder-sub centre dot when it can be pinned down.
            let refined = refine_with_sub_dot(bin, &h, size, finder.module)
                .and_then(|dot| {
                    let (w, hh) = (size.width() as f32, size.height() as f32);
                    let src = [
                        Point::new(0.0, 0.0),
                        Point::new(7.0, 0.0),
                        Point::new(w - 2.5, hh - 2.5),
                        Point::new(0.0, 7.0),
                    ];
                    let dst = [dst[0], dst[1], Point::new(dot.0, dot.1), dst[3]];
                    Homography::from_correspondences(src, dst).ok()
                })
                .unwrap_or(h);

            match try_grid(frame, &refined, size, threshold, &decoder) {
                Ok(mut sym) => {
                    let (w, hh) = (size.width() as f64, size.height() as f64);
                    let quad = [
                        refined.map_f64(0.0, 0.0),
                        refined.map_f64(w, 0.0),
                        refined.map_f64(w, hh),
                        refined.map_f64(0.0, hh),
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
    Err(last)
}

/// Sample the rectangular `size` grid through `h` and decode it.
fn try_grid(
    frame: &GrayFrame<'_>,
    h: &Homography,
    size: RmqrSize,
    threshold: u8,
    decoder: &RmqrDecoder,
) -> Result<Symbol> {
    let (w, hh) = (size.width(), size.height());
    let thr = f64::from(threshold);
    let mut matrix = BitMatrix::new(w, hh, QUIET_ZONE);
    for y in 0..hh {
        for x in 0..w {
            let (px, py) = h.map_f64(x as f64 + 0.5, y as f64 + 0.5);
            if sample_bilinear(frame, px, py) <= thr {
                matrix.set(x, y, true);
            }
        }
    }
    decoder.decode_matrix(&matrix)
}

/// Find the finder-sub centre dot: an isolated dark module predicted at grid
/// `(w-2.5, h-2.5)`. The finder-anchored homography can drift a couple of modules by
/// the far corner, so nearby offsets are probed; a hit must flood into a component no
/// bigger than ~2 modules (the dot, not the surrounding 5×5 ring), whose centroid
/// becomes the anchor.
fn refine_with_sub_dot(
    bin: &BinaryImage,
    h: &Homography,
    size: RmqrSize,
    module: f32,
) -> Option<(f32, f32)> {
    let (gw, gh) = (size.width() as f64, size.height() as f64);
    let max_span = (module * 2.2).max(3.0);
    let mut offsets = vec![(0.0f64, 0.0f64)];
    for r in [0.5f64, 1.0, 1.5, 2.0, 2.5] {
        for i in 0..8 {
            let a = f64::from(i) * std::f64::consts::FRAC_PI_4;
            offsets.push((r * a.cos(), r * a.sin()));
        }
    }
    for (dx, dy) in offsets {
        let (px, py) = h.map_f64(gw - 2.5 + dx, gh - 2.5 + dy);
        if px < 0.0 || py < 0.0 {
            continue;
        }
        let seed = (px.round() as usize, py.round() as usize);
        let pixels = flood_region(bin, seed, true);
        if pixels.is_empty() || pixels.len() > (max_span * max_span) as usize {
            continue;
        }
        let (min_x, max_x) = pixels
            .iter()
            .fold((usize::MAX, 0), |(lo, hi), &(x, _)| (lo.min(x), hi.max(x)));
        let (min_y, max_y) = pixels
            .iter()
            .fold((usize::MAX, 0), |(lo, hi), &(_, y)| (lo.min(y), hi.max(y)));
        if (max_x - min_x) as f32 > max_span || (max_y - min_y) as f32 > max_span {
            continue;
        }
        let n = pixels.len() as f32;
        let (sx, sy) = pixels.iter().fold((0f32, 0f32), |(ax, ay), &(x, y)| {
            (ax + x as f32, ay + y as f32)
        });
        // Pixel centres average to the dot centre.
        return Some((sx / n + 0.5, sy / n + 0.5));
    }
    None
}
