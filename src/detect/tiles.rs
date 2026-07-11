//! Barcode-texture tiling and region clustering.
//!
//! Finder patterns locate QR/Aztec fiducials, but Data Matrix, PDF417 and every 1D
//! symbology have none. What they *do* share is a dense field of dark/light edges that
//! blank background and smooth scene content lack. This stage measures that.
//!
//! The reduced dark mask is split into a grid of square tiles. For each tile we count
//! horizontal and vertical dark/light transitions in one pass; a tile whose transition
//! density clears a threshold is "active". Active tiles are grouped by 8-connected
//! flood fill into regions, and each region's horizontal-vs-vertical transition balance
//! gives a coarse family guess: strongly one-directional edges read as a 1D barcode,
//! balanced edges as a 2D matrix.

use super::grid::DownGrid;

/// Per-tile transition tallies over the reduced dark mask.
struct TileStats {
    /// Tiles across.
    cols: usize,
    /// Tiles down.
    rows: usize,
    /// Tile edge length in reduced pixels.
    tile: usize,
    /// Horizontal transition count per tile (row-major, `cols * rows`).
    htrans: Vec<u32>,
    /// Vertical transition count per tile.
    vtrans: Vec<u32>,
    /// Pixel count per tile (border tiles are smaller).
    area: Vec<u32>,
}

/// The coarse layout family a region is guessed to belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Family {
    /// Balanced two-directional edge field: a 2D matrix code.
    Matrix,
    /// Strongly one-directional edge field: a 1D / stacked linear code.
    Linear,
}

/// A clustered candidate region in reduced-pixel coordinates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Region {
    /// Inclusive-exclusive bounding box in reduced pixels: `[x0, x1) × [y0, y1)`.
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
    /// Coarse family guess.
    pub family: Family,
}

impl Region {
    /// Region area in reduced pixels.
    pub fn area(&self) -> usize {
        (self.x1 - self.x0) * (self.y1 - self.y0)
    }
}

impl TileStats {
    /// Count transitions per tile in a single pass over the dark mask.
    fn build(grid: &DownGrid, tile: usize) -> TileStats {
        let tile = tile.max(1);
        let cols = grid.width.div_ceil(tile);
        let rows = grid.height.div_ceil(tile);
        let mut htrans = vec![0u32; cols * rows];
        let mut vtrans = vec![0u32; cols * rows];
        let mut area = vec![0u32; cols * rows];

        for y in 0..grid.height {
            let ty = y / tile;
            let base = ty * cols;
            for x in 0..grid.width {
                let tx = x / tile;
                let idx = base + tx;
                area[idx] += 1;
                let d = grid.dark(x, y);
                if x >= 1 && d != grid.dark(x - 1, y) {
                    htrans[idx] += 1;
                }
                if y >= 1 && d != grid.dark(x, y - 1) {
                    vtrans[idx] += 1;
                }
            }
        }
        TileStats {
            cols,
            rows,
            tile,
            htrans,
            vtrans,
            area,
        }
    }
}

/// Detect candidate regions in `grid`.
///
/// * `tile` — tile edge length in reduced pixels.
/// * `edge_density` — minimum `(h + v) transitions / pixel` for a tile to be active.
/// * `min_tiles` — minimum active tiles for a region to be reported.
/// * `aniso` — `|h - v| / (h + v)` above which a region is guessed [`Family::Linear`].
pub(crate) fn regions(
    grid: &DownGrid,
    tile: usize,
    edge_density: f32,
    min_tiles: usize,
    aniso: f32,
) -> Vec<Region> {
    let stats = TileStats::build(grid, tile);
    let cols = stats.cols;
    let rows = stats.rows;

    // Activity mask over the tile grid.
    let active: Vec<bool> = (0..cols * rows)
        .map(|i| {
            let a = stats.area[i];
            if a == 0 {
                return false;
            }
            let density = (stats.htrans[i] + stats.vtrans[i]) as f32 / a as f32;
            density >= edge_density
        })
        .collect();

    // 8-connected flood fill over active tiles.
    let mut visited = vec![false; cols * rows];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut out = Vec::new();

    for sy in 0..rows {
        for sx in 0..cols {
            let start = sy * cols + sx;
            if !active[start] || visited[start] {
                continue;
            }
            visited[start] = true;
            stack.push((sx, sy));

            let mut min_tx = sx;
            let mut max_tx = sx;
            let mut min_ty = sy;
            let mut max_ty = sy;
            let mut tiles = 0usize;
            let mut h_sum = 0u64;
            let mut v_sum = 0u64;

            while let Some((cx, cy)) = stack.pop() {
                let idx = cy * cols + cx;
                tiles += 1;
                h_sum += u64::from(stats.htrans[idx]);
                v_sum += u64::from(stats.vtrans[idx]);
                min_tx = min_tx.min(cx);
                max_tx = max_tx.max(cx);
                min_ty = min_ty.min(cy);
                max_ty = max_ty.max(cy);

                let x0 = cx.saturating_sub(1);
                let x1 = (cx + 1).min(cols - 1);
                let y0 = cy.saturating_sub(1);
                let y1 = (cy + 1).min(rows - 1);
                for ny in y0..=y1 {
                    for nx in x0..=x1 {
                        let nidx = ny * cols + nx;
                        if active[nidx] && !visited[nidx] {
                            visited[nidx] = true;
                            stack.push((nx, ny));
                        }
                    }
                }
            }

            if tiles < min_tiles {
                continue;
            }

            let total = (h_sum + v_sum) as f32;
            let anisotropy = if total > 0.0 {
                (h_sum as f32 - v_sum as f32).abs() / total
            } else {
                0.0
            };
            let family = if anisotropy >= aniso {
                Family::Linear
            } else {
                Family::Matrix
            };

            let t = stats.tile;
            out.push(Region {
                x0: min_tx * t,
                y0: min_ty * t,
                x1: ((max_tx + 1) * t).min(grid.width),
                y1: ((max_ty + 1) * t).min(grid.height),
                family,
            });
        }
    }
    out
}
