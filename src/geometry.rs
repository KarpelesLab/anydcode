//! Geometric primitives used to describe where a symbol sits inside a frame.
//!
//! Coordinates are in pixel space with the origin at the top-left of the frame,
//! `x` increasing to the right and `y` increasing downward.

/// A point in pixel space. Sub-pixel precision is intentional: detectors refine
/// module/finder centers to fractional coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Horizontal coordinate (pixels, increasing right).
    pub x: f32,
    /// Vertical coordinate (pixels, increasing down).
    pub y: f32,
}

impl Point {
    /// Construct a point.
    pub const fn new(x: f32, y: f32) -> Self {
        Point { x, y }
    }

    /// Euclidean distance to another point.
    pub fn distance(self, other: Point) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
}

/// A convex quadrilateral outline of a detected symbol, corners ordered
/// clockwise starting from the top-left as seen in the symbol's own frame.
///
/// For 1D codes the quad still applies: it bounds the full bar region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    /// The four corners, clockwise from top-left.
    pub corners: [Point; 4],
}

impl Quad {
    /// Construct a quad from four clockwise corners.
    pub const fn new(corners: [Point; 4]) -> Self {
        Quad { corners }
    }

    /// Axis-aligned bounding box as `(min, max)` points.
    pub fn bounds(&self) -> (Point, Point) {
        let mut min = self.corners[0];
        let mut max = self.corners[0];
        for c in &self.corners[1..] {
            min.x = min.x.min(c.x);
            min.y = min.y.min(c.y);
            max.x = max.x.max(c.x);
            max.y = max.y.max(c.y);
        }
        (min, max)
    }

    /// Centroid of the four corners.
    pub fn center(&self) -> Point {
        let (sx, sy) = self
            .corners
            .iter()
            .fold((0.0, 0.0), |(sx, sy), p| (sx + p.x, sy + p.y));
        Point::new(sx / 4.0, sy / 4.0)
    }
}

/// Where a decoded symbol was found within a frame, plus its resolved module grid
/// where applicable. This is populated by detection/decoding, and is not required
/// for round-trip re-encoding (it describes the capture, not the payload).
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    /// Outline of the symbol in the source frame.
    pub outline: Quad,
    /// Estimated rotation of the symbol in radians, clockwise, if known.
    pub rotation: Option<f32>,
    /// Estimated module (smallest bar/cell) size in pixels, if known.
    pub module_size: Option<f32>,
}
