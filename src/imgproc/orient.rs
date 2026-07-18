//! Dominant gradient-orientation estimation.
//!
//! A linear barcode is the most orientation-coherent texture there is: every bar edge
//! points the gradient along the *reading* axis, so the gradient-orientation histogram
//! of a barcode crop has one overwhelming peak (modulo 180° — a gradient cannot tell a
//! code from its upside-down self). [`dominant_gradient_angle`] measures that peak and
//! how dominant it is, letting a scanning pipeline *derotate* a crop whose code is not
//! within its scanline sweep instead of blindly re-scanning at every angle.

use crate::image::GrayFrame;

/// Histogram bins across the half-circle: 3° per bin.
const BINS: usize = 60;

/// Minimum squared gradient magnitude for a pixel to vote; rejects sensor noise and
/// flat background so the histogram is built from real edges.
const MIN_MAG2: f32 = 24.0 * 24.0;

/// Fraction of total edge energy that must fall within ±2 bins (±7.5°) of the peak for
/// the orientation to count as *dominant*. Barcodes score far above this; text, matrix
/// codes and scene texture spread their energy and fall below.
const MIN_COHERENCE: f32 = 0.4;

/// Estimate the dominant edge orientation of `frame` in radians, in `(-π/2, π/2]`,
/// measured from the +x axis — for a barcode this is the *reading* direction (bars are
/// perpendicular to it). Returns `None` when the texture has no sufficiently dominant
/// orientation (nothing bar-like to derotate for).
pub fn dominant_gradient_angle(frame: &GrayFrame<'_>) -> Option<f32> {
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
    if total <= 0.0 {
        return None;
    }

    // Light circular 1-2-1 smoothing so the peak does not split across a bin edge.
    let mut smooth = [0.0f32; BINS];
    for i in 0..BINS {
        let l = hist[(i + BINS - 1) % BINS];
        let r = hist[(i + 1) % BINS];
        smooth[i] = 0.25 * l + 0.5 * hist[i] + 0.25 * r;
    }
    let peak = (0..BINS)
        .max_by(|&a, &b| smooth[a].partial_cmp(&smooth[b]).unwrap())
        .unwrap();

    // Coherence: energy near the peak vs everywhere. Uses the raw histogram so the
    // smoothing above cannot inflate it.
    let near: f32 = (-2i32..=2)
        .map(|d| hist[(peak as i32 + d).rem_euclid(BINS as i32) as usize])
        .sum();
    if near / total < MIN_COHERENCE {
        return None;
    }

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
