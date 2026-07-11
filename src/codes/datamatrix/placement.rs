//! ECC 200 module placement (ISO/IEC 16022 Annex F) and finder/timing borders.
//!
//! [`placement_map`] implements the "utah" diagonal placement that maps codeword
//! bits onto the *mapping matrix* (the data area, with every region's border
//! stripped). [`render_borders`] wraps a filled mapping matrix with the L-shaped
//! solid finder and the alternating timing pattern for each data region;
//! [`strip_borders`] is its inverse, used when decoding.

use super::tables::SquareSpec;
use crate::output::BitMatrix;

/// The result of running Annex F placement over an `nrows × ncols` mapping matrix.
pub struct Placement {
    /// For each codeword, the eight `(row, col)` module positions. Index 0 is the
    /// most-significant bit (weight 128), index 7 the least-significant (weight 1).
    pub codewords: Vec<[(usize, usize); 8]>,
    /// Modules set to a fixed dark value (the bottom-right corner special case);
    /// they carry no data.
    pub fixed_dark: Vec<(usize, usize)>,
}

struct Placer {
    nrow: usize,
    ncol: usize,
    cells: Vec<[(usize, usize); 8]>,
    occupied: Vec<bool>,
    chr: usize,
}

impl Placer {
    fn new(nrow: usize, ncol: usize) -> Self {
        Placer {
            nrow,
            ncol,
            cells: Vec::new(),
            occupied: vec![false; nrow * ncol],
            chr: 0,
        }
    }

    fn is_occupied(&self, row: usize, col: usize) -> bool {
        self.occupied[row * self.ncol + col]
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
        self.occupied[r * self.ncol + c] = true;
        let idx = self.chr;
        self.cells[idx][bit - 1] = (r, c);
    }

    fn begin_char(&mut self) {
        self.chr = self.cells.len();
        self.cells.push([(0, 0); 8]);
    }

    fn utah(&mut self, row: isize, col: isize) {
        self.begin_char();
        self.module(row - 2, col - 2, 1);
        self.module(row - 2, col - 1, 2);
        self.module(row - 1, col - 2, 3);
        self.module(row - 1, col - 1, 4);
        self.module(row - 1, col, 5);
        self.module(row, col - 2, 6);
        self.module(row, col - 1, 7);
        self.module(row, col, 8);
    }

    fn corner1(&mut self) {
        self.begin_char();
        let (nr, nc) = (self.nrow as isize, self.ncol as isize);
        self.module(nr - 1, 0, 1);
        self.module(nr - 1, 1, 2);
        self.module(nr - 1, 2, 3);
        self.module(0, nc - 2, 4);
        self.module(0, nc - 1, 5);
        self.module(1, nc - 1, 6);
        self.module(2, nc - 1, 7);
        self.module(3, nc - 1, 8);
    }

    fn corner2(&mut self) {
        self.begin_char();
        let (nr, nc) = (self.nrow as isize, self.ncol as isize);
        self.module(nr - 3, 0, 1);
        self.module(nr - 2, 0, 2);
        self.module(nr - 1, 0, 3);
        self.module(0, nc - 4, 4);
        self.module(0, nc - 3, 5);
        self.module(0, nc - 2, 6);
        self.module(0, nc - 1, 7);
        self.module(1, nc - 1, 8);
    }

    fn corner3(&mut self) {
        self.begin_char();
        let (nr, nc) = (self.nrow as isize, self.ncol as isize);
        self.module(nr - 3, 0, 1);
        self.module(nr - 2, 0, 2);
        self.module(nr - 1, 0, 3);
        self.module(0, nc - 2, 4);
        self.module(0, nc - 1, 5);
        self.module(1, nc - 1, 6);
        self.module(2, nc - 1, 7);
        self.module(3, nc - 1, 8);
    }

    fn corner4(&mut self) {
        self.begin_char();
        let (nr, nc) = (self.nrow as isize, self.ncol as isize);
        self.module(nr - 1, 0, 1);
        self.module(nr - 1, nc - 1, 2);
        self.module(0, nc - 3, 3);
        self.module(0, nc - 2, 4);
        self.module(0, nc - 1, 5);
        self.module(1, nc - 3, 6);
        self.module(1, nc - 2, 7);
        self.module(1, nc - 1, 8);
    }
}

