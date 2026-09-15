//! Micro QR module layout: the single finder pattern, edge timing patterns,
//! format information (with its own BCH mask), the four data masks and the data
//! zig-zag path.
//!
//! [`Canvas`] is the shared workspace for both encoding (place data, mask, stamp
//! format info) and decoding (read format info, then read data along the same path).

use super::tables::{decode_format, format_bits};
use super::{MicroMask, MicroVersion};
#[cfg(feature = "alloc")]
use crate::output::BitMatrix;
use crate::output::MatrixBuf;

/// Quiet-zone width required around a Micro QR symbol, in modules (ISO/IEC 18004
/// specifies 2 for Micro QR).
pub const QUIET_ZONE: usize = 2;

/// A square Micro QR module grid over a bit-packed [`MatrixBuf`]. Which cells are
/// function patterns is computed ([`is_function_module`]), not stored.
pub struct Canvas<'a> {
    size: usize,
    grid: MatrixBuf<'a>,
}

impl<'a> Canvas<'a> {
    /// Bytes of storage a canvas for `version` needs.
    pub const fn storage_len(version: MicroVersion) -> usize {
        let size = 11 + 2 * version as usize; // M1 = 0 … M4 = 3
        MatrixBuf::bytes_for(size, size)
    }

    /// Build a canvas for `version` over `storage` with all function patterns placed,
    /// but no data or format bits yet.
    ///
    /// # Errors
    /// [`crate::Error::Capacity`] if `storage` is shorter than [`Canvas::storage_len`].
    pub fn new(version: MicroVersion, storage: &'a mut [u8]) -> crate::Result<Self> {
        let size = version.size();
        let grid = MatrixBuf::new(storage, size, size, QUIET_ZONE)?;
        let mut c = Canvas { size, grid };
        c.place_finder();
        c.place_timing();
        Ok(c)
    }

    /// Rebuild a canvas from an already-sampled matrix of the given version.
    #[cfg(feature = "alloc")]
    pub fn from_matrix(
        version: MicroVersion,
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
        Ok(Canvas { size, grid })
    }

    /// Module value at `(x, y)`.
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.grid.get(x, y)
    }

    /// The finished module grid.
    pub fn into_grid(self) -> MatrixBuf<'a> {
        self.grid
    }

    fn place_finder(&mut self) {
        // 7x7 finder at the top-left corner; its separator is light, as a fresh grid.
        for dy in 0..7 {
            for dx in 0..7 {
                let ring = dx == 0 || dx == 6 || dy == 0 || dy == 6;
                let core = (2..=4).contains(&dx) && (2..=4).contains(&dy);
                self.grid.set(dx, dy, ring || core);
            }
        }
    }

    fn place_timing(&mut self) {
        // Horizontal timing along the top row, vertical along the left column.
        for i in (8..self.size).step_by(2) {
            self.grid.set(i, 0, true);
            self.grid.set(0, i, true);
        }
    }

    /// The ordered `(x, y)` data-module coordinates: two columns at a time from the
    /// right, zig-zagging up then down. No interior timing column to skip.
    pub fn data_path(&self) -> DataPath {
        DataPath {
            size: self.size,
            col: self.size - 1,
            row: 0,
            left: false,
            upward: true,
        }
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

    /// XOR the mask pattern across all data modules. Applying the same mask twice
    /// restores the original grid.
    pub fn apply_mask(&mut self, mask: MicroMask) {
        for y in 0..self.size {
            for x in 0..self.size {
                if Canvas::mask_bit(mask, x, y) && !is_function_module(x, y) {
                    self.grid.toggle(x, y);
                }
            }
        }
    }

    /// Stamp the 15-bit format information for `symbol_number` + `mask`.
    pub fn place_format(&mut self, symbol_number: u8, mask: MicroMask) {
        let bits = format_bits(symbol_number, mask.index());
        for i in 0..8 {
            self.grid.set(8, 1 + i, (bits >> i) & 1 != 0); // vertical strip
            self.grid.set(1 + i, 8, (bits >> (14 - i)) & 1 != 0); // horizontal strip
        }
    }

    /// Write one data bit into the module at `(x, y)`.
    pub fn place_data_bit(&mut self, x: usize, y: usize, dark: bool) {
        self.grid.set(x, y, dark);
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

/// Iterator over the data-module coordinates of a version, in placement order.
pub struct DataPath {
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
                // Strip finished: the last strip is columns 2 and 1.
                if self.col <= 2 {
                    return None;
                }
                self.col -= 2;
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
            if !is_function_module(x, y) {
                return Some((x, y));
            }
        }
    }
}

/// Whether `(x, y)` is a function-pattern / reserved module: the finder with its
/// separator and format strips (the top-left 9×9 block) or an edge timing pattern.
/// The layout is the same for every version.
pub fn is_function_module(x: usize, y: usize) -> bool {
    (x <= 8 && y <= 8) || x == 0 || y == 0
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
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
            let side = v.size();
            assert_eq!(
                Canvas::storage_len(v),
                MatrixBuf::bytes_for(side, side),
                "{v:?}"
            );
            let mut storage = [0u8; Canvas::storage_len(MicroVersion::M4)];
            let c = Canvas::new(v, &mut storage).unwrap();
            let mut seen = std::collections::HashSet::new();
            for p in c.data_path() {
                assert!(!is_function_module(p.0, p.1));
                assert!(seen.insert(p), "duplicate module {p:?} in {v:?}");
            }
            assert_eq!(seen.len(), data_module_count(v), "{v:?}");
        }
    }
}
