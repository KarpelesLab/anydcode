//! Static GS1 DataBar specification tables (ISO/IEC 24724:2011).
//!
//! The numeric tables here are transcribed from Annex A/B of the standard. They
//! match the reference values published in the standard and are cross-checked
//! against two independent implementations (zint's `rss.c`/`rss.h` and BWIPP's
//! `databaromni`/`databarlimited`); see the round-trip and reference-vector tests
//! in `tests/databar.rs`.

// ======== Combinations table (ISO/IEC 24724:2011 Annex B `combins`) ========

/// `combins[n][r]` = C(n, r), the number of ways to choose `r` from `n`.
///
/// Sized `18 x 6`: DataBar Limited uses at most 19 modules (`n <= 17`) and 7
/// elements (`r <= 5`), which bounds every lookup the width algorithms perform.
#[rustfmt::skip]
const COMBINS: [[i64; 6]; 18] = [
    [1,  1,   1,   1,    1,    1],
    [1,  1,   1,   1,    1,    1],
    [1,  2,   1,   1,    1,    1],
    [1,  3,   3,   1,    1,    1],
    [1,  4,   6,   4,    1,    1],
    [1,  5,  10,  10,    5,    1],
    [1,  6,  15,  20,   15,    6],
    [1,  7,  21,  35,   35,   21],
    [1,  8,  28,  56,   70,   56],
    [1,  9,  36,  84,  126,  126],
    [1, 10,  45, 120,  210,  252],
    [1, 11,  55, 165,  330,  462],
    [1, 12,  66, 220,  495,  792],
    [1, 13,  78, 286,  715, 1287],
    [1, 14,  91, 364, 1001, 2002],
    [1, 15, 105, 455, 1365, 3003],
    [1, 16, 120, 560, 1820, 4368],
    [1, 17, 136, 680, 2380, 6188],
];

/// C(n, r) with out-of-range indices returning 0.
///
/// Valid DataBar element generation never leaves the tabulated range; the guard
/// only prevents panics when *decoding* a malformed width sequence, where 0
/// (no combinations) is the mathematically correct degenerate value.
pub fn combins(n: i64, r: i64) -> i64 {
    if n < 0 || r < 0 || n as usize >= COMBINS.len() || r as usize >= 6 {
        return 0;
    }
    COMBINS[n as usize][r as usize]
}

// ======== DataBar Omnidirectional (RSS-14) tables ========

/// Group cumulative sums `Gsum` (Table 1 outside groups 0-4, Table 2 inside 5-8).
pub const OMN_G_SUM: [i32; 9] = [0, 161, 961, 2015, 2715, 0, 336, 1036, 1516];

/// Per-group `Teven`/`Todd` divisor (outside `Teven` 0-4, inside `Todd` 5-8).
pub const OMN_T_EVEN_ODD: [i32; 9] = [1, 10, 34, 70, 126, 4, 20, 48, 81];

/// Odd-element module totals per group, indices 0-8; even totals follow at 9-17
/// (`16 - odd` outside, `15 - odd` inside).
pub const OMN_MODULES: [i32; 18] = [12, 10, 8, 6, 4, 5, 7, 9, 11, 4, 6, 8, 10, 12, 10, 8, 6, 4];

/// Widest permitted odd element per group; even uses `9 - widest`.
pub const OMN_WIDEST: [i32; 9] = [8, 6, 4, 3, 1, 2, 4, 6, 8];

/// Finder patterns (Table 4): 9 patterns of 5 element widths each.
#[rustfmt::skip]
pub const OMN_FINDER: [[u16; 5]; 9] = [
    [3, 8, 2, 1, 1],
    [3, 5, 5, 1, 1],
    [3, 3, 7, 1, 1],
    [3, 1, 9, 1, 1],
    [2, 7, 4, 1, 1],
    [2, 5, 6, 1, 1],
    [2, 3, 8, 1, 1],
    [1, 5, 7, 1, 1],
    [1, 3, 9, 1, 1],
];

/// Checksum weights (Table 5): 4 data characters x 8 elements.
#[rustfmt::skip]
pub const OMN_CHECK_WEIGHT: [[i32; 8]; 4] = [
    [ 1,  3,  9, 27,  2,  6, 18, 54],
    [ 4, 12, 36, 29,  8, 24, 72, 58],
    [16, 48, 65, 37, 32, 17, 51, 74],
    [64, 34, 23, 69, 49, 68, 46, 59],
];

// ======== DataBar Limited tables ========

/// Per-group `Teven` divisor (Table 6).
pub const LTD_T_EVEN: [i32; 7] = [28, 728, 6454, 203, 2408, 1, 16632];

/// Odd-element module totals per group; even uses `26 - odd`.
pub const LTD_MODULES: [i32; 7] = [17, 13, 9, 15, 11, 19, 7];

/// Widest permitted odd element per group; even uses `9 - widest`.
pub const LTD_WIDEST: [i32; 7] = [6, 5, 3, 5, 4, 8, 1];

