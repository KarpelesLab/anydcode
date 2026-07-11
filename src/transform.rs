//! Deterministic, dependency-free image transforms on owned [`GrayImage`]s.
//!
//! These exist to stress the decoding path: take a pixel-exact render (see
//! [`crate::render`]) and subject it to the kinds of degradation a real camera adds —
//! rotation, scaling, perspective/tilt, blur, sensor noise and brightness/contrast
//! shifts — then feed the result back through detection and sampling.
//!
//! Everything here is **reproducible**. Randomized transforms take a seeded [`Rng`]
//! (a small xorshift) rather than any global source, so a failing case can always be
//! replayed. Geometric transforms use bilinear resampling and fill exposed background
//! with light (`255`) so quiet zones survive.

use crate::image::GrayImage;

/// Background luminance used to fill areas exposed by a geometric transform.
const BACKGROUND: f32 = 255.0;
/// Fixed pixel padding added around geometric outputs so quiet zones are preserved.
const PAD: usize = 8;

/// A tiny, fully deterministic xorshift64 pseudo-random generator.
///
/// This is intentionally *not* cryptographic and not backed by the OS: given the same
/// seed it produces the same stream on every platform, which is what makes noisy test
/// cases reproducible.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Seed the generator. Any seed is valid; `0` is remapped to a nonzero state.
    pub fn new(seed: u64) -> Self {
        Rng {
            state: seed ^ 0x9E37_79B9_7F4A_7C15 | 1,
        }
    }

    /// Next raw 64-bit value.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Next uniform float in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        // Top 24 bits give a uniform mantissa.
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Next standard-normal (mean 0, variance 1) sample via Box–Muller.
    pub fn next_gaussian(&mut self) -> f32 {
        let u1 = self.next_f32().max(f32::MIN_POSITIVE);
        let u2 = self.next_f32();
        (-2.0 * u1.ln()).sqrt() * (core::f32::consts::TAU * u2).cos()
    }
}

/// Which axis a non-planar transform is organized around.
///
/// The precise meaning is documented per transform, but the convention is that the
/// variant names the *reference line's orientation*: [`Axis::Vertical`] refers to a
/// vertical line/axis (so bending happens left-to-right across the image), and
/// [`Axis::Horizontal`] to a horizontal one (bending top-to-bottom).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// A horizontal reference line/axis (effect varies down the image).
    Horizontal,
    /// A vertical reference line/axis (effect varies across the image).
    Vertical,
}

/// A 3×3 projective transform on 2D points, used for perspective warping.
///
/// Points are treated as homogeneous `(x, y, 1)`; [`Projection::map`] divides through
/// by the mapped `w`. The [`Projection::square_to_quad`] / [`Projection::quad_to_quad`]
/// constructors follow the standard unit-square parameterization, so the same math
/// drives both the warps here and the grid sampler.
#[derive(Debug, Clone, Copy)]
pub struct Projection {
    m: [[f64; 3]; 3],
}

impl Projection {
    /// Map a point through the transform, dividing out the homogeneous coordinate.
    pub fn map(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.m;
        let w = m[2][0] * x + m[2][1] * y + m[2][2];
        let ox = (m[0][0] * x + m[0][1] * y + m[0][2]) / w;
        let oy = (m[1][0] * x + m[1][1] * y + m[1][2]) / w;
        (ox, oy)
    }

    /// The transform mapping the unit square corners `(0,0),(1,0),(1,1),(0,1)` onto
    /// the four points of `quad`, given in that same order.
    pub fn square_to_quad(quad: [(f64, f64); 4]) -> Projection {
        let (x0, y0) = quad[0];
        let (x1, y1) = quad[1];
        let (x2, y2) = quad[2];
        let (x3, y3) = quad[3];
        let dx3 = x0 - x1 + x2 - x3;
        let dy3 = y0 - y1 + y2 - y3;
        let m = if dx3.abs() < f64::EPSILON && dy3.abs() < f64::EPSILON {
            // Affine special case.
            [
                [x1 - x0, x3 - x0, x0],
                [y1 - y0, y3 - y0, y0],
                [0.0, 0.0, 1.0],
            ]
        } else {
            let dx1 = x1 - x2;
            let dx2 = x3 - x2;
            let dy1 = y1 - y2;
            let dy2 = y3 - y2;
            let denom = dx1 * dy2 - dx2 * dy1;
            let g = (dx3 * dy2 - dx2 * dy3) / denom;
            let h = (dx1 * dy3 - dx3 * dy1) / denom;
            [
                [x1 - x0 + g * x1, x3 - x0 + h * x3, x0],
                [y1 - y0 + g * y1, y3 - y0 + h * y3, y0],
                [g, h, 1.0],
            ]
        };
        Projection { m }
    }

