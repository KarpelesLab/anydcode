//! Connected-component labeling over a [`BinaryImage`].
//!
//! Samplers locate a code's finder patterns by finding dark blobs and inspecting
//! their size, shape and position. [`connected_components`] groups the dark pixels
//! into connected regions (4- or 8-neighbour) and reports each region's area,
//! bounding box and centroid. Labeling uses an explicit-stack flood fill, so it is
//! deterministic and never recurses (no stack-overflow risk on large blobs).

use crate::imgproc::binary::BinaryImage;

/// Pixel adjacency used when growing a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connectivity {
    /// Up, down, left, right.
    Four,
    /// The four edge neighbours plus the four diagonals.
    Eight,
}

/// An inclusive axis-aligned bounding box in pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundingBox {
    /// Smallest x of any pixel in the box.
    pub min_x: usize,
    /// Smallest y of any pixel in the box.
    pub min_y: usize,
    /// Largest x of any pixel in the box (inclusive).
    pub max_x: usize,
    /// Largest y of any pixel in the box (inclusive).
    pub max_y: usize,
}

impl BoundingBox {
    /// Width in pixels (inclusive of both ends).
    pub fn width(&self) -> usize {
        self.max_x - self.min_x + 1
    }

    /// Height in pixels (inclusive of both ends).
    pub fn height(&self) -> usize {
        self.max_y - self.min_y + 1
    }
}

/// One connected region of dark pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Component {
    /// 1-based label assigned in raster-scan discovery order.
    pub label: u32,
    /// Number of pixels in the region.
    pub area: usize,
    /// Inclusive bounding box.
    pub bounds: BoundingBox,
    /// Centroid `(x, y)` as the mean of member pixel coordinates.
    pub centroid: (f32, f32),
}

/// Label all connected regions of dark (`true`) pixels in `img`.
///
/// Components are returned in the order their first pixel is met in a top-to-bottom,
/// left-to-right scan, with `label` running `1, 2, 3, …`.
pub fn connected_components(img: &BinaryImage, conn: Connectivity) -> Vec<Component> {
    let w = img.width();
    let h = img.height();
    let mut visited = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut next_label: u32 = 1;

    let neighbors: &[(isize, isize)] = match conn {
        Connectivity::Four => &[(1, 0), (-1, 0), (0, 1), (0, -1)],
        Connectivity::Eight => &[
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ],
    };

    for sy in 0..h {
        for sx in 0..w {
            let idx = sy * w + sx;
            if visited[idx] || !img.get(sx, sy) {
                continue;
            }
            // Flood-fill this component.
            let mut area = 0usize;
            let mut sum_x = 0u64;
            let mut sum_y = 0u64;
            let mut min_x = sx;
            let mut min_y = sy;
            let mut max_x = sx;
            let mut max_y = sy;

            visited[idx] = true;
            stack.push((sx, sy));
            while let Some((x, y)) = stack.pop() {
                area += 1;
                sum_x += x as u64;
                sum_y += y as u64;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);

                for &(dx, dy) in neighbors {
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let (nx, ny) = (nx as usize, ny as usize);
                    let nidx = ny * w + nx;
                    if !visited[nidx] && img.get(nx, ny) {
                        visited[nidx] = true;
                        stack.push((nx, ny));
                    }
                }
            }

            out.push(Component {
                label: next_label,
                area,
                bounds: BoundingBox {
                    min_x,
                    min_y,
                    max_x,
                    max_y,
                },
                centroid: (sum_x as f32 / area as f32, sum_y as f32 / area as f32),
            });
            next_label += 1;
        }
    }
    out
}

/// Collect the pixels of the single region of `value`-pixels containing `seed`
/// (8-connected). Empty when the seed has the wrong value or is out of bounds.
///
/// Complements [`connected_components`] (which labels *every* dark region but keeps
/// only summary statistics): samplers that already know a point on their fiducial —
/// a finder ring, a bullseye ring, an enclosed quiet annulus — flood just that
/// region and keep the pixels for geometry extraction.
pub fn flood_region(img: &BinaryImage, seed: (usize, usize), value: bool) -> Vec<(usize, usize)> {
    let (w, h) = (img.width(), img.height());
    if seed.0 >= w || seed.1 >= h || img.get(seed.0, seed.1) != value {
        return Vec::new();
    }
    let mut visited = vec![false; w * h];
    let mut stack = vec![seed];
    let mut out = Vec::new();
    visited[seed.1 * w + seed.0] = true;
    while let Some((x, y)) = stack.pop() {
        out.push((x, y));
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                    continue;
                }
                let idx = ny as usize * w + nx as usize;
                if !visited[idx] && img.get(nx as usize, ny as usize) == value {
                    visited[idx] = true;
                    stack.push((nx as usize, ny as usize));
                }
            }
        }
    }
    out
}

