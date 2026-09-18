//! QR module layout: function-pattern placement, the data-bit zigzag path, data
//! masking, format/version information (with BCH), and mask-penalty scoring.
//!
//! [`Canvas`] is the shared workspace for both encoding (place data, then mask, then
//! stamp format info) and decoding (read format info, unmask, then read data along
//! the same path). It works over a bit-packed [`MatrixBuf`], so the encoder can run
//! entirely in caller-provided memory.

use super::tables::alignment_positions;
use super::{EcLevel, Mask, Version};
#[cfg(feature = "alloc")]
use crate::output::BitMatrix;
use crate::output::MatrixBuf;

/// Quiet-zone width required around a QR symbol, in modules.
pub const QUIET_ZONE: usize = 4;

/// A square QR module grid for one version. Which cells are function patterns is
/// computed ([`is_function_module`]), not stored.
pub struct Canvas<'a> {
    version: Version,
    size: usize,
    grid: MatrixBuf<'a>,
}

impl<'a> Canvas<'a> {
    /// Bytes of storage a canvas for `version` needs.
    pub const fn storage_len(version: Version) -> usize {
        let size = 17 + 4 * version.0 as usize;
        MatrixBuf::bytes_for(size, size)
    }

    /// Build a canvas for `version` over `storage` with all function patterns placed,
    /// but no data, format or version bits yet.
    ///
    /// # Errors
    /// [`crate::Error::Capacity`] if `storage` is shorter than [`Canvas::storage_len`].
    pub fn new(version: Version, storage: &'a mut [u8]) -> crate::Result<Self> {
        let size = version.size();
        let grid = MatrixBuf::new(storage, size, size, QUIET_ZONE)?;
        let mut c = Canvas {
            version,
            size,
            grid,
        };
        c.place_finders();
        c.place_timing();
        c.place_alignment();
        // The always-dark module.
        c.grid.set(8, size - 8, true);
        Ok(c)
    }

    /// Rebuild a canvas from an already-sampled matrix of the given version: every
    /// module value is taken from `matrix`.
    #[cfg(feature = "alloc")]
    pub fn from_matrix(
        version: Version,
        matrix: &BitMatrix,
        storage: &'a mut [u8],
    ) -> crate::Result<Self> {
        let size = version.size();
        let mut grid = MatrixBuf::new(storage, size, size, QUIET_ZONE)?;
        for y in 0..size {
            for x in 0..size {
                if matrix.get(x, y) {
                    grid.set(x, y, true);
                }
            }
        }
        Ok(Canvas {
            version,
            size,
            grid,
        })
    }

