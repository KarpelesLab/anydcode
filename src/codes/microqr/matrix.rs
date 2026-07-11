//! Micro QR module layout: the single finder pattern, edge timing patterns,
//! format information (with its own BCH mask), the four data masks and the data
//! zig-zag path.
//!
//! [`Canvas`] is the shared workspace for both encoding (place data, mask, stamp
//! format info) and decoding (read format info, then read data along the same path).

use super::tables::{decode_format, format_bits};
use super::{MicroMask, MicroVersion};
use crate::output::BitMatrix;

/// Quiet-zone width required around a Micro QR symbol, in modules (ISO/IEC 18004
/// specifies 2 for Micro QR).
pub const QUIET_ZONE: usize = 2;

/// A square module grid plus a parallel map of reserved (function-pattern) cells.
pub struct Canvas {
    size: usize,
    /// `true` = dark module.
    dark: Vec<bool>,
    /// `true` = function-pattern / reserved cell (not part of the data stream).
    reserved: Vec<bool>,
}

impl Canvas {
    /// Build a canvas for `version` with all function patterns placed and reserved,
    /// but no data or format bits yet.
    pub fn new(version: MicroVersion) -> Self {
        let size = version.size();
        let mut c = Canvas {
            size,
            dark: vec![false; size * size],
            reserved: vec![false; size * size],
        };
        c.place_finder();
        c.place_timing();
        c.reserve_format();
        c
    }

    /// Rebuild a canvas from an already-sampled matrix of the given version.
    pub fn from_matrix(version: MicroVersion, matrix: &BitMatrix) -> Self {
        let mut c = Canvas::new(version);
        for y in 0..c.size {
            for x in 0..c.size {
                c.dark[y * c.size + x] = matrix.get(x, y);
            }
        }
        c
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.size + x
    }

    /// Module value at `(x, y)`.
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.dark[self.idx(x, y)]
    }

    fn set(&mut self, x: usize, y: usize, dark: bool) {
        let i = self.idx(x, y);
        self.dark[i] = dark;
    }

    fn set_fn(&mut self, x: usize, y: usize, dark: bool) {
        let i = self.idx(x, y);
        self.dark[i] = dark;
        self.reserved[i] = true;
    }

    fn reserve(&mut self, x: usize, y: usize) {
        let i = self.idx(x, y);
        self.reserved[i] = true;
    }

    fn place_finder(&mut self) {
        // 7x7 finder at the top-left corner.
        for dy in 0..7 {
            for dx in 0..7 {
                let ring = dx == 0 || dx == 6 || dy == 0 || dy == 6;
                let core = (2..=4).contains(&dx) && (2..=4).contains(&dy);
                self.set_fn(dx, dy, ring || core);
            }
        }
        // Separator: light L on the inner edges (column 7 rows 0..7, row 7 cols 0..7).
        for i in 0..8 {
            self.set_fn(7, i, false);
            self.set_fn(i, 7, false);
        }
    }

    fn place_timing(&mut self) {
        // Horizontal timing along the top row, vertical along the left column.
        for i in 8..self.size {
            self.set_fn(i, 0, i % 2 == 0);
            self.set_fn(0, i, i % 2 == 0);
        }
    }

    fn reserve_format(&mut self) {
        for i in 1..=8 {
            self.reserve(8, i); // vertical strip, column 8
            self.reserve(i, 8); // horizontal strip, row 8
        }
    }

    /// The ordered `(x, y)` data-module coordinates: two columns at a time from the
    /// right, zig-zagging up then down. No interior timing column to skip.
    pub fn data_path(&self) -> Vec<(usize, usize)> {
        let s = self.size;
        let mut path = Vec::new();
        let mut upward = true;
        let mut col = s as i32 - 1;
        while col > 0 {
            for i in 0..s {
                let y = if upward { s - 1 - i } else { i };
                for c in [col, col - 1] {
                    let x = c as usize;
                    if !self.reserved[self.idx(x, y)] {
                        path.push((x, y));
                    }
                }
            }
            upward = !upward;
            col -= 2;
        }
        path
    }

    /// Whether the Micro QR mask flips the module at `(x, y)`.
    pub fn mask_bit(mask: MicroMask, x: usize, y: usize) -> bool {
        let (i, j) = (y, x);
        match mask.index() {
            0 => i % 2 == 0,
            1 => (i / 2 + j / 3) % 2 == 0,
            2 => ((i * j) % 2 + (i * j) % 3) % 2 == 0,
            _ => ((i + j) % 2 + (i * j) % 3) % 2 == 0,
        }
    }

    /// XOR the mask pattern across all non-reserved (data) modules.
    pub fn apply_mask(&mut self, mask: MicroMask) {
        for y in 0..self.size {
            for x in 0..self.size {
                let i = self.idx(x, y);
                if !self.reserved[i] && Canvas::mask_bit(mask, x, y) {
                    self.dark[i] ^= true;
                }
            }
        }
    }

    /// Stamp the 15-bit format information for `symbol_number` + `mask`.
    pub fn place_format(&mut self, symbol_number: u8, mask: MicroMask) {
        let bits = format_bits(symbol_number, mask.index());
        for i in 0..8 {
            self.set_fn(8, 1 + i, (bits >> i) & 1 != 0); // vertical strip
            self.set_fn(1 + i, 8, (bits >> (14 - i)) & 1 != 0); // horizontal strip
        }
    }

    /// Write one data bit into the module at `(x, y)`.
    pub fn place_data_bit(&mut self, x: usize, y: usize, dark: bool) {
        self.set(x, y, dark);
    }

    /// Convert to a [`BitMatrix`] with the Micro QR quiet zone recorded.
    pub fn to_bitmatrix(&self) -> BitMatrix {
        let mut m = BitMatrix::new(self.size, self.size, QUIET_ZONE);
        for y in 0..self.size {
            for x in 0..self.size {
                if self.dark[self.idx(x, y)] {
                    m.set(x, y, true);
                }
            }
        }
        m
    }

    /// The ISO/IEC 18004 Micro QR mask evaluation score (higher is better): it
    /// rewards dark modules on the right and bottom edges.
    pub fn evaluate(&self) -> u32 {
        let s = self.size;
        let mut sum1 = 0u32; // rightmost column
        let mut sum2 = 0u32; // bottom row
        for k in 1..s {
            sum1 += self.get(s - 1, k) as u32;
            sum2 += self.get(k, s - 1) as u32;
        }
        if sum1 <= sum2 {
            sum1 * 16 + sum2
        } else {
            sum2 * 16 + sum1
        }
    }

    /// Read the format information, returning `(symbol_number, mask)` after BCH
    /// correction.
    pub fn read_format(&self) -> Option<(u8, u8)> {
        let mut raw = 0u16;
        for i in 0..8 {
            raw |= (self.get(8, 1 + i) as u16) << i;
        }
        for i in 0..7 {
            raw |= (self.get(1 + i, 8) as u16) << (14 - i);
        }
        decode_format(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::microqr::tables::data_module_count;

    #[test]
    fn data_path_covers_all_data_modules() {
        for v in [
            MicroVersion::M1,
            MicroVersion::M2,
            MicroVersion::M3,
            MicroVersion::M4,
        ] {
            let c = Canvas::new(v);
            let path = c.data_path();
            assert_eq!(path.len(), data_module_count(v), "{v:?}");
            let mut seen = std::collections::HashSet::new();
            for p in &path {
                assert!(seen.insert(*p), "duplicate module {p:?} in {v:?}");
            }
        }
    }
}