/// Build the Annex F placement map for an `nrows × ncols` mapping matrix.
pub fn placement_map(nrows: usize, ncols: usize) -> Placement {
    let mut p = Placer::new(nrows, ncols);
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
    let mut fixed_dark = Vec::new();
    if !p.is_occupied(nrows - 1, ncols - 1) {
        fixed_dark.push((nrows - 1, ncols - 1));
        fixed_dark.push((nrows - 2, ncols - 2));
    }

    Placement {
        codewords: p.cells,
        fixed_dark,
    }
}

/// Wrap a filled `mapping` matrix (row-major, `mapping_size²` booleans) with the
/// finder/timing borders of every data region, producing the full symbol matrix.
pub fn render_borders(spec: &SquareSpec, mapping: &[bool]) -> BitMatrix {
    let ms = spec.mapping_size();
    let rr = spec.region_size;
    let size = spec.symbol_size;
    let mut m = BitMatrix::new(size, size, QUIET_ZONE);

    // Mirrors ISO/IEC 16022: for each region, the top edge and right edge carry the
    // alternating timing pattern, while the left edge and bottom edge are solid.
    let mut out_y = 0usize;
    for y in 0..ms {
        // Top edge of a region row: alternating (dark on even output columns).
        if y % rr == 0 {
            for out_x in 0..size {
                m.set(out_x, out_y, out_x % 2 == 0);
            }
            out_y += 1;
        }
        let mut out_x = 0usize;
        for x in 0..ms {
            // Left edge of a region column: solid dark.
            if x % rr == 0 {
                m.set(out_x, out_y, true);
                out_x += 1;
            }
            m.set(out_x, out_y, mapping[y * ms + x]);
            out_x += 1;
            // Right edge of a region column: alternating (dark on even mapping rows).
            if x % rr == rr - 1 {
                m.set(out_x, out_y, y % 2 == 0);
                out_x += 1;
            }
        }
        out_y += 1;
        // Bottom edge of a region row: solid dark.
        if y % rr == rr - 1 {
            for out_x in 0..size {
                m.set(out_x, out_y, true);
            }
            out_y += 1;
        }
    }
    m
}

/// Recover the mapping matrix (row-major booleans) from a full symbol matrix by
/// removing every region's finder/timing border. The inverse of [`render_borders`].
pub fn strip_borders(spec: &SquareSpec, m: &BitMatrix) -> Vec<bool> {
    let ms = spec.mapping_size();
    let rr = spec.region_size;
    let mut mapping = vec![false; ms * ms];

    let mut out_y = 0usize;
    for y in 0..ms {
        if y % rr == 0 {
            out_y += 1; // skip the region's top timing row
        }
        let mut out_x = 0usize;
        for x in 0..ms {
            if x % rr == 0 {
                out_x += 1; // skip the region's left finder column
            }
            mapping[y * ms + x] = m.get(out_x, out_y);
            out_x += 1;
            if x % rr == rr - 1 {
                out_x += 1; // skip the region's right timing column
            }
        }
        out_y += 1;
        if y % rr == rr - 1 {
            out_y += 1; // skip the region's bottom finder row
        }
    }
    mapping
}

/// Quiet-zone width required around a Data Matrix symbol, in modules (ISO/IEC 16022
/// specifies at least one module on every side).
pub const QUIET_ZONE: usize = 1;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::datamatrix::tables::all_squares;

    #[test]
    fn placement_covers_every_codeword() {
        for spec in all_squares() {
            let ms = spec.mapping_size();
            let pm = placement_map(ms, ms);
            assert_eq!(
                pm.codewords.len(),
                spec.total_cw(),
                "symbol {} codeword count",
                spec.symbol_size
            );
            // Every module is either a placed bit or a fixed corner or an unused
            // slack module; placed bits must be unique and in range.
            let mut seen = vec![0u32; ms * ms];
            for cw in &pm.codewords {
                for &(r, c) in cw {
                    assert!(r < ms && c < ms);
                    seen[r * ms + c] += 1;
                }
            }
            for &(r, c) in &pm.fixed_dark {
                seen[r * ms + c] += 1;
            }
            assert!(
                seen.iter().all(|&n| n <= 1),
                "overlap in symbol {}",
                spec.symbol_size
            );
        }
    }

    #[test]
    fn borders_round_trip() {
        for spec in all_squares() {
            let ms = spec.mapping_size();
            // A deterministic pseudo-random mapping fill.
            let mapping: Vec<bool> = (0..ms * ms)
                .map(|i| (i * 2654435761usize) & 0x40 != 0)
                .collect();
            let full = render_borders(spec, &mapping);
            let back = strip_borders(spec, &full);
            assert_eq!(
                back, mapping,
                "border round trip for symbol {}",
                spec.symbol_size
            );
        }
    }
}
