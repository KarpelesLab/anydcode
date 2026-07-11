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

// ======== DataBar Expanded tables (ISO/IEC 24724:2011 Tables 8, 14, 15, 16) ========

/// Group cumulative sums `Gsum` for the 12-bit symbol-character value (Table 8).
pub const EXP_G_SUM: [i32; 5] = [0, 348, 1388, 2948, 3988];

/// Per-group even-element divisor `Teven` (Table 8).
pub const EXP_T_EVEN: [i32; 5] = [4, 20, 52, 104, 204];

/// Odd-element module totals per group; even uses `17 - odd` (Table 8).
pub const EXP_MODULES: [i32; 5] = [12, 10, 8, 6, 4];

/// Widest permitted odd element per group; even uses `9 - widest` (Table 8).
pub const EXP_WIDEST: [i32; 5] = [7, 5, 4, 3, 1];

/// Checksum weight rows (Table 14): 23 rows of 8 element weights.
#[rustfmt::skip]
pub const EXP_CHECK_WEIGHT: [[i32; 8]; 23] = [
    [  1,   3,   9,  27,  81,  32,  96,  77],
    [ 20,  60, 180, 118, 143,   7,  21,  63],
    [189, 145,  13,  39, 117, 140, 209, 205],
    [193, 157,  49, 147,  19,  57, 171,  91],
    [ 62, 186, 136, 197, 169,  85,  44, 132],
    [185, 133, 188, 142,   4,  12,  36, 108],
    [113, 128, 173,  97,  80,  29,  87,  50],
    [150,  28,  84,  41, 123, 158,  52, 156],
    [ 46, 138, 203, 187, 139, 206, 196, 166],
    [ 76,  17,  51, 153,  37, 111, 122, 155],
    [ 43, 129, 176, 106, 107, 110, 119, 146],
    [ 16,  48, 144,  10,  30,  90,  59, 177],
    [109, 116, 137, 200, 178, 112, 125, 164],
    [ 70, 210, 208, 202, 184, 130, 179, 115],
    [134, 191, 151,  31,  93,  68, 204, 190],
    [148,  22,  66, 198, 172,  94,  71,   2],
    [  6,  18,  54, 162,  64, 192, 154,  40],
    [120, 149,  25,  75,  14,  42, 126, 167],
    [ 79,  26,  78,  23,  69, 207, 199, 175],
    [103,  98,  83,  38, 114, 131, 182, 124],
    [161,  61, 183, 127, 170,  88,  53, 159],
    [ 55, 165,  73,   8,  24,  72,   5,  15],
    [ 45, 135, 194, 160,  58, 174, 100,  89],
];

/// Finder patterns (Table 15): 12 patterns of 5 element widths each. Even indices
/// are left-to-right forms; odd indices are their right-to-left reversals.
#[rustfmt::skip]
pub const EXP_FINDER: [[u16; 5]; 12] = [
    [1, 8, 4, 1, 1],
    [1, 1, 4, 8, 1],
    [3, 6, 4, 1, 1],
    [1, 1, 4, 6, 3],
    [3, 4, 6, 1, 1],
    [1, 1, 6, 4, 3],
    [3, 2, 8, 1, 1],
    [1, 1, 8, 2, 3],
    [2, 6, 5, 1, 1],
    [1, 1, 5, 6, 2],
    [2, 2, 9, 1, 1],
    [1, 1, 9, 2, 2],
];

/// Finder sequence (Table 16): for a symbol with `p = (symbol_chars - 1) / 2 - 1`,
/// row `p` gives the 1-based finder-pattern indices in symbol-character-pair order.
/// Only the 10 rows a non-stacked symbol can use (`p` in `0..=9`) are tabulated.
#[rustfmt::skip]
pub const EXP_FINDER_SEQUENCE: [[u8; 11]; 10] = [
    [1,  2, 0, 0, 0,  0,  0,  0,  0,  0,  0],
    [1,  4, 3, 0, 0,  0,  0,  0,  0,  0,  0],
    [1,  6, 3, 8, 0,  0,  0,  0,  0,  0,  0],
    [1, 10, 3, 8, 5,  0,  0,  0,  0,  0,  0],
    [1, 10, 3, 8, 7, 12,  0,  0,  0,  0,  0],
    [1, 10, 3, 8, 9, 12, 11,  0,  0,  0,  0],
    [1,  2, 3, 4, 5,  6,  7,  8,  0,  0,  0],
    [1,  2, 3, 4, 5,  6,  7, 10,  9,  0,  0],
    [1,  2, 3, 4, 5,  6,  7, 10, 11, 12,  0],
    [1,  2, 3, 4, 5,  8,  7, 10,  9, 12, 11],
];

/// Checksum weight-row selector: `EXP_WEIGHT_ROWS[(data_chars - 2) / 2][i]` is the
/// [`EXP_CHECK_WEIGHT`] row applied to data character `i`.
#[rustfmt::skip]
pub const EXP_WEIGHT_ROWS: [[u8; 21]; 10] = [
    [0,  1,  2, 0, 0,  0, 0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0],
    [0,  5,  6, 3, 4,  0, 0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0],
    [0,  9, 10, 3, 4, 13, 14, 0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0],
    [0, 17, 18, 3, 4, 13, 14, 7,  8,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0],
    [0, 17, 18, 3, 4, 13, 14, 11, 12, 21, 22, 0,  0,  0,  0,  0,  0,  0,  0,  0,  0],
    [0, 17, 18, 3, 4, 13, 14, 15, 16, 21, 22, 19, 20, 0,  0,  0,  0,  0,  0,  0,  0],
    [0,  1,  2, 3, 4,  5, 6,  7,  8,  9, 10, 11, 12, 13, 14, 0,  0,  0,  0,  0,  0],
    [0,  1,  2, 3, 4,  5, 6,  7,  8,  9, 10, 11, 12, 17, 18, 15, 16, 0,  0,  0,  0],
    [0,  1,  2, 3, 4,  5, 6,  7,  8,  9, 10, 11, 12, 17, 18, 19, 20, 21, 22, 0,  0],
    [0,  1,  2, 3, 4,  5, 6,  7,  8, 13, 14, 11, 12, 17, 18, 15, 16, 21, 22, 19, 20],
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
