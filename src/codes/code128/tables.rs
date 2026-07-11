//! Code 128 symbol-value table and shared symbol constants.
//!
//! Every symbol value `0..=106` has an 11-module bar/space pattern of six elements
//! (bar, space, bar, space, bar, space). The pattern is stored as the run-width
//! string used throughout the Code 128 literature (e.g. value `0` is `"212222"` =
//! bar 2, space 1, bar 2, space 2, bar 2, space 2). The physical Stop is the value
//! `106` pattern `"233111"` with a two-module terminating bar appended, giving the
//! 13-module [`STOP`] pattern `"2331112"`.
//!
//! Table and worked example verified against the Wikipedia "Code 128" article.

/// The 107 Code 128 symbol patterns, indexed by symbol value `0..=106`.
///
/// Each string is a run of element widths starting with a bar; every entry sums to
/// 11 modules. Value `106` is the Stop *character* (`"233111"`); the physical Stop
/// bar pattern is [`STOP`].
pub const PATTERNS: [&str; 107] = [
    "212222", // 0
    "222122", // 1
    "222221", // 2
    "121223", // 3
    "121322", // 4
    "131222", // 5
    "122213", // 6
    "122312", // 7
    "132212", // 8
    "221213", // 9
    "221312", // 10
    "231212", // 11
    "112232", // 12
    "122132", // 13
    "122231", // 14
    "113222", // 15
    "123122", // 16
    "123221", // 17
    "223211", // 18
    "221132", // 19
    "221231", // 20
    "213212", // 21
    "223112", // 22
    "312131", // 23
    "311222", // 24
    "321122", // 25
    "321221", // 26
    "312212", // 27
    "322112", // 28
    "322211", // 29
    "212123", // 30
    "212321", // 31
    "232121", // 32
    "111323", // 33
    "131123", // 34
    "131321", // 35
    "112313", // 36
    "132113", // 37
    "132311", // 38
    "211313", // 39
    "231113", // 40
    "231311", // 41
    "112133", // 42
    "112331", // 43
    "132131", // 44
    "113123", // 45
    "113321", // 46
    "133121", // 47
    "313121", // 48
    "211331", // 49
    "231131", // 50
    "213113", // 51
    "213311", // 52
    "213131", // 53
    "311123", // 54
    "311321", // 55
    "331121", // 56
    "312113", // 57
    "312311", // 58
    "332111", // 59
    "314111", // 60
    "221411", // 61
    "431111", // 62
    "111224", // 63
    "111422", // 64
    "121124", // 65
    "121421", // 66
    "141122", // 67
    "141221", // 68
    "112214", // 69
    "112412", // 70
    "122114", // 71
    "122411", // 72
    "142112", // 73
    "142211", // 74
    "241211", // 75
    "221114", // 76
    "413111", // 77
    "241112", // 78
    "134111", // 79
    "111242", // 80
    "121142", // 81
    "121241", // 82
    "114212", // 83
    "124112", // 84
    "124211", // 85
    "411212", // 86
    "421112", // 87
    "421211", // 88
    "212141", // 89
    "214121", // 90
    "412121", // 91
    "111143", // 92
    "111341", // 93
    "131141", // 94
    "114113", // 95
    "114311", // 96
    "411113", // 97
    "411311", // 98
    "113141", // 99
    "114131", // 100
    "311141", // 101
    "411131", // 102
    "211412", // 103 Start A
    "211214", // 104 Start B
    "211232", // 105 Start C
    "233111", // 106 Stop character
];

/// The 13-module physical Stop pattern (`106` plus the two-module terminating bar).
pub const STOP: &str = "2331112";

/// Start Code A symbol value.
pub const START_A: u8 = 103;
/// Start Code B symbol value.
pub const START_B: u8 = 104;
/// Start Code C symbol value.
pub const START_C: u8 = 105;
/// Stop symbol value.
pub const STOP_VALUE: u8 = 106;

/// Switch-to-Code-A symbol value (from B or C).
pub const CODE_A: u8 = 101;
/// Switch-to-Code-B symbol value (from A or C).
pub const CODE_B: u8 = 100;
/// Switch-to-Code-C symbol value (from A or B).
pub const CODE_C: u8 = 99;
/// Shift symbol value (shift the next single character to the other of A/B).
pub const SHIFT: u8 = 98;
/// FNC1 symbol value (identical in all three code sets).
pub const FNC1: u8 = 102;

/// The three Code 128 code sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeSet {
    /// Code Set A: ASCII control (0–31) and upper-case/symbols (32–95).
    A,
    /// Code Set B: ASCII 32–127.
    B,
    /// Code Set C: pairs of digits (`00`–`99`).
    C,
}

impl CodeSet {
    /// The start symbol value that begins this code set.
    pub fn start_value(self) -> u8 {
        match self {
            CodeSet::A => START_A,
            CodeSet::B => START_B,
            CodeSet::C => START_C,
        }
    }

    /// The code set begun by a start symbol value, if it is a start value.
    pub fn from_start(value: u8) -> Option<CodeSet> {
        match value {
            START_A => Some(CodeSet::A),
            START_B => Some(CodeSet::B),
            START_C => Some(CodeSet::C),
            _ => None,
        }
    }
}

/// Look up the symbol value whose pattern matches the six element widths, if any.
pub fn value_for_widths(widths: &[usize]) -> Option<u8> {
    if widths.len() != 6 {
        return None;
    }
    let mut buf = [0u8; 6];
    for (slot, &w) in buf.iter_mut().zip(widths) {
        if !(1..=4).contains(&w) {
            return None;
        }
        *slot = b'0' + w as u8;
    }
    let s = core::str::from_utf8(&buf).ok()?;
    PATTERNS.iter().position(|p| *p == s).map(|v| v as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_sums_to_11() {
        for (value, pat) in PATTERNS.iter().enumerate() {
            let sum: u32 = pat.bytes().map(|b| (b - b'0') as u32).sum();
            assert_eq!(sum, 11, "value {value} pattern {pat} width sum");
            assert_eq!(pat.len(), 6, "value {value} must have six elements");
        }
    }

    #[test]
    fn stop_pattern_is_13_modules() {
        let sum: u32 = STOP.bytes().map(|b| (b - b'0') as u32).sum();
        assert_eq!(sum, 13);
        assert_eq!(STOP.len(), 7);
    }

    #[test]
    fn patterns_are_unique() {
        for (i, a) in PATTERNS.iter().enumerate() {
            for (j, b) in PATTERNS.iter().enumerate().skip(i + 1) {
                assert_ne!(a, b, "values {i} and {j} collide");
            }
        }
    }
}
