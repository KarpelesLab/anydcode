//! Sub-pixel grid sampling: read a module grid out of a warped frame.
//!
//! This is the toolkit's payoff primitive. Given a [`Homography`] mapping
//! module-grid coordinates to image pixels and the grid's module count, [`sample_grid`]
//! visits each module's centre, reads the luminance there by bilinear interpolation,
//! thresholds it, and writes the result into a [`BitMatrix`] the decoder consumes.
//!
//! Grid coordinates run `0..dim` across the code; module `(col, row)` has its centre
//! at grid coordinate `(col + 0.5, row + 0.5)`. So a caller solves the homography from
//! the code's four corners at grid `(0,0),(dim,0),(dim,dim),(0,dim)` and hands it here.

use crate::image::GrayFrame;
use crate::imgproc::binary::BinaryImage;
use crate::imgproc::homography::Homography;
use crate::output::BitMatrix;

/// Bilinearly interpolate luminance at fractional pixel `(x, y)`.
///
/// Coordinates are clamped to the image, so sampling a module centre that maps just
/// outside the frame degrades gracefully instead of failing. Returns a value in the
/// `0.0..=255.0` range.
pub fn sample_bilinear(frame: &GrayFrame<'_>, x: f64, y: f64) -> f64 {
    let w = frame.width();
    let h = frame.height();
    // Clamp into the valid interpolation range [0, dim-1].
    let xf = x.clamp(0.0, (w - 1) as f64);
    let yf = y.clamp(0.0, (h - 1) as f64);
    let x0 = xf.floor() as usize;
    let y0 = yf.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let tx = xf - x0 as f64;
    let ty = yf - y0 as f64;

    let p00 = f64::from(frame.get_unchecked(x0, y0));
    let p10 = f64::from(frame.get_unchecked(x1, y0));
    let p01 = f64::from(frame.get_unchecked(x0, y1));
    let p11 = f64::from(frame.get_unchecked(x1, y1));

    let top = p00 + (p10 - p00) * tx;
    let bottom = p01 + (p11 - p01) * tx;
    top + (bottom - top) * ty
}

/// Sample a `dim × dim` module grid out of `frame`.
///
/// `homography` maps grid coordinates (`0..dim` per axis) to image pixels. Each module
/// centre is read with [`sample_bilinear`] and marked dark when its luminance is
/// `<= threshold`. `quiet_zone` is copied into the returned [`BitMatrix`].
pub fn sample_grid(
    frame: &GrayFrame<'_>,
    homography: &Homography,
    dim: usize,
    threshold: u8,
    quiet_zone: usize,
) -> BitMatrix {
    let mut matrix = BitMatrix::new(dim, dim, quiet_zone);
    let thr = f64::from(threshold);
    for row in 0..dim {
        for col in 0..dim {
            let gx = col as f64 + 0.5;
            let gy = row as f64 + 0.5;
            let (px, py) = homography.map_f64(gx, gy);
            let lum = sample_bilinear(frame, px, py);
            matrix.set(col, row, lum <= thr);
        }
    }
    matrix
}

