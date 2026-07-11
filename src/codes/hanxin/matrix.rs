//! Han Xin module layout: corner finder patterns, finder separators, the reserved
//! function-information region, the row-major data path, data masking, and the
//! function-information codeword (version/EC/mask + GF(2^4) Reed–Solomon).
//!
//! All geometry mirrors zint `backend/hanxin.c` (`hx_setup_grid`,
//! `hx_set_function_info`, `hx_apply_bitmask`, `hx_evaluate`) for versions 1–3.
//!
//! Each grid cell is a status byte identical to zint's convention:
//! - `0x00` empty data cell (light), `0x01` data cell (dark)
//! - `0x10` function cell (light/reserved), `0x11` function cell (dark/reserved)
//!
//! so *dark* is `cell & 0x01` and *function/reserved* is `cell & 0xF0`.

use super::gf::GaloisField;
use super::tables::{FINDER_BOTTOM_RIGHT, FINDER_SIDE, FINDER_TOP_LEFT};
use super::{EcLevel, Mask, Version};
use crate::output::BitMatrix;

/// Quiet-zone width required around a Han Xin symbol, in modules (ISO/IEC 20830 §5).
pub const QUIET_ZONE: usize = 3;

const DATA_LIGHT: u8 = 0x00;
const DATA_DARK: u8 = 0x01;
const FUNC_LIGHT: u8 = 0x10;
const FUNC_DARK: u8 = 0x11;

/// A Han Xin module grid with per-cell status bytes.
pub struct Grid {
    /// Side length in modules.
    pub size: usize,
    cells: Vec<u8>,
}

impl Grid {
    /// Build a fresh grid for `version` with finder patterns, separators and the
    /// reserved function-information region placed (no data, mask or function info).
    pub fn new(version: Version) -> Self {
        let size = version.size();
        let mut g = Grid {
            size,
            cells: vec![DATA_LIGHT; size * size],
        };
        g.place_finders();
        g.place_separators();
        g.reserve_function_region();
        g
    }

    /// Rebuild a grid of `version` from a sampled matrix: function patterns are
    /// reserved as usual, then every module's darkness is taken from `matrix`.
    pub fn from_matrix(version: Version, matrix: &BitMatrix) -> Self {
        let mut g = Grid::new(version);
        for y in 0..g.size {
            for x in 0..g.size {
                let i = y * g.size + x;
                let dark = matrix.get(x, y);
                // Preserve the function/reserved flag from the fresh layout; only the
                // darkness bit comes from the sampled matrix.
                let func = g.cells[i] & 0xF0;
                g.cells[i] = func | (dark as u8);
            }
        }
        g
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.size + x
    }

    /// Whether `(x, y)` is a function/reserved cell (not part of the data stream).
    pub fn is_function(&self, x: usize, y: usize) -> bool {
        self.cells[self.idx(x, y)] & 0xF0 != 0
    }

    /// Whether `(x, y)` is a dark module.
    pub fn dark(&self, x: usize, y: usize) -> bool {
        self.cells[self.idx(x, y)] & 0x01 != 0
    }

    /// Set the darkness of a data cell.
    pub fn set_data(&mut self, x: usize, y: usize, dark: bool) {
        let i = self.idx(x, y);
        self.cells[i] = if dark { DATA_DARK } else { DATA_LIGHT };
    }

    fn set_func(&mut self, x: usize, y: usize, dark: bool) {
        let i = self.idx(x, y);
        self.cells[i] = if dark { FUNC_DARK } else { FUNC_LIGHT };
    }

    fn place_one_finder(&mut self, ox: usize, oy: usize, pattern: &[u8; 7]) {
        for (yp, &row) in pattern.iter().enumerate() {
            for xp in 0..7 {
                let dark = row & (0x40 >> xp) != 0;
                self.set_func(ox + xp, oy + yp, dark);
            }
        }
    }

    fn place_finders(&mut self) {
        let s = self.size;
        self.place_one_finder(0, 0, &FINDER_TOP_LEFT);
        self.place_one_finder(s - 7, 0, &FINDER_SIDE);
        self.place_one_finder(0, s - 7, &FINDER_SIDE);
        self.place_one_finder(s - 7, s - 7, &FINDER_BOTTOM_RIGHT);
    }

