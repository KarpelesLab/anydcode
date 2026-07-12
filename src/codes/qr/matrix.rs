//! QR module layout: function-pattern placement, the data-bit zigzag path, data
//! masking, format/version information (with BCH), and mask-penalty scoring.
//!
//! [`Canvas`] is the shared workspace for both encoding (place data, then mask, then
//! stamp format info) and decoding (read format info, unmask, then read data along
//! the same path).

use super::tables::alignment_positions;
use super::{EcLevel, Mask, Version};
use crate::output::BitMatrix;

/// Quiet-zone width required around a QR symbol, in modules.
pub const QUIET_ZONE: usize = 4;

/// A square module grid plus a parallel map of which cells are function patterns
/// (reserved and never carry data).
pub struct Canvas {
    version: Version,
    size: usize,
    /// `true` = dark module.
    dark: Vec<bool>,
    /// `true` = function-pattern / reserved cell (not part of the data stream).
    reserved: Vec<bool>,
}

impl Canvas {
    /// Build a canvas for `version` with all function patterns placed and reserved,
    /// but no data, format or version bits yet.
    pub fn new(version: Version) -> Self {
        let size = version.size();
        let mut c = Canvas {
            version,
            size,
            dark: vec![false; size * size],
            reserved: vec![false; size * size],
        };
        c.place_finders();
        c.place_timing();
        c.place_alignment();
        c.reserve_format_and_dark();
        c.reserve_version();
        c
    }

    /// Rebuild a canvas from an already-sampled matrix of the given version: function
    /// patterns are reserved as usual, and every module value is taken from `matrix`.
    pub fn from_matrix(version: Version, matrix: &BitMatrix) -> Self {
        let mut c = Canvas::new(version);
        for y in 0..c.size {
            for x in 0..c.size {
                let i = y * c.size + x;
                c.dark[i] = matrix.get(x, y);
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

    fn place_finders(&mut self) {
        let s = self.size;
        for &(ox, oy) in &[(0usize, 0usize), (s - 7, 0), (0, s - 7)] {
            for dy in 0..7 {
                for dx in 0..7 {
                    let ring = dx == 0 || dx == 6 || dy == 0 || dy == 6;
                    let core = (2..=4).contains(&dx) && (2..=4).contains(&dy);
                    self.set_fn(ox + dx, oy + dy, ring || core);
                }
            }
        }
        // Separators: the one-module light border on the inner edges of each finder.
        for i in 0..8 {
            self.set_fn(i, 7, false);
            self.set_fn(7, i, false);
            self.set_fn(s - 1 - i, 7, false);
            self.set_fn(s - 8, i, false);
            self.set_fn(i, s - 8, false);
            self.set_fn(7, s - 1 - i, false);
        }
    }

    fn place_timing(&mut self) {
        for i in 8..self.size - 8 {
            let dark = i % 2 == 0;
            self.set_fn(i, 6, dark);
            self.set_fn(6, i, dark);
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
                        self.set_fn((cx as i32 + dx) as usize, (cy as i32 + dy) as usize, dark);
                    }
                }
            }
        }
    }

    fn reserve_format_and_dark(&mut self) {
        let s = self.size;
        for i in 0..9 {
            self.reserve(8, i);
            self.reserve(i, 8);
        }
        for i in 0..8 {
            self.reserve(8, s - 1 - i);
            self.reserve(s - 1 - i, 8);
        }
        // The always-dark module.
        self.set_fn(8, s - 8, true);
    }

    fn reserve_version(&mut self) {
        if self.version.number() < 7 {
            return;
        }
        let s = self.size;
        for i in 0..18 {
            let a = s - 11 + i % 3;
            let b = i / 3;
            self.reserve(a, b);
            self.reserve(b, a);
        }
    }

    /// The ordered list of `(x, y)` data-module coordinates: two columns at a time
    /// from the right, zigzagging up then down, skipping the vertical timing column.
    pub fn data_path(&self) -> Vec<(usize, usize)> {
        let s = self.size;
        let mut path = Vec::new();
        let mut upward = true;
        let mut col = s as i32 - 1;
        while col > 0 {
            if col == 6 {
                col -= 1; // skip vertical timing column
            }
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

    /// XOR the mask pattern across all non-reserved (data) modules.
    pub fn apply_mask(&mut self, mask: Mask) {
        for y in 0..self.size {
            for x in 0..self.size {
                let i = self.idx(x, y);
                if !self.reserved[i] && Canvas::mask_bit(mask, x, y) {
                    self.dark[i] ^= true;
                }
            }
        }
    }

    /// Stamp both copies of the 15-bit format information for `level`+`mask`.
    pub fn place_format(&mut self, level: EcLevel, mask: Mask) {
        let bits = format_bits(level, mask);
        let s = self.size;
        for i in 0..=5 {
            self.set_fn(8, i, bit(bits, i));
        }
        self.set_fn(8, 7, bit(bits, 6));
        self.set_fn(8, 8, bit(bits, 7));
        self.set_fn(7, 8, bit(bits, 8));
        for i in 9..15 {
            self.set_fn(14 - i, 8, bit(bits, i));
        }
        for i in 0..8 {
            self.set_fn(s - 1 - i, 8, bit(bits, i));
        }
        for i in 8..15 {
            self.set_fn(8, s - 15 + i, bit(bits, i));
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
            self.set_fn(a, c, b);
            self.set_fn(c, a, b);
        }
    }

    /// Write one data bit into the module at `(x, y)` (used by the encoder while
    /// walking [`Canvas::data_path`]).
    pub fn place_data_bit(&mut self, x: usize, y: usize, dark: bool) {
        self.set(x, y, dark);
    }

    /// Convert to a [`BitMatrix`] with the QR quiet zone recorded.
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

    /// The ISO/IEC 18004 mask penalty score for the current module values, summing
    /// the four standard rules. Lower is better; used to pick a data mask.
    pub fn penalty(&self) -> u32 {
        let s = self.size;
        let get = |x: usize, y: usize| self.dark[y * s + x];
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
        let dark = self.dark.iter().filter(|&&d| d).count() as u32;
        let total = (s * s) as u32;
        let percent = dark * 100 / total;
        let k = (percent as i32 - 50).unsigned_abs() / 5;
        score += k * 10;

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

#[cfg(test)]
mod tests {
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

    #[test]
    fn data_path_covers_all_data_modules() {
        let v = Version::new(1).unwrap();
        let c = Canvas::new(v);
        let path = c.data_path();
        // Version 1: 26 codewords * 8 = 208 data bits.
        assert_eq!(path.len(), 208);
        // No duplicates.
        let mut seen = std::collections::HashSet::new();
        for p in &path {
            assert!(seen.insert(*p), "duplicate module {p:?}");
        }
    }
}
