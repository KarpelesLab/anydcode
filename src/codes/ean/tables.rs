//! Element patterns, parity tables, guards, and the numeric helpers (check digits,
//! UPC-E expansion) shared by the EAN/UPC encoder and decoder.
//!
//! References (all consulted for the tables below):
//! - GS1 General Specifications, section 5 (bar/space element patterns).
//! - Wikipedia, "International Article Number" (EAN-13/EAN-8 L/G/R codes and the
//!   first-digit parity table).
//! - Wikipedia, "Universal Product Code" (UPC-A, UPC-E number-system parity).
//! - Wikipedia, "EAN-5" and "EAN-2" (add-on checksum and parity tables).

/// The seven-module L-code (odd parity) patterns for digits 0..=9, MSB first
/// (`1` = bar). `R` is the bitwise complement; `G` is `R` reversed. These three
/// families are the standard EAN/UPC element encodings.
const L_PATTERNS: [u8; 10] = [
    0b0001101, // 0
    0b0011001, // 1
    0b0010011, // 2
    0b0111101, // 3
    0b0100011, // 4
    0b0110001, // 5
    0b0101111, // 6
    0b0111011, // 7
    0b0110111, // 8
    0b0001011, // 9
];

/// Expand a 7-bit pattern (MSB first) into modules, `true` = bar.
fn bits7(pattern: u8) -> [bool; 7] {
    let mut out = [false; 7];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = (pattern >> (6 - i)) & 1 == 1;
    }
    out
}

/// L-code (left-hand odd parity) modules for a digit value 0..=9.
pub fn l_code(d: u8) -> [bool; 7] {
    bits7(L_PATTERNS[d as usize])
}

/// R-code (right-hand) modules: the bitwise complement of the L-code.
pub fn r_code(d: u8) -> [bool; 7] {
    bits7((!L_PATTERNS[d as usize]) & 0x7F)
}

/// G-code (left-hand even parity) modules: the R-code read in reverse.
pub fn g_code(d: u8) -> [bool; 7] {
    let r = r_code(d);
    let mut g = [false; 7];
    for i in 0..7 {
        g[i] = r[6 - i];
    }
    g
}

/// Left-hand element modules for a digit, choosing G (even) when `even` else L (odd).
pub fn left_code(d: u8, even: bool) -> [bool; 7] {
    if even { g_code(d) } else { l_code(d) }
}

/// Match a 7-module slice against the L and G tables, returning `(digit, is_even)`.
pub fn match_left(m: &[bool]) -> Option<(u8, bool)> {
    for d in 0..10u8 {
        if m == l_code(d) {
            return Some((d, false));
        }
        if m == g_code(d) {
            return Some((d, true));
        }
    }
    None
}

/// Match a 7-module slice against the R table, returning the digit value.
pub fn match_right(m: &[bool]) -> Option<u8> {
    (0..10u8).find(|&d| m == r_code(d))
}

// ---- Guard and delineator patterns (`true` = bar) --------------------------

/// Normal start/end guard: `101`.
pub const NORMAL_GUARD: [bool; 3] = [true, false, true];
/// Centre guard used by EAN-13/EAN-8/UPC-A: `01010`.
pub const CENTER_GUARD: [bool; 5] = [false, true, false, true, false];
/// UPC-E end guard: `010101`.
pub const UPCE_END_GUARD: [bool; 6] = [false, true, false, true, false, true];
/// Add-on start guard: `1011`.
pub const ADDON_LEAD: [bool; 4] = [true, false, true, true];
/// Add-on inter-digit delineator: `01`.
pub const ADDON_SEP: [bool; 2] = [false, true];

// ---- Parity tables ---------------------------------------------------------

/// EAN-13 first-digit parity: for each leading digit, the odd/even (L/G) pattern
/// of the six left-hand digits. `true` = even (G).
pub const EAN13_FIRST_PARITY: [[bool; 6]; 10] = [
    [false, false, false, false, false, false], // 0: LLLLLL
    [false, false, true, false, true, true],    // 1: LLGLGG
    [false, false, true, true, false, true],    // 2: LLGGLG
    [false, false, true, true, true, false],    // 3: LLGGGL
    [false, true, false, false, true, true],    // 4: LGLLGG
    [false, true, true, false, false, true],    // 5: LGGLLG
    [false, true, true, true, false, false],    // 6: LGGGLL
    [false, true, false, true, false, true],    // 7: LGLGLG
    [false, true, false, true, true, false],    // 8: LGLGGL
    [false, true, true, false, true, false],    // 9: LGGLGL
];

/// UPC-E parity for number system 0, indexed by check digit; `true` = even (G).
/// Number system 1 uses the bitwise inverse of each row.
pub const UPCE_PARITY_NS0: [[bool; 6]; 10] = [
    [true, true, true, false, false, false], // 0: EEEOOO
    [true, true, false, true, false, false], // 1: EEOEOO
    [true, true, false, false, true, false], // 2: EEOOEO
    [true, true, false, false, false, true], // 3: EEOOOE
    [true, false, true, true, false, false], // 4: EOEEOO
    [true, false, false, true, true, false], // 5: EOOEEO
    [true, false, false, false, true, true], // 6: EOOOEE
    [true, false, true, false, true, false], // 7: EOEOEO
    [true, false, true, false, false, true], // 8: EOEOOE
    [true, false, false, true, false, true], // 9: EOOEOE
];

