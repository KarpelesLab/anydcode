//! Barcode-texture tiling and region clustering.
//!
//! Finder patterns locate QR/Aztec fiducials, but Data Matrix, PDF417 and every 1D
//! symbology have none. What they *do* share is a dense field of dark/light edges that
//! blank background and smooth scene content lack. This stage measures that.
//!
//! The reduced image is split into a grid of square tiles, and each tile's luminance
//! gradients are summed into a 2×2 **structure tensor**. Three things fall out of it:
//!
//! * how much of the tile is edge at all (its *edge density*) — a tile below the floor
//!   is background and ignored;
//! * how far the tile's edges agree on one direction (its *coherence*, 0‥1) — the bars
//!   of a linear code all point the gradient along the reading axis, so a barcode tile
//!   scores near 1 **at any rotation**, while a 2D matrix, text and scene texture spread
//!   their gradients and score low;
//! * for a coherent tile, *which* direction that is — the code's reading axis.
//!
//! Each active tile is labelled linear or matrix from its own coherence before any
//! clustering, and flood fill then only joins tiles of the same label — and, for linear
//! tiles, a compatible axis. Labelling per tile is what lets a barcode be pulled out of
//! a busy scene: the print around it is edge-dense too, but incoherent, so it never
//! merges into the barcode's component. A linear region reports its axis and an
//! *oriented* box, so a rotated barcode yields a tight box and a known scan direction
//! instead of a loose axis-aligned one.
//!
//! Gradients, unlike a binarized mask, do not depend on a threshold being right: a code
//! in shadow or on a dark surface has the same edges, only weaker, and the edge test
//! adapts to each tile's own contrast.

use super::grid::DownGrid;
use alloc::{vec, vec::Vec};

/// Least luminance range (max − min) a tile must span to be considered at all. Sensor
/// noise alone spans ~25 levels across a tile; print of any legible contrast far more.
const MIN_TILE_CONTRAST: i32 = 36;

/// A pixel is an *edge pixel* when its gradient magnitude (per-pixel 2×2 differences) reaches
/// this fraction of the tile's luminance range …
const EDGE_FRACTION: f32 = 0.25;
/// … and at least this absolute value, the noise floor.
const MIN_EDGE_GRADIENT: i32 = 16;

/// Largest angle (radians) between the axes of two neighbouring linear tiles for them
/// to belong to the same code. Generous enough for a label wrapped around a bottle,
/// tight enough to keep a barcode apart from differently-slanted print beside it.
const MAX_AXIS_STEP: f32 = 20.0 * core::f32::consts::PI / 180.0;

/// Per-tile gradient statistics.
#[derive(Debug, Clone, Copy, Default)]
struct Tile {
    /// Structure tensor sums over the tile's edge pixels.
    sxx: f32,
    syy: f32,
    sxy: f32,
    /// Number of edge pixels.
    edges: u32,
    /// Pixel count (border tiles are smaller).
    area: u32,
}

impl Tile {
    /// Gradient coherence in `[0, 1]`: 1 when every edge shares one direction.
    fn coherence(&self) -> f32 {
        let trace = self.sxx + self.syy;
        if trace <= 0.0 {
            return 0.0;
        }
        let d = self.sxx - self.syy;
        (d * d + 4.0 * self.sxy * self.sxy).sqrt() / trace
    }

    /// Dominant gradient direction — the reading axis — in `(-π/2, π/2]`.
    fn axis(&self) -> f32 {
        axis_of(self.sxx, self.syy, self.sxy)
    }
}

/// Dominant direction of a structure tensor, in `(-π/2, π/2]`.
fn axis_of(sxx: f32, syy: f32, sxy: f32) -> f32 {
    0.5 * (2.0 * sxy).atan2(sxx - syy)
}

/// Angle between two axes (orientations mod π), in `[0, π/2]`.
fn axis_distance(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(core::f32::consts::PI);
    d.min(core::f32::consts::PI - d)
}

