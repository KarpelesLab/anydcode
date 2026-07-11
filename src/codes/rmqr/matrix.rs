//! rMQR module layout: the finder and finder-sub patterns, corner subpatterns,
//! edge/alignment timing, the two 18-bit format-information copies, the single data
//! mask and the data zig-zag path.
//!
//! [`Canvas`] is the shared workspace for encoding and decoding. Placement follows
//! ISO/IEC 23941 §7.7 and is cross-checked bit-for-bit against the reference
//! `rmqrcode` implementation for single-block and symmetric multi-block sizes.
//! (`rmqrcode` mis-interleaves asymmetric multi-block sizes, so those are validated
//! by round-trip against our ISO-standard interleaving instead.)

use super::tables::{alignment_columns, format_bits, info};
use super::{RmqrEcLevel, RmqrSize};
use crate::output::BitMatrix;

/// Quiet-zone width required around an rMQR symbol, in modules.
pub const QUIET_ZONE: usize = 2;

/// XOR mask for the finder-side format-information copy.
const FMT_MASK_A: u32 = 0b011111101010110010;
/// XOR mask for the finder-sub-side format-information copy.
const FMT_MASK_B: u32 = 0b100000101001111011;

/// A rectangular module grid plus a parallel map of defined (function-pattern or
/// format) cells; the remaining cells carry the data stream.
pub struct Canvas {
    size: RmqrSize,
    width: usize,
    height: usize,
    dark: Vec<bool>,
    /// `true` = function-pattern / format cell (not part of the data stream).
    defined: Vec<bool>,
}

impl Canvas {
    /// Build a canvas for `size` with all function patterns placed and the format
    /// region reserved, but no data or format values yet.
    pub fn new(size: RmqrSize) -> Self {
        let width = size.width();
        let height = size.height();
        let mut c = Canvas {
            size,
            width,
            height,
            dark: vec![false; width * height],
            defined: vec![false; width * height],
        };
        c.place_finder();
        c.place_finder_sub();
        c.place_corner();
        c.place_alignment();
        c.place_timing();
        c.reserve_format();
        c
    }

    /// Rebuild a canvas from an already-sampled matrix of the given size.
    pub fn from_matrix(size: RmqrSize, matrix: &BitMatrix) -> Self {
        let mut c = Canvas::new(size);
        for y in 0..c.height {
            for x in 0..c.width {
                c.dark[y * c.width + x] = matrix.get(x, y);
            }
        }
        c
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }

