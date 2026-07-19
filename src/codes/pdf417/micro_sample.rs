//! MicroPDF417 image sampling: pixels → [`BitMatrix`] → [`Symbol`].
//!
//! MicroPDF417 has no start/stop guard columns to lock onto — its Row Address
//! Patterns vary per row — but it compensates with rigidity: only 34 fixed size
//! variants exist, every one a solid rectangular block of ink whose outer boundary
//! *is* the symbol (the left RAP opens with a bar, the stop module closes every row,
//! and the top and bottom rows are bar-dense). The sampler leans on that:
//!
//! 1. binarize (global Otsu, then an adaptive fallback);
//! 2. take the dark-pixel cloud's four corners (rotation-robust farthest-point
//!    method, as the Data Matrix sampler does — the frame is assumed to contain one
//!    symbol on a clean quiet zone, which is what a located crop delivers);
//! 3. for each of the four rotations, try the size variants whose module aspect
//!    ratio matches the measured quad, nearest first: build the corner homography,
//!    sample the rectangular grid, and let [`MicroPdf417Decoder`] — which derives
//!    the variant from the grid geometry and Reed–Solomon-corrects the codewords —
//!    arbitrate the true hypothesis.

use super::micro::{
    MICRO_ROW_HEIGHT, MicroPdf417Decoder, NUM_VARIANTS, VAR_COLS, VAR_ROWS, variant_width,
};
use crate::error::{Error, Result};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::imgproc::binary::BinaryImage;
use crate::imgproc::components::extreme_quad;
use crate::imgproc::homography::Homography;
use crate::imgproc::sample::sample_bilinear;
use crate::imgproc::threshold::{adaptive_binarize_bradley, otsu_binarize, otsu_threshold};
use crate::output::BitMatrix;
use crate::symbol::Symbol;

/// Reject aspect hypotheses further than this factor from the measured quad.
const MAX_ASPECT_MISMATCH: f32 = 1.4;

/// Locate, sample and structurally decode the MicroPDF417 symbol in `frame`.
pub fn scan_micro(frame: &GrayFrame<'_>) -> Result<Symbol> {
    let threshold = otsu_threshold(frame);
    let mut last = Error::undecodable("no MicroPDF417 candidate geometry");
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
    // The whole dark cloud is taken as the symbol.
    let mut pixels = Vec::new();
    for y in 0..bin.height() {
        for x in 0..bin.width() {
            if bin.get(x, y) {
                pixels.push((x, y));
            }
        }
    }
    if pixels.len() < 64 {
        return Err(Error::undecodable("no MicroPDF417 candidate geometry"));
    }
    let Some(mut corners) = extreme_quad(&pixels) else {
        return Err(Error::undecodable("no MicroPDF417 candidate geometry"));
    };
    if shoelace(&corners) < 0.0 {
        corners.swap(1, 3);
    }

    // Measured side lengths give the aspect each rotation hypothesis must match.
    let side = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).hypot(a.1 - b.1);
    let decoder = MicroPdf417Decoder::new();
    let mut last = Error::undecodable("MicroPDF417 geometry did not decode");

    for rot in 0..4usize {
        let quad = [
            corners[rot % 4],
            corners[(rot + 1) % 4],
            corners[(rot + 2) % 4],
            corners[(rot + 3) % 4],
        ];
        // Under this rotation the grid x-axis runs quad[0]→quad[1].
        let qw = (side(quad[0], quad[1]) + side(quad[3], quad[2])) / 2.0;
        let qh = (side(quad[0], quad[3]) + side(quad[1], quad[2])) / 2.0;
        if qw < 4.0 || qh < 4.0 {
            continue;
        }
        let measured = qw / qh;

        // Variants nearest the measured aspect first.
        let mut order: Vec<(f32, usize)> = (0..NUM_VARIANTS)
            .filter_map(|v| {
                let w = variant_width(VAR_COLS[v] as usize) as f32;
                let h = (VAR_ROWS[v] as usize * MICRO_ROW_HEIGHT) as f32;
                let ratio = (w / h) / measured;
                let mismatch = ratio.max(1.0 / ratio);
                (mismatch <= MAX_ASPECT_MISMATCH).then_some((mismatch, v))
            })
            .collect();
        order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        for (_, v) in order {
            let w = variant_width(VAR_COLS[v] as usize);
            let h = VAR_ROWS[v] as usize * MICRO_ROW_HEIGHT;
            let src = [
                Point::new(0.0, 0.0),
                Point::new(w as f32, 0.0),
                Point::new(w as f32, h as f32),
                Point::new(0.0, h as f32),
            ];
            let dst = quad.map(|(x, y)| Point::new(x, y));
            let Ok(hom) = Homography::from_correspondences(src, dst) else {
                continue;
            };
            let thr = f64::from(threshold);
            let mut matrix = BitMatrix::new(w, h, 1);
            for y in 0..h {
                for x in 0..w {
                    let (px, py) = hom.map_f64(x as f64 + 0.5, y as f64 + 0.5);
                    if sample_bilinear(frame, px, py) <= thr {
                        matrix.set(x, y, true);
                    }
                }
            }
            match decoder.decode_matrix(&matrix) {
                Ok(mut sym) => {
                    sym.location = Some(Location {
                        outline: Quad::new(dst),
                        rotation: None,
                        module_size: Some(qw / w as f32),
                    });
                    return Ok(sym);
                }
                Err(e) => last = e,
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