    fn place_separators(&mut self) {
        let s = self.size;
        for i in 0..8 {
            // Top-left.
            self.set_func(i, 7, false);
            self.set_func(7, i, false);
            // Top-right.
            self.set_func(s - i - 1, 7, false);
            self.set_func(7, s - i - 1, false);
            // Bottom-left.
            self.set_func(s - 8, i, false);
            self.set_func(i, s - 8, false);
            // Bottom-right.
            self.set_func(s - i - 1, s - 8, false);
            self.set_func(s - 8, s - i - 1, false);
        }
    }

    fn reserve_function_region(&mut self) {
        let s = self.size;
        for i in 0..9 {
            // Top-left.
            self.set_func(i, 8, false);
            self.set_func(8, i, false);
            // Top-right.
            self.set_func(s - i - 1, 8, false);
            self.set_func(8, s - i - 1, false);
            // Bottom-left.
            self.set_func(s - 9, i, false);
            self.set_func(i, s - 9, false);
            // Bottom-right.
            self.set_func(s - i - 1, s - 9, false);
            self.set_func(s - 9, s - i - 1, false);
        }
    }

    /// Row-major list of `(x, y)` coordinates of all data (non-function) cells.
    pub fn data_path(&self) -> Vec<(usize, usize)> {
        let mut path = Vec::new();
        for y in 0..self.size {
            for x in 0..self.size {
                if !self.is_function(x, y) {
                    path.push((x, y));
                }
            }
        }
        path
    }

    /// Whether data-mask `pattern` (1–3; 0 is the null mask) flips module `(x, y)`.
    /// Uses zint's 1-indexed coordinates `i = y + 1`, `j = x + 1`.
    pub fn mask_flip(pattern: u8, x: usize, y: usize) -> bool {
        let i = y + 1;
        let j = x + 1;
        match pattern {
            1 => (i + j).is_multiple_of(2),
            2 => ((i + j) % 3 + j % 3).is_multiple_of(2),
            3 => (i % j + j % i + i % 3 + j % 3).is_multiple_of(2),
            _ => false,
        }
    }

    /// XOR data-mask `pattern` across every data (non-function) cell.
    pub fn apply_mask(&mut self, pattern: u8) {
        if pattern == 0 {
            return;
        }
        for y in 0..self.size {
            for x in 0..self.size {
                let i = self.idx(x, y);
                if self.cells[i] & 0xF0 == 0 && Grid::mask_flip(pattern, x, y) {
                    self.cells[i] ^= 0x01;
                }
            }
        }
    }

    /// Stamp the function information (version/EC/mask + GF(2^4) RS) into all four
    /// corners.
    pub fn place_function_info(&mut self, version: Version, level: EcLevel, mask: Mask) {
        let bits = function_info_bits(version, level, mask);
        let s = self.size;
        for i in 0..9 {
            self.set_func(i, 8, bits[i]);
            self.set_func(s - 1 - i, s - 9, bits[i]);

            self.set_func(8, 8 - i, bits[i + 8]);
            self.set_func(s - 9, s - 9 + i, bits[i + 8]);

            self.set_func(s - 9, i, bits[i + 17]);
            self.set_func(8, s - 1 - i, bits[i + 17]);

            self.set_func(s - 9 + i, 8, bits[i + 25]);
            self.set_func(8 - i, s - 9, bits[i + 25]);
        }
    }

    /// Read the function information back, returning `(version, level, mask)` after
    /// GF(2^4) RS correction. `size_version` is the version implied by the grid size.
    pub fn read_function_info(&self, size_version: Version) -> Option<(Version, EcLevel, Mask)> {
        let s = self.size;
        let mut bits = [false; 34];
        for i in 0..9 {
            bits[i] = self.dark(i, 8);
            bits[i + 8] = self.dark(8, 8 - i);
            bits[i + 17] = self.dark(s - 9, i);
            bits[i + 25] = self.dark(s - 9 + i, 8);
        }
        decode_function_info(&bits, size_version)
    }