    /// Module value at `(x, y)`.
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.dark[self.idx(x, y)]
    }

    fn is_defined(&self, x: usize, y: usize) -> bool {
        self.defined[self.idx(x, y)]
    }

    fn set_fn(&mut self, x: usize, y: usize, dark: bool) {
        let i = self.idx(x, y);
        self.dark[i] = dark;
        self.defined[i] = true;
    }

    fn place_finder(&mut self) {
        for y in 0..7 {
            for x in 0..7 {
                let ring = x == 0 || x == 6 || y == 0 || y == 6;
                let core = (2..=4).contains(&x) && (2..=4).contains(&y);
                self.set_fn(x, y, ring || core);
            }
        }
        // Separator (light) along the finder's inner edges.
        for n in 0..8 {
            if n < self.height {
                self.set_fn(7, n, false);
            }
            if self.height >= 9 {
                self.set_fn(n, 7, false);
            }
        }
    }

    fn place_finder_sub(&mut self) {
        let (w, h) = (self.width, self.height);
        for i in 0..5 {
            for j in 0..5 {
                let dark = i == 0 || i == 4 || j == 0 || j == 4;
                self.set_fn(w - 1 - j, h - 1 - i, dark);
            }
        }
        self.set_fn(w - 3, h - 3, true);
    }

    fn place_corner(&mut self) {
        let (w, h) = (self.width, self.height);
        // Bottom-left.
        self.set_fn(0, h - 1, true);
        self.set_fn(1, h - 1, true);
        self.set_fn(2, h - 1, true);
        if h >= 11 {
            self.set_fn(0, h - 2, true);
            self.set_fn(1, h - 2, false);
        }
        // Top-right.
        self.set_fn(w - 1, 0, true);
        self.set_fn(w - 2, 0, true);
        self.set_fn(w - 1, 1, true);
        self.set_fn(w - 2, 1, false);
    }

    fn place_alignment(&mut self) {
        let (w, h) = (self.width, self.height);
        for &cx in alignment_columns(w) {
            for i in 0..3 {
                for j in 0..3 {
                    let dark = i == 0 || i == 2 || j == 0 || j == 2;
                    let x = cx + j - 1;
                    self.set_fn(x, i, dark);
                    self.set_fn(x, h - 1 - i, dark);
                }
            }
        }
    }

    fn place_timing(&mut self) {
        let (w, h) = (self.width, self.height);
        // Horizontal timing on the top and bottom rows (empty cells only).
        for x in 0..w {
            let dark = x % 2 == 0;
            for y in [0, h - 1] {
                if !self.is_defined(x, y) {
                    self.set_fn(x, y, dark);
                }
            }
        }
        // Vertical timing on the left/right edges and any alignment columns.
        let mut cols = vec![0, w - 1];
        cols.extend_from_slice(alignment_columns(w));
        for y in 0..h {
            let dark = y % 2 == 0;
            for &x in &cols {
                if !self.is_defined(x, y) {
                    self.set_fn(x, y, dark);
                }
            }
        }
    }

    /// The 36 coordinates (two 18-bit copies) that carry format information.
    fn format_cells(&self) -> Vec<(usize, usize)> {
        let (w, h) = (self.width, self.height);
        let mut cells = Vec::with_capacity(36);
        for n in 0..18 {
            cells.push((8 + n / 5, 1 + n % 5)); // copy A
        }
        for n in 0..15 {
            cells.push((w - 8 + n / 5, h - 6 + n % 5)); // copy B main block
        }
        cells.push((w - 5, h - 6));
        cells.push((w - 4, h - 6));
        cells.push((w - 3, h - 6));
        cells
    }

    fn reserve_format(&mut self) {
        for (x, y) in self.format_cells() {
            let i = self.idx(x, y);
            self.defined[i] = true;
        }
    }

    /// Stamp both copies of the 18-bit format information for the size + EC level.
    pub fn place_format(&mut self, ec: RmqrEcLevel) {
        let (w, h) = (self.width, self.height);
        let fmt = format_bits(self.size, ec == RmqrEcLevel::H);
        let a = fmt ^ FMT_MASK_A;
        for n in 0..18u32 {
            self.set_fn(8 + n as usize / 5, 1 + n as usize % 5, (a >> n) & 1 != 0);
        }
        let b = fmt ^ FMT_MASK_B;
        for n in 0..15u32 {
            self.set_fn(
                w - 8 + n as usize / 5,
                h - 6 + n as usize % 5,
                (b >> n) & 1 != 0,
            );
        }
        self.set_fn(w - 5, h - 6, (b >> 15) & 1 != 0);
        self.set_fn(w - 4, h - 6, (b >> 16) & 1 != 0);
        self.set_fn(w - 3, h - 6, (b >> 17) & 1 != 0);
    }

    /// The ordered `(x, y)` data-cell coordinates, length `count`, following the
    /// ISO/IEC 23941 symbol-character placement walk (two columns at a time, up then
    /// down, starting from the finder-sub corner).
    pub fn data_path(&self, count: usize) -> Vec<(usize, usize)> {
        let (w, h) = (self.width as i32, self.height as i32);
        let mut path = Vec::with_capacity(count);
        let mut cx: i32 = w - 2;
        let mut cy: i32 = h - 6;
        let mut dy: i32 = -1;
        while path.len() < count && cx >= -1 {
            for x in [cx, cx - 1] {
                if x < 0 || x >= w {
                    continue;
                }
                if !self.is_defined(x as usize, cy as usize) {
                    path.push((x as usize, cy as usize));
                    if path.len() == count {
                        return path;
                    }
                }
            }
            if dy < 0 && cy == 1 {
                cx -= 2;
                dy = 1;
            } else if dy > 0 && cy == h - 2 {
                cx -= 2;
                dy = -1;
            } else {
                cy += dy;
            }
        }
        path
    }

    /// Whether the (single) rMQR data mask flips the module at `(x, y)`.
    pub fn mask_bit(x: usize, y: usize) -> bool {
        (y / 2 + x / 3).is_multiple_of(2)
    }

    /// Write one data bit into the module at `(x, y)`.
    pub fn place_data_bit(&mut self, x: usize, y: usize, dark: bool) {
        let i = self.idx(x, y);
        self.dark[i] = dark;
    }

    /// XOR the data mask across the given data cells.
    pub fn apply_mask(&mut self, cells: &[(usize, usize)]) {
        for &(x, y) in cells {
            if Canvas::mask_bit(x, y) {
                let i = self.idx(x, y);
                self.dark[i] ^= true;
            }
        }
    }

    /// Convert to a [`BitMatrix`] with the rMQR quiet zone recorded.
    pub fn to_bitmatrix(&self) -> BitMatrix {
        let mut m = BitMatrix::new(self.width, self.height, QUIET_ZONE);
        for y in 0..self.height {
            for x in 0..self.width {
                if self.dark[self.idx(x, y)] {
                    m.set(x, y, true);
                }
            }
        }
        m
    }

    /// Read the format information from either copy, returning `(size, ec)`.
    pub fn read_format(&self) -> Option<(RmqrSize, RmqrEcLevel)> {
        let (w, h) = (self.width, self.height);
        let mut raw_a = 0u32;
        for n in 0..18 {
            if self.get(8 + n / 5, 1 + n % 5) {
                raw_a |= 1 << n;
            }
        }
        let mut raw_b = 0u32;
        for n in 0..15 {
            if self.get(w - 8 + n / 5, h - 6 + n % 5) {
                raw_b |= 1 << n;
            }
        }
        if self.get(w - 5, h - 6) {
            raw_b |= 1 << 15;
        }
        if self.get(w - 4, h - 6) {
            raw_b |= 1 << 16;
        }
        if self.get(w - 3, h - 6) {
            raw_b |= 1 << 17;
        }
        decode_format(raw_a ^ FMT_MASK_A).or_else(|| decode_format(raw_b ^ FMT_MASK_B))
    }

    /// Total data-region cells for a level (codeword bits + remainder bits).
    pub fn data_cell_count(&self, ec: RmqrEcLevel) -> usize {
        let i = info(self.size);
        i.total_codewords(ec) * 8 + i.remainder
    }
}

