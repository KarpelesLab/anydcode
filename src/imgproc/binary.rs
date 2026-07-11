//! An owned binary image: one bit per pixel, `true` = dark (foreground).
//!
//! Binarization ([`crate::imgproc::threshold`]) produces a [`BinaryImage`]; connected
//! components and morphology consume it. The convention `dark = true` matches
//! [`BitMatrix`](crate::output::BitMatrix), where a set bit is a dark module, so a
//! barcode's dark modules and finder blobs are the foreground throughout the toolkit.

use crate::error::{Error, Result};

/// A `width × height` grid of booleans, `true` meaning a dark (foreground) pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryImage {
    width: usize,
    height: usize,
    bits: Vec<bool>,
}

impl BinaryImage {
    /// A new all-light (`false`) image. Panics if either dimension is zero.
    pub fn new(width: usize, height: usize) -> Self {
        assert!(
            width > 0 && height > 0,
            "BinaryImage dimensions must be > 0"
        );
        BinaryImage {
            width,
            height,
            bits: vec![false; width * height],
        }
    }

    /// Wrap an existing `width * height` bit buffer (row-major, top-left origin).
    ///
    /// # Errors
    /// Returns [`Error::InvalidParameter`] if a dimension is zero or `bits.len()`
    /// does not equal `width * height`.
    pub fn from_bits(width: usize, height: usize, bits: Vec<bool>) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::invalid_parameter("zero-sized binary image"));
        }
        if bits.len() != width * height {
            return Err(Error::invalid_parameter(format!(
                "bits length {} does not match {width}x{height}",
                bits.len()
            )));
        }
        Ok(BinaryImage {
            width,
            height,
            bits,
        })
    }

    /// Image width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Image height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Value at `(x, y)`; `false` (light) if out of bounds.
    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.bits[y * self.width + x]
    }

    /// Set the value at `(x, y)`.
    ///
    /// # Panics
    /// Panics if the coordinates are out of bounds.
    pub fn set(&mut self, x: usize, y: usize, dark: bool) {
        assert!(
            x < self.width && y < self.height,
            "BinaryImage::set out of bounds"
        );
        self.bits[y * self.width + x] = dark;
    }

    /// Number of dark (`true`) pixels.
    pub fn count_dark(&self) -> usize {
        self.bits.iter().filter(|&&b| b).count()
    }

    /// Read-only view of the raw row-major bit buffer.
    pub fn bits(&self) -> &[bool] {
        &self.bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_all_light() {
        let img = BinaryImage::new(4, 3);
        assert_eq!(img.width(), 4);
        assert_eq!(img.height(), 3);
        assert_eq!(img.count_dark(), 0);
        assert!(!img.get(0, 0));
    }

    #[test]
    fn set_and_get() {
        let mut img = BinaryImage::new(3, 3);
        img.set(1, 2, true);
        assert!(img.get(1, 2));
        assert!(!img.get(0, 0));
        assert_eq!(img.count_dark(), 1);
    }

    #[test]
    fn out_of_bounds_get_is_light() {
        let img = BinaryImage::new(2, 2);
        assert!(!img.get(5, 5));
        assert!(!img.get(0, 100));
    }

    #[test]
    fn from_bits_validates_length() {
        assert!(BinaryImage::from_bits(2, 2, vec![false; 4]).is_ok());
        assert!(BinaryImage::from_bits(2, 2, vec![false; 3]).is_err());
        assert!(BinaryImage::from_bits(0, 2, vec![]).is_err());
    }
}