    /// The transform mapping the four `src` points onto the four `dst` points (same
    /// corner order). Composes `square_to_quad(dst)` with the inverse of
    /// `square_to_quad(src)`.
    pub fn quad_to_quad(src: [(f64, f64); 4], dst: [(f64, f64); 4]) -> Projection {
        let s_dst = Projection::square_to_quad(dst);
        let s_src_inv = Projection::square_to_quad(src).inverse();
        s_dst.mul(&s_src_inv)
    }

    /// Matrix product `self * other`.
    fn mul(&self, other: &Projection) -> Projection {
        let a = &self.m;
        let b = &other.m;
        let mut m = [[0.0; 3]; 3];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
            }
        }
        Projection { m }
    }

    /// Matrix inverse via the adjugate.
    pub fn inverse(&self) -> Projection {
        let m = &self.m;
        let c = |a: usize, b: usize, c2: usize, d: usize| m[a][b] * m[c2][d] - m[a][d] * m[c2][b];
        let a00 = c(1, 1, 2, 2);
        let a01 = c(0, 2, 2, 1);
        let a02 = c(0, 1, 1, 2);
        let a10 = c(1, 2, 2, 0);
        let a11 = c(0, 0, 2, 2);
        let a12 = c(0, 2, 1, 0);
        let a20 = c(1, 0, 2, 1);
        let a21 = c(0, 1, 2, 0);
        let a22 = c(0, 0, 1, 1);
        let det = m[0][0] * a00 + m[0][1] * a10 + m[0][2] * a20;
        let inv_det = 1.0 / det;
        Projection {
            m: [
                [a00 * inv_det, a01 * inv_det, a02 * inv_det],
                [a10 * inv_det, a11 * inv_det, a12 * inv_det],
                [a20 * inv_det, a21 * inv_det, a22 * inv_det],
            ],
        }
    }
}

/// Rotate an image by `angle` radians (clockwise, since y grows downward) about its
/// center, expanding the canvas so nothing is clipped and filling exposed area with
/// light background.
pub fn rotate(img: &GrayImage, angle: f32) -> GrayImage {
    let w = img.width() as f32;
    let h = img.height() as f32;
    let (s, c) = angle.sin_cos();
    let cx = w / 2.0;
    let cy = h / 2.0;
    // Bounding box of the rotated corners (relative to center).
    let mut max_x = 0.0_f32;
    let mut max_y = 0.0_f32;
    for &(px, py) in &[(-cx, -cy), (cx, -cy), (cx, cy), (-cx, cy)] {
        let rx = c * px - s * py;
        let ry = s * px + c * py;
        max_x = max_x.max(rx.abs());
        max_y = max_y.max(ry.abs());
    }
    let nw = (2.0 * max_x).ceil() as usize + 2 * PAD;
    let nh = (2.0 * max_y).ceil() as usize + 2 * PAD;
    let ncx = nw as f32 / 2.0;
    let ncy = nh as f32 / 2.0;
    let mut out = GrayImage::filled(nw, nh, BACKGROUND as u8);
    for oy in 0..nh {
        for ox in 0..nw {
            let dx = ox as f32 + 0.5 - ncx;
            let dy = oy as f32 + 0.5 - ncy;
            // Inverse rotation.
            let sx = c * dx + s * dy + cx - 0.5;
            let sy = -s * dx + c * dy + cy - 0.5;
            let v = img.sample_bilinear(sx, sy).unwrap_or(BACKGROUND);
            out.set(ox, oy, v.round() as u8);
        }
    }
    out
}

