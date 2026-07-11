//! Rasterize an abstract [`Encoding`] into an owned grayscale [`GrayImage`].
//!
//! Encoders produce module *geometry*, not pixels (see [`crate::output`]). This
//! module bridges that gap for testing and image-based round-trips: it paints each
//! module as a square block of `scale × scale` pixels, surrounds the symbol with the
//! symbology's required quiet zone, and follows the crate convention that a **dark**
//! module renders to **low** luminance (`0`) on a light (`255`) background.
//!
//! The output is deterministic and pixel-exact, which makes it the fixed starting
//! point for the image transforms in [`crate::transform`].

use crate::image::GrayImage;
use crate::output::{BitMatrix, Encoding, LinearPattern};

/// Luminance of a dark module.
const DARK: u8 = 0;
/// Luminance of a light module / quiet zone.
const LIGHT: u8 = 255;
/// Default bar height (in modules) used when rasterizing a 1D [`LinearPattern`].
const LINEAR_HEIGHT_MODULES: usize = 24;

/// Render an [`Encoding`] at `scale` pixels per module.
///
/// The quiet zone recorded on the encoding is included on every side. `scale` must be
/// at least 1.
///
/// # Panics
/// Panics if `scale` is zero.
pub fn render(encoding: &Encoding, scale: usize) -> GrayImage {
    match encoding {
        Encoding::Matrix(m) => render_matrix(m, scale),
        Encoding::Linear(l) => render_linear(l, scale),
    }
}

/// Render a 2D [`BitMatrix`] at `scale` pixels per module, framed by its quiet zone.
///
/// # Panics
/// Panics if `scale` is zero.
pub fn render_matrix(matrix: &BitMatrix, scale: usize) -> GrayImage {
    assert!(scale > 0, "render scale must be >= 1");
    let qz = matrix.quiet_zone;
    let mods_w = matrix.width() + 2 * qz;
    let mods_h = matrix.height() + 2 * qz;
    let mut img = GrayImage::filled(mods_w * scale, mods_h * scale, LIGHT);
    for my in 0..matrix.height() {
        for mx in 0..matrix.width() {
            if matrix.get(mx, my) {
                fill_block(&mut img, (mx + qz) * scale, (my + qz) * scale, scale);
            }
        }
    }
    img
}

/// Render a 1D [`LinearPattern`] as a bar image `LINEAR_HEIGHT_MODULES` modules tall,
/// with the quiet zone on the left and right (and a matching light margin top/bottom).
///
/// # Panics
/// Panics if `scale` is zero.
pub fn render_linear(pattern: &LinearPattern, scale: usize) -> GrayImage {
    assert!(scale > 0, "render scale must be >= 1");
    let qz = pattern.quiet_zone;
    let mods_w = pattern.modules.len() + 2 * qz;
    let mods_h = LINEAR_HEIGHT_MODULES + 2 * qz;
    let mut img = GrayImage::filled(mods_w * scale, mods_h * scale, LIGHT);
    for (i, &bar) in pattern.modules.iter().enumerate() {
        if bar {
            let x0 = (i + qz) * scale;
            for my in 0..LINEAR_HEIGHT_MODULES {
                fill_block(&mut img, x0, (my + qz) * scale, scale);
            }
        }
    }
    img
}

/// Paint one `scale × scale` dark block with its top-left corner at `(px, py)`.
fn fill_block(img: &mut GrayImage, px: usize, py: usize, scale: usize) {
    for dy in 0..scale {
        for dx in 0..scale {
            img.set(px + dx, py + dy, DARK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_dimensions_and_quiet_zone() {
        let mut m = BitMatrix::new(3, 3, 2);
        m.set(0, 0, true);
        m.set(2, 2, true);
        let img = render_matrix(&m, 4);
        // (3 + 2*2) modules * 4 px = 28 px per side.
        assert_eq!(img.width(), 28);
        assert_eq!(img.height(), 28);
        // Quiet-zone corner is light.
        assert_eq!(img.get(0, 0), LIGHT);
        // Module (0,0) sits at pixel (2*4, 2*4) and is dark.
        assert_eq!(img.get(8, 8), DARK);
        // Module (1,1) is light.
        assert_eq!(img.get(12, 12), LIGHT);
    }
}