    /// Module value at `(x, y)`.
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.grid.get(x, y)
    }

    /// The finished module grid.
    pub fn into_grid(self) -> MatrixBuf<'a> {
        self.grid
    }

    fn place_finders(&mut self) {
        let s = self.size;
        for &(ox, oy) in &[(0usize, 0usize), (s - 7, 0), (0, s - 7)] {
            for dy in 0..7 {
                for dx in 0..7 {
                    let ring = dx == 0 || dx == 6 || dy == 0 || dy == 6;
                    let core = (2..=4).contains(&dx) && (2..=4).contains(&dy);
                    self.grid.set(ox + dx, oy + dy, ring || core);
                }
            }
        }
        // Separators are light, which a fresh grid already is.
    }

    fn place_timing(&mut self) {
        for i in (8..self.size - 8).step_by(2) {
            self.grid.set(i, 6, true);
            self.grid.set(6, i, true);
        }
    }

    fn place_alignment(&mut self) {
        let positions = alignment_positions(self.version);
        let last = self.size - 7;
        for &cy in positions {
            for &cx in positions {
                let (cx, cy) = (cx as usize, cy as usize);
                // Skip the three centers that fall on finder patterns.
                let on_finder = (cy == 6 && (cx == 6 || cx == last)) || (cy == last && cx == 6);
                if on_finder {
                    continue;
                }
                for dy in -2i32..=2 {
                    for dx in -2i32..=2 {
                        let dark = dx.abs().max(dy.abs()) != 1;
                        self.grid
                            .set((cx as i32 + dx) as usize, (cy as i32 + dy) as usize, dark);
                    }
                }
            }
        }
    }

    /// The ordered `(x, y)` data-module coordinates: two columns at a time from the
    /// right, zigzagging up then down, skipping the vertical timing column.
    pub fn data_path(&self) -> DataPath {
        DataPath {
            version: self.version,
            size: self.size,
            col: self.size - 1,
            row: 0,
            left: false,
            upward: true,
        }
    }

    /// Whether the mask flips the module at `(x, y)` (x = column j, y = row i).
    pub fn mask_bit(mask: Mask, x: usize, y: usize) -> bool {
        let (i, j) = (y, x);
        match mask.index() {
            0 => (i + j) % 2 == 0,
            1 => i % 2 == 0,
            2 => j % 3 == 0,
            3 => (i + j) % 3 == 0,
            4 => (i / 2 + j / 3) % 2 == 0,
            5 => (i * j) % 2 + (i * j) % 3 == 0,
            6 => ((i * j) % 2 + (i * j) % 3) % 2 == 0,
            _ => ((i + j) % 2 + (i * j) % 3) % 2 == 0,
        }
    }

    /// XOR the mask pattern across all data modules. Applying the same mask twice
    /// restores the original grid.
    pub fn apply_mask(&mut self, mask: Mask) {
        for y in 0..self.size {
            for x in 0..self.size {
                if Canvas::mask_bit(mask, x, y) && !is_function_module(self.version, x, y) {
                    self.grid.toggle(x, y);
                }
            }
        }
    }

    /// Stamp both copies of the 15-bit format information for `level`+`mask`.
    pub fn place_format(&mut self, level: EcLevel, mask: Mask) {
        let bits = format_bits(level, mask);
        let s = self.size;
        for i in 0..=5 {
            self.grid.set(8, i, bit(bits, i));
        }
        self.grid.set(8, 7, bit(bits, 6));
        self.grid.set(8, 8, bit(bits, 7));
        self.grid.set(7, 8, bit(bits, 8));
        for i in 9..15 {
            self.grid.set(14 - i, 8, bit(bits, i));
        }
        for i in 0..8 {
            self.grid.set(s - 1 - i, 8, bit(bits, i));
        }
        for i in 8..15 {
            self.grid.set(8, s - 15 + i, bit(bits, i));
        }
    }

    /// Stamp both copies of the 18-bit version information (versions ≥ 7 only).
    pub fn place_version(&mut self) {
        if self.version.number() < 7 {
            return;
        }
        let bits = version_bits(self.version);
        let s = self.size;
        for i in 0..18 {
            let b = bit32(bits, i);
            let a = s - 11 + i % 3;
            let c = i / 3;
            self.grid.set(a, c, b);
            self.grid.set(c, a, b);
        }
    }

    /// Write one data bit into the module at `(x, y)` (used by the encoder while
    /// walking [`Canvas::data_path`]).
    pub fn place_data_bit(&mut self, x: usize, y: usize, dark: bool) {
        self.grid.set(x, y, dark);
    }

    /// The ISO/IEC 18004 mask penalty score for the current module values, summing
    /// the four standard rules. Lower is better; used to pick a data mask.
    pub fn penalty(&self) -> u32 {
        let s = self.size;
        let get = |x: usize, y: usize| self.grid.get(x, y);
        let mut score = 0u32;

        // Rule 1: runs of ≥5 same-colour modules in rows and columns.
        for line in 0..s {
            let mut run_row = (get(0, line), 1u32);
            let mut run_col = (get(line, 0), 1u32);
            for k in 1..s {
                for (cur, run) in [(get(k, line), &mut run_row), (get(line, k), &mut run_col)] {
                    if cur == run.0 {
                        run.1 += 1;
                    } else {
                        if run.1 >= 5 {
                            score += 3 + (run.1 - 5);
                        }
                        *run = (cur, 1);
                    }
                }
            }
            for run in [run_row, run_col] {
                if run.1 >= 5 {
                    score += 3 + (run.1 - 5);
                }
            }
        }

        // Rule 2: 2x2 blocks of one colour.
        for y in 0..s - 1 {
            for x in 0..s - 1 {
                let c = get(x, y);
                if get(x + 1, y) == c && get(x, y + 1) == c && get(x + 1, y + 1) == c {
                    score += 3;
                }
            }
        }

        // Rule 3: finder-like 1:1:3:1:1 patterns with a 4-module light run on one side.
        const A: [bool; 11] = [
            true, false, true, true, true, false, true, false, false, false, false,
        ];
        const B: [bool; 11] = [
            false, false, false, false, true, false, true, true, true, false, true,
        ];
        for y in 0..s {
            for x in 0..s - 10 {
                let row_match = |pat: &[bool; 11]| (0..11).all(|k| get(x + k, y) == pat[k]);
                if row_match(&A) || row_match(&B) {
                    score += 40;
                }
                let col_match = |pat: &[bool; 11]| (0..11).all(|k| get(y, x + k) == pat[k]);
                if col_match(&A) || col_match(&B) {
                    score += 40;
                }
            }
        }

        // Rule 4: deviation of the dark-module proportion from 50%.
        score += balance_penalty(self.grid.count_dark() as u32, (s * s) as u32);

        score
    }

    /// Read the format information, returning `(level, mask)` after BCH correction.
    pub fn read_format(&self) -> Option<(EcLevel, Mask)> {
        // Copy 1: column 8 (rows 0..6,7,8) then row 8 (col 7, then 5..0).
        let mut raw1 = 0u16;
        for i in 0..=5 {
            raw1 |= (self.get(8, i) as u16) << i;
        }
        raw1 |= (self.get(8, 7) as u16) << 6;
        raw1 |= (self.get(8, 8) as u16) << 7;
        raw1 |= (self.get(7, 8) as u16) << 8;
        for i in 9..15 {
            raw1 |= (self.get(14 - i, 8) as u16) << i;
        }
        // Copy 2.
        let s = self.size;
        let mut raw2 = 0u16;
        for i in 0..8 {
            raw2 |= (self.get(s - 1 - i, 8) as u16) << i;
        }
        for i in 8..15 {
            raw2 |= (self.get(8, s - 15 + i) as u16) << i;
        }
        // Decode both copies and keep the higher-confidence one (fewest corrected
        // bits). Preferring copy 1 unconditionally — as a plain `or_else` would — can
        // lock onto a copy that BCH "corrected" to the wrong format when the real code
        // is damaged there; cross-checking against copy 2 avoids that.
        match (decode_format_conf(raw1), decode_format_conf(raw2)) {
            (Some((v1, d1)), Some((v2, d2))) => Some(if d1 <= d2 { v1 } else { v2 }),
            (Some((v, _)), None) | (None, Some((v, _))) => Some(v),
            (None, None) => None,
        }
    }
}

