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
/// Panics if `scale` is zero, the matrix has no modules and no quiet zone, or the
/// pixel size overflows `usize`.
pub fn render_matrix(matrix: &BitMatrix, scale: usize) -> GrayImage {
    assert!(scale > 0, "render scale must be >= 1");
    let qz = matrix.quiet_zone;
    let mut img = GrayImage::filled(
        pixel_extent(matrix.width(), qz, scale),
        pixel_extent(matrix.height(), qz, scale),
        LIGHT,
    );
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
/// Panics if `scale` is zero, the pattern has no modules and no quiet zone, or the
/// pixel size overflows `usize`.
pub fn render_linear(pattern: &LinearPattern, scale: usize) -> GrayImage {
    assert!(scale > 0, "render scale must be >= 1");
    let qz = pattern.quiet_zone;
    let mut img = GrayImage::filled(
        pixel_extent(pattern.modules.len(), qz, scale),
        pixel_extent(LINEAR_HEIGHT_MODULES, qz, scale),
        LIGHT,
    );
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

/// Pixels spanned by `modules` plus a quiet zone on both sides, at `scale` pixels per
/// module.
///
/// Checked: `quiet_zone` is a public field, and a wrapped product would not fail — it
/// would size a smaller canvas and paint the modules at wrapped offsets, e.g. silently
/// dropping the quiet zone when `2 * quiet_zone` wraps to zero.
fn pixel_extent(modules: usize, quiet_zone: usize, scale: usize) -> usize {
    quiet_zone
        .checked_mul(2)
        .and_then(|margin| margin.checked_add(modules))
        .and_then(|total| total.checked_mul(scale))
        .expect("render size overflows usize")
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
        // Each dark module is exactly one scale x scale block: 2 modules x 16 pixels.
        assert_eq!(img.pixels().iter().filter(|&&p| p == DARK).count(), 32);
        assert_eq!(
            (img.get(11, 11), img.get(7, 8), img.get(8, 7)),
            (DARK, LIGHT, LIGHT)
        );
        assert_eq!((img.get(19, 19), img.get(20, 20)), (DARK, LIGHT));
        assert_eq!(render(&Encoding::Matrix(m), 4), img);
    }

    #[test]
    fn non_square_matrix_keeps_its_aspect() {
        let mut m = BitMatrix::new(5, 2, 1);
        m.set(4, 1, true);
        let img = render_matrix(&m, 3);
        assert_eq!((img.width(), img.height()), (21, 12));
        assert_eq!(img.get(15, 6), DARK);
        assert_eq!(img.get(17, 8), DARK);
        assert_eq!(img.get(18, 8), LIGHT, "right quiet zone");
        assert_eq!(img.get(14, 6), LIGHT);
        // Scale 1, no quiet zone: one pixel per module.
        m.quiet_zone = 0;
        let img = render_matrix(&m, 1);
        assert_eq!((img.width(), img.height()), (5, 2));
        assert_eq!(
            img.pixels(),
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 0]
        );
    }

    #[test]
    fn linear_bars_span_the_bar_height_between_quiet_zones() {
        let p = LinearPattern {
            modules: std::vec![true, false, true, true],
            quiet_zone: 2,
        };
        let img = render_linear(&p, 3);
        assert_eq!(img.width(), (4 + 2 * 2) * 3);
        assert_eq!(img.height(), (LINEAR_HEIGHT_MODULES + 2 * 2) * 3);
        let (top, bottom) = (2 * 3, (2 + LINEAR_HEIGHT_MODULES) * 3 - 1);
        for (module, bar) in p.modules.iter().enumerate() {
            let want = if *bar { DARK } else { LIGHT };
            for x in (2 + module) * 3..(3 + module) * 3 {
                assert_eq!(img.get(x, top), want, "x {x}");
                assert_eq!(img.get(x, bottom), want, "x {x}");
                assert_eq!(img.get(x, top - 1), LIGHT, "margin above x {x}");
                assert_eq!(img.get(x, bottom + 1), LIGHT, "margin below x {x}");
            }
        }
        assert_eq!((img.get(5, top), img.get(18, top)), (LIGHT, LIGHT));
        assert_eq!(render(&Encoding::Linear(p), 3), img);
        // An empty pattern is just its quiet zone.
        let blank = LinearPattern {
            modules: std::vec::Vec::new(),
            quiet_zone: 1,
        };
        let img = render_linear(&blank, 1);
        assert_eq!((img.width(), img.height()), (2, LINEAR_HEIGHT_MODULES + 2));
        assert!(img.pixels().iter().all(|&p| p == LIGHT));
    }

    #[test]
    #[should_panic(expected = "render scale must be >= 1")]
    fn zero_scale_panics() {
        let _ = render_matrix(&BitMatrix::new(3, 3, 0), 0);
    }

    #[test]
    #[should_panic(expected = "render scale must be >= 1")]
    fn zero_scale_panics_for_linear() {
        let _ = render_linear(&LinearPattern::new(), 0);
    }

    /// `2 * quiet_zone` wraps to 0 here, and every block offset wraps back into the
    /// image: unchecked, a release build returned a symbol with no quiet zone at all.
    #[test]
    #[should_panic(expected = "render size overflows usize")]
    fn overflowing_quiet_zone_panics() {
        let mut m = BitMatrix::new(3, 3, 1usize << (usize::BITS - 1));
        m.set(1, 1, true);
        let _ = render_matrix(&m, 2);
    }

    #[test]
    #[should_panic(expected = "render size overflows usize")]
    fn overflowing_scale_panics() {
        let p = LinearPattern {
            modules: std::vec![true; 4],
            quiet_zone: 0,
        };
        let _ = render_linear(&p, usize::MAX / 2);
    }
}