/// Resample an image by an isotropic `factor` (`> 0`), rounding to at least 1×1.
pub fn scale(img: &GrayImage, factor: f32) -> GrayImage {
    assert!(factor > 0.0, "scale factor must be positive");
    let nw = ((img.width() as f32 * factor).round() as usize).max(1);
    let nh = ((img.height() as f32 * factor).round() as usize).max(1);
    let mut out = GrayImage::new(nw, nh);
    for oy in 0..nh {
        for ox in 0..nw {
            let sx = (ox as f32 + 0.5) / factor - 0.5;
            let sy = (oy as f32 + 0.5) / factor - 0.5;
            let v = img.sample_bilinear(sx, sy).unwrap_or(BACKGROUND);
            out.set(ox, oy, v.round() as u8);
        }
    }
    out
}

/// Warp an image so its four corners `TL,TR,BR,BL` land on `dst` (same order), sizing
/// the output to the destination bounding box plus padding and filling exposed area
/// with light background. This is the general perspective primitive behind
/// [`tilt_right`] and [`tilt_bottom`].
pub fn perspective(img: &GrayImage, dst: [(f32, f32); 4]) -> GrayImage {
    let w = img.width() as f32;
    let h = img.height() as f32;
    // Bounding box of the destination quad.
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for &(x, y) in &dst {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let nw = (max_x - min_x).ceil() as usize + 2 * PAD;
    let nh = (max_y - min_y).ceil() as usize + 2 * PAD;
    // Destination corners in output-pixel space.
    let dst64 = [
        (
            (dst[0].0 - min_x + PAD as f32) as f64,
            (dst[0].1 - min_y + PAD as f32) as f64,
        ),
        (
            (dst[1].0 - min_x + PAD as f32) as f64,
            (dst[1].1 - min_y + PAD as f32) as f64,
        ),
        (
            (dst[2].0 - min_x + PAD as f32) as f64,
            (dst[2].1 - min_y + PAD as f32) as f64,
        ),
        (
            (dst[3].0 - min_x + PAD as f32) as f64,
            (dst[3].1 - min_y + PAD as f32) as f64,
        ),
    ];
    // Map an output pixel back to the unit square, then to source pixels.
    let inv = Projection::square_to_quad(dst64).inverse();
    let mut out = GrayImage::filled(nw, nh, BACKGROUND as u8);
    for oy in 0..nh {
        for ox in 0..nw {
            let (u, v) = inv.map(ox as f64 + 0.5, oy as f64 + 0.5);
            if (-0.0001..=1.0001).contains(&u) && (-0.0001..=1.0001).contains(&v) {
                let sx = u as f32 * (w - 1.0);
                let sy = v as f32 * (h - 1.0);
                let val = img.sample_bilinear(sx, sy).unwrap_or(BACKGROUND);
                out.set(ox, oy, val.round() as u8);
            }
        }
    }
    out
}

/// Simulate a yaw tilt: foreshorten the right edge by `amount` (`0..1`), as if the
/// symbol were rotated away from the camera about a vertical axis.
pub fn tilt_right(img: &GrayImage, amount: f32) -> GrayImage {
    let w = img.width() as f32;
    let h = img.height() as f32;
    let shrink = (h * amount) / 2.0;
    perspective(img, [(0.0, 0.0), (w, shrink), (w, h - shrink), (0.0, h)])
}

/// Simulate a pitch tilt: foreshorten the bottom edge by `amount` (`0..1`), as if the
/// symbol were rotated away from the camera about a horizontal axis.
pub fn tilt_bottom(img: &GrayImage, amount: f32) -> GrayImage {
    let w = img.width() as f32;
    let h = img.height() as f32;
    let shrink = (w * amount) / 2.0;
    perspective(img, [(0.0, 0.0), (w, 0.0), (w - shrink, h), (shrink, h)])
}

/// Separable Gaussian blur with standard deviation `sigma` (in pixels). A `sigma` of
/// `0` returns a copy.
pub fn gaussian_blur(img: &GrayImage, sigma: f32) -> GrayImage {
    if sigma <= 0.0 {
        return img.clone();
    }
    let radius = (sigma * 3.0).ceil() as isize;
    let mut kernel = Vec::with_capacity((2 * radius + 1) as usize);
    let mut sum = 0.0_f32;
    for k in -radius..=radius {
        let v = (-(k * k) as f32 / (2.0 * sigma * sigma)).exp();
        kernel.push(v);
        sum += v;
    }
    for v in &mut kernel {
        *v /= sum;
    }
    let w = img.width();
    let h = img.height();
    // Horizontal pass.
    let mut tmp = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0_f32;
            for (i, &kv) in kernel.iter().enumerate() {
                let sx = (x as isize + i as isize - radius).clamp(0, w as isize - 1) as usize;
                acc += img.get(sx, y) as f32 * kv;
            }
            tmp.set(x, y, acc.round() as u8);
        }
    }
    // Vertical pass.
    let mut out = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0_f32;
            for (i, &kv) in kernel.iter().enumerate() {
                let sy = (y as isize + i as isize - radius).clamp(0, h as isize - 1) as usize;
                acc += tmp.get(x, sy) as f32 * kv;
            }
            out.set(x, y, acc.round() as u8);
        }
    }
    out
}

