//! Frame buffer types for the live-video decoding path.
//!
//! The library's input boundary is a single-channel luminance buffer. Callers own
//! capture and color conversion; this keeps the crate portable and dependency-free.
//! A [`GrayFrame`] borrows the caller's pixel data so per-frame decoding allocates
//! nothing on the hot path.

use crate::error::{Error, Result};

/// A borrowed 8-bit grayscale (luminance) view of one video frame or still image.
///
/// The buffer may be wider than the image via `stride` (bytes per row), which lets
/// callers pass a sub-region or a padded/aligned buffer without copying.
#[derive(Debug, Clone, Copy)]
pub struct GrayFrame<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
    stride: usize,
}

impl<'a> GrayFrame<'a> {
    /// Wrap a tightly-packed `width * height` luminance buffer (stride == width).
    ///
    /// # Errors
    /// Returns [`Error::InvalidFrame`] if the buffer is too small or dimensions are zero.
    pub fn new(data: &'a [u8], width: usize, height: usize) -> Result<Self> {
        Self::with_stride(data, width, height, width)
    }

    /// Wrap a luminance buffer with an explicit row `stride` in bytes.
    ///
    /// # Errors
    /// Returns [`Error::InvalidFrame`] if `stride < width`, any dimension is zero,
    /// or the buffer cannot hold `stride * height` bytes.
    pub fn with_stride(data: &'a [u8], width: usize, height: usize, stride: usize) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidFrame {
                detail: "zero-sized frame".into(),
            });
        }
        if stride < width {
            return Err(Error::InvalidFrame {
                detail: "stride smaller than width".into(),
            });
        }
        // Last row only needs `width` bytes, not a full stride.
        let required = stride
            .checked_mul(height - 1)
            .and_then(|r| r.checked_add(width));
        match required {
            Some(req) if data.len() >= req => Ok(GrayFrame {
                data,
                width,
                height,
                stride,
            }),
            _ => Err(Error::InvalidFrame {
                detail: "buffer too small for dimensions".into(),
            }),
        }
    }

    /// Image width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Image height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Luminance at `(x, y)`. Returns `None` if out of bounds.
    pub fn get(&self, x: usize, y: usize) -> Option<u8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(self.data[y * self.stride + x])
    }

    /// Luminance at `(x, y)` without bounds checking on the coordinates.
    ///
    /// # Panics
    /// Panics (in debug) or reads out of the logical image (in release) if the
    /// coordinates are outside `width x height`.
    pub fn get_unchecked(&self, x: usize, y: usize) -> u8 {
        debug_assert!(x < self.width && y < self.height);
        self.data[y * self.stride + x]
    }

    /// Iterate one row of luminance values, left to right.
    pub fn row(&self, y: usize) -> Option<&'a [u8]> {
        if y >= self.height {
            return None;
        }
        let start = y * self.stride;
        Some(&self.data[start..start + self.width])
    }
}

/// An owned, tightly-packed 8-bit grayscale image.
///
/// This is the mutable counterpart to the borrowed [`GrayFrame`]: encoders/renderers
/// and image transforms produce a `GrayImage`, and [`GrayImage::as_frame`] hands the
/// decoding path a zero-copy [`GrayFrame`] view over it. Pixel `0` is fully dark,
/// `255` fully light — matching the convention that a set [`crate::output::BitMatrix`]
/// module renders to low luminance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrayImage {
    data: Vec<u8>,
    width: usize,
    height: usize,
}

impl GrayImage {
    /// A new image with every pixel set to `fill`.
    ///
    /// # Panics
    /// Panics if either dimension is zero.
    pub fn filled(width: usize, height: usize, fill: u8) -> Self {
        assert!(
            width > 0 && height > 0,
            "GrayImage dimensions must be nonzero"
        );
        GrayImage {
            data: vec![fill; width * height],
            width,
            height,
        }
    }

    /// A new all-black image (every pixel `0`).
    ///
    /// # Panics
    /// Panics if either dimension is zero.
    pub fn new(width: usize, height: usize) -> Self {
        Self::filled(width, height, 0)
    }

    /// Wrap an existing tightly-packed `width * height` buffer.
    ///
    /// # Panics
    /// Panics if `data.len() != width * height` or a dimension is zero.
    pub fn from_raw(width: usize, height: usize, data: Vec<u8>) -> Self {
        assert!(
            width > 0 && height > 0,
            "GrayImage dimensions must be nonzero"
        );
        assert_eq!(data.len(), width * height, "buffer size mismatch");
        GrayImage {
            data,
            width,
            height,
        }
    }

    /// Image width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Image height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// The raw row-major pixel buffer.
    pub fn pixels(&self) -> &[u8] {
        &self.data
    }

    /// Luminance at `(x, y)`, or `0` if out of bounds.
    pub fn get(&self, x: usize, y: usize) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.data[y * self.width + x]
    }

    /// Set the pixel at `(x, y)`.
    ///
    /// # Panics
    /// Panics if the coordinates are out of bounds.
    pub fn set(&mut self, x: usize, y: usize, value: u8) {
        assert!(
            x < self.width && y < self.height,
            "GrayImage::set out of bounds"
        );
        self.data[y * self.width + x] = value;
    }

    /// Borrow this image as a [`GrayFrame`] for the decoding path (zero copy).
    pub fn as_frame(&self) -> GrayFrame<'_> {
        // Dimensions are guaranteed nonzero and the buffer exactly `w*h`.
        GrayFrame::new(&self.data, self.width, self.height).expect("valid GrayImage dimensions")
    }

    /// Bilinearly sample luminance at fractional `(x, y)`.
    ///
    /// Returns `None` when the sample point lies outside the image so callers can
    /// substitute a background value. Coordinates are in pixel units with pixel
    /// centers at integer coordinates.
    pub fn sample_bilinear(&self, x: f32, y: f32) -> Option<f32> {
        if x < 0.0 || y < 0.0 || !x.is_finite() || !y.is_finite() {
            return None;
        }
        let x0 = x.floor();
        let y0 = y.floor();
        let ix0 = x0 as usize;
        let iy0 = y0 as usize;
        if ix0 >= self.width || iy0 >= self.height {
            return None;
        }
        let ix1 = (ix0 + 1).min(self.width - 1);
        let iy1 = (iy0 + 1).min(self.height - 1);
        let fx = x - x0;
        let fy = y - y0;
        let p00 = self.data[iy0 * self.width + ix0] as f32;
        let p10 = self.data[iy0 * self.width + ix1] as f32;
        let p01 = self.data[iy1 * self.width + ix0] as f32;
        let p11 = self.data[iy1 * self.width + ix1] as f32;
        let top = p00 + (p10 - p00) * fx;
        let bot = p01 + (p11 - p01) * fx;
        Some(top + (bot - top) * fy)
    }
}