    /// The ISO/IEC 20830 mask-penalty score (zint `hx_evaluate`) obtained by applying
    /// `mask` to the current (unmasked) data grid and stamping the matching function
    /// info. Lower is better; used to auto-select a mask. The grid itself is not
    /// modified.
    pub fn penalty(&self, version: Version, level: EcLevel, mask: Mask) -> u32 {
        let pattern = mask.index();
        let mut local = vec![0u8; self.size * self.size];
        for (i, &c) in self.cells.iter().enumerate() {
            local[i] = c & 0x01;
        }
        if pattern != 0 {
            for y in 0..self.size {
                for x in 0..self.size {
                    let i = self.idx(x, y);
                    if self.cells[i] & 0xF0 == 0 && Grid::mask_flip(pattern, x, y) {
                        local[i] ^= 0x01;
                    }
                }
            }
        }
        stamp_function_info(&mut local, self.size, version, level, mask);
        evaluate(&local, self.size)
    }

    /// Convert to a [`BitMatrix`] with the Han Xin quiet zone recorded.
    pub fn to_bitmatrix(&self) -> BitMatrix {
        let mut m = BitMatrix::new(self.size, self.size, QUIET_ZONE);
        for y in 0..self.size {
            for x in 0..self.size {
                if self.dark(x, y) {
                    m.set(x, y, true);
                }
            }
        }
        m
    }
}

/// The 34 function-information bits: 8-bit `version+20`, 2-bit `EC-1`, 2-bit mask,
/// four 4-bit GF(2^4) RS EC symbols, then six zero pad bits.
fn function_info_bits(version: Version, level: EcLevel, mask: Mask) -> [bool; 34] {
    let mut bits = [false; 34];
    let mut bp = 0usize;
    let push = |bits: &mut [bool; 34], value: u32, len: usize, bp: &mut usize| {
        for k in (0..len).rev() {
            bits[*bp] = (value >> k) & 1 != 0;
            *bp += 1;
        }
    };
    push(&mut bits, (version.number() as u32) + 20, 8, &mut bp);
    push(&mut bits, level.field_value() as u32, 2, &mut bp);
    push(&mut bits, mask.index() as u32, 2, &mut bp);

    // Pack the first 12 bits into three GF(2^4) data symbols (MSB first).
    let mut cw = [0u8; 3];
    for (i, c) in cw.iter_mut().enumerate() {
        for j in 0..4 {
            if bits[i * 4 + j] {
                *c |= 0x08 >> j;
            }
        }
    }
    let ecc = GaloisField::function().rs_encode(&cw, 4, 1);
    for &sym in &ecc {
        push(&mut bits, sym as u32, 4, &mut bp);
    }
    // Remaining bits (28..34) stay zero.
    bits
}

/// Decode 34 function-info bits (with GF(2^4) RS correction) to (version, EC, mask).
fn decode_function_info(
    bits: &[bool; 34],
    size_version: Version,
) -> Option<(Version, EcLevel, Mask)> {
    // Recover the 3 data + 4 EC GF(2^4) symbols and RS-correct them.
    let mut block = [0u8; 7];
    for (i, b) in block.iter_mut().enumerate() {
        let mut sym = 0u8;
        for j in 0..4 {
            if bits[i * 4 + j] {
                sym |= 0x08 >> j;
            }
        }
        *b = sym;
    }
    let corrected = GaloisField::function()
        .rs_decode(&block, 4, 1)
        .unwrap_or_else(|| block.to_vec());

    // Reassemble the 12 data bits.
    let mut v = 0u32;
    for &sym in &corrected[..3] {
        v = (v << 4) | sym as u32;
    }
    let version_field = (v >> 4) & 0xFF;
    let level_field = ((v >> 2) & 0x03) as u8;
    let mask_field = (v & 0x03) as u8;

    let version_num = version_field.checked_sub(20)? as u8;
    // Trust the geometric size over the (correctable) field if they disagree only
    // due to unrecoverable noise; require agreement for a confident decode.
    if version_num != size_version.number() {
        return None;
    }
    let version = Version::new(version_num)?;
    let level = EcLevel::from_field_value(level_field)?;
    let mask = Mask::new(mask_field)?;
    Some((version, level, mask))
}