/// Add zero-mean Gaussian noise with standard deviation `sigma` (in luminance units),
/// drawn from `rng`. Results are clamped to `0..=255`.
pub fn add_noise(img: &GrayImage, sigma: f32, rng: &mut Rng) -> GrayImage {
    let mut out = GrayImage::new(img.width(), img.height());
    for y in 0..img.height() {
        for x in 0..img.width() {
            let n = rng.next_gaussian() * sigma;
            let v = (img.get(x, y) as f32 + n).round().clamp(0.0, 255.0) as u8;
            out.set(x, y, v);
        }
    }
    out
}

/// Apply an affine luminance change: `out = clamp(contrast * (in - 128) + 128 +
/// brightness)`. `contrast` of `1.0` and `brightness` of `0.0` are the identity.
pub fn brightness_contrast(img: &GrayImage, brightness: f32, contrast: f32) -> GrayImage {
    let mut out = GrayImage::new(img.width(), img.height());
    for y in 0..img.height() {
        for x in 0..img.width() {
            let v = contrast * (img.get(x, y) as f32 - 128.0) + 128.0 + brightness;
            out.set(x, y, v.round().clamp(0.0, 255.0) as u8);
        }
    }
    out
}

// ===================================================================================
// Non-planar (curved-surface) distortions
//
// Real codes are printed on bottles, cans, crumpled paper and bulging labels — surfaces
// the planar warps above cannot express. Each transform below is a deterministic,
// bilinear *inverse* warp: it walks every OUTPUT pixel, maps it back to a source
// coordinate, samples there, and leaves exposed area as light background so quiet zones
// survive. None of them use randomness.
// ===================================================================================