/// The four corners of a roughly square/quadrilateral pixel cloud, in cyclic order,
/// by the rotation-robust farthest-point method: the pixel farthest from the centroid
/// is a corner, the pixel farthest from it is the opposite corner, and the extreme
/// pixels on each side of that diagonal complete the quad. Each returned point is the
/// corner pixel's **outer corner** (its centre pushed half a pixel away from the
/// cloud centroid on each axis) — the geometrically meaningful boundary point, whose
/// half-pixel accuracy matters once a small fiducial is extrapolated across a whole
/// symbol. Returns `None` for clouds too small to carry corners.
pub fn extreme_quad(pixels: &[(usize, usize)]) -> Option<[(f32, f32); 4]> {
    if pixels.len() < 4 {
        return None;
    }
    let n = pixels.len() as f32;
    let (sx, sy) = pixels.iter().fold((0f32, 0f32), |(ax, ay), &(x, y)| {
        (ax + x as f32, ay + y as f32)
    });
    let (cx, cy) = (sx / n, sy / n);

    let far_from = |px: f32, py: f32| -> (f32, f32) {
        let mut best = (px, py);
        let mut best_d = -1.0f32;
        for &(x, y) in pixels {
            let (dx, dy) = (x as f32 - px, y as f32 - py);
            let d = dx * dx + dy * dy;
            if d > best_d {
                best_d = d;
                best = (x as f32, y as f32);
            }
        }
        best
    };
    let a = far_from(cx, cy);
    let c = far_from(a.0, a.1);

    // Extremes on each side of the diagonal a–c (largest perpendicular offset).
    let (dx, dy) = (c.0 - a.0, c.1 - a.1);
    let mut b = a;
    let mut d = a;
    let mut best_pos = 0.0f32;
    let mut best_neg = 0.0f32;
    for &(x, y) in pixels {
        let cross = (x as f32 - a.0) * dy - (y as f32 - a.1) * dx;
        if cross > best_pos {
            best_pos = cross;
            b = (x as f32, y as f32);
        } else if cross < best_neg {
            best_neg = cross;
            d = (x as f32, y as f32);
        }
    }
    if best_pos == 0.0 || best_neg == 0.0 {
        return None;
    }
    // Pixel index → the pixel's outer corner: centre (+0.5) then half a pixel
    // outward from the centroid per axis.
    let outer = |p: (f32, f32)| {
        (
            p.0 + 0.5 + 0.5 * (p.0 - cx).signum(),
            p.1 + 0.5 + 0.5 * (p.1 - cy).signum(),
        )
    };
    Some([outer(a), outer(b), outer(c), outer(d)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw_rect(img: &mut BinaryImage, x0: usize, y0: usize, x1: usize, y1: usize) {
        for y in y0..y1 {
            for x in x0..x1 {
                img.set(x, y, true);
            }
        }
    }

    #[test]
    fn counts_separate_blobs() {
        let mut img = BinaryImage::new(20, 20);
        draw_rect(&mut img, 1, 1, 4, 4); // 3x3 blob, area 9, first row y=1
        draw_rect(&mut img, 10, 10, 15, 13); // 5x3 blob, area 15, first row y=10
        draw_rect(&mut img, 16, 2, 18, 5); // 2x3 blob, area 6, first row y=2

        let comps = connected_components(&img, Connectivity::Four);
        assert_eq!(comps.len(), 3);
        // Discovery order is raster scan, so the y=2 blob precedes the y=10 blob.
        let areas: Vec<usize> = comps.iter().map(|c| c.area).collect();
        assert_eq!(areas, vec![9, 6, 15]);
    }

    #[test]
    fn locates_blob_bounds_and_centroid() {
        let mut img = BinaryImage::new(20, 20);
        draw_rect(&mut img, 4, 6, 8, 10); // x in 4..8, y in 6..10
        let comps = connected_components(&img, Connectivity::Four);
        assert_eq!(comps.len(), 1);
        let c = comps[0];
        assert_eq!(c.area, 16);
        assert_eq!(
            c.bounds,
            BoundingBox {
                min_x: 4,
                min_y: 6,
                max_x: 7,
                max_y: 9
            }
        );
        assert_eq!(c.bounds.width(), 4);
        assert_eq!(c.bounds.height(), 4);
        assert!((c.centroid.0 - 5.5).abs() < 1e-4);
        assert!((c.centroid.1 - 7.5).abs() < 1e-4);
    }

    #[test]
    fn connectivity_matters_for_diagonal_touch() {
        // Two 1-pixel blobs touching only at a corner.
        let mut img = BinaryImage::new(5, 5);
        img.set(1, 1, true);
        img.set(2, 2, true);

        // 4-connectivity: two separate components.
        assert_eq!(connected_components(&img, Connectivity::Four).len(), 2);
        // 8-connectivity: one component of area 2.
        let eight = connected_components(&img, Connectivity::Eight);
        assert_eq!(eight.len(), 1);
        assert_eq!(eight[0].area, 2);
    }

    #[test]
    fn empty_image_has_no_components() {
        let img = BinaryImage::new(8, 8);
        assert!(connected_components(&img, Connectivity::Eight).is_empty());
    }

    #[test]
    fn labels_are_sequential_in_scan_order() {
        let mut img = BinaryImage::new(10, 10);
        img.set(0, 0, true); // discovered first
        img.set(9, 9, true); // discovered second
        let comps = connected_components(&img, Connectivity::Four);
        assert_eq!(comps[0].label, 1);
        assert_eq!(comps[0].centroid, (0.0, 0.0));
        assert_eq!(comps[1].label, 2);
        assert_eq!(comps[1].centroid, (9.0, 9.0));
    }
}
