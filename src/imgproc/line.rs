//! Line fitting: total-least-squares and a seeded RANSAC edge fitter.
//!
//! Locating a code's borders means fitting straight lines to noisy edge points.
//! [`fit_line_least_squares`] gives the best orthogonal-distance fit to all points;
//! [`ransac_line`] tolerates outliers by sampling minimal two-point models with a
//! caller-supplied [`Prng`], keeping the whole thing reproducible.
//!
//! Lines are stored in normalized implicit form `a·x + b·y + c = 0` with
//! `a² + b² = 1`, so [`Line::distance`] is a true signed perpendicular distance.

use crate::geometry::Point;
use crate::imgproc::rng::Prng;

/// A 2D line in normalized implicit form `a·x + b·y + c = 0` with `a² + b² = 1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    /// `x` coefficient of the unit normal.
    pub a: f32,
    /// `y` coefficient of the unit normal.
    pub b: f32,
    /// Offset term.
    pub c: f32,
}

impl Line {
    /// The line through two distinct points, or `None` if they coincide.
    pub fn from_points(p1: Point, p2: Point) -> Option<Line> {
        let dx = p2.x - p1.x;
        let dy = p2.y - p1.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < f32::EPSILON {
            return None;
        }
        // Normal is perpendicular to the direction (dx, dy).
        let a = dy / len;
        let b = -dx / len;
        let c = -(a * p1.x + b * p1.y);
        Some(Line { a, b, c })
    }

    /// Signed perpendicular distance from `p` to the line.
    pub fn distance(&self, p: Point) -> f32 {
        self.a * p.x + self.b * p.y + self.c
    }
}

/// Total-least-squares line fit minimizing squared *perpendicular* distances.
///
/// Returns `None` for fewer than two points or a fully degenerate (single-point)
/// cluster. Unlike an `y = mx + b` regression this handles vertical lines and treats
/// x and y symmetrically.
pub fn fit_line_least_squares(points: &[Point]) -> Option<Line> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len() as f64;
    let mut mx = 0.0f64;
    let mut my = 0.0f64;
    for p in points {
        mx += f64::from(p.x);
        my += f64::from(p.y);
    }
    mx /= n;
    my /= n;

    let mut sxx = 0.0f64;
    let mut syy = 0.0f64;
    let mut sxy = 0.0f64;
    for p in points {
        let dx = f64::from(p.x) - mx;
        let dy = f64::from(p.y) - my;
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }

    if sxx + syy < 1e-12 {
        return None; // all points coincide
    }

    // The line normal is the eigenvector of the smallest eigenvalue of the 2×2
    // scatter matrix [[sxx, sxy], [sxy, syy]].
    let diff = sxx - syy;
    let disc = (diff * diff + 4.0 * sxy * sxy).sqrt();
    let lambda_min = 0.5 * (sxx + syy - disc);

    // Solve (M - lambda_min·I)·n = 0. Pick the better-conditioned row.
    let (mut a, mut b) = if (sxx - lambda_min).abs() > (syy - lambda_min).abs() {
        (-sxy, sxx - lambda_min)
    } else {
        (syy - lambda_min, -sxy)
    };
    let len = (a * a + b * b).sqrt();
    if len < 1e-12 {
        // Degenerate isotropic scatter; fall back to an axis-aligned normal.
        a = 1.0;
        b = 0.0;
    } else {
        a /= len;
        b /= len;
    }
    let c = -(a * mx + b * my);
    Some(Line {
        a: a as f32,
        b: b as f32,
        c: c as f32,
    })
}