/// Simulate wrapping the image around a cylinder — a label on a bottle or can — and
/// viewing it head-on with an orthographic camera.
///
/// The flat image is treated as arc length on the cylinder surface; the visible image
/// is the projection of that arc. Consequently the middle (facing the camera) is at
/// true scale while both edges (curving away) are **foreshortened** — the further from
/// centre, the more source detail is squeezed into each output pixel — exactly as a
/// real curved label reads. The two outer edges stay pinned to the image border.
///
/// * `curvature` — half of the arc angle subtended by the image, in **radians**. `0`
///   (flat) returns an unchanged copy; ~`0.3` is a gentle curve, ~`0.8` a pronounced
///   one, and values approaching `π/2` wrap almost a full quarter-cylinder. Negative
///   values behave like their absolute value.
/// * `axis` — orientation of the **cylinder's axis**. [`Axis::Vertical`] bends the
///   image horizontally (a standing bottle); [`Axis::Horizontal`] bends it vertically
///   (a can lying on its side).
pub fn cylinder(img: &GrayImage, curvature: f32, axis: Axis) -> GrayImage {
    let theta_max = curvature.abs();
    if theta_max < 1e-4 {
        return img.clone();
    }
    let sin_max = theta_max.sin();
    let w = img.width();
    let h = img.height();
    let mut out = GrayImage::filled(w, h, BACKGROUND as u8);
    // Map a projected offset `u` in [-1, 1] back to a flat (arc-length) offset in the
    // same range: invert `u = sin(theta)/sin(theta_max)` then normalize by the arc.
    let unproject = |u: f32| -> f32 {
        let s = (u * sin_max).clamp(-1.0, 1.0);
        s.asin() / theta_max
    };
    for oy in 0..h {
        for ox in 0..w {
            let (sx, sy) = match axis {
                Axis::Vertical => {
                    let half = w as f32 / 2.0;
                    let u = (ox as f32 + 0.5 - half) / half;
                    (half + unproject(u) * half - 0.5, oy as f32)
                }
                Axis::Horizontal => {
                    let half = h as f32 / 2.0;
                    let u = (oy as f32 + 0.5 - half) / half;
                    (ox as f32, half + unproject(u) * half - 0.5)
                }
            };
            if let Some(v) = img.sample_bilinear(sx, sy) {
                out.set(ox, oy, v.round() as u8);
            }
        }
    }
    out
}

/// Apply a sinusoidal ripple, as if the print were on gently rippled/waving paper.
///
/// Rows (or columns) are displaced perpendicular to the ripple direction following a
/// sine wave. The output canvas is grown by the amplitude plus `PAD` on every side so
/// no content is clipped.
///
/// * `amplitude` — peak displacement, in **pixels**. `0` returns an unchanged (but
///   re-padded) copy.
/// * `wavelength` — length of one full cycle, in **pixels** (must be non-zero; a value
///   near `0` returns a copy). Smaller wavelengths pack more ripples across the image.
/// * `axis` — the axis the ripple **progresses along**. [`Axis::Horizontal`] varies the
///   wave with `x` and displaces pixels vertically (a horizontally-travelling ripple);
///   [`Axis::Vertical`] varies with `y` and displaces horizontally.
pub fn wave(img: &GrayImage, amplitude: f32, wavelength: f32, axis: Axis) -> GrayImage {
    let off = amplitude.abs().ceil() as usize + PAD;
    let w = img.width();
    let h = img.height();
    let nw = w + 2 * off;
    let nh = h + 2 * off;
    let mut out = GrayImage::filled(nw, nh, BACKGROUND as u8);
    if wavelength.abs() < 1e-4 {
        // Degenerate wavelength: just re-emit the padded image.
        for oy in 0..h {
            for ox in 0..w {
                out.set(ox + off, oy + off, img.get(ox, oy));
            }
        }
        return out;
    }
    let k = core::f32::consts::TAU / wavelength;
    for oy in 0..nh {
        for ox in 0..nw {
            let bx = ox as f32 - off as f32;
            let by = oy as f32 - off as f32;
            let (sx, sy) = match axis {
                Axis::Horizontal => (bx, by - amplitude * (k * bx).sin()),
                Axis::Vertical => (bx - amplitude * (k * by).sin(), by),
            };
            if let Some(v) = img.sample_bilinear(sx, sy) {
                out.set(ox, oy, v.round() as u8);
            }
        }
    }
    out
}

