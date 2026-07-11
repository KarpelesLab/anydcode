//! The downscaled working image shared by every detector stage.
//!
//! The frame-level locator is the hot path, so it deliberately does **not** read every
//! source pixel. [`DownGrid`] *subsamples* the frame — taking the centre pixel of each
//! `scale × scale` block — so building it costs only `1/scale²` of a full-frame pass.
//! That is the key asymmetry versus a full decode, which must binarize at native
//! resolution. It then Otsu-thresholds the reduced image (reusing
//! [`crate::imgproc::threshold::otsu_threshold`]) and keeps both the reduced luminance
//! and the dark mask around for the finder and tile passes.
//!
//! Subsampling trades a little robustness to sensor noise for speed, which is the right
//! call for a coarse locator whose hits are re-checked by the heavier analysis pass.
//! All coordinates produced downstream are in *downscaled* pixel space; the public API
//! multiplies them back up by [`DownGrid::scale`] before returning quads.

use crate::image::GrayFrame;
use crate::imgproc::threshold::otsu_threshold;

/// A reduced-resolution view of one frame plus its binarization.
#[derive(Debug)]
pub(crate) struct DownGrid {
    /// Reduced width in pixels.
    pub width: usize,
    /// Reduced height in pixels.
    pub height: usize,
    /// Integer downscale factor mapping reduced pixels back to full resolution.
    pub scale: usize,
    /// Row-major reduced luminance (`width * height`).
    pub luma: Vec<u8>,
    /// Row-major dark mask (`true` = dark), same dimensions as [`Self::luma`].
    pub dark: Vec<bool>,
}

impl DownGrid {
    /// Build the reduced grid from `frame` by subsampling the centre of each block.
    pub fn build(frame: &GrayFrame<'_>, scale: usize) -> DownGrid {
        let scale = scale.max(1);
        let w = frame.width();
        let h = frame.height();
        let width = (w / scale).max(1);
        let height = (h / scale).max(1);
        let off = scale / 2;

        let mut luma = vec![0u8; width * height];
        for dy in 0..height {
            let sy = (dy * scale + off).min(h - 1);
            let dst = dy * width;
            for dx in 0..width {
                let sx = (dx * scale + off).min(w - 1);
                luma[dst + dx] = frame.get_unchecked(sx, sy);
            }
        }

        // Reuse the shared Otsu implementation on the reduced image.
        let threshold = {
            let view = GrayFrame::new(&luma, width, height).expect("reduced dimensions valid");
            otsu_threshold(&view)
        };
        let dark: Vec<bool> = luma.iter().map(|&v| v <= threshold).collect();

        DownGrid {
            width,
            height,
            scale,
            luma,
            dark,
        }
    }

    /// Dark-mask value at reduced `(x, y)`; `false` if out of bounds.
    #[inline]
    pub fn dark(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.dark[y * self.width + x]
    }

    /// Reduced luminance at `(x, y)`; `0` if out of bounds.
    #[inline]
    pub fn luma(&self, x: usize, y: usize) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.luma[y * self.width + x]
    }
}
