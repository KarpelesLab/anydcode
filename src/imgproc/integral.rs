//! Summed-area tables (integral images) for O(1) rectangular sums.
//!
//! An integral image stores, at each corner, the sum of all pixels above and to the
//! left of it. The sum over any axis-aligned rectangle is then four table lookups.
//! Adaptive thresholding and blob analysis lean on this to stay linear-time.
//!
//! Two flavours are provided: the plain sum of luminance and the sum of *squared*
//! luminance (needed for local variance in Sauvola thresholding).

use crate::image::GrayFrame;

/// A summed-area table over an 8-bit luminance image.
///
/// The backing buffer is `(width + 1) × (height + 1)`, with a zero first row and
/// column, so rectangle queries never need special-casing at the border.
#[derive(Debug, Clone)]
pub struct IntegralImage {
    width: usize,
    height: usize,
    /// `(width + 1) * (height + 1)` prefix sums, row-major.
    sum: Vec<u64>,
}

impl IntegralImage {
    /// Build a summed-area table of luminance from `frame`.
    pub fn from_frame(frame: &GrayFrame<'_>) -> Self {
        Self::build(frame, false)
    }

    /// Build a summed-area table of **squared** luminance from `frame`.
    ///
    /// Combined with [`IntegralImage::from_frame`] this yields local mean and
    /// variance in constant time per window.
    pub fn squared_from_frame(frame: &GrayFrame<'_>) -> Self {
        Self::build(frame, true)
    }

    fn build(frame: &GrayFrame<'_>, squared: bool) -> Self {
        let width = frame.width();
        let height = frame.height();
        let stride = width + 1;
        let mut sum = vec![0u64; stride * (height + 1)];
        for y in 0..height {
            let mut row_acc: u64 = 0;
            for x in 0..width {
                let v = u64::from(frame.get_unchecked(x, y));
                let v = if squared { v * v } else { v };
                row_acc += v;
                // S(x+1, y+1) = row prefix + S(x+1, y)
                sum[(y + 1) * stride + (x + 1)] = row_acc + sum[y * stride + (x + 1)];
            }
        }
        IntegralImage { width, height, sum }
    }

    /// Source image width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Source image height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Sum of pixels in the half-open rectangle `[x0, x1) × [y0, y1)`.
    ///
    /// Coordinates are clamped to the image, so callers may pass windows that hang
    /// over the border. Returns `0` for an empty rectangle.
    pub fn rect_sum(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> u64 {
        let x0 = x0.min(self.width);
        let y0 = y0.min(self.height);
        let x1 = x1.min(self.width);
        let y1 = y1.min(self.height);
        if x1 <= x0 || y1 <= y0 {
            return 0;
        }
        let stride = self.width + 1;
        let a = self.sum[y1 * stride + x1];
        let b = self.sum[y0 * stride + x1];
        let c = self.sum[y1 * stride + x0];
        let d = self.sum[y0 * stride + x0];
        a + d - b - c
    }

    /// Sum and pixel count of the window of half-width `radius` centred on `(cx, cy)`,
    /// clamped to the image. The count reflects clamping so a border window is
    /// averaged over its real (smaller) area.
    pub fn window_sum_count(&self, cx: usize, cy: usize, radius: usize) -> (u64, usize) {
        let x0 = cx.saturating_sub(radius);
        let y0 = cy.saturating_sub(radius);
        let x1 = (cx + radius + 1).min(self.width);
        let y1 = (cy + radius + 1).min(self.height);
        let sum = self.rect_sum(x0, y0, x1, y1);
        let count = (x1 - x0) * (y1 - y0);
        (sum, count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(data: &[u8], w: usize, h: usize) -> GrayFrame<'_> {
        GrayFrame::new(data, w, h).unwrap()
    }

    #[test]
    fn rect_sum_matches_bruteforce() {
        // 4x3 ramp image.
        let w = 4;
        let h = 3;
        let data: Vec<u8> = (0..(w * h) as u8).collect();
        let f = frame(&data, w, h);
        let ii = IntegralImage::from_frame(&f);

        for x0 in 0..=w {
            for y0 in 0..=h {
                for x1 in x0..=w {
                    for y1 in y0..=h {
                        let mut expected = 0u64;
                        for y in y0..y1 {
                            for x in x0..x1 {
                                expected += u64::from(data[y * w + x]);
                            }
                        }
                        assert_eq!(
                            ii.rect_sum(x0, y0, x1, y1),
                            expected,
                            "rect ({x0},{y0})-({x1},{y1})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn total_sum_and_squared() {
        let data = [10u8, 20, 30, 40];
        let f = frame(&data, 2, 2);
        let ii = IntegralImage::from_frame(&f);
        assert_eq!(ii.rect_sum(0, 0, 2, 2), 100);
        let sq = IntegralImage::squared_from_frame(&f);
        assert_eq!(sq.rect_sum(0, 0, 2, 2), 100 + 400 + 900 + 1600);
    }

    #[test]
    fn window_clamps_at_border() {
        let data = [1u8; 9];
        let f = frame(&data, 3, 3);
        let ii = IntegralImage::from_frame(&f);
        // Corner window of radius 1 covers a 2x2 region.
        let (s, c) = ii.window_sum_count(0, 0, 1);
        assert_eq!((s, c), (4, 4));
        // Centre window of radius 1 covers the whole 3x3.
        let (s, c) = ii.window_sum_count(1, 1, 1);
        assert_eq!((s, c), (9, 9));
    }

    #[test]
    fn rect_sum_clamps_overhang() {
        let data = [1u8; 4];
        let f = frame(&data, 2, 2);
        let ii = IntegralImage::from_frame(&f);
        assert_eq!(ii.rect_sum(0, 0, 100, 100), 4);
        assert_eq!(ii.rect_sum(5, 5, 10, 10), 0);
    }
}