/// Fit a line to `points` by RANSAC, tolerating outliers.
///
/// Runs `iterations` rounds, each sampling two distinct points, and keeps the model
/// with the most inliers (points within `threshold` perpendicular distance). The
/// winning model is refined by a least-squares fit over its inliers. Randomness is
/// drawn from `rng`, so results are fully reproducible for a given seed.
///
/// Returns the fitted [`Line`] together with the indices of its inliers, or `None`
/// if fewer than two points are supplied.
pub fn ransac_line(
    points: &[Point],
    iterations: usize,
    threshold: f32,
    rng: &mut Prng,
) -> Option<(Line, Vec<usize>)> {
    if points.len() < 2 {
        return None;
    }
    let thr = threshold.abs();
    let mut best_line: Option<Line> = None;
    let mut best_inliers: Vec<usize> = Vec::new();

    for _ in 0..iterations.max(1) {
        let i = rng.below(points.len());
        let mut j = rng.below(points.len());
        if j == i {
            j = (j + 1) % points.len();
        }
        let Some(candidate) = Line::from_points(points[i], points[j]) else {
            continue;
        };
        let inliers: Vec<usize> = points
            .iter()
            .enumerate()
            .filter(|&(_, &p)| candidate.distance(p).abs() <= thr)
            .map(|(idx, _)| idx)
            .collect();
        if inliers.len() > best_inliers.len() {
            best_inliers = inliers;
            best_line = Some(candidate);
        }
    }

    let line = best_line?;
    // Refine over the inlier set for a tighter fit.
    let inlier_pts: Vec<Point> = best_inliers.iter().map(|&idx| points[idx]).collect();
    let refined = fit_line_least_squares(&inlier_pts).unwrap_or(line);
    Some((refined, best_inliers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_through_two_points() {
        let l = Line::from_points(Point::new(0.0, 0.0), Point::new(1.0, 1.0)).unwrap();
        // Point on the line has ~zero distance.
        assert!(l.distance(Point::new(2.0, 2.0)).abs() < 1e-5);
        // Off-line point: (1,0) is sqrt(2)/2 away.
        assert!((l.distance(Point::new(1.0, 0.0)).abs() - (0.5f32).sqrt()).abs() < 1e-5);
    }

    #[test]
    fn coincident_points_have_no_line() {
        assert!(Line::from_points(Point::new(3.0, 3.0), Point::new(3.0, 3.0)).is_none());
    }

    #[test]
    fn least_squares_recovers_horizontal() {
        let pts: Vec<Point> = (0..10).map(|i| Point::new(i as f32, 5.0)).collect();
        let l = fit_line_least_squares(&pts).unwrap();
        // Horizontal line y = 5 -> normal (0, ±1), offset ∓5.
        for p in &pts {
            assert!(l.distance(*p).abs() < 1e-4);
        }
        assert!(l.a.abs() < 1e-4);
        assert!((l.b.abs() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn least_squares_recovers_vertical() {
        // A regression on y would blow up here; TLS handles it.
        let pts: Vec<Point> = (0..10).map(|i| Point::new(7.0, i as f32)).collect();
        let l = fit_line_least_squares(&pts).unwrap();
        for p in &pts {
            assert!(l.distance(*p).abs() < 1e-4);
        }
        assert!(l.b.abs() < 1e-4);
        assert!((l.a.abs() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn least_squares_recovers_diagonal_with_noise() {
        // y = 2x + 1 with small alternating perturbation off the line.
        let pts: Vec<Point> = (0..20)
            .map(|i| {
                let x = i as f32;
                let noise = if i % 2 == 0 { 0.05 } else { -0.05 };
                Point::new(x, 2.0 * x + 1.0 + noise)
            })
            .collect();
        let l = fit_line_least_squares(&pts).unwrap();
        // Residuals stay tiny and the slope implied by the normal is ~2.
        for p in &pts {
            assert!(l.distance(*p).abs() < 0.1);
        }
        // Normal (a, b); line slope = -a/b should be ~2.
        let slope = -l.a / l.b;
        assert!((slope - 2.0).abs() < 0.05, "slope {slope}");
    }

    #[test]
    fn ransac_ignores_outliers() {
        // 30 inliers on y = 3, plus 10 gross outliers scattered off it.
        let mut pts: Vec<Point> = (0..30).map(|i| Point::new(i as f32, 3.0)).collect();
        for i in 0..10 {
            pts.push(Point::new(i as f32 * 2.0, 40.0 + i as f32));
        }
        let mut rng = Prng::new(12345);
        let (line, inliers) = ransac_line(&pts, 200, 0.5, &mut rng).unwrap();
        // All 30 true inliers recovered, outliers excluded.
        assert_eq!(inliers.len(), 30);
        for &idx in &inliers {
            assert!(line.distance(pts[idx]).abs() < 0.5);
        }
    }

    #[test]
    fn ransac_is_reproducible() {
        let pts: Vec<Point> = (0..40)
            .map(|i| Point::new(i as f32, 0.5 * i as f32))
            .collect();
        let mut a = Prng::new(777);
        let mut b = Prng::new(777);
        let ra = ransac_line(&pts, 50, 0.25, &mut a).unwrap();
        let rb = ransac_line(&pts, 50, 0.25, &mut b).unwrap();
        assert_eq!(ra.1, rb.1);
        assert_eq!(ra.0, rb.0);
    }
}
