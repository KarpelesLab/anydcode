//! Codablock F symbol tables (the Code 128 character set).
//!
//! Codablock F stacks Code 128 rows, so every symbol value `0..=105` renders as the
//! Code 128 six-element width pattern. This table is transcribed from the Code 128
//! module and cross-checked against zint's `zint_C128Table`. Value `106` is the Stop
//! character; it renders as the 13-module [`STOP`] pattern (with the terminating bar).

/// The 106 renderable Code 128 symbol patterns, indexed by value `0..=105`.
pub const C128_PATTERNS: [&str; 106] = [
    "212222", "222122", "222221", "121223", "121322", "131222", "122213", "122312", "132212",
    "221213", "221312", "231212", "112232", "122132", "122231", "113222", "123122", "123221",
    "223211", "221132", "221231", "213212", "223112", "312131", "311222", "321122", "321221",
    "312212", "322112", "322211", "212123", "212321", "232121", "111323", "131123", "131321",
    "112313", "132113", "132311", "211313", "231113", "231311", "112133", "112331", "132131",
    "113123", "113321", "133121", "313121", "211331", "231131", "213113", "213311", "213131",
    "311123", "311321", "331121", "312113", "312311", "332111", "314111", "221411", "431111",
    "111224", "111422", "121124", "121421", "141122", "141221", "112214", "112412", "122114",
    "122411", "142112", "142211", "241211", "221114", "413111", "241112", "134111", "111242",
    "121142", "121241", "114212", "124112", "124211", "411212", "421112", "421211", "212141",
    "214121", "412121", "111143", "111341", "131141", "114113", "114311", "411113", "411311",
    "113141", "114131", "311141", "411131", "211412", "211214", "211232",
];

/// The 13-module physical Stop pattern (value `106` plus the terminating bar).
pub const STOP: &str = "2331112";

/// Start Code A symbol value (every Codablock F row begins with this).
pub const START_A: u8 = 103;
/// Stop symbol value.
pub const STOP_VALUE: u8 = 106;

/// Look up the Code 128 symbol value whose pattern matches six element widths.
pub fn value_for_widths(widths: &[u8]) -> Option<u8> {
    if widths.len() != 6 {
        return None;
    }
    let mut buf = [0u8; 6];
    for (slot, &w) in buf.iter_mut().zip(widths) {
        if !(1..=6).contains(&w) {
            return None;
        }
        *slot = b'0' + w;
    }
    let s = core::str::from_utf8(&buf).ok()?;
    C128_PATTERNS.iter().position(|p| *p == s).map(|v| v as u8)
}

/// The modulo-103 Code 128 row check over the symbols preceding the check position.
pub(crate) fn row_check(syms: &[u8]) -> u8 {
    let mut sum = syms[0] as u32 % 103;
    for (pos, &v) in syms.iter().enumerate().skip(1) {
        sum = (sum + v as u32 * pos as u32) % 103;
    }
    sum as u8
}

/// The two data check characters K1 and K2 (Codablock F Annex F), over the raw source.
pub(crate) fn k1k2(data: &[u8]) -> (u8, u8) {
    let mut s1: u32 = 0;
    let mut s2: u32 = 0;
    for (i, &b) in data.iter().enumerate() {
        s1 = (s1 + (i as u32 + 1) * b as u32) % 86;
        s2 = (s2 + i as u32 * b as u32) % 86;
    }
    (s1 as u8, s2 as u8)
}

/// Encode a row-indicator / check sum into a Code 128 value in the given set.
/// Sets A and B use the same (Set B) mapping per the Codablock F spec; Set C is direct.
pub(crate) fn sum_to_value(sum: u8, set: Set) -> u8 {
    match set {
        Set::C => sum,
        _ => {
            if sum <= 31 {
                sum + 64
            } else if sum <= 47 {
                sum - 32
            } else {
                sum - 22
            }
        }
    }
}

/// A Code 128 code set (A/B used for data, C only for filler switches).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Set {
    /// Code Set A.
    A,
    /// Code Set B.
    B,
    /// Code Set C.
    C,
}
