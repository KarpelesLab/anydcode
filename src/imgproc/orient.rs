//! Dominant gradient-orientation estimation.
//!
//! A linear barcode is the most orientation-coherent texture there is: every bar edge
//! points the gradient along the *reading* axis, so the gradient-orientation histogram
//! of a barcode crop has one overwhelming peak (modulo 180° — a gradient cannot tell a
//! code from its upside-down self). [`dominant_gradient_angle`] measures that peak and
//! how dominant it is, letting a scanning pipeline *derotate* a crop whose code is not
//! within its scanline sweep instead of blindly re-scanning at every angle.

use crate::image::GrayFrame;
use alloc::vec::Vec;

/// Histogram bins across the half-circle: 3° per bin.
const BINS: usize = 60;

/// Minimum squared gradient magnitude for a pixel to vote; rejects sensor noise and
/// flat background so the histogram is built from real edges.
const MIN_MAG2: f32 = 24.0 * 24.0;

/// Fraction of total edge energy that must fall within ±2 bins (±7.5°) of the peak for
/// the orientation to count as *dominant*. Barcodes score far above this; text, matrix
/// codes and scene texture spread their energy and fall below.
const MIN_COHERENCE: f32 = 0.4;

/// Share of the edge energy a *secondary* orientation peak must hold (within ±2 bins)
/// to be reported by [`gradient_angle_peaks`]. A barcode sharing its crop with text or
/// a label border is no longer the majority of the edge energy, but it is still a
/// sharp peak; the scan that follows is what decides whether it was a code.
const MIN_PEAK_SHARE: f32 = 0.18;

/// Estimate the dominant edge orientation of `frame` in radians, in `(-π/2, π/2]`,
/// measured from the +x axis — for a barcode this is the *reading* direction (bars are
/// perpendicular to it). Returns `None` when the texture has no sufficiently dominant
/// orientation (nothing bar-like to derotate for).
pub fn dominant_gradient_angle(frame: &GrayFrame<'_>) -> Option<f32> {
    let hist = orientation_histogram(frame)?;
    let total: f32 = hist.iter().sum();
    let peak = smoothed_peak(&hist, &[])?;
    if share(&hist, peak) / total < MIN_COHERENCE {
        return None;
    }
    refine_peak(&hist, peak)
}

/// Up to `max` distinct edge-orientation peaks of `frame`, strongest first, each in
/// radians in `(-π/2, π/2]` like [`dominant_gradient_angle`] — candidate reading axes
/// for a linear code that shares the frame with other print.
///
/// Where [`dominant_gradient_angle`] demands one orientation own the texture, this
/// reports every sharp peak holding a meaningful share of the edge energy, at least 20°
/// apart. It is meant to *propose* scan axes, not to classify: a caller scans along
/// each and lets the decoders' own validation decide.
pub fn gradient_angle_peaks(frame: &GrayFrame<'_>, max: usize) -> Vec<f32> {
    let Some(hist) = orientation_histogram(frame) else {
        return Vec::new();
    };
    let total: f32 = hist.iter().sum();
    let mut peaks: Vec<usize> = Vec::new();
    let mut out = Vec::new();
    while out.len() < max {
        let Some(peak) = smoothed_peak(&hist, &peaks) else {
            break;
        };
        if share(&hist, peak) / total < MIN_PEAK_SHARE {
            break;
        }
        peaks.push(peak);
        out.extend(refine_peak(&hist, peak));
    }
    out
}

/// Histogram of gradient orientation mod π weighted by squared magnitude, or `None` for
/// a frame too small or too flat to have one.
fn orientation_histogram(frame: &GrayFrame<'_>) -> Option<[f32; BINS]> {
    let w = frame.width();
    let h = frame.height();
    if w < 8 || h < 8 {
        return None;
    }

    // Histogram of gradient orientation mod π, weighted by squared magnitude.
    let mut hist = [0.0f32; BINS];
    let mut total = 0.0f32;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let gx =
                f32::from(frame.get_unchecked(x + 1, y)) - f32::from(frame.get_unchecked(x - 1, y));
            let gy =
                f32::from(frame.get_unchecked(x, y + 1)) - f32::from(frame.get_unchecked(x, y - 1));
            let mag2 = gx * gx + gy * gy;
            if mag2 < MIN_MAG2 {
                continue;
            }
            let a = gy.atan2(gx).rem_euclid(core::f32::consts::PI);
            let bin = ((a / core::f32::consts::PI) * BINS as f32) as usize % BINS;
            hist[bin] += mag2;
            total += mag2;
        }
    }
    (total > 0.0).then_some(hist)
}