/// The UPC-E parity pattern for a number system (0 or 1) and check digit.
pub fn upce_parity(ns: u8, check: u8) -> [bool; 6] {
    let base = UPCE_PARITY_NS0[check as usize];
    if ns == 1 {
        let mut inv = [false; 6];
        for i in 0..6 {
            inv[i] = !base[i];
        }
        inv
    } else {
        base
    }
}

/// Reverse the UPC-E parity lookup: given the six observed parities (`true` = even),
/// recover `(number_system, check_digit)`, or `None` if no combination matches.
pub fn upce_from_parity(parity: &[bool; 6]) -> Option<(u8, u8)> {
    for ns in 0..2u8 {
        for check in 0..10u8 {
            if upce_parity(ns, check) == *parity {
                return Some((ns, check));
            }
        }
    }
    None
}

/// EAN-5 add-on parity, indexed by the add-on checksum; `true` = even (G).
pub const EAN5_PARITY: [[bool; 5]; 10] = [
    [true, true, false, false, false], // 0: GGLLL
    [true, false, true, false, false], // 1: GLGLL
    [true, false, false, true, false], // 2: GLLGL
    [true, false, false, false, true], // 3: GLLLG
    [false, true, true, false, false], // 4: LGGLL
    [false, false, true, true, false], // 5: LLGGL
    [false, false, false, true, true], // 6: LLLGG
    [false, true, false, true, false], // 7: LGLGL
    [false, true, false, false, true], // 8: LGLLG
    [false, false, true, false, true], // 9: LLGLG
];

/// EAN-2 add-on parity, indexed by `value % 4`; `true` = even (G).
pub const EAN2_PARITY: [[bool; 2]; 4] = [
    [false, false], // 0: OO
    [false, true],  // 1: OE
    [true, false],  // 2: EO
    [true, true],   // 3: EE
];

// ---- Numeric helpers -------------------------------------------------------

/// The EAN/UPC mod-10 check digit for the given data digit *values* (0..=9, excluding
/// the check position), as a value 0..=9. The rightmost data digit is weighted 3, the
/// next 1, alternating leftward.
pub fn check_digit(data: &[u8]) -> u8 {
    let mut sum = 0u32;
    for (i, &d) in data.iter().rev().enumerate() {
        let w = if i % 2 == 0 { 3 } else { 1 };
        sum += w * u32::from(d);
    }
    ((10 - (sum % 10)) % 10) as u8
}

/// The EAN-5 add-on checksum digit (0..=9) from digit *values*: weighted 3, 9, 3, 9, 3.
pub fn ean5_checksum(digits: &[u8]) -> u8 {
    let mut sum = 0u32;
    for (i, &d) in digits.iter().enumerate() {
        let w = if i % 2 == 0 { 3 } else { 9 };
        sum += w * u32::from(d);
    }
    (sum % 10) as u8
}

/// Expand a UPC-E payload (number system `ns` plus six payload digit values) into
/// the eleven UPC-A data digits (number system + ten), excluding the check digit.
///
/// The last payload digit selects one of four zero-insertion rules (GS1 spec /
/// Wikipedia, "Universal Product Code").
pub fn upce_expand(ns: u8, payload: &[u8; 6]) -> [u8; 11] {
    let d = payload;
    let z = 0u8;
    match d[5] {
        0..=2 => [ns, d[0], d[1], d[5], z, z, z, z, d[2], d[3], d[4]],
        3 => [ns, d[0], d[1], d[2], z, z, z, z, z, d[3], d[4]],
        4 => [ns, d[0], d[1], d[2], d[3], z, z, z, z, z, d[4]],
        _ => [ns, d[0], d[1], d[2], d[3], d[4], z, z, z, z, d[5]],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digits(s: &str) -> Vec<u8> {
        s.bytes().map(|b| b - b'0').collect()
    }

    /// EAN-13 mod-10: `590123412345` → check digit `7`.
    /// Source: GS1 check-digit calculator; the number is a common worked example.
    #[test]
    fn ean13_check_reference() {
        assert_eq!(check_digit(&digits("590123412345")), 7);
    }

    /// UPC-A mod-10: `03600029145` → check digit `2`.
    /// Source: Wikipedia, "Universal Product Code" (036000291452).
    #[test]
    fn upca_check_reference() {
        assert_eq!(check_digit(&digits("03600029145")), 2);
    }

    /// UPC-E `04252614` (number system 0, payload `425261`) expands to UPC-A
    /// `04210000526` whose check digit is `4`. Source: Wikipedia, "Universal Product
    /// Code" (UPC-E ⟷ UPC-A example 042100005264 ⟷ 04252614).
    #[test]
    fn upce_expand_reference() {
        let expanded = upce_expand(0, &[4, 2, 5, 2, 6, 1]);
        assert_eq!(expanded, [0, 4, 2, 1, 0, 0, 0, 0, 5, 2, 6]);
        assert_eq!(check_digit(&expanded), 4);
    }

    /// EAN-5 price add-on `52495` ($24.95) → checksum `1`.
    /// Source: Wikipedia, "EAN-5" worked example.
    #[test]
    fn ean5_checksum_reference() {
        assert_eq!(ean5_checksum(&digits("52495")), 1);
    }

    /// L/R are complements and G is R reversed, for every digit.
    #[test]
    fn element_relations() {
        for d in 0..10u8 {
            let l = l_code(d);
            let r = r_code(d);
            let g = g_code(d);
            for i in 0..7 {
                assert_eq!(l[i], !r[i], "L/R complement, digit {d}");
                assert_eq!(g[i], r[6 - i], "G is R reversed, digit {d}");
            }
        }
    }
}
