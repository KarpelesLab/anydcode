//! Binarization: global Otsu and adaptive/local thresholds.
//!
//! [`otsu_threshold`] picks one global threshold by maximizing between-class
//! variance — ideal for evenly-lit, bimodal images. Real captures rarely are, so
//! [`adaptive_binarize_bradley`] and [`adaptive_binarize_sauvola`] threshold each
//! pixel against statistics of its local neighbourhood, staying correct under
//! gradients and vignetting. All produce a [`BinaryImage`] with `dark = true`.

use crate::image::GrayFrame;
use crate::imgproc::binary::BinaryImage;
use crate::imgproc::integral::IntegralImage;

/// Compute a global threshold by Otsu's method (maximizing between-class variance).
///
/// Returns the luminance value `t` such that pixels with value `<= t` are considered
/// dark. For a perfectly bimodal image this lands between the two modes.
pub fn otsu_threshold(frame: &GrayFrame<'_>) -> u8 {
    // Build the 256-bin luminance histogram.
    let mut hist = [0u64; 256];
    for y in 0..frame.height() {
        for x in 0..frame.width() {
            hist[frame.get_unchecked(x, y) as usize] += 1;
        }
    }
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return 127;
    }

    let sum_all: u64 = hist
        .iter()
        .enumerate()
        .map(|(i, &c)| i as u64 * c)
        .sum::<u64>();

    let mut weight_bg: u64 = 0; // pixels with value <= t
    let mut sum_bg: u64 = 0; // weighted intensity of background
    let mut best_var = -1.0f64;
    let mut best_t = 0u8;

    for (t, &count) in hist.iter().enumerate() {
        weight_bg += count;
        if weight_bg == 0 {
            continue;
        }
        let weight_fg = total - weight_bg;
        if weight_fg == 0 {
            break;
        }
        sum_bg += t as u64 * count;
        let mean_bg = sum_bg as f64 / weight_bg as f64;
        let mean_fg = (sum_all - sum_bg) as f64 / weight_fg as f64;
        let diff = mean_bg - mean_fg;
        // Between-class variance (up to the constant 1/total^2).
        let var = weight_bg as f64 * weight_fg as f64 * diff * diff;
        if var > best_var {
            best_var = var;
            best_t = t as u8;
        }
    }
    best_t
}

/// Binarize `frame` with a single global Otsu threshold. Pixels with luminance
/// `<= t` become dark (`true`).
pub fn otsu_binarize(frame: &GrayFrame<'_>) -> BinaryImage {
    let t = u64::from(otsu_threshold(frame));
    let mut img = BinaryImage::new(frame.width(), frame.height());
    for y in 0..frame.height() {
        for x in 0..frame.width() {
            let v = u64::from(frame.get_unchecked(x, y));
            img.set(x, y, v <= t);
        }
    }
    img
}

/// Adaptive binarization after Bradley & Roth (a Wellner-style local mean test).
///
/// A pixel is dark when its luminance is more than `t` fraction below the mean of
/// the `(2·radius+1)²` window centred on it. Typical values: `radius ≈ image/16`,
/// `t ≈ 0.15`. `t` is clamped to `[0, 1)`.
pub fn adaptive_binarize_bradley(frame: &GrayFrame<'_>, radius: usize, t: f32) -> BinaryImage {
    let t = t.clamp(0.0, 0.999) as f64;
    let integral = IntegralImage::from_frame(frame);
    let mut img = BinaryImage::new(frame.width(), frame.height());
    for y in 0..frame.height() {
        for x in 0..frame.width() {
            let (sum, count) = integral.window_sum_count(x, y, radius);
            let mean = sum as f64 / count as f64;
            let v = f64::from(frame.get_unchecked(x, y));
            // Dark if the pixel is meaningfully below the local mean.
            img.set(x, y, v < mean * (1.0 - t));
        }
    }
    img
}

