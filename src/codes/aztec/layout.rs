//! Aztec symbol geometry: the central bullseye, orientation marks, reference grid,
//! mode-message ring positions and the spiral "domino" data path.
//!
//! A [`Layout`] is built from `(compact, layers)` and knows how to draw the fixed
//! patterns and to enumerate, in message-bit order, the module coordinates of the
//! mode message and the data region. Encoder and decoder share this so placement and
//! reading are guaranteed to agree.

use super::tables::total_bits_in_layer;
use crate::output::BitMatrix;

/// Fixed geometry of one Aztec symbol size.
#[derive(Debug, Clone)]
pub struct Layout {
    /// Compact (`true`) or full-range (`false`) symbol.
    pub compact: bool,
    /// Number of data layers.
    pub layers: usize,
    /// The base matrix size (before reference-grid insertion).
    pub base_size: usize,
    /// The physical matrix side length (with reference grid).
    pub size: usize,
    /// Physical index of the symbol centre.
    pub center: usize,
    /// Maps a base coordinate (`0..base_size`) to a physical coordinate, inserting
    /// reference-grid lines for full symbols. Identity for compact symbols.
    pub alignment_map: Vec<usize>,
}

impl Layout {
    /// Build the layout for a `(compact, layers)` combination.
    pub fn new(compact: bool, layers: usize) -> Layout {
        let base_size = (if compact { 11 } else { 14 }) + layers * 4;
        let (size, alignment_map) = if compact {
            (base_size, (0..base_size).collect())
        } else {
            let size = base_size + 1 + 2 * ((base_size / 2 - 1) / 15);
            let orig_center = base_size / 2;
            let center = size / 2;
            let mut map = vec![0usize; base_size];
            for i in 0..orig_center {
                let new_offset = i + i / 15;
                map[orig_center - i - 1] = center - new_offset - 1;
                map[orig_center + i] = center + new_offset + 1;
            }
            (size, map)
        };
        Layout {
            compact,
            layers,
            base_size,
            size,
            center: size / 2,
            alignment_map,
        }
    }

    /// Whether physical coordinate `p` lies on a reference-grid line (full symbols).
    fn is_grid_line(&self, p: usize) -> bool {
        if self.compact {
            return false;
        }
        let d = p.abs_diff(self.center);
        d.is_multiple_of(16)
    }

    /// Draw the reference grid (full symbols only): a centred checkerboard along every
    /// grid line, dark where `(x + y)` is even.
    fn draw_reference_grid(&self, m: &mut BitMatrix) {
        if self.compact {
            return;
        }
        for y in 0..self.size {
            for x in 0..self.size {
                if (self.is_grid_line(x) || self.is_grid_line(y)) && (x + y) % 2 == 0 {
                    m.set(x, y, true);
                }
            }
        }
    }

    /// Draw the concentric bullseye rings around the centre.
    fn draw_bullseye(&self, m: &mut BitMatrix) {
        let c = self.center;
        let rings = if self.compact { 5 } else { 7 };
        let mut i = 0;
        while i < rings {
            for j in (c - i)..=(c + i) {
                m.set(j, c - i, true);
                m.set(j, c + i, true);
                m.set(c - i, j, true);
                m.set(c + i, j, true);
            }
            i += 2;
        }
    }

    /// Draw the orientation marks at the corners of the mode-message ring. The
    /// asymmetric pattern (top-left carries an extra module, bottom-right is empty)
    /// fixes the reading orientation per ISO/IEC 24778.
    fn draw_orientation(&self, m: &mut BitMatrix) {
        let c = self.center;
        let r = if self.compact { 5 } else { 7 };
        // Top-left: two modules; top-right and bottom-left: one; bottom-right: none.
        m.set(c - r, c - r, true);
        m.set(c - r, c - r + 1, true);
        m.set(c + r, c - r, true);
        m.set(c - r, c + r, true);
    }

    /// Coordinates of the mode-message ring, indexed by mode-message bit.
    pub fn mode_positions(&self) -> Vec<(usize, usize)> {
        let c = self.center;
        if self.compact {
            let mut pos = vec![(0, 0); 28];
            for i in 0..7 {
                let offset = c - 3 + i;
                pos[i] = (offset, c - 5);
                pos[i + 7] = (c + 5, offset);
                pos[20 - i] = (offset, c + 5);
                pos[27 - i] = (c - 5, offset);
            }
            pos
        } else {
            let mut pos = vec![(0, 0); 40];
            for i in 0..10 {
                let offset = c - 5 + i + i / 5;
                pos[i] = (offset, c - 7);
                pos[i + 10] = (c + 7, offset);
                pos[29 - i] = (offset, c + 7);
                pos[39 - i] = (c - 7, offset);
            }
            pos
        }
    }

    /// Coordinates of the data region, indexed by data-message bit. The order is the
    /// spiral of two-module dominoes, layer by layer from the outside in.
    pub fn data_positions(&self) -> Vec<(usize, usize)> {
        let am = &self.alignment_map;
        let b = self.base_size;
        let low = if self.compact { 9 } else { 12 };
        let total = total_bits_in_layer(self.layers, self.compact);
        let mut pos = vec![(0usize, 0usize); total];
        let mut row_offset = 0usize;
        for i in 0..self.layers {
            let row_size = (self.layers - i) * 4 + low;
            for j in 0..row_size {
                let col = j * 2;
                for k in 0..2 {
                    pos[row_offset + col + k] = (am[i * 2 + k], am[i * 2 + j]);
                    pos[row_offset + row_size * 2 + col + k] =
                        (am[i * 2 + j], am[b - 1 - i * 2 - k]);
                    pos[row_offset + row_size * 4 + col + k] =
                        (am[b - 1 - i * 2 - k], am[b - 1 - i * 2 - j]);
                    pos[row_offset + row_size * 6 + col + k] =
                        (am[b - 1 - i * 2 - j], am[i * 2 + k]);
                }
            }
            row_offset += row_size * 8;
        }
        pos
    }

    /// Draw all fixed patterns (grid, bullseye, orientation) onto a fresh matrix.
    pub fn draw_fixed(&self, m: &mut BitMatrix) {
        self.draw_reference_grid(m);
        self.draw_bullseye(m);
        self.draw_orientation(m);
    }
}