/// Penalty rule 4: 10 points for every full 5% step by which the dark-module
/// proportion deviates from 50%, in either direction alike.
fn balance_penalty(dark: u32, total: u32) -> u32 {
    (dark * 2).abs_diff(total) * 10 / total * 10
}

/// Iterator over the data-module coordinates of a version, in placement order.
pub struct DataPath {
    version: Version,
    size: usize,
    /// Right column of the current two-column strip.
    col: usize,
    /// Step within the strip's vertical sweep.
    row: usize,
    /// Whether the next module is the strip's left column.
    left: bool,
    upward: bool,
}

impl Iterator for DataPath {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<(usize, usize)> {
        loop {
            if self.row == self.size {
                // Strip finished: move two columns left, skipping the timing column.
                if self.col < 2 {
                    return None;
                }
                self.col -= 2;
                if self.col == 6 {
                    self.col -= 1;
                }
                self.row = 0;
                self.upward = !self.upward;
            }
            let y = if self.upward {
                self.size - 1 - self.row
            } else {
                self.row
            };
            let x = if self.left { self.col - 1 } else { self.col };
            if self.left {
                self.row += 1;
            }
            self.left = !self.left;
            if !is_function_module(self.version, x, y) {
                return Some((x, y));
            }
        }
    }
}

/// Whether `(x, y)` is a function-pattern / reserved module for `version`: finders
/// and separators, timing patterns, alignment patterns, format and version areas,
/// and the always-dark module. Such modules never carry data.
pub fn is_function_module(version: Version, x: usize, y: usize) -> bool {
    let s = version.size();
    // Finders + separators + format areas (and the dark module).
    if (x <= 8 && y <= 8) || (x >= s - 8 && y <= 8) || (x <= 8 && y >= s - 8) {
        return true;
    }
    // Timing patterns.
    if x == 6 || y == 6 {
        return true;
    }
    // Version information blocks.
    if version.number() >= 7
        && ((x >= s - 11 && x < s - 8 && y < 6) || (y >= s - 11 && y < s - 8 && x < 6))
    {
        return true;
    }
    // Alignment patterns, except the three centers that would overlap a finder.
    let positions = alignment_positions(version);
    let last = s - 7;
    positions.iter().any(|&cy| {
        let cy = cy as usize;
        cy.abs_diff(y) <= 2
            && positions.iter().any(|&cx| {
                let cx = cx as usize;
                let on_finder = (cy == 6 && (cx == 6 || cx == last)) || (cy == last && cx == 6);
                !on_finder && cx.abs_diff(x) <= 2
            })
    })
}