/// Adaptive binarization by Sauvola's method.
///
/// Thresholds each pixel at `T = mean · (1 + k·(std/r − 1))`, where the local mean
/// and standard deviation come from the `(2·radius+1)²` window. `k` (≈ `0.2..0.5`)
/// controls sensitivity and `r` is the dynamic range of the standard deviation
/// (`128` for 8-bit). This adapts to both brightness and local contrast, so faint
/// modules in bright regions and vice-versa are both recovered.
pub fn adaptive_binarize_sauvola(
    frame: &GrayFrame<'_>,
    radius: usize,
    k: f32,
    r: f32,
) -> BinaryImage {
    let k = f64::from(k);
    let r = f64::from(r).max(1.0);
    let integral = IntegralImage::from_frame(frame);
    let sq_integral = IntegralImage::squared_from_frame(frame);
    let mut img = BinaryImage::new(frame.width(), frame.height());
    for y in 0..frame.height() {
        for x in 0..frame.width() {
            let (sum, count) = integral.window_sum_count(x, y, radius);
            let (sq_sum, _) = sq_integral.window_sum_count(x, y, radius);
            let n = count as f64;
            let mean = sum as f64 / n;
            // Variance = E[x^2] - E[x]^2, guarded against tiny negatives from rounding.
            let var = (sq_sum as f64 / n - mean * mean).max(0.0);
            let std = var.sqrt();
            let threshold = mean * (1.0 + k * (std / r - 1.0));
            let v = f64::from(frame.get_unchecked(x, y));
            img.set(x, y, v <= threshold);
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render a `w×h` frame from a closure giving luminance at each pixel.
    fn render<F: Fn(usize, usize) -> u8>(w: usize, h: usize, f: F) -> Vec<u8> {
        let mut v = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                v[y * w + x] = f(x, y);
            }
        }
        v
    }

    #[test]
    fn otsu_splits_bimodal_between_modes() {
        // Half the pixels at 50, half at 200.
        let w = 20;
        let h = 20;
        let data = render(w, h, |x, _| if x < w / 2 { 50 } else { 200 });
        let f = GrayFrame::new(&data, w, h).unwrap();
        let t = otsu_threshold(&f);
        assert!(
            (50..200).contains(&t),
            "threshold {t} not strictly between the two modes"
        );
    }

    #[test]
    fn otsu_binarize_separates_regions() {
        let w = 16;
        let h = 16;
        // Dark disk-ish square in the middle at 40, background 220.
        let data = render(w, h, |x, y| {
            if (4..12).contains(&x) && (4..12).contains(&y) {
                40
            } else {
                220
            }
        });
        let f = GrayFrame::new(&data, w, h).unwrap();
        let bin = otsu_binarize(&f);
        assert!(bin.get(8, 8), "centre should be dark");
        assert!(!bin.get(0, 0), "corner should be light");
        assert_eq!(bin.count_dark(), 8 * 8);
    }

    #[test]
    fn adaptive_beats_global_on_gradient() {
        // A strong left-to-right illumination gradient, with dark squares laid on top.
        // Globally the bright right side swamps the dark left side; a global threshold
        // cannot recover all squares, but an adaptive local one can.
        let w = 64;
        let h = 32;
        let square = |x: usize, y: usize| {
            // Two squares: one on the dark side, one on the bright side.
            let s1 = (8..16).contains(&x) && (12..20).contains(&y);
            let s2 = (48..56).contains(&x) && (12..20).contains(&y);
            s1 || s2
        };
        let data = render(w, h, |x, y| {
            // Background ramps from 60 (left) to 250 (right).
            let bg = 60 + (x as u32 * 190 / (w as u32 - 1));
            let bg = bg as u8;
            if square(x, y) {
                // Each square is 60 luminance below its local background. This dip is
                // large enough for local methods to catch yet — crucially — no single
                // global threshold can mark both squares dark while keeping the ramped
                // background light, which is exactly what the assertions below check.
                bg.saturating_sub(60)
            } else {
                bg
            }
        });
        let f = GrayFrame::new(&data, w, h).unwrap();

        // Bradley adaptive: both squares recovered, backgrounds stay light.
        let bin = adaptive_binarize_bradley(&f, 8, 0.10);
        assert!(bin.get(11, 15), "left square dark under adaptive");
        assert!(bin.get(51, 15), "right square dark under adaptive");
        assert!(!bin.get(30, 2), "background light under adaptive");

        // Sauvola likewise recovers both squares.
        let bin_s = adaptive_binarize_sauvola(&f, 8, 0.2, 128.0);
        assert!(bin_s.get(11, 15), "left square dark under sauvola");
        assert!(bin_s.get(51, 15), "right square dark under sauvola");

        // Demonstrate the global method genuinely fails here: a single Otsu threshold
        // cannot mark both squares dark while leaving the bright background light.
        let global = otsu_binarize(&f);
        let both_squares = global.get(11, 15) && global.get(51, 15);
        let clean_bg = !global.get(30, 2) && !global.get(60, 2);
        assert!(
            !(both_squares && clean_bg),
            "global threshold unexpectedly handled the gradient"
        );
    }
}
