//! ECC 200 module placement (ISO/IEC 16022 Annex F) and finder/timing borders.
//!
//! [`place`] implements the "utah" diagonal placement that maps codeword bits onto
//! the *mapping matrix* (the data area, with every region's border stripped),
//! reporting each module position to a visitor. It needs no allocation: the only
//! working state is a caller-provided occupancy bitmap. The encoder writes codeword
//! bits through it and the decoder reads them back, so both share one placement.
//!
//! [`mapping_to_symbol`] converts mapping-matrix coordinates to full-symbol
//! coordinates, and [`draw_borders`] paints the L-shaped solid finder and the
//! alternating timing pattern for every data region.

use super::tables::SquareSpec;
#[cfg(feature = "encode")]
use crate::output::MatrixBuf;

/// Quiet-zone width required around a Data Matrix symbol, in modules (ISO/IEC 16022
/// specifies at least one module on every side).
pub const QUIET_ZONE: usize = 1;

/// Bytes of occupancy bitmap [`place`] needs for an `nrows × ncols` mapping matrix.
pub const fn occupancy_bytes(nrows: usize, ncols: usize) -> usize {
    (nrows * ncols).div_ceil(8)
}

/// Annex F placement state: the occupancy bitmap plus the codeword visitor.
struct Placer<'o, F> {
    nrow: usize,
    ncol: usize,
    occupied: &'o mut [u8],
    /// Index of the codeword currently being placed.
    chr: usize,
    visit: F,
}

impl<F: FnMut(usize, usize, usize, usize)> Placer<'_, F> {
    fn is_occupied(&self, row: usize, col: usize) -> bool {
        let i = row * self.ncol + col;
        self.occupied[i / 8] & (1 << (i % 8)) != 0
    }

    /// Record bit `bit` (1..=8) of the current codeword at `(row, col)`, wrapping
    /// negative coordinates per Annex F.
    fn module(&mut self, mut row: isize, mut col: isize, bit: usize) {
        let nrow = self.nrow as isize;
        let ncol = self.ncol as isize;
        if row < 0 {
            row += nrow;
            col += 4 - ((nrow + 4) % 8);
        }
        if col < 0 {
            col += ncol;
            row += 4 - ((ncol + 4) % 8);
        }
        let r = row as usize;
        let c = col as usize;
        let i = r * self.ncol + c;
        self.occupied[i / 8] |= 1 << (i % 8);
        (self.visit)(self.chr, bit - 1, r, c);
    }

    fn char_at(&mut self, positions: [(isize, isize); 8]) {
        for (bit, (r, c)) in positions.into_iter().enumerate() {
            self.module(r, c, bit + 1);
        }
        self.chr += 1;
    }

    fn utah(&mut self, row: isize, col: isize) {
        self.char_at([
            (row - 2, col - 2),
            (row - 2, col - 1),
            (row - 1, col - 2),
            (row - 1, col - 1),
            (row - 1, col),
            (row, col - 2),
            (row, col - 1),
            (row, col),
        ]);
    }

    fn corner1(&mut self) {
        let (nr, nc) = (self.nrow as isize, self.ncol as isize);
        self.char_at([
            (nr - 1, 0),
            (nr - 1, 1),
            (nr - 1, 2),
            (0, nc - 2),
            (0, nc - 1),
            (1, nc - 1),
            (2, nc - 1),
            (3, nc - 1),
        ]);
    }

    fn corner2(&mut self) {
        let (nr, nc) = (self.nrow as isize, self.ncol as isize);
        self.char_at([
            (nr - 3, 0),
            (nr - 2, 0),
            (nr - 1, 0),
            (0, nc - 4),
            (0, nc - 3),
            (0, nc - 2),
            (0, nc - 1),
            (1, nc - 1),
        ]);
    }

    fn corner3(&mut self) {
        let (nr, nc) = (self.nrow as isize, self.ncol as isize);
        self.char_at([
            (nr - 3, 0),
            (nr - 2, 0),
            (nr - 1, 0),
            (0, nc - 2),
            (0, nc - 1),
            (1, nc - 1),
            (2, nc - 1),
            (3, nc - 1),
        ]);
    }

    fn corner4(&mut self) {
        let (nr, nc) = (self.nrow as isize, self.ncol as isize);
        self.char_at([
            (nr - 1, 0),
            (nr - 1, nc - 1),
            (0, nc - 3),
            (0, nc - 2),
            (0, nc - 1),
            (1, nc - 3),
            (1, nc - 2),
            (1, nc - 1),
        ]);
    }
}