fn bit(bits: u16, i: usize) -> bool {
    (bits >> i) & 1 != 0
}

fn bit32(bits: u32, i: usize) -> bool {
    (bits >> i) & 1 != 0
}

/// Compute the 15-bit format code (BCH + mask XOR) for a level/mask.
fn format_bits(level: EcLevel, mask: Mask) -> u16 {
    let data = ((level.format_bits() as u16) << 3) | mask.index() as u16;
    let mut rem = data;
    for _ in 0..10 {
        rem = (rem << 1) ^ (((rem >> 9) & 1) * 0x537);
    }
    ((data << 10) | (rem & 0x3FF)) ^ 0x5412
}

/// Decode a 15-bit format code back to `(level, mask)`, correcting up to the BCH
/// capacity by nearest-codeword search (Hamming distance ≤ 3).
#[cfg(test)]
fn decode_format(raw: u16) -> Option<(EcLevel, Mask)> {
    decode_format_conf(raw).map(|(v, _)| v)
}

/// Like [`decode_format`] but also returns the number of corrected bits (the Hamming
/// distance to the nearest valid format codeword), so callers can compare the
/// confidence of the two format copies.
fn decode_format_conf(raw: u16) -> Option<((EcLevel, Mask), u32)> {
    let mut best = None;
    let mut best_dist = u32::MAX;
    for data in 0u16..32 {
        let level = EcLevel::from_format_bits((data >> 3) as u8);
        let mask = Mask::new((data & 0b111) as u8)?;
        let candidate = format_bits(level, mask);
        let dist = (candidate ^ raw).count_ones();
        if dist < best_dist {
            best_dist = dist;
            best = Some((level, mask));
        }
    }
    best.filter(|_| best_dist <= 3).map(|v| (v, best_dist))
}

/// Compute the 18-bit version code (version ≥ 7) with its 12-bit BCH remainder.
fn version_bits(version: Version) -> u32 {
    let v = version.number() as u32;
    let mut rem = v;
    for _ in 0..12 {
        rem = (rem << 1) ^ (((rem >> 11) & 1) * 0x1F25);
    }
    (v << 12) | (rem & 0xFFF)
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::super::tables::{ec_blocks, remainder_bits};
    use super::*;

    #[test]
    fn format_roundtrips_all() {
        for lvl in [EcLevel::L, EcLevel::M, EcLevel::Q, EcLevel::H] {
            for m in 0..8 {
                let mask = Mask::new(m).unwrap();
                let bits = format_bits(lvl, mask);
                assert_eq!(decode_format(bits), Some((lvl, mask)));
                // Single-bit error must still decode.
                let corrupted = bits ^ (1 << (m as u16 % 15));
                assert_eq!(decode_format(corrupted), Some((lvl, mask)));
            }
        }
    }

    /// Rule 4 is symmetric about 50%: 40.5% dark is 9.5% off, one full 5% step, just
    /// like 59.5% (flooring the percentage first used to score the former as two).
    #[test]
    fn balance_penalty_is_symmetric() {
        assert_eq!(balance_penalty(500, 1000), 0);
        assert_eq!(balance_penalty(549, 1000), 0);
        assert_eq!(balance_penalty(451, 1000), 0);
        assert_eq!(balance_penalty(550, 1000), 10);
        assert_eq!(balance_penalty(450, 1000), 10);
        assert_eq!(balance_penalty(595, 1000), 10);
        assert_eq!(balance_penalty(405, 1000), 10);
        assert_eq!(balance_penalty(400, 1000), 20);
        assert_eq!(balance_penalty(0, 1000), 100);
        assert_eq!(balance_penalty(1000, 1000), 100);
    }

    #[test]
    fn data_path_covers_all_data_modules() {
        for v in 1..=40 {
            let version = Version::new(v).unwrap();
            let side = version.size();
            assert_eq!(
                Canvas::storage_len(version),
                MatrixBuf::bytes_for(side, side)
            );
            let mut storage = [0u8; Canvas::storage_len(Version(40))];
            let c = Canvas::new(version, &mut storage).unwrap();
            let mut seen = std::collections::HashSet::new();
            for p in c.data_path() {
                assert!(!is_function_module(version, p.0, p.1));
                assert!(seen.insert(p), "v{v}: duplicate module {p:?}");
            }
            // Every data module is visited: codewords plus remainder bits.
            let expected =
                ec_blocks(version, EcLevel::L).total_codewords() * 8 + remainder_bits(version);
            assert_eq!(seen.len(), expected, "v{v}");
        }
    }
}