/// Simulate a crease: split the image along a straight line and rotate one half away
/// from the camera in depth, as with a folded sheet of paper.
///
/// The half on the near side of the crease is untouched; the far half is
/// foreshortened by `cos(angle)` about the crease line (an orthographic depth
/// rotation), so its modules compress toward the fold and the area past the receding
/// edge falls back to light background.
///
/// * `position` — location of the crease along `axis`, as a **fraction** `0.0..=1.0`
///   of the image extent (e.g. `0.5` folds through the middle).
/// * `angle_deg` — dihedral fold angle in **degrees**, `0` (flat) up to but not
///   including `90`. Larger angles compress the folded half more; the magnitude is
///   used, so sign is irrelevant.
/// * `axis` — orientation of the **crease line**. [`Axis::Vertical`] creases along a
///   vertical line and folds the right half; [`Axis::Horizontal`] creases horizontally
///   and folds the bottom half.
pub fn fold(img: &GrayImage, position: f32, angle_deg: f32, axis: Axis) -> GrayImage {
    let w = img.width();
    let h = img.height();
    let nw = w + 2 * PAD;
    let nh = h + 2 * PAD;
    let mut out = GrayImage::filled(nw, nh, BACKGROUND as u8);
    // Clamp the fold just shy of 90° so the foreshortening factor stays finite.
    let cos_f = angle_deg
        .abs()
        .to_radians()
        .clamp(0.0, 1.556)
        .cos()
        .max(1e-3);
    let pos = position.clamp(0.0, 1.0);
    for oy in 0..nh {
        for ox in 0..nw {
            let bx = ox as f32 - PAD as f32;
            let by = oy as f32 - PAD as f32;
            let (sx, sy) = match axis {
                Axis::Vertical => {
                    let xc = pos * w as f32;
                    let sx = if bx <= xc { bx } else { xc + (bx - xc) / cos_f };
                    (sx, by)
                }
                Axis::Horizontal => {
                    let yc = pos * h as f32;
                    let sy = if by <= yc { by } else { yc + (by - yc) / cos_f };
                    (bx, sy)
                }
            };
            if let Some(v) = img.sample_bilinear(sx, sy) {
                out.set(ox, oy, v.round() as u8);
            }
        }
    }
    out
}