/// Circular bin distance.
fn bin_distance(a: usize, b: usize) -> usize {
    let d = a.abs_diff(b);
    d.min(BINS - d)
}

/// The strongest bin of the lightly smoothed histogram that is at least 20° from every
/// bin in `taken`.
fn smoothed_peak(hist: &[f32; BINS], taken: &[usize]) -> Option<usize> {
    // Circular 1-2-1 smoothing so the peak does not split across a bin edge.
    let smooth =
        |i: usize| 0.25 * hist[(i + BINS - 1) % BINS] + 0.5 * hist[i] + 0.25 * hist[(i + 1) % BINS];
    let min_gap = BINS * 20 / 180;
    (0..BINS)
        .filter(|&i| taken.iter().all(|&t| bin_distance(i, t) >= min_gap))
        .max_by(|&a, &b| smooth(a).total_cmp(&smooth(b)))
}

/// Edge energy within ±2 bins of `peak`. Uses the raw histogram so smoothing cannot
/// inflate it.
fn share(hist: &[f32; BINS], peak: usize) -> f32 {
    (-2i32..=2)
        .map(|d| hist[(peak as i32 + d).rem_euclid(BINS as i32) as usize])
        .sum()
}

/// Sub-bin angle of the peak at bin `peak`.
fn refine_peak(hist: &[f32; BINS], peak: usize) -> Option<f32> {
    // Sub-bin angle: circular mean over the peak neighbourhood in double-angle space
    // (which keeps 0 and π identified, exactly the mod-π symmetry of orientations).
    let (mut sx, mut sy) = (0.0f64, 0.0f64);
    for d in -2i32..=2 {
        let i = (peak as i32 + d).rem_euclid(BINS as i32) as usize;
        let centre = (i as f64 + 0.5) / BINS as f64 * core::f64::consts::PI;
        sx += f64::from(hist[i]) * (2.0 * centre).cos();
        sy += f64::from(hist[i]) * (2.0 * centre).sin();
    }
    if sx == 0.0 && sy == 0.0 {
        return None;
    }
    let mut angle = (sy.atan2(sx) / 2.0) as f32; // (-π/2, π/2]
    if angle <= -core::f32::consts::FRAC_PI_2 {
        angle += core::f32::consts::PI;
    }
    Some(angle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::GrayImage;

    /// Bars of period 8 px perpendicular to `angle` (so the *reading* axis is `angle`),
    /// with soft (tanh-profile, ~1 px) edges. A camera never delivers hard staircase
    /// edges — real optics blur every transition — and aliased steps would put
    /// spurious diagonal energy into the gradient histogram that no real capture has.
    fn bars(angle: f32) -> GrayImage {
        let (s, c) = angle.sin_cos();
        let mut img = GrayImage::new(128, 128);
        for y in 0..128 {
            for x in 0..128 {
                // Position along the reading axis, phase within the 2-bar period.
                let t = c * x as f32 + s * y as f32;
                let ph = (t / 8.0).rem_euclid(2.0);
                // Distance (px) to the nearest bar edge; dark half is ph < 1.
                let d = ph.min((1.0 - ph).abs()).min(2.0 - ph) * 8.0;
                let soft = (d / 1.2).tanh();
                let side = if ph < 1.0 { -1.0 } else { 1.0 };
                img.set(x, y, (127.5 + 127.4 * side * soft).round() as u8);
            }
        }
        img
    }

    #[test]
    fn recovers_bar_orientation() {
        for deg in [0.0f32, 20.0, 45.0, 90.0, -30.0, -75.0] {
            let angle = deg.to_radians();
            let img = bars(angle);
            let got = dominant_gradient_angle(&img.as_frame())
                .unwrap_or_else(|| panic!("no orientation for {deg}°"));
            // Compare modulo π.
            let mut diff = (got - angle).rem_euclid(core::f32::consts::PI);
            if diff > core::f32::consts::FRAC_PI_2 {
                diff = core::f32::consts::PI - diff;
            }
            assert!(
                diff.to_degrees() < 4.0,
                "angle {deg}°: got {:.1}° (diff {:.1}°)",
                got.to_degrees(),
                diff.to_degrees()
            );
        }
    }

    #[test]
    fn rejects_isotropic_texture() {
        // A checkerboard has equal horizontal and vertical edge energy.
        let mut img = GrayImage::filled(96, 96, 255);
        for y in 0..96 {
            for x in 0..96 {
                if (x / 6 + y / 6) % 2 == 0 {
                    img.set(x, y, 0);
                }
            }
        }
        assert!(dominant_gradient_angle(&img.as_frame()).is_none());
    }
}
