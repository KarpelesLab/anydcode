//! A 3×3 projective transform (homography) between two planes.
//!
//! Sampling a matrix code needs a map from *module-grid* coordinates to *image*
//! pixels. Under perspective this map is a homography, recoverable from four
//! point correspondences (the code's four corners). [`Homography::from_correspondences`]
//! solves the 8-unknown linear system directly; [`Homography::map`] applies the
//! forward map and [`Homography::inverse`] gives the pixel→grid direction.
//!
//! All coordinates use [`Point`] (`f32`); the solve itself runs in `f64` for
//! numerical headroom.

use crate::error::{Error, Result};
use crate::geometry::Point;

/// A 3×3 projective transform, stored row-major as `[m00, m01, m02, m10, …, m22]`.
///
/// A point `(x, y)` maps to `(X/W, Y/W)` where
/// `X = m00·x + m01·y + m02`, `Y = m10·x + m11·y + m12`, `W = m20·x + m21·y + m22`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography {
    m: [f64; 9],
}

impl Homography {
    /// The identity transform.
    pub fn identity() -> Self {
        Homography {
            m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    /// Build the raw matrix directly (row-major). Mostly for tests and composition.
    pub fn from_matrix(m: [f64; 9]) -> Self {
        Homography { m }
    }

    /// Raw row-major matrix coefficients.
    pub fn matrix(&self) -> [f64; 9] {
        self.m
    }

    /// Solve the homography mapping the four `src` points to the four `dst` points.
    ///
    /// The correspondences must be in matching order and no three source points may
    /// be collinear (otherwise the system is singular).
    ///
    /// # Errors
    /// Returns [`Error::InvalidParameter`] if the configuration is degenerate and the
    /// linear system cannot be solved.
    pub fn from_correspondences(src: [Point; 4], dst: [Point; 4]) -> Result<Self> {
        // Each correspondence contributes two rows to A·h = b, with h = the first 8
        // matrix entries and m22 fixed to 1.
        let mut a = [[0.0f64; 8]; 8];
        let mut b = [0.0f64; 8];
        for i in 0..4 {
            let sx = f64::from(src[i].x);
            let sy = f64::from(src[i].y);
            let dx = f64::from(dst[i].x);
            let dy = f64::from(dst[i].y);

            let r0 = 2 * i;
            a[r0] = [sx, sy, 1.0, 0.0, 0.0, 0.0, -dx * sx, -dx * sy];
            b[r0] = dx;

            let r1 = 2 * i + 1;
            a[r1] = [0.0, 0.0, 0.0, sx, sy, 1.0, -dy * sx, -dy * sy];
            b[r1] = dy;
        }

        let h = solve8(&mut a, &mut b)
            .ok_or_else(|| Error::invalid_parameter("degenerate homography correspondences"))?;

        Ok(Homography {
            m: [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0],
        })
    }

    /// Build the homography mapping the unit square `(0,0),(1,0),(1,1),(0,1)` to the
    /// four `dst` corners (clockwise from top-left). Convenient for grid sampling
    /// where module-grid space is normalized to a unit square.
    ///
    /// # Errors
    /// As [`Homography::from_correspondences`].
    pub fn from_unit_square(dst: [Point; 4]) -> Result<Self> {
        let src = [
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(1.0, 1.0),
            Point::new(0.0, 1.0),
        ];
        Self::from_correspondences(src, dst)
    }

    /// Apply the forward map to a point.
    pub fn map(&self, p: Point) -> Point {
        let (x, y) = (f64::from(p.x), f64::from(p.y));
        let m = &self.m;
        let xw = m[0] * x + m[1] * y + m[2];
        let yw = m[3] * x + m[4] * y + m[5];
        let w = m[6] * x + m[7] * y + m[8];
        Point::new((xw / w) as f32, (yw / w) as f32)
    }

    /// Apply the forward map to explicit `f64` coordinates, returning `f64` for use
    /// inside numeric loops (e.g. rendering/sampling) without the `f32` round-trip.
    pub fn map_f64(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.m;
        let xw = m[0] * x + m[1] * y + m[2];
        let yw = m[3] * x + m[4] * y + m[5];
        let w = m[6] * x + m[7] * y + m[8];
        (xw / w, yw / w)
    }

    /// The inverse transform (image→grid if this is grid→image).
    ///
    /// # Errors
    /// Returns [`Error::InvalidParameter`] if the matrix is singular.
    pub fn inverse(&self) -> Result<Homography> {
        let m = &self.m;
        // Cofactor / adjugate inverse. The homogeneous scale is irrelevant, but we
        // normalize by the determinant for well-conditioned coefficients.
        let c00 = m[4] * m[8] - m[5] * m[7];
        let c01 = m[5] * m[6] - m[3] * m[8];
        let c02 = m[3] * m[7] - m[4] * m[6];
        let det = m[0] * c00 + m[1] * c01 + m[2] * c02;
        if det.abs() < 1e-12 {
            return Err(Error::invalid_parameter("singular homography"));
        }
        let inv_det = 1.0 / det;
        // adjugate = transpose of cofactor matrix.
        let inv = [
            c00 * inv_det,
            (m[2] * m[7] - m[1] * m[8]) * inv_det,
            (m[1] * m[5] - m[2] * m[4]) * inv_det,
            c01 * inv_det,
            (m[0] * m[8] - m[2] * m[6]) * inv_det,
            (m[2] * m[3] - m[0] * m[5]) * inv_det,
            c02 * inv_det,
            (m[1] * m[6] - m[0] * m[7]) * inv_det,
            (m[0] * m[4] - m[1] * m[3]) * inv_det,
        ];
        Ok(Homography { m: inv })
    }
}

/// Solve the 8×8 system `a · x = b` by Gaussian elimination with partial pivoting.
/// Returns `None` if the matrix is singular. Consumes (mutates) `a` and `b`.
fn solve8(a: &mut [[f64; 8]; 8], b: &mut [f64; 8]) -> Option<[f64; 8]> {
    const N: usize = 8;
    for col in 0..N {
        // Partial pivot: find the row with the largest magnitude in this column.
        let mut pivot = col;
        let mut best = a[col][col].abs();
        for (row, r) in a.iter().enumerate().skip(col + 1) {
            let v = r[col].abs();
            if v > best {
                best = v;
                pivot = row;
            }
        }
        if best < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);

        // Eliminate below. Copy the pivot row (an array is `Copy`) so the target
        // rows can be mutated without aliasing it.
        let pivot_row = a[col];
        let pivot_b = b[col];
        for row in (col + 1)..N {
            let factor = a[row][col] / pivot_row[col];
            if factor != 0.0 {
                for (k, ak) in a[row].iter_mut().enumerate().skip(col) {
                    *ak -= factor * pivot_row[k];
                }
                b[row] -= factor * pivot_b;
            }
        }
    }

    // Back-substitution.
    let mut x = [0.0f64; N];
    for i in (0..N).rev() {
        let mut acc = b[i];
        for k in (i + 1)..N {
            acc -= a[i][k] * x[k];
        }
        x[i] = acc / a[i][i];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point, eps: f32) -> bool {
        (a.x - b.x).abs() < eps && (a.y - b.y).abs() < eps
    }

    #[test]
    fn identity_maps_unchanged() {
        let h = Homography::identity();
        let p = Point::new(3.5, -2.0);
        assert!(close(h.map(p), p, 1e-6));
    }

    #[test]
    fn maps_unit_square_to_quad() {
        let dst = [
            Point::new(10.0, 20.0),
            Point::new(110.0, 25.0),
            Point::new(100.0, 130.0),
            Point::new(15.0, 120.0),
        ];
        let h = Homography::from_unit_square(dst).unwrap();
        // Corners land on the destination quad.
        assert!(close(h.map(Point::new(0.0, 0.0)), dst[0], 1e-3));
        assert!(close(h.map(Point::new(1.0, 0.0)), dst[1], 1e-3));
        assert!(close(h.map(Point::new(1.0, 1.0)), dst[2], 1e-3));
        assert!(close(h.map(Point::new(0.0, 1.0)), dst[3], 1e-3));
    }

    #[test]
    fn inverse_round_trips() {
        let dst = [
            Point::new(12.0, 8.0),
            Point::new(200.0, 30.0),
            Point::new(190.0, 210.0),
            Point::new(30.0, 180.0),
        ];
        let h = Homography::from_unit_square(dst).unwrap();
        let hi = h.inverse().unwrap();
        // Grid point -> image -> back to grid.
        for &(gx, gy) in &[(0.25f32, 0.75f32), (0.5, 0.5), (0.9, 0.1)] {
            let g = Point::new(gx, gy);
            let round = hi.map(h.map(g));
            assert!(close(round, g, 1e-3), "round trip failed for {g:?}");
        }
        // And the other direction from an image point.
        let img = Point::new(100.0, 90.0);
        let round = h.map(hi.map(img));
        assert!(close(round, img, 1e-3));
    }

    #[test]
    fn perspective_is_genuinely_nonaffine() {
        // A trapezoid forces a non-zero perspective term, so the midpoint of the top
        // edge does not map to the midpoint of grid (0.5, 0): verify the map is not
        // a plain affine transform.
        let dst = [
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            Point::new(70.0, 100.0),
            Point::new(30.0, 100.0),
        ];
        let h = Homography::from_unit_square(dst).unwrap();
        let mid_bottom = h.map(Point::new(0.5, 1.0));
        assert!((mid_bottom.x - 50.0).abs() < 1e-3);
        let mid = h.map(Point::new(0.5, 0.5));
        // Under pure affine the centre x would be 50; perspective pulls it, but by
        // symmetry of this trapezoid it stays 50 — instead check the y compression.
        assert!((mid.x - 50.0).abs() < 1e-3);
        assert!(mid.y > 50.0, "perspective should compress the far edge");
    }

    #[test]
    fn degenerate_correspondences_error() {
        // Three collinear source points -> singular.
        let src = [
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(2.0, 0.0),
            Point::new(0.0, 1.0),
        ];
        let dst = [
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(2.0, 0.0),
            Point::new(0.0, 1.0),
        ];
        assert!(Homography::from_correspondences(src, dst).is_err());
    }

    #[test]
    fn singular_matrix_inverse_errors() {
        let h = Homography::from_matrix([1.0, 2.0, 3.0, 2.0, 4.0, 6.0, 1.0, 1.0, 1.0]);
        assert!(h.inverse().is_err());
    }
}