/// Per-tile classification assigned before clustering.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Label {
    /// Below the edge-density floor: ignored.
    Inactive,
    /// Active and coherent: a patch of bars reading along the given axis.
    Linear(f32),
    /// Active but incoherent: a 2D matrix or plain scene texture.
    Matrix,
}

/// The coarse layout family a region is guessed to belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Family {
    /// Two-directional edge field: a 2D matrix code.
    Matrix,
    /// One-directional edge field: a 1D / stacked linear code.
    Linear,
}

/// A box aligned with a linear region's reading axis, in reduced-pixel coordinates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OrientedBox {
    /// Centre.
    pub cx: f32,
    pub cy: f32,
    /// Reading axis (the direction scan lines should run), radians in `(-π/2, π/2]`.
    pub angle: f32,
    /// Half-extent along the reading axis.
    pub half_read: f32,
    /// Half-extent along the bars.
    pub half_bars: f32,
    /// Fraction of the box covered by the region's own tiles (rotation-normalised, so a
    /// solid block scores ~1 at any angle). A barcode fills its box with coherent tiles;
    /// a chain of tiles along a rule or a box border, or a few scattered strokes bridged
    /// together, spans a box it mostly leaves empty.
    pub fill: f32,
}

impl OrientedBox {
    /// The four corners, clockwise in the box's own frame starting from the corner at
    /// the start of the reading axis on the upper side.
    pub fn corners(&self) -> [(f32, f32); 4] {
        let (s, c) = self.angle.sin_cos();
        let (ux, uy) = (c * self.half_read, s * self.half_read);
        let (vx, vy) = (-s * self.half_bars, c * self.half_bars);
        [
            (self.cx - ux - vx, self.cy - uy - vy),
            (self.cx + ux - vx, self.cy + uy - vy),
            (self.cx + ux + vx, self.cy + uy + vy),
            (self.cx - ux + vx, self.cy - uy + vy),
        ]
    }
}

/// A clustered candidate region in reduced-pixel coordinates.
///
/// The boxes are the *core* — the clustered tiles only, no quiet zone. The caller grows
/// linear boxes for downstream scanning; keeping the core here lets it measure region
/// statistics (bar coherence, overlap) on the code itself.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Region {
    /// Inclusive-exclusive axis-aligned bounding box: `[x0, x1) × [y0, y1)`.
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
    /// Coarse family guess.
    pub family: Family,
    /// For [`Family::Linear`]: the box aligned with the reading axis.
    pub oriented: Option<OrientedBox>,
}

impl Region {
    /// Region area in reduced pixels.
    pub fn area(&self) -> usize {
        (self.x1 - self.x0) * (self.y1 - self.y0)
    }
}

