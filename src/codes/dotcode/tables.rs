//! Static tables for DotCode: the 9-bit symbol-character dot patterns and their
//! reverse lookup.
//!
//! The dot patterns are taken from AIM ISS DotCode Rev. 4.0 Annex C. Each of the
//! 113 codeword values (`0..=112`) maps to a 9-bit pattern; the pattern's bits are
//! written most-significant-first into the dot stream (2 header bits for the mask
//! precede the codeword patterns). The values are cross-checked byte-for-byte
//! against zint's `dc_dot_patterns[]` (`backend/dotcode.c`).

// Special codeword values (shared by encoder and decoder).
pub(crate) const LATCH_A: u8 = 101;
pub(crate) const LATCH_B_FROM_A: u8 = 102;
pub(crate) const LATCH_BC: u8 = 106; // Latch B (from C) / Latch C (from A/B)
pub(crate) const FNC1: u8 = 107;
pub(crate) const FNC2: u8 = 108;
pub(crate) const FNC3: u8 = 109; // Reader Init, or Bin-terminate-latch-A
pub(crate) const UPPER_SHIFT_A: u8 = 110;
pub(crate) const UPPER_SHIFT_B: u8 = 111;
pub(crate) const BIN_LATCH: u8 = 112;

/// The DotCode Galois field prime.
pub const DC_GF: u16 = 113;

/// DotCode symbol-character dot patterns, from AIM ISS DotCode Rev 4.0 Annex C.
///
/// Index = codeword value (`0..=112`); value = the 9-bit dot pattern.
pub const DC_DOT_PATTERNS: [u16; 113] = [
    0x155, 0x0ab, 0x0ad, 0x0b5, 0x0d5, 0x156, 0x15a, 0x16a, 0x1aa, 0x0ae, 0x0b6, 0x0ba, 0x0d6,
    0x0da, 0x0ea, 0x12b, 0x12d, 0x135, 0x14b, 0x14d, 0x153, 0x159, 0x165, 0x169, 0x195, 0x1a5,
    0x1a9, 0x057, 0x05b, 0x05d, 0x06b, 0x06d, 0x075, 0x097, 0x09b, 0x09d, 0x0a7, 0x0b3, 0x0b9,
    0x0cb, 0x0cd, 0x0d3, 0x0d9, 0x0e5, 0x0e9, 0x12e, 0x136, 0x13a, 0x14e, 0x15c, 0x166, 0x16c,
    0x172, 0x174, 0x196, 0x19a, 0x1a6, 0x1ac, 0x1b2, 0x1b4, 0x1ca, 0x1d2, 0x1d4, 0x05e, 0x06e,
    0x076, 0x07a, 0x09e, 0x0bc, 0x0ce, 0x0dc, 0x0e6, 0x0ec, 0x0f2, 0x0f4, 0x117, 0x11b, 0x11d,
    0x127, 0x133, 0x139, 0x147, 0x163, 0x171, 0x18b, 0x18d, 0x193, 0x199, 0x1a3, 0x1b1, 0x1c5,
    0x1c9, 0x1d1, 0x02f, 0x037, 0x03b, 0x03d, 0x04f, 0x067, 0x073, 0x079, 0x08f, 0x0c7, 0x0e3,
    0x0f1, 0x11e, 0x13c, 0x178, 0x18e, 0x19c, 0x1b8, 0x1c6, 0x1cc,
];

/// Reverse of [`DC_DOT_PATTERNS`]: recover the codeword value for a 9-bit pattern.
///
/// Returns `None` for patterns that are not valid DotCode symbol characters (e.g.
/// the all-ones pad pattern `0x1ff`).
pub fn codeword_for_pattern(pattern: u16) -> Option<u8> {
    DC_DOT_PATTERNS
        .iter()
        .position(|&p| p == pattern)
        .map(|i| i as u8)
}

/// Is `(column, row)` a reserved corner dot (holds one of the final six bits)?
pub(crate) fn is_corner(column: usize, row: usize, width: usize, height: usize) -> bool {
    if column == 0 && row == 0 {
        return true;
    }
    if height & 1 == 1 {
        if (column == width - 2 && row == 0) || (column == width - 1 && row == 1) {
            return true;
        }
        if column == 0 && row == height - 1 {
            return true;
        }
    } else {
        if column == width - 1 && row == 0 {
            return true;
        }
        if (column == 0 && row == height - 2) || (column == 1 && row == height - 1) {
            return true;
        }
    }
    (column == width - 2 && row == height - 1) || (column == width - 1 && row == height - 2)
}

/// Row-major grid indices of the six reserved corner dots.
pub(crate) fn corner_indices(width: usize, height: usize) -> [usize; 6] {
    if height & 1 == 1 {
        [
            0,
            width - 2,
            width * 2 - 1,
            (height - 1) * width - 1,
            (height - 1) * width,
            height * width - 2,
        ]
    } else {
        [
            0,
            width - 1,
            (height - 2) * width,
            (height - 1) * width - 1,
            (height - 1) * width + 1,
            height * width - 2,
        ]
    }
}

/// Dots needed for `data_length` data codewords: the two mask bits plus nine per data
/// and check codeword (`3 + data_length / 2` check codewords).
pub(crate) fn min_dots_for(data_length: usize) -> usize {
    9 * (data_length + 3 + data_length / 2) + 2
}

/// The number of data codewords a `width × height` symbol carries: the most that
/// fit, since an encoder turns every spare group of dots that can hold another
/// codeword into a pad codeword. `None` if not even one fits.
pub(crate) fn data_length_for_size(width: usize, height: usize) -> Option<usize> {
    let n_dots = (width * height) / 2;
    (1..).take_while(|&dl| min_dots_for(dl) <= n_dots).last()
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn corner_indices_are_the_corner_cells() {
        for (w, h) in [(6usize, 5usize), (5, 6), (21, 14), (14, 21), (200, 199)] {
            let mut expected: Vec<usize> = (0..h)
                .flat_map(|r| (0..w).map(move |c| (c, r)))
                .filter(|&(c, r)| (c + r) % 2 == 0 && is_corner(c, r, w, h))
                .map(|(c, r)| r * w + c)
                .collect();
            expected.sort_unstable();
            assert_eq!(corner_indices(w, h).to_vec(), expected, "{w}x{h}");
        }
    }

    #[test]
    fn patterns_are_distinct_and_9bit() {
        for (i, &p) in DC_DOT_PATTERNS.iter().enumerate() {
            assert!(p <= 0x1ff, "pattern {i} exceeds 9 bits");
            assert_eq!(codeword_for_pattern(p), Some(i as u8));
        }
        // The pad pattern (nine set bits) is not a valid symbol character.
        assert_eq!(codeword_for_pattern(0x1ff), None);
    }
}