/// Stamp function info onto a pure 0/1 darkness grid (used by penalty evaluation).
fn stamp_function_info(
    local: &mut [u8],
    size: usize,
    version: Version,
    level: EcLevel,
    mask: Mask,
) {
    let bits = function_info_bits(version, level, mask);
    let set = |local: &mut [u8], x: usize, y: usize, v: bool| {
        local[y * size + x] = v as u8;
    };
    for i in 0..9 {
        set(local, i, 8, bits[i]);
        set(local, size - 1 - i, size - 9, bits[i]);
        set(local, 8, 8 - i, bits[i + 8]);
        set(local, size - 9, size - 9 + i, bits[i + 8]);
        set(local, size - 9, i, bits[i + 17]);
        set(local, 8, size - 1 - i, bits[i + 17]);
        set(local, size - 9 + i, 8, bits[i + 25]);
        set(local, 8 - i, size - 9, bits[i + 25]);
    }
}

/// zint `hx_evaluate`: finder-like 1:1:1:1:3 patterns (penalty 50) plus same-colour
/// runs of length ≥ 3 (penalty `run * 4`), over rows and columns.
fn evaluate(local: &[u8], size: usize) -> u32 {
    let at = |x: usize, y: usize| local[y * size + x] != 0;
    let mut result: u32 = 0;

    // Test 1 — vertical finder-like patterns.
    for x in 0..size {
        let mut y = 0usize;
        while y + 6 < size {
            if at(x, y)
                && at(x, y + 1) != at(x, y + 5)
                && at(x, y + 2)
                && !at(x, y + 3)
                && at(x, y + 4)
                && at(x, y + 6)
            {
                let mut before = 0i32;
                let mut b = y as i32 - 1;
                while b >= y as i32 - 3 {
                    if b < 0 {
                        before = 3;
                        break;
                    }
                    if at(x, b as usize) {
                        break;
                    }
                    before += 1;
                    b -= 1;
                }
                if before == 3 {
                    result += 50;
                } else {
                    let mut after = 0i32;
                    let mut a = y + 7;
                    while a <= y + 9 {
                        if a >= size {
                            after = 3;
                            break;
                        }
                        if at(x, a) {
                            break;
                        }
                        after += 1;
                        a += 1;
                    }
                    if after == 3 {
                        result += 50;
                    }
                }
                y += 1;
            }
            y += 1;
        }
    }

    // Test 1 — horizontal finder-like patterns.
    let h1: [bool; 7] = [true, false, true, false, true, true, true];
    let h2: [bool; 7] = [true, true, true, false, true, false, true];
    for y in 0..size {
        let mut x = 0usize;
        while x + 6 < size {
            let window: [bool; 7] = [
                at(x, y),
                at(x + 1, y),
                at(x + 2, y),
                at(x + 3, y),
                at(x + 4, y),
                at(x + 5, y),
                at(x + 6, y),
            ];
            if window == h1 || window == h2 {
                let mut before = 0i32;
                let mut b = x as i32 - 1;
                while b >= x as i32 - 3 {
                    if b < 0 {
                        before = 3;
                        break;
                    }
                    if at(b as usize, y) {
                        break;
                    }
                    before += 1;
                    b -= 1;
                }
                if before == 3 {
                    result += 50;
                } else {
                    let mut after = 0i32;
                    let mut a = x + 7;
                    while a <= x + 9 {
                        if a >= size {
                            after = 3;
                            break;
                        }
                        if at(a, y) {
                            break;
                        }
                        after += 1;
                        a += 1;
                    }
                    if after == 3 {
                        result += 50;
                    }
                }
                x += 1;
            }
            x += 1;
        }
    }

    // Test 2 — same-colour runs, vertical then horizontal.
    for x in 0..size {
        let mut block = 0u32;
        let mut state = false;
        for y in 0..size {
            if at(x, y) == state {
                block += 1;
            } else {
                if block >= 3 {
                    result += block * 4;
                }
                block = 1;
                state = at(x, y);
            }
        }
        if block >= 3 {
            result += block * 4;
        }
    }
    for y in 0..size {
        let mut block = 0u32;
        let mut state = false;
        for x in 0..size {
            if at(x, y) == state {
                block += 1;
            } else {
                if block >= 3 {
                    result += block * 4;
                }
                block = 1;
                state = at(x, y);
            }
        }
        if block >= 3 {
            result += block * 4;
        }
    }

    result
}