/// Accumulate gradient statistics per tile.
fn tile_stats(grid: &DownGrid, tile: usize, cols: usize, rows: usize) -> Vec<Tile> {
    let (w, h) = (grid.width, grid.height);
    let mut tiles = vec![Tile::default(); cols * rows];
    if w < 3 || h < 3 {
        return tiles;
    }

    for ty in 0..rows {
        let (y0, y1) = (ty * tile, ((ty + 1) * tile).min(h));
        for tx in 0..cols {
            let (x0, x1) = (tx * tile, ((tx + 1) * tile).min(w));
            let t = &mut tiles[ty * cols + tx];
            t.area = ((x1 - x0) * (y1 - y0)) as u32;

            // The tile's own contrast sets what counts as an edge inside it.
            let (mut lo, mut hi) = (255u8, 0u8);
            for y in y0..y1 {
                for &v in &grid.luma[y * w + x0..y * w + x1] {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            let range = i32::from(hi) - i32::from(lo);
            if range < MIN_TILE_CONTRAST {
                continue;
            }
            let floor = ((range as f32 * EDGE_FRACTION) as i32).max(MIN_EDGE_GRADIENT);

            // 2×2 differences (both components measured at the same half-pixel point),
            // not central ones. A central difference responds as sin(ω) to a pattern
            // of spatial frequency ω, which near the pixel limit — exactly where fine
            // bars live — flattens the larger gradient component and rotates the
            // measured axis toward 45° (by 7° for two-pixel modules at 17°). The 2×2
            // operator's error there is under half that, and of the opposite sign.
            for y in y0..y1.min(h - 1) {
                for x in x0..x1.min(w - 1) {
                    let i = y * w + x;
                    let (a, b) = (i32::from(grid.luma[i]), i32::from(grid.luma[i + 1]));
                    let (c, d) = (i32::from(grid.luma[i + w]), i32::from(grid.luma[i + w + 1]));
                    // Twice the gradient; `floor` is doubled to match below.
                    let gx = (b - a) + (d - c);
                    let gy = (c - a) + (d - b);
                    let floor = 2 * floor;
                    // Euclidean magnitude: an |gx| + |gy| test admits diagonal
                    // gradients more readily than axis-aligned ones of equal strength,
                    // which drags every measured axis toward 45°.
                    if gx * gx + gy * gy < floor * floor {
                        continue;
                    }
                    let (gx, gy) = (gx as f32, gy as f32);
                    t.sxx += gx * gx;
                    t.syy += gy * gy;
                    t.sxy += gx * gy;
                    t.edges += 1;
                }
            }
        }
    }
    tiles
}

/// Detect candidate regions in `grid`.
///
/// * `tile` — tile edge length in reduced pixels.
/// * `edge_density` — minimum fraction of a tile's pixels that must be edge pixels for
///   the tile to be active.
/// * `min_tiles` — minimum active tiles for a region to be reported.
/// * `coherence` — gradient coherence at or above which a tile is labelled linear.
pub(crate) fn regions(
    grid: &DownGrid,
    tile: usize,
    edge_density: f32,
    min_tiles: usize,
    coherence: f32,
    debug: bool,
) -> Vec<Region> {
    let tile = tile.max(2);
    let cols = grid.width.div_ceil(tile);
    let rows = grid.height.div_ceil(tile);
    let tiles = tile_stats(grid, tile, cols, rows);

    // Per-tile label decided *before* clustering, so a tile's own edge field — not the
    // average over a whole merged blob — chooses its family.
    let label: Vec<Label> = tiles
        .iter()
        .map(|t| {
            if t.area == 0 || (t.edges as f32) < edge_density * t.area as f32 {
                Label::Inactive
            } else if t.coherence() >= coherence {
                Label::Linear(t.axis())
            } else {
                Label::Matrix
            }
        })
        .collect();

    if debug {
        // One character per tile: '.' inactive, a digit for a matrix tile's coherence
        // (tenths), a letter for a linear tile's axis ('a' = -90°, 10° per letter).
        for ty in 0..rows {
            let line: alloc::string::String = (0..cols)
                .map(|tx| match label[ty * cols + tx] {
                    Label::Inactive => '.',
                    Label::Matrix => {
                        let c = (tiles[ty * cols + tx].coherence() * 10.0) as u32;
                        char::from_digit(c.min(9), 10).unwrap_or('?')
                    }
                    Label::Linear(a) => {
                        (b'a' + ((a.to_degrees() + 90.0) / 10.0).clamp(0.0, 17.0) as u8) as char
                    }
                })
                .collect();
            std::eprintln!("tiles {line}");
        }
    }

    // Flood fill that only merges compatible tiles. For *linear* tiles expansion
    // reaches Chebyshev distance 2, bridging a one-tile gap: a bar or space wider than
    // a tile (3–4-module runs at a coarse module scale) has no edges inside it, and the
    // resulting inactive seam would otherwise shatter one barcode into fragments that
    // read as implausibly narrow. Matrix tiles stay 8-connected — a 2D code is
    // edge-dense throughout, and bridging would only glue it to nearby scene texture.
    let mut visited = vec![false; cols * rows];
    let mut stack: Vec<usize> = Vec::new();
    let mut members: Vec<usize> = Vec::new();
    let mut out = Vec::new();

    for start in 0..cols * rows {
        if label[start] == Label::Inactive || visited[start] {
            continue;
        }
        let linear = matches!(label[start], Label::Linear(_));
        let reach = if linear { 2 } else { 1 };
        visited[start] = true;
        stack.push(start);
        members.clear();

        while let Some(cur) = stack.pop() {
            members.push(cur);
            let (cx, cy) = (cur % cols, cur / cols);
            for ny in cy.saturating_sub(reach)..=(cy + reach).min(rows - 1) {
                for nx in cx.saturating_sub(reach)..=(cx + reach).min(cols - 1) {
                    let n = ny * cols + nx;
                    if visited[n] {
                        continue;
                    }
                    let joins = match (label[cur], label[n]) {
                        (Label::Matrix, Label::Matrix) => true,
                        (Label::Linear(a), Label::Linear(b)) => {
                            axis_distance(a, b) <= MAX_AXIS_STEP
                        }
                        _ => false,
                    };
                    if joins {
                        visited[n] = true;
                        stack.push(n);
                    }
                }
            }
        }

        if members.len() < min_tiles {
            continue;
        }

        let (mut min_tx, mut max_tx, mut min_ty, mut max_ty) = (cols, 0, rows, 0);
        for &m in &members {
            min_tx = min_tx.min(m % cols);
            max_tx = max_tx.max(m % cols);
            min_ty = min_ty.min(m / cols);
            max_ty = max_ty.max(m / cols);
        }

        // A 2D matrix code always extends in both axes; a one-tile-thin strip of
        // incoherent tiles is not one. These strips appear where a barcode's bars
        // terminate (the row of bar-ends mixes in the other gradient direction) and as
        // thin runs of text — reject them so the matrix family stays meaningful.
        // Linear codes are legitimately thin.
        if !linear && (max_tx == min_tx || max_ty == min_ty) {
            continue;
        }

        let oriented = linear.then(|| oriented_box(&tiles, &members, cols, tile, grid));
        out.push(Region {
            x0: min_tx * tile,
            y0: min_ty * tile,
            x1: ((max_tx + 1) * tile).min(grid.width),
            y1: ((max_ty + 1) * tile).min(grid.height),
            family: if linear {
                Family::Linear
            } else {
                Family::Matrix
            },
            oriented,
        });
    }
    out
}

/// The box around `members` aligned with their pooled reading axis.
fn oriented_box(
    tiles: &[Tile],
    members: &[usize],
    cols: usize,
    tile: usize,
    grid: &DownGrid,
) -> OrientedBox {
    // Pool the tensors: the region's axis is its energy-weighted consensus.
    let (mut sxx, mut syy, mut sxy) = (0.0f32, 0.0f32, 0.0f32);
    for &m in members {
        sxx += tiles[m].sxx;
        syy += tiles[m].syy;
        sxy += tiles[m].sxy;
    }
    let angle = axis_of(sxx, syy, sxy);
    let (s, c) = angle.sin_cos();

    // Extent of the member tiles' corners along the reading axis (u) and the bars (v).
    let (mut u0, mut u1, mut v0, mut v1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for &m in members {
        let (tx, ty) = (m % cols, m / cols);
        let (x0, y0) = ((tx * tile) as f32, (ty * tile) as f32);
        let x1 = (((tx + 1) * tile).min(grid.width)) as f32;
        let y1 = (((ty + 1) * tile).min(grid.height)) as f32;
        for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
            let u = c * x + s * y;
            let v = -s * x + c * y;
            u0 = u0.min(u);
            u1 = u1.max(u);
            v0 = v0.min(v);
            v1 = v1.max(v);
        }
    }
    let (um, vm) = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
    OrientedBox {
        cx: c * um - s * vm,
        cy: s * um + c * vm,
        angle,
        half_read: (u1 - u0) / 2.0,
        half_bars: (v1 - v0) / 2.0,
        // The box is measured around whole tiles, and a square tile projects onto a
        // rotated axis as (|cos| + |sin|) tile — 1.41 tiles at 45° — so normalise the
        // box back to the extent the same tiles would have had axis-aligned.
        fill: (members.len() * tile * tile) as f32 * (c.abs() + s.abs()).powi(2)
            / ((u1 - u0) * (v1 - v0)).max(1.0),
    }
}
