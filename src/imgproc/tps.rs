//! Thin-plate-spline scattered-data interpolation and a general dense linear solver.
//!
//! A [`Homography`](crate::imgproc::Homography) maps one plane onto another and so can
//! only model a *flat* surface under perspective. A code printed on a bottle, a can, a
//! folded flyer or a rippled sheet lies on a curved surface no single homography fits.
//! Given a set of known correspondences — for a QR symbol its finder and alignment
//! patterns — a **thin-plate spline (TPS)** interpolates a smooth non-planar warp that
//! passes through every one of them and bends gracefully in between.
//!
//! The surface is modelled per output axis: two scalar TPS functions of the source
//! coordinates, one predicting the image `x` of a source point and one its image `y`.
//! Each is `f(p) = a0 + a1·px + a2·py + Σ wᵢ·U(‖p − cᵢ‖)` with the biharmonic kernel
//! `U(r) = r²·ln r`; the coefficients solve the `(N+3)×(N+3)` system that forces the
//! spline through all `N` control values while keeping its bending energy minimal.
//! With few control points it degrades to a near-affine fit, so it is safe to use even
//! when only a handful of anchors are found.

/// Solve the dense linear system `a · x = b` for one or more right-hand sides.
///
/// `a` is an `n×n` matrix (row-major, `a[row][col]`) and `b` is `n×m` (`m` right-hand
/// sides as columns). Solves by Gaussian elimination with partial pivoting, consuming
/// both inputs, and returns the `n×m` solution, or `None` if `a` is singular.
pub fn solve_linear_system(mut a: Vec<Vec<f64>>, mut b: Vec<Vec<f64>>) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    if n == 0 || a.iter().any(|r| r.len() != n) || b.len() != n {
        return None;
    }
    let m = b[0].len();
    if b.iter().any(|r| r.len() != m) {
        return None;
    }

    for col in 0..n {
        // Partial pivot: the row with the largest magnitude in this column.
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

        // Copy the pivot rows out so the target rows can be updated without aliasing.
        let pivot_row = a[col].clone();
        let pivot_b = b[col].clone();
        let piv = pivot_row[col];
        for (row_a, row_b) in a.iter_mut().zip(b.iter_mut()).skip(col + 1) {
            let factor = row_a[col] / piv;
            if factor != 0.0 {
                for (ak, pk) in row_a.iter_mut().zip(pivot_row.iter()).skip(col) {
                    *ak -= factor * pk;
                }
                for (bk, pbk) in row_b.iter_mut().zip(pivot_b.iter()) {
                    *bk -= factor * pbk;
                }
            }
        }
    }

    // Back-substitution for every right-hand side at once.
    let mut x = vec![vec![0.0f64; m]; n];
    for i in (0..n).rev() {
        for k in 0..m {
            let mut acc = b[i][k];
            for j in (i + 1)..n {
                acc -= a[i][j] * x[j][k];
            }
            x[i][k] = acc / a[i][i];
        }
    }
    Some(x)
}

/// The TPS biharmonic kernel `U(r) = r²·ln r`, with `U(0) = 0`.
#[inline]
fn kernel(r2: f64) -> f64 {
    if r2 <= 0.0 {
        0.0
    } else {
        // r²·ln r = 0.5·r²·ln(r²); using the squared distance avoids a `sqrt`.
        0.5 * r2 * r2.ln()
    }
}

/// A smooth non-planar map from a 2D source plane to a 2D destination plane, fitted to
/// pass through a set of control-point correspondences.
///
/// Built with [`ThinPlateSpline::fit`] and evaluated with [`ThinPlateSpline::map`]. For
/// the QR sampler the source plane is module-grid space and the destination is image
/// pixels, so a warped code samples correctly where a single homography cannot.
#[derive(Debug, Clone)]
pub struct ThinPlateSpline {
    /// Source-space control points `cᵢ`.
    ctrl: Vec<(f64, f64)>,
    /// Coefficients `[w₀..w_{N-1}, a0, a1, a2]` for the destination-x spline.
    coef_x: Vec<f64>,
    /// Coefficients `[w₀..w_{N-1}, a0, a1, a2]` for the destination-y spline.
    coef_y: Vec<f64>,
}

