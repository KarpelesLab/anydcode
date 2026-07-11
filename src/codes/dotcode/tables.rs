//! Static tables for DotCode: the 9-bit symbol-character dot patterns and their
//! reverse lookup.
//!
//! The dot patterns are taken from AIM ISS DotCode Rev. 4.0 Annex C. Each of the
//! 113 codeword values (`0..=112`) maps to a 9-bit pattern; the pattern's bits are
//! written most-significant-first into the dot stream (2 header bits for the mask
//! precede the codeword patterns). The values are cross-checked byte-for-byte
//! against zint's `dc_dot_patterns[]` (`backend/dotcode.c`).

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

#[cfg(test)]
mod tests {
    use super::*;

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
