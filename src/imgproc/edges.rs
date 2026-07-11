//! Optional edge and morphology helpers.
//!
//! [`sobel_magnitude`] gives per-pixel gradient strength, useful for seeding the edge
//! points a border-line fit ([`crate::imgproc::line`]) consumes. [`erode`] and
//! [`dilate`] are the two basic binary-morphology operators samplers use to clean up
//! a thresholded image (close pinholes, drop specks) before component labeling.

use crate::image::GrayFrame;
use crate::imgproc::binary::BinaryImage;

/// Sobel gradient magnitude at every pixel, returned row-major as `width × height`.
///
/// Uses the standard 3×3 Sobel kernels; the magnitude is `sqrt(gx² + gy²)`. Border
/// pixels replicate the nearest in-bounds sample. Values can exceed `255`, so the
/// result is `f32` rather than a re-quantized image.
pub fn sobel_magnitude(frame: &GrayFrame<'_>) -> Vec<f32> {
    let w = frame.width();
    let h = frame.height();
    let at = |x: isize, y: isize| -> f32 {
        let xc = x.clamp(0, w as isize - 1) as usize;
        let yc = y.clamp(0, h as isize - 1) as usize;
        f32::from(frame.get_unchecked(xc, yc))
    };
    let mut out = vec![0.0f32; w * h];
    for y in 0..h as isize {
        for x in 0..w as isize {
            let tl = at(x - 1, y - 1);
            let tc = at(x, y - 1);
            let tr = at(x + 1, y - 1);
            let ml = at(x - 1, y);
            let mr = at(x + 1, y);
            let bl = at(x - 1, y + 1);
            let bc = at(x, y + 1);
            let br = at(x + 1, y + 1);
            let gx = (tr + 2.0 * mr + br) - (tl + 2.0 * ml + bl);
            let gy = (bl + 2.0 * bc + br) - (tl + 2.0 * tc + tr);
            out[y as usize * w + x as usize] = (gx * gx + gy * gy).sqrt();
        }
    }
    out
}

/// Morphological erosion with a 3×3 square structuring element.
///
/// A pixel stays dark only if all of its 8-neighbours (and itself) are dark; pixels
/// outside the image count as light. Shrinks blobs and removes isolated specks.
pub fn erode(img: &BinaryImage) -> BinaryImage {
    let w = img.width();
    let h = img.height();
    let mut out = BinaryImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut keep = true;
            'scan: for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx < 0
                        || ny < 0
                        || nx >= w as isize
                        || ny >= h as isize
                        || !img.get(nx as usize, ny as usize)
                    {
                        keep = false;
                        break 'scan;
                    }
                }
            }
            out.set(x, y, keep);
        }
    }
    out
}

/// Morphological dilation with a 3×3 square structuring element.
///
/// A pixel becomes dark if any of its 8-neighbours (or itself) is dark. Grows blobs
/// and closes small pinholes.
pub fn dilate(img: &BinaryImage) -> BinaryImage {
    let w = img.width();
    let h = img.height();
    let mut out = BinaryImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut hit = false;
            'scan: for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx >= 0
                        && ny >= 0
                        && nx < w as isize
                        && ny < h as isize
                        && img.get(nx as usize, ny as usize)
                    {
                        hit = true;
                        break 'scan;
                    }
                }
            }
            out.set(x, y, hit);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sobel_flat_image_has_no_gradient() {
        let data = [128u8; 25];
        let f = GrayFrame::new(&data, 5, 5).unwrap();
        let mag = sobel_magnitude(&f);
        assert!(mag.iter().all(|&m| m.abs() < 1e-4));
    }

    #[test]
    fn sobel_detects_vertical_edge() {
        // Left half dark (0), right half bright (255): a strong vertical edge.
        let w = 6;
        let h = 5;
        let mut data = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                data[y * w + x] = if x < w / 2 { 0 } else { 255 };
            }
        }
        let f = GrayFrame::new(&data, w, h).unwrap();
        let mag = sobel_magnitude(&f);
        // The gradient peaks at the boundary column (x == 2 or 3), not in flat areas.
        let edge = mag[2 * w + 2].max(mag[2 * w + 3]);
        let flat = mag[2 * w]; // far-left, flat region
        assert!(edge > 100.0, "edge magnitude {edge}");
        assert!(flat < 1e-4, "flat magnitude {flat}");
    }

    #[test]
    fn erode_removes_speck_and_shrinks() {
        let mut img = BinaryImage::new(7, 7);
        // Isolated speck.
        img.set(1, 1, true);
        // 3x3 block at (3..6, 3..6).
        for y in 3..6 {
            for x in 3..6 {
                img.set(x, y, true);
            }
        }
        let e = erode(&img);
        // Speck gone.
        assert!(!e.get(1, 1));
        // Block eroded to its single centre pixel.
        assert!(e.get(4, 4));
        assert!(!e.get(3, 3));
        assert_eq!(e.count_dark(), 1);
    }

    #[test]
    fn dilate_grows_and_fills_pinhole() {
        let mut img = BinaryImage::new(7, 7);
        // Solid 3x3 with a pinhole in the centre.
        for y in 2..5 {
            for x in 2..5 {
                img.set(x, y, true);
            }
        }
        img.set(3, 3, false); // pinhole
        let d = dilate(&img);
        // Pinhole filled and blob grew by one ring.
        assert!(d.get(3, 3));
        assert!(d.get(1, 3));
        assert!(d.get(5, 3));
    }

    #[test]
    fn erode_then_dilate_is_opening() {
        // Opening removes a thin protrusion but keeps the bulk.
        let mut img = BinaryImage::new(9, 9);
        for y in 3..6 {
            for x in 3..6 {
                img.set(x, y, true);
            }
        }
        img.set(6, 4, true); // one-pixel spur
        let opened = dilate(&erode(&img));
        assert!(opened.get(4, 4), "core survives opening");
        assert!(!opened.get(6, 4), "spur removed by opening");
    }
}