/// Group cumulative sums for DataBar Limited.
pub const LTD_G_SUM: [i32; 7] = [0, 183064, 820064, 1000776, 1491021, 1979845, 1996939];

/// Checksum weights (Table 7): 2 rows x 14 elements.
#[rustfmt::skip]
pub const LTD_CHECK_WEIGHT: [[i32; 14]; 2] = [
    [ 1,  3,  9, 27, 81, 65, 17, 51, 64, 14, 42, 37, 22, 66],
    [20, 60,  2,  6, 18, 54, 73, 41, 34, 13, 39, 28, 84, 74],
];

/// Finder / checksum patterns (Annex C): 89 patterns of 14 element widths each.
#[rustfmt::skip]
pub const LTD_FINDER: [[u16; 14]; 89] = [
    [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 3, 3, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 3, 2, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 1, 1, 3, 3, 1, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 2, 1, 1, 3, 2, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 2, 1, 2, 3, 1, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 3, 1, 1, 3, 1, 1, 1],
    [1, 1, 1, 1, 1, 2, 1, 1, 1, 1, 3, 2, 1, 1],
    [1, 1, 1, 1, 1, 2, 1, 1, 1, 2, 3, 1, 1, 1],
    [1, 1, 1, 1, 1, 2, 1, 2, 1, 1, 3, 1, 1, 1],
    [1, 1, 1, 1, 1, 3, 1, 1, 1, 1, 3, 1, 1, 1],
    [1, 1, 1, 2, 1, 1, 1, 1, 1, 1, 3, 2, 1, 1],
    [1, 1, 1, 2, 1, 1, 1, 1, 1, 2, 3, 1, 1, 1],
    [1, 1, 1, 2, 1, 1, 1, 2, 1, 1, 3, 1, 1, 1],
    [1, 1, 1, 2, 1, 2, 1, 1, 1, 1, 3, 1, 1, 1],
    [1, 1, 1, 3, 1, 1, 1, 1, 1, 1, 3, 1, 1, 1],
    [1, 2, 1, 1, 1, 1, 1, 1, 1, 1, 3, 2, 1, 1],
    [1, 2, 1, 1, 1, 1, 1, 1, 1, 2, 3, 1, 1, 1],
    [1, 2, 1, 1, 1, 1, 1, 2, 1, 1, 3, 1, 1, 1],
    [1, 2, 1, 1, 1, 2, 1, 1, 1, 1, 3, 1, 1, 1],
    [1, 2, 1, 2, 1, 1, 1, 1, 1, 1, 3, 1, 1, 1],
    [1, 3, 1, 1, 1, 1, 1, 1, 1, 1, 3, 1, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 1, 2, 1, 2, 3, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 1, 2, 3, 2, 1, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 2, 2, 1, 2, 2, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 1, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 3, 2, 1, 2, 1, 1, 1],
    [1, 1, 1, 1, 1, 2, 1, 1, 2, 1, 2, 2, 1, 1],
    [1, 1, 1, 1, 1, 2, 1, 1, 2, 2, 2, 1, 1, 1],
    [1, 1, 1, 1, 1, 2, 1, 2, 2, 1, 2, 1, 1, 1],
    [1, 1, 1, 1, 1, 3, 1, 1, 2, 1, 2, 1, 1, 1],
    [1, 1, 1, 2, 1, 1, 1, 1, 2, 1, 2, 2, 1, 1],
    [1, 1, 1, 2, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1],
    [1, 1, 1, 2, 1, 1, 1, 2, 2, 1, 2, 1, 1, 1],
    [1, 1, 1, 2, 1, 2, 1, 1, 2, 1, 2, 1, 1, 1],
    [1, 1, 1, 3, 1, 1, 1, 1, 2, 1, 2, 1, 1, 1],
    [1, 2, 1, 1, 1, 1, 1, 1, 2, 1, 2, 2, 1, 1],
    [1, 2, 1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1],
    [1, 2, 1, 1, 1, 1, 1, 2, 2, 1, 2, 1, 1, 1],
    [1, 2, 1, 1, 1, 2, 1, 1, 2, 1, 2, 1, 1, 1],
    [1, 2, 1, 2, 1, 1, 1, 1, 2, 1, 2, 1, 1, 1],
    [1, 3, 1, 1, 1, 1, 1, 1, 2, 1, 2, 1, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 1, 3, 1, 1, 3, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 1, 3, 2, 1, 2, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 2, 3, 1, 1, 2, 1, 1],
    [1, 1, 1, 2, 1, 1, 1, 1, 3, 1, 1, 2, 1, 1],
    [1, 2, 1, 1, 1, 1, 1, 1, 3, 1, 1, 2, 1, 1],
    [1, 1, 1, 1, 1, 1, 2, 1, 1, 1, 2, 3, 1, 1],
    [1, 1, 1, 1, 1, 1, 2, 1, 1, 2, 2, 2, 1, 1],
    [1, 1, 1, 1, 1, 1, 2, 1, 1, 3, 2, 1, 1, 1],
    [1, 1, 1, 1, 1, 1, 2, 2, 1, 1, 2, 2, 1, 1],
    [1, 1, 1, 2, 1, 1, 2, 1, 1, 1, 2, 2, 1, 1],
    [1, 1, 1, 2, 1, 1, 2, 1, 1, 2, 2, 1, 1, 1],
    [1, 1, 1, 2, 1, 1, 2, 2, 1, 1, 2, 1, 1, 1],
    [1, 1, 1, 2, 1, 2, 2, 1, 1, 1, 2, 1, 1, 1],
    [1, 1, 1, 3, 1, 1, 2, 1, 1, 1, 2, 1, 1, 1],
    [1, 2, 1, 1, 1, 1, 2, 1, 1, 1, 2, 2, 1, 1],
    [1, 2, 1, 1, 1, 1, 2, 1, 1, 2, 2, 1, 1, 1],
    [1, 2, 1, 2, 1, 1, 2, 1, 1, 1, 2, 1, 1, 1],
    [1, 1, 1, 1, 2, 1, 1, 1, 1, 1, 2, 3, 1, 1],
    [1, 1, 1, 1, 2, 1, 1, 1, 1, 2, 2, 2, 1, 1],
    [1, 1, 1, 1, 2, 1, 1, 1, 1, 3, 2, 1, 1, 1],
    [1, 1, 1, 1, 2, 1, 1, 2, 1, 1, 2, 2, 1, 1],
    [1, 1, 1, 1, 2, 1, 1, 2, 1, 2, 2, 1, 1, 1],
    [1, 1, 1, 1, 2, 2, 1, 1, 1, 1, 2, 2, 1, 1],
    [1, 2, 1, 1, 2, 1, 1, 1, 1, 1, 2, 2, 1, 1],
    [1, 2, 1, 1, 2, 1, 1, 1, 1, 2, 2, 1, 1, 1],
    [1, 2, 1, 1, 2, 1, 1, 2, 1, 1, 2, 1, 1, 1],
    [1, 2, 1, 1, 2, 2, 1, 1, 1, 1, 2, 1, 1, 1],
    [1, 2, 1, 2, 2, 1, 1, 1, 1, 1, 2, 1, 1, 1],
    [1, 3, 1, 1, 2, 1, 1, 1, 1, 1, 2, 1, 1, 1],
    [1, 1, 2, 1, 1, 1, 1, 1, 1, 1, 2, 3, 1, 1],
    [1, 1, 2, 1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1],
    [1, 1, 2, 1, 1, 1, 1, 1, 1, 3, 2, 1, 1, 1],
    [1, 1, 2, 1, 1, 1, 1, 2, 1, 1, 2, 2, 1, 1],
    [1, 1, 2, 1, 1, 1, 1, 2, 1, 2, 2, 1, 1, 1],
    [1, 1, 2, 1, 1, 1, 1, 3, 1, 1, 2, 1, 1, 1],
    [1, 1, 2, 1, 1, 2, 1, 1, 1, 1, 2, 2, 1, 1],
    [1, 1, 2, 1, 1, 2, 1, 1, 1, 2, 2, 1, 1, 1],
    [1, 1, 2, 2, 1, 1, 1, 1, 1, 1, 2, 2, 1, 1],
    [2, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1],
    [2, 1, 1, 1, 1, 1, 1, 1, 1, 3, 2, 1, 1, 1],
    [2, 1, 1, 1, 1, 1, 1, 2, 1, 1, 2, 2, 1, 1],
    [2, 1, 1, 1, 1, 1, 1, 2, 1, 2, 2, 1, 1, 1],
    [2, 1, 1, 1, 1, 1, 1, 3, 1, 1, 2, 1, 1, 1],
    [2, 1, 1, 1, 1, 2, 1, 1, 1, 2, 2, 1, 1, 1],
    [2, 1, 1, 1, 1, 2, 1, 2, 1, 1, 2, 1, 1, 1],
    [2, 1, 1, 2, 1, 1, 1, 1, 1, 2, 2, 1, 1, 1],
    [2, 1, 1, 1, 1, 1, 1, 1, 2, 2, 1, 2, 1, 1],
];

/// GS1 mod-10 check digit for a 13-digit GTIN body, returned as an ASCII digit.
///
/// Positions are weighted 3, 1, 3, 1, ... from the left (equivalently 3 on the
/// rightmost data digit), matching ISO/IEC 7812 / GS1 General Specifications.
pub fn gtin_check_digit(body: &[u8]) -> u8 {
    // `body.len()` is 13 here; the first weight is 3 for an odd length.
    let mut factor = if body.len() % 2 == 1 { 3 } else { 1 };
    let mut sum = 0i32;
    for &c in body {
        sum += factor * (c - b'0') as i32;
        factor ^= 0x02; // toggles between 3 and 1
    }
    b'0' + ((10 - (sum % 10)) % 10) as u8
}
