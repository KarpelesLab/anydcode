//! Barcode-texture tiling and region clustering.
//!
//! Finder patterns locate QR/Aztec fiducials, but Data Matrix, PDF417 and every 1D
//! symbology have none. What they *do* share is a dense field of dark/light edges that
//! blank background and smooth scene content lack. This stage measures that.
//!
//! The reduced dark mask is split into a grid of square tiles. For each tile we count
//! horizontal and vertical dark/light transitions in one pass; a tile whose transition
//! density clears a threshold is "active". Each active tile is then *labelled* by its
//! own edge balance before any clustering: a tile whose horizontal and vertical
//! transition counts are strongly one-directional is a **linear** tile (and remembers
//! whether horizontal or vertical edges dominate), while a balanced tile is a **matrix**
//! tile. Flood fill then groups only tiles that share the *same* label into regions.
//!
//! Per-tile labelling before clustering is what lets a 1D barcode be pulled out of a
//! busy scene. A barcode is a compact patch of tiles that all lean the same way
//! (vertical bars ⇒ horizontal-edge-dominant); the printed text and artwork around it
//! are edge-dense too, but *isotropic*, so they label as matrix and never merge into the
//! barcode's component. Labelling the whole active blob at once — the old approach —
//! averaged the barcode's strong anisotropy away against the surrounding text and lost
//! the code entirely.

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

/// Per-tile classification assigned before clustering. Flood fill only joins tiles that
/// carry the same label, keeping a directional barcode patch out of the isotropic text
/// and artwork it sits amongst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Label {
    /// Below the edge-density floor: ignored.
    Inactive,
    /// Active and horizontal-edge-dominant (upright bars).
    LinearH,
    /// Active and vertical-edge-dominant (bars rotated ~90°).
    LinearV,
    /// Active but balanced: a 2D matrix or plain scene texture.
    Matrix,
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
///
/// The box is the *core* box — the clustered tiles only, no quiet zone. The caller
/// grows linear boxes for downstream scanning; keeping the core here lets it measure
/// region statistics (bar coherence, overlap) on the code itself.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Region {
    /// Inclusive-exclusive bounding box in reduced pixels: `[x0, x1) × [y0, y1)`.
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
    /// Coarse family guess.
    pub family: Family,
    /// For [`Family::Linear`]: `true` when horizontal edges dominate (upright bars, the
    /// code reads along x), `false` for the ~90°-rotated case. Meaningless for matrix.
    pub reads_horizontal: bool,
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

        let w = grid.width;
        for y in 0..grid.height {
            let ty = y / tile;
            let base = ty * cols;
            let row = &grid.dark[y * w..(y + 1) * w];
            let prev = (y >= 1).then(|| &grid.dark[(y - 1) * w..y * w]);
            for (x, &d) in row.iter().enumerate() {
                let idx = base + x / tile;
                area[idx] += 1;
                if x >= 1 && d != row[x - 1] {
                    htrans[idx] += 1;
                }
                if let Some(prev) = prev
                    && d != prev[x]
                {
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

    // Per-tile label decided *before* clustering, so a tile's own edge balance — not the
    // average over a whole merged blob — chooses its family. Flood fill later joins only
    // tiles that carry the same label.
    let label: Vec<Label> = (0..cols * rows)
        .map(|i| {
            let a = stats.area[i];
            if a == 0 {
                return Label::Inactive;
            }
            let h = stats.htrans[i];
            let v = stats.vtrans[i];
            let total = h + v;
            if total as f32 / (a as f32) < edge_density {
                return Label::Inactive;
            }
            // Anisotropy of this single tile. Vertical bars cross many row scans and few
            // column scans, so a barcode tile is strongly horizontal-dominant (and a
            // 90°-rotated one vertical-dominant); text and artwork are balanced.
            let anisotropy = (h as f32 - v as f32).abs() / total as f32;
            if anisotropy >= aniso {
                if h >= v {
                    Label::LinearH
                } else {
                    Label::LinearV
                }
            } else {
                Label::Matrix
            }
        })
        .collect();

    // 8-connected flood fill that only merges tiles sharing the start tile's label.
    let mut visited = vec![false; cols * rows];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut out = Vec::new();

    for sy in 0..rows {
        for sx in 0..cols {
            let start = sy * cols + sx;
            let seed = label[start];
            if seed == Label::Inactive || visited[start] {
                continue;
            }
            visited[start] = true;
            stack.push((sx, sy));

            let mut min_tx = sx;
            let mut max_tx = sx;
            let mut min_ty = sy;
            let mut max_ty = sy;
            let mut tiles = 0usize;

            while let Some((cx, cy)) = stack.pop() {
                tiles += 1;
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
                        if !visited[nidx] && label[nidx] == seed {
                            visited[nidx] = true;
                            stack.push((nx, ny));
                        }
                    }
                }
            }

            if tiles < min_tiles {
                continue;
            }

            let family = match seed {
                Label::LinearH | Label::LinearV => Family::Linear,
                _ => Family::Matrix,
            };

            // A 2D matrix code always extends in both axes; a one-tile-thin strip of
            // balanced tiles is not one. These strips appear where a barcode's bars
            // terminate (the row of bar-ends adds vertical edges that cancel the
            // horizontal dominance) and as thin runs of text — reject them so the
            // matrix family stays meaningful. Linear codes are legitimately thin.
            if family == Family::Matrix && (max_tx - min_tx < 1 || max_ty - min_ty < 1) {
                continue;
            }

            let t = stats.tile;
            out.push(Region {
                x0: min_tx * t,
                y0: min_ty * t,
                x1: ((max_tx + 1) * t).min(grid.width),
                y1: ((max_ty + 1) * t).min(grid.height),
                family,
                reads_horizontal: seed != Label::LinearV,
            });
        }
    }
    out
}