/// Sample a `dim × dim` module grid out of an already-binarized [`BinaryImage`].
///
/// Like [`sample_grid`] but each module centre takes the value of the nearest binary
/// pixel (no interpolation needed since the source is already 1-bit).
pub fn sample_grid_binary(
    img: &BinaryImage,
    homography: &Homography,
    dim: usize,
    quiet_zone: usize,
) -> BitMatrix {
    let mut matrix = BitMatrix::new(dim, dim, quiet_zone);
    for row in 0..dim {
        for col in 0..dim {
            let gx = col as f64 + 0.5;
            let gy = row as f64 + 0.5;
            let (px, py) = homography.map_f64(gx, gy);
            let xi = px.round().max(0.0) as usize;
            let yi = py.round().max(0.0) as usize;
            matrix.set(col, row, img.get(xi, yi));
        }
    }
    matrix
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Point;

    fn matrices_equal(a: &BitMatrix, b: &BitMatrix) -> bool {
        if a.width() != b.width() || a.height() != b.height() {
            return false;
        }
        for y in 0..a.height() {
            for x in 0..a.width() {
                if a.get(x, y) != b.get(x, y) {
                    return false;
                }
            }
        }
        true
    }

    /// Build a deterministic pseudo-random dim×dim pattern.
    fn make_pattern(dim: usize) -> BitMatrix {
        let mut m = BitMatrix::new(dim, dim, 0);
        let mut state: u32 = 0x1234_5678;
        for y in 0..dim {
            for x in 0..dim {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                m.set(x, y, (state >> 16) & 1 == 1);
            }
        }
        m
    }

    #[test]
    fn bilinear_interpolates_midpoints() {
        // 2x2 image: 0, 100 / 200, 40. Centre (0.5, 0.5) is the average = 85.
        let data = [0u8, 100, 200, 40];
        let f = GrayFrame::new(&data, 2, 2).unwrap();
        assert!((sample_bilinear(&f, 0.0, 0.0) - 0.0).abs() < 1e-9);
        assert!((sample_bilinear(&f, 1.0, 0.0) - 100.0).abs() < 1e-9);
        assert!((sample_bilinear(&f, 0.5, 0.0) - 50.0).abs() < 1e-9);
        assert!((sample_bilinear(&f, 0.5, 0.5) - 85.0).abs() < 1e-9);
    }

    #[test]
    fn bilinear_clamps_out_of_bounds() {
        let data = [10u8, 20, 30, 40];
        let f = GrayFrame::new(&data, 2, 2).unwrap();
        // Far outside -> clamps to nearest corner.
        assert!((sample_bilinear(&f, -5.0, -5.0) - 10.0).abs() < 1e-9);
        assert!((sample_bilinear(&f, 99.0, 99.0) - 40.0).abs() < 1e-9);
    }

    /// The headline test: render a pattern through a known perspective warp into a
    /// GrayFrame, then sample it back and require an exact match.
    #[test]
    fn recovers_pattern_through_perspective_warp() {
        let dim = 12usize;
        let pattern = make_pattern(dim);

        // Image canvas and a genuinely perspective destination quad for the grid
        // corners at grid coords (0,0),(dim,0),(dim,dim),(0,dim).
        let iw = 220usize;
        let ih = 200usize;
        let quad = [
            Point::new(25.0, 20.0),
            Point::new(195.0, 40.0),
            Point::new(180.0, 185.0),
            Point::new(35.0, 165.0),
        ];
        let src = [
            Point::new(0.0, 0.0),
            Point::new(dim as f32, 0.0),
            Point::new(dim as f32, dim as f32),
            Point::new(0.0, dim as f32),
        ];
        let grid_to_img = Homography::from_correspondences(src, quad).unwrap();
        let img_to_grid = grid_to_img.inverse().unwrap();

        // Render: each image pixel takes the colour of the module it falls in.
        let mut buf = vec![255u8; iw * ih]; // light background
        for py in 0..ih {
            for px in 0..iw {
                let (gx, gy) = img_to_grid.map_f64(px as f64 + 0.5, py as f64 + 0.5);
                if gx >= 0.0 && gy >= 0.0 && gx < dim as f64 && gy < dim as f64 {
                    let col = gx as usize;
                    let row = gy as usize;
                    if pattern.get(col, row) {
                        buf[py * iw + px] = 0; // dark module
                    }
                }
            }
        }
        let frame = GrayFrame::new(&buf, iw, ih).unwrap();

        // Sample back and require an exact recovery.
        let sampled = sample_grid(&frame, &grid_to_img, dim, 128, 0);
        assert!(
            matrices_equal(&sampled, &pattern),
            "sampled grid did not match original pattern\n{sampled:?}\n{pattern:?}"
        );
    }

    #[test]
    fn recovers_pattern_from_binary_source() {
        let dim = 10usize;
        let pattern = make_pattern(dim);

        // Simple axis-aligned scaling: each module is a 7x7 block.
        let scale = 7.0f32;
        let src = [
            Point::new(0.0, 0.0),
            Point::new(dim as f32, 0.0),
            Point::new(dim as f32, dim as f32),
            Point::new(0.0, dim as f32),
        ];
        let dst = [
            Point::new(0.0, 0.0),
            Point::new(dim as f32 * scale, 0.0),
            Point::new(dim as f32 * scale, dim as f32 * scale),
            Point::new(0.0, dim as f32 * scale),
        ];
        let grid_to_img = Homography::from_correspondences(src, dst).unwrap();
        let img_to_grid = grid_to_img.inverse().unwrap();

        let side = (dim as f32 * scale) as usize;
        let mut bin = BinaryImage::new(side, side);
        for py in 0..side {
            for px in 0..side {
                let (gx, gy) = img_to_grid.map_f64(px as f64 + 0.5, py as f64 + 0.5);
                if gx >= 0.0 && gy >= 0.0 && gx < dim as f64 && gy < dim as f64 {
                    bin.set(px, py, pattern.get(gx as usize, gy as usize));
                }
            }
        }

        let sampled = sample_grid_binary(&bin, &grid_to_img, dim, 0);
        assert!(matrices_equal(&sampled, &pattern));
    }
}