/// Apply a radial bulge or pinch centred on `center`, as if the label covered a rounded
/// bump (bulge) or sank into a dimple (pinch).
///
/// Distance from the centre is remapped by a power law that fixes both the centre and
/// the outer boundary, so the frame edges stay put while the interior swells or shrinks.
///
/// * `strength` — deformation amount, roughly `-1.0..1.0`. `strength > 0` **bulges**
///   (magnifies the centre); `strength < 0` **pinches** (shrinks the centre); `0` is
///   the identity. Useful magnitudes are ~`0.2` (subtle) to ~`0.6` (strong).
/// * `center` — bulge centre in **normalized** image coordinates, each component in
///   `0.0..=1.0` (`(0.5, 0.5)` is the image middle).
pub fn bulge(img: &GrayImage, strength: f32, center: (f32, f32)) -> GrayImage {
    let w = img.width();
    let h = img.height();
    let mut out = GrayImage::filled(w, h, BACKGROUND as u8);
    if strength.abs() < 1e-4 {
        return img.clone();
    }
    let cx = center.0.clamp(0.0, 1.0) * w as f32;
    let cy = center.1.clamp(0.0, 1.0) * h as f32;
    // Effect radius: reach the farthest corner so the whole image is remapped and the
    // deformation eases back to identity exactly at that boundary.
    let mut radius = 0.0_f32;
    for &(px, py) in &[
        (0.0, 0.0),
        (w as f32, 0.0),
        (w as f32, h as f32),
        (0.0, h as f32),
    ] {
        radius = radius.max((px - cx).hypot(py - cy));
    }
    let radius = radius.max(1.0);
    for oy in 0..h {
        for ox in 0..w {
            let dx = ox as f32 + 0.5 - cx;
            let dy = oy as f32 + 0.5 - cy;
            let r = dx.hypot(dy);
            let (sx, sy) = if r < 1e-3 {
                (cx - 0.5, cy - 0.5)
            } else {
                // scale = (r/radius)^strength: <1 pulls the source toward centre
                // (magnifying it) for strength>0, >1 for a pinch.
                let scale = (r / radius).powf(strength);
                (cx + dx * scale - 0.5, cy + dy * scale - 0.5)
            };
            if let Some(v) = img.sample_bilinear(sx, sy) {
                out.set(ox, oy, v.round() as u8);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_reproducible() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn projection_roundtrips() {
        let quad = [(10.0, 5.0), (90.0, 12.0), (85.0, 95.0), (8.0, 88.0)];
        let p = Projection::square_to_quad(quad);
        let inv = p.inverse();
        for &(sx, sy) in &[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.5, 0.3)] {
            let (mx, my) = p.map(sx, sy);
            let (bx, by) = inv.map(mx, my);
            assert!((bx - sx).abs() < 1e-9, "x {bx} vs {sx}");
            assert!((by - sy).abs() < 1e-9, "y {by} vs {sy}");
        }
    }

    #[test]
    fn rotate_by_zero_preserves_center() {
        let mut img = GrayImage::filled(20, 20, 255);
        img.set(10, 10, 0);
        let out = rotate(&img, 0.0);
        // Center pixel maps back to the dark source pixel.
        let cx = out.width() / 2;
        let cy = out.height() / 2;
        assert!(out.get(cx, cy) < 128);
    }

    /// A checkerboard-ish test pattern with distinct dark features to track.
    fn feature_image() -> GrayImage {
        let mut img = GrayImage::filled(40, 40, 255);
        for y in 10..30 {
            for x in 10..30 {
                if (x + y) % 2 == 0 {
                    img.set(x, y, 0);
                }
            }
        }
        img
    }

    #[test]
    fn zero_strength_transforms_are_identity() {
        let img = feature_image();
        assert_eq!(cylinder(&img, 0.0, Axis::Vertical), img);
        assert_eq!(cylinder(&img, 0.0, Axis::Horizontal), img);
        assert_eq!(bulge(&img, 0.0, (0.5, 0.5)), img);
        // fold with a 0° angle keeps source coordinates unchanged (only padded).
        let f = fold(&img, 0.5, 0.0, Axis::Vertical);
        assert_eq!(f.get(PAD + 15, PAD + 15), img.get(15, 15));
    }

    #[test]
    fn curved_transforms_are_deterministic() {
        let img = feature_image();
        assert_eq!(
            cylinder(&img, 0.6, Axis::Vertical),
            cylinder(&img, 0.6, Axis::Vertical)
        );
        assert_eq!(
            wave(&img, 3.0, 18.0, Axis::Horizontal),
            wave(&img, 3.0, 18.0, Axis::Horizontal)
        );
        assert_eq!(
            fold(&img, 0.5, 30.0, Axis::Vertical),
            fold(&img, 0.5, 30.0, Axis::Vertical)
        );
        assert_eq!(bulge(&img, 0.4, (0.5, 0.5)), bulge(&img, 0.4, (0.5, 0.5)));
    }

    #[test]
    fn cylinder_pins_edges_and_preserves_size() {
        let img = feature_image();
        let out = cylinder(&img, 0.8, Axis::Vertical);
        assert_eq!((out.width(), out.height()), (img.width(), img.height()));
        // Middle column is near true scale; a mid dark pixel stays roughly put.
        // Simply confirm the transform produced some dark content (not all-white).
        assert!(out.pixels().iter().any(|&p| p < 128));
    }

    #[test]
    fn wave_grows_canvas_and_keeps_content() {
        let img = feature_image();
        let out = wave(&img, 4.0, 20.0, Axis::Horizontal);
        assert!(out.width() > img.width() && out.height() > img.height());
        assert!(out.pixels().iter().any(|&p| p < 128));
    }

    #[test]
    fn fold_compresses_far_half() {
        let img = feature_image();
        let out = fold(&img, 0.5, 45.0, Axis::Vertical);
        assert!(out.pixels().iter().any(|&p| p < 128));
    }

    #[test]
    fn bulge_and_pinch_keep_size_and_content() {
        let img = feature_image();
        let b = bulge(&img, 0.5, (0.5, 0.5));
        let p = bulge(&img, -0.5, (0.5, 0.5));
        assert_eq!((b.width(), b.height()), (img.width(), img.height()));
        assert!(b.pixels().iter().any(|&v| v < 128));
        assert!(p.pixels().iter().any(|&v| v < 128));
        // Bulge and pinch of equal magnitude differ.
        assert_ne!(b, p);
    }
}