/// Run Annex F placement over an `nrows × ncols` mapping matrix.
///
/// `visit(codeword, bit, row, col)` is called once per placed module, where `bit` 0
/// is the most-significant bit (weight 128) and 7 the least-significant (weight 1).
/// `occupied` is working storage of at least [`occupancy_bytes`] bytes (its contents
/// are overwritten). Returns the number of codewords placed and whether the
/// bottom-right fixed pattern applies: when it does, mapping modules
/// `(nrows-1, ncols-1)` and `(nrows-2, ncols-2)` are dark and carry no data.
///
/// # Panics
/// Panics if `occupied` is shorter than [`occupancy_bytes`]`(nrows, ncols)`.
pub fn place<F: FnMut(usize, usize, usize, usize)>(
    nrows: usize,
    ncols: usize,
    occupied: &mut [u8],
    visit: F,
) -> (usize, bool) {
    let occupied = &mut occupied[..occupancy_bytes(nrows, ncols)];
    occupied.fill(0);
    let mut p = Placer {
        nrow: nrows,
        ncol: ncols,
        occupied,
        chr: 0,
        visit,
    };
    let nr = nrows as isize;
    let nc = ncols as isize;
    let mut row: isize = 4;
    let mut col: isize = 0;

    loop {
        // Corner cases, evaluated before each diagonal sweep.
        if row == nr && col == 0 {
            p.corner1();
        }
        if row == nr - 2 && col == 0 && !ncols.is_multiple_of(4) {
            p.corner2();
        }
        if row == nr - 2 && col == 0 && ncols % 8 == 4 {
            p.corner3();
        }
        if row == nr + 4 && col == 2 && ncols.is_multiple_of(8) {
            p.corner4();
        }

        // Sweep up and to the right.
        loop {
            if row < nr && col >= 0 && !p.is_occupied(row as usize, col as usize) {
                p.utah(row, col);
            }
            row -= 2;
            col += 2;
            if row < 0 || col >= nc {
                break;
            }
        }
        row += 1;
        col += 3;

        // Sweep down and to the left.
        loop {
            if row >= 0 && col < nc && !p.is_occupied(row as usize, col as usize) {
                p.utah(row, col);
            }
            row += 2;
            col -= 2;
            if row >= nr || col < 0 {
                break;
            }
        }
        row += 3;
        col += 1;

        if row >= nr && col >= nc {
            break;
        }
    }

    // Fixed pattern in the bottom-right corner if it was never populated.
    let fixed = !p.is_occupied(nrows - 1, ncols - 1);
    (p.chr, fixed)
}

/// Full-symbol `(x, y)` of mapping-matrix module `(row, col)`: each data region adds a
/// one-module border on all four sides.
pub fn mapping_to_symbol(spec: &SquareSpec, row: usize, col: usize) -> (usize, usize) {
    let rr = spec.region_size;
    (col + 1 + 2 * (col / rr), row + 1 + 2 * (row / rr))
}

/// Paint every data region's finder/timing border into `m` (a `symbol_size²` grid).
///
/// Mirrors ISO/IEC 16022: for each region, the top edge and right edge carry the
/// alternating timing pattern, while the left edge and bottom edge are solid. The top
/// and bottom edge rows span the full symbol width.
#[cfg(feature = "encode")]
pub fn draw_borders(spec: &SquareSpec, m: &mut MatrixBuf<'_>) {
    let size = spec.symbol_size;
    let step = spec.region_size + 2;
    for oy in 0..size {
        let t = oy % step;
        if t == 0 {
            // Top edge: alternating, dark on even columns.
            for ox in 0..size {
                m.set(ox, oy, ox % 2 == 0);
            }
        } else if t == step - 1 {
            // Bottom edge: solid.
            for ox in 0..size {
                m.set(ox, oy, true);
            }
        } else {
            for ox in (0..size).step_by(step) {
                // Left edge solid; right edge alternating, dark on odd rows.
                m.set(ox, oy, true);
                m.set(ox + step - 1, oy, oy % 2 == 1);
            }
        }
    }
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;
    use crate::codes::datamatrix::tables::all_squares;
    use alloc::vec;

    #[test]
    fn placement_covers_every_codeword() {
        for spec in all_squares() {
            let ms = spec.mapping_size();
            let mut occ = vec![0u8; occupancy_bytes(ms, ms)];
            let mut seen = vec![0u32; ms * ms];
            let mut bits = 0usize;
            let (count, fixed) = place(ms, ms, &mut occ, |cw, bit, r, c| {
                assert!(r < ms && c < ms);
                assert_eq!(cw * 8 + bit, bits, "visit order");
                bits += 1;
                seen[r * ms + c] += 1;
            });
            assert_eq!(
                count,
                spec.total_cw(),
                "symbol {} codeword count",
                spec.symbol_size
            );
            if fixed {
                seen[(ms - 1) * ms + ms - 1] += 1;
                seen[(ms - 2) * ms + ms - 2] += 1;
            }
            // Placed bits and fixed corner modules must be unique and in range.
            assert!(
                seen.iter().all(|&n| n <= 1),
                "overlap in symbol {}",
                spec.symbol_size
            );
        }
    }

    #[test]
    fn borders_leave_mapping_modules_free() {
        for spec in all_squares() {
            let ms = spec.mapping_size();
            let size = spec.symbol_size;
            let mut storage = vec![0u8; MatrixBuf::bytes_for(size, size)];
            let mut m = MatrixBuf::new(&mut storage, size, size, QUIET_ZONE).unwrap();
            draw_borders(spec, &mut m);
            // Every non-mapping module is a border; the mapping ones stay untouched.
            let mut is_mapping = vec![false; size * size];
            for r in 0..ms {
                for c in 0..ms {
                    let (x, y) = mapping_to_symbol(spec, r, c);
                    assert!(!is_mapping[y * size + x]);
                    is_mapping[y * size + x] = true;
                    assert!(!m.get(x, y));
                }
            }
            let borders = is_mapping.iter().filter(|&&b| !b).count();
            assert_eq!(borders, size * size - ms * ms);
            // The finder L: left column and bottom row fully dark.
            assert!((0..size).all(|i| m.get(0, i) && m.get(i, size - 1)));
        }
    }
}
