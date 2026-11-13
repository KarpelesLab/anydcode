//! MaxiCode rasterization: [`BitMatrix`] → [`GrayImage`].
//!
//! A MaxiCode module is a hexagon, alternate rows sit half a module to the right,
//! and the centre of the symbol is a bullseye of three dark rings — none of which
//! the square-module [`crate::render`] can draw. [`render_hex`] paints the symbol as
//! it is printed, anti-aliased, at any (fractional) module size.

use super::tables::{HEIGHT, WIDTH};
use super::{CENTER_COL, CENTER_ROW, RING_EDGES, ROW_PITCH};
use crate::image::GrayImage;
use crate::output::BitMatrix;
use alloc::vec;

/// Width of a dark hexagon as a fraction of the module pitch: printed symbols leave
/// a light gap between neighbouring dark modules.
const HEX_FILL: f32 = 0.88;
/// Sub-samples per pixel and axis for anti-aliasing.
const SUB: usize = 4;

/// Render a 30×33 MaxiCode module grid as the printed symbol: hexagonal modules
/// `module_px` pixels apart (centre to centre along a row), odd rows offset by half
/// a module, the bullseye in the centre, and the matrix's quiet zone (in modules)
/// on every side. Dark is `0`, light `255`.
///
/// # Panics
/// Panics if the matrix is not 30×33 modules or `module_px` is not a finite value
/// of at least 2.
pub fn render_hex(matrix: &BitMatrix, module_px: f32) -> GrayImage {
    assert!(
        matrix.width() == WIDTH && matrix.height() == HEIGHT,
        "a MaxiCode grid is 30×33 modules"
    );
    assert!(
        module_px.is_finite() && module_px >= 2.0,
        "module_px must be at least 2"
    );
    let w = module_px;
    let row = ROW_PITCH as f32 * w;
    // Half the point-to-point height of a full-size hexagon.
    let half_height = w / (2.0 * ROW_PITCH as f32);
    let margin = matrix.quiet_zone as f32 * w;
    let width = (WIDTH as f32 * w + 2.0 * margin) as usize + 1;
    let height = ((HEIGHT - 1) as f32 * row + 2.0 * half_height + 2.0 * margin) as usize + 1;
    let center = |r: usize, c: usize| {
        let shift = if r % 2 == 1 { 1.0 } else { 0.5 };
        (
            margin + (c as f32 + shift) * w,
            margin + half_height + r as f32 * row,
        )
    };

    // Covered sub-samples per pixel, 0..=SUB².
    let mut cover = vec![0u8; width * height];
    let mut paint = |x0: f32, y0: f32, x1: f32, y1: f32, inside: &dyn Fn(f32, f32) -> bool| {
        let (px0, py0) = (x0.max(0.0) as usize, y0.max(0.0) as usize);
        let (px1, py1) = (
            (x1.max(0.0) as usize).min(width - 1),
            (y1.max(0.0) as usize).min(height - 1),
        );
        for py in py0..=py1 {
            for px in px0..=px1 {
                let mut n = 0u8;
                for sy in 0..SUB {
                    for sx in 0..SUB {
                        let x = px as f32 + (sx as f32 + 0.5) / SUB as f32;
                        let y = py as f32 + (sy as f32 + 0.5) / SUB as f32;
                        n += u8::from(inside(x, y));
                    }
                }
                let c = &mut cover[py * width + px];
                *c = (*c + n).min((SUB * SUB) as u8);
            }
        }
    };

    // Hexagons, points up and down: |dx| ≤ ρ and |dx|/2 + |dy|·√3/2 ≤ ρ.
    let rho = HEX_FILL * w / 2.0;
    for r in 0..HEIGHT {
        for c in 0..WIDTH {
            if !matrix.get(c, r) {
                continue;
            }
            let (cx, cy) = center(r, c);
            let reach = rho / ROW_PITCH as f32;
            paint(cx - rho, cy - reach, cx + rho, cy + reach, &|x, y| {
                let (dx, dy) = ((x - cx).abs(), (y - cy).abs());
                dx <= rho && 0.5 * dx + ROW_PITCH as f32 * dy <= rho
            });
        }
    }

    // The bullseye: dark between ring edges 0–1, 2–3 and 4–5.
    let (bx, by) = center(CENTER_ROW, CENTER_COL);
    let edges = RING_EDGES.map(|e| {
        let r = e as f32 * w;
        r * r
    });
    let outer = RING_EDGES[5] as f32 * w;
    paint(bx - outer, by - outer, bx + outer, by + outer, &|x, y| {
        let d = (x - bx) * (x - bx) + (y - by) * (y - by);
        (d >= edges[0] && d < edges[1])
            || (d >= edges[2] && d < edges[3])
            || (d >= edges[4] && d < edges[5])
    });

    let mut img = GrayImage::filled(width, height, 255);
    let full = (SUB * SUB) as u32;
    for y in 0..height {
        for x in 0..width {
            let n = u32::from(cover[y * width + x]);
            if n > 0 {
                img.set(x, y, (255 - (n * 255 + full / 2) / full) as u8);
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Size, quiet zone, bullseye and two hexagons, at a fractional module size.
    #[test]
    fn draws_the_symbol_where_the_geometry_says() {
        let mut m = BitMatrix::new(WIDTH, HEIGHT, 1);
        m.set(0, 0, true);
        m.set(28, 1, true);
        let w = 7.5f32;
        let row = ROW_PITCH as f32;
        let img = render_hex(&m, w);
        assert_eq!(img.width(), (32.0 * w) as usize + 1);
        let height = (32.0 * row + 1.0 / row + 2.0) * w;
        assert_eq!(img.height(), height as usize + 1);

        let at = |x: f32, y: f32| img.get(x as usize, y as usize);
        let top = w + w / (2.0 * row);
        // Module (0, 0) sits half a module in from the quiet zone; (1, 28), in the
        // offset row below, a whole one further than its column says.
        assert_eq!(at(1.5 * w, top), 0);
        assert_eq!(at(2.5 * w, top), 255);
        assert_eq!(at(30.0 * w, top + row * w), 0);
        assert_eq!(at(0.5 * w, top), 255, "quiet zone");
        // The bullseye: light centre, then dark and light rings out to 4.5 modules.
        let (bx, by) = (15.5 * w, top + 16.0 * row * w);
        for (r, dark) in [
            (0.0, false),
            (1.0, true),
            (1.75, false),
            (2.5, true),
            (3.3, false),
            (4.1, true),
            (4.9, false),
        ] {
            assert_eq!(at(bx + r * w, by) == 0, dark, "radius {r}");
            assert_eq!(at(bx, by - r * w) == 0, dark, "radius {r} upwards");
        }
    }

    #[test]
    #[should_panic(expected = "30×33")]
    fn rejects_a_grid_that_is_not_a_maxicode() {
        render_hex(&BitMatrix::new(30, 30, 1), 8.0);
    }
}
