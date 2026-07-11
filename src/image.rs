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