/// Decode an unmasked 18-bit format word to `(size, ec)` by nearest-codeword search
/// over the 64 valid words (Hamming distance ≤ 3).
fn decode_format(raw: u32) -> Option<(RmqrSize, RmqrEcLevel)> {
    let mut best = None;
    let mut best_dist = u32::MAX;
    for vi in 0..32u8 {
        let size = RmqrSize::from_indicator(vi).unwrap();
        for ec in [RmqrEcLevel::M, RmqrEcLevel::H] {
            let candidate = format_bits(size, ec == RmqrEcLevel::H);
            let dist = (candidate ^ raw).count_ones();
            if dist < best_dist {
                best_dist = dist;
                best = Some((size, ec));
            }
        }
    }
    if best_dist <= 3 { best } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_path_covers_all_data_cells() {
        for size in RmqrSize::all() {
            let c = Canvas::new(size);
            for ec in [RmqrEcLevel::M, RmqrEcLevel::H] {
                let count = c.data_cell_count(ec);
                let path = c.data_path(count);
                assert_eq!(path.len(), count, "{} {:?}", size.name(), ec);
                let mut seen = std::collections::HashSet::new();
                for p in &path {
                    assert!(seen.insert(*p), "duplicate {p:?} in {}", size.name());
                }
            }
        }
    }

    #[test]
    fn format_roundtrips() {
        for size in RmqrSize::all() {
            for ec in [RmqrEcLevel::M, RmqrEcLevel::H] {
                let raw = format_bits(size, ec == RmqrEcLevel::H);
                assert_eq!(decode_format(raw), Some((size, ec)), "{}", size.name());
            }
        }
    }
}