impl ThinPlateSpline {
    /// Fit a spline mapping each `src[i]` to `dst[i]`.
    ///
    /// Needs at least three non-collinear control points; returns `None` if there are
    /// fewer than three correspondences, the two slices differ in length, or the system
    /// is singular (e.g. all control points collinear).
    pub fn fit(src: &[(f64, f64)], dst: &[(f64, f64)]) -> Option<Self> {
        let n = src.len();
        if n < 3 || dst.len() != n {
            return None;
        }
        let size = n + 3;
        let mut a = vec![vec![0.0f64; size]; size];

        // K block: pairwise kernel of the control points.
        for i in 0..n {
            for j in 0..n {
                let dx = src[i].0 - src[j].0;
                let dy = src[i].1 - src[j].1;
                a[i][j] = kernel(dx * dx + dy * dy);
            }
        }
        // P and Pᵀ blocks: the affine part `[1, px, py]`.
        for i in 0..n {
            a[i][n] = 1.0;
            a[i][n + 1] = src[i].0;
            a[i][n + 2] = src[i].1;
            a[n][i] = 1.0;
            a[n + 1][i] = src[i].0;
            a[n + 2][i] = src[i].1;
        }
        // Bottom-right 3×3 block is left zero.

        // Two right-hand sides: destination x and destination y. The last three rows
        // (the affine constraints) are zero.
        let mut b = vec![vec![0.0f64; 2]; size];
        for i in 0..n {
            b[i][0] = dst[i].0;
            b[i][1] = dst[i].1;
        }

        let sol = solve_linear_system(a, b)?;
        let coef_x = sol.iter().map(|r| r[0]).collect();
        let coef_y = sol.iter().map(|r| r[1]).collect();
        Some(ThinPlateSpline {
            ctrl: src.to_vec(),
            coef_x,
            coef_y,
        })
    }

    /// Map a source point to its destination through the fitted warp.
    pub fn map(&self, x: f64, y: f64) -> (f64, f64) {
        let n = self.ctrl.len();
        let eval = |coef: &[f64]| {
            let mut v = coef[n] + coef[n + 1] * x + coef[n + 2] * y;
            for (i, &(cx, cy)) in self.ctrl.iter().enumerate() {
                let dx = x - cx;
                let dy = y - cy;
                v += coef[i] * kernel(dx * dx + dy * dy);
            }
            v
        };
        (eval(&self.coef_x), eval(&self.coef_y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_simple_system() {
        // 2x + y = 5 ; x - y = 1  ->  x = 2, y = 1.
        let a = vec![vec![2.0, 1.0], vec![1.0, -1.0]];
        let b = vec![vec![5.0], vec![1.0]];
        let x = solve_linear_system(a, b).unwrap();
        assert!((x[0][0] - 2.0).abs() < 1e-9);
        assert!((x[1][0] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn singular_system_is_none() {
        let a = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        let b = vec![vec![1.0], vec![2.0]];
        assert!(solve_linear_system(a, b).is_none());
    }

    #[test]
    fn interpolates_through_every_anchor() {
        // An arbitrary curved mapping sampled at scattered points; the spline must
        // reproduce it exactly at those points.
        let src = [
            (0.0, 0.0),
            (10.0, 0.0),
            (0.0, 10.0),
            (10.0, 10.0),
            (5.0, 5.0),
            (3.0, 8.0),
        ];
        let f = |x: f64, y: f64| (x + 0.05 * y * y, y - 0.03 * x * x);
        let dst: Vec<(f64, f64)> = src.iter().map(|&(x, y)| f(x, y)).collect();
        let tps = ThinPlateSpline::fit(&src, &dst).unwrap();
        for (&s, &d) in src.iter().zip(dst.iter()) {
            let (mx, my) = tps.map(s.0, s.1);
            assert!((mx - d.0).abs() < 1e-6, "x at {s:?}: {mx} vs {}", d.0);
            assert!((my - d.1).abs() < 1e-6, "y at {s:?}: {my} vs {}", d.1);
        }
    }

    #[test]
    fn reproduces_affine_maps_exactly() {
        // A pure affine map lies in the TPS null space, so it is reproduced everywhere,
        // not only at the anchors.
        let src = [(0.0, 0.0), (4.0, 1.0), (1.0, 5.0), (6.0, 6.0)];
        let affine = |x: f64, y: f64| (2.0 * x + 0.5 * y + 3.0, -0.5 * x + 1.5 * y - 1.0);
        let dst: Vec<(f64, f64)> = src.iter().map(|&(x, y)| affine(x, y)).collect();
        let tps = ThinPlateSpline::fit(&src, &dst).unwrap();
        for &(x, y) in &[(2.5, 3.5), (10.0, -4.0), (0.0, 7.0)] {
            let (mx, my) = tps.map(x, y);
            let (ex, ey) = affine(x, y);
            assert!((mx - ex).abs() < 1e-6);
            assert!((my - ey).abs() < 1e-6);
        }
    }
}
