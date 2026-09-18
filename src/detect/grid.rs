//! The downscaled working image shared by every detector stage.
//!
//! [`DownGrid`] reduces the frame by an integer factor — each reduced pixel is the mean
//! of its `scale × scale` source block — and binarizes the result. Everything after
//! this works on the small image, which is what makes the locator fast enough to run on
//! every frame. Averaging (rather than picking one pixel per block) is what keeps fine
//! bars from aliasing into moiré and halves the sensor noise the texture pass sees.
//!
//! # Binarization
//!
//! The dark mask is thresholded **locally**: each pixel is compared with the mean
//! luminance of the neighbourhood of blocks around it. A single global threshold (Otsu)
//! is only right when the whole frame is evenly lit, and a live camera frame never is —
//! a lamp falloff, a shadow across the label, or a code on a dark product put the code's
//! own light/dark split far from the frame's, and a code that binarizes to one solid
//! tone has no finder pattern and no texture for the later stages to find. The
//! block-mean scheme costs two light passes over the reduced image.
//!
//! All coordinates produced downstream are in *downscaled* pixel space; the public API
//! multiplies them back up by [`DownGrid::scale`] before returning quads.

use crate::image::GrayFrame;
use alloc::{vec, vec::Vec};

/// Edge length, in reduced pixels, of the blocks whose means form the local threshold
/// surface. The threshold at a pixel is the mean over the 5×5 blocks around it, i.e. a
/// 40-pixel window: wider than any finder pattern or bar the locator can resolve, small
/// enough to follow a lighting gradient.
const BLOCK: usize = 8;

/// A pixel must be at least this much darker than its neighbourhood mean to count as
/// dark. Keeps flat regions — where the pixel and the mean differ only by noise — from
/// binarizing to salt-and-pepper texture.
const DARK_MARGIN: i32 = 6;

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
    /// Build the reduced grid from `frame` by averaging each `scale × scale` block.
    pub fn build(frame: &GrayFrame<'_>, scale: usize) -> DownGrid {
        let scale = scale.max(1);
        let w = frame.width();
        let h = frame.height();
        let width = (w / scale).max(1);
        let height = (h / scale).max(1);

        let mut luma = vec![0u8; width * height];
        if scale == 1 {
            for (dy, dst) in luma.chunks_exact_mut(width).enumerate() {
                for (dx, v) in dst.iter_mut().enumerate() {
                    *v = frame.get_unchecked(dx, dy);
                }
            }
        } else {
            // Blocks never run off the frame: width * scale <= w unless the frame is
            // narrower than one block, which the clamp below covers.
            let area = (scale * scale) as u32;
            let mut acc = vec![0u32; width];
            for dy in 0..height {
                acc.fill(0);
                for sy in dy * scale..(dy + 1) * scale {
                    let sy = sy.min(h - 1);
                    for (dx, a) in acc.iter_mut().enumerate() {
                        for sx in dx * scale..(dx + 1) * scale {
                            *a += u32::from(frame.get_unchecked(sx.min(w - 1), sy));
                        }
                    }
                }
                for (v, &a) in luma[dy * width..(dy + 1) * width].iter_mut().zip(&acc) {
                    *v = (a / area) as u8;
                }
            }
        }

        let dark = local_dark_mask(&luma, width, height);
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

/// Threshold `luma` against the mean of the 5×5 [`BLOCK`]s around each pixel.
fn local_dark_mask(luma: &[u8], width: usize, height: usize) -> Vec<bool> {
    let bw = width.div_ceil(BLOCK);
    let bh = height.div_ceil(BLOCK);

    // Per-block luminance sums and pixel counts (edge blocks are partial).
    let mut sum = vec![0u32; bw * bh];
    let mut cnt = vec![0u32; bw * bh];
    for y in 0..height {
        let brow = (y / BLOCK) * bw;
        for (x, &v) in luma[y * width..(y + 1) * width].iter().enumerate() {
            sum[brow + x / BLOCK] += u32::from(v);
            cnt[brow + x / BLOCK] += 1;
        }
    }

    // Threshold per block: the pooled mean of its 5×5 block neighbourhood.
    let mut thr = vec![0i32; bw * bh];
    for by in 0..bh {
        for bx in 0..bw {
            let (mut s, mut c) = (0u32, 0u32);
            for ny in by.saturating_sub(2)..(by + 3).min(bh) {
                for nx in bx.saturating_sub(2)..(bx + 3).min(bw) {
                    s += sum[ny * bw + nx];
                    c += cnt[ny * bw + nx];
                }
            }
            thr[by * bw + bx] = (s / c.max(1)) as i32 - DARK_MARGIN;
        }
    }

    let mut dark = vec![false; width * height];
    for y in 0..height {
        let brow = (y / BLOCK) * bw;
        let row = &luma[y * width..(y + 1) * width];
        for (x, d) in dark[y * width..(y + 1) * width].iter_mut().enumerate() {
            *d = i32::from(row[x]) <= thr[brow + x / BLOCK];
        }
    }
    dark
}
