//! Fixed MaxiCode tables: the module-placement grid, the code-set character tables
//! and the fixed orientation modules.
//!
//! # Module grid (hex-to-cell convention)
//!
//! A MaxiCode is 33 rows of hexagonal modules, 30 per row, with alternate rows
//! offset by half a module and a circular "bullseye" finder in the centre. AnyDCode
//! represents the symbol as a plain [`BitMatrix`](crate::output::BitMatrix) of
//! **width 30, height 33**: cell `(x, y)` is the module in row `y`, column `x` of
//! ISO/IEC 16023 Figure 5 (the offset of odd rows is a rendering concern only — the
//! logical grid is a rectangle). This mapping is stable and fully reversible.
//!
//! [`MAXI_GRID`] gives, for every cell, the 1-based *module sequence number* 1..=864
//! from ISO/IEC 16023 Figure 5 (value `0` = a finder/orientation cell that carries no
//! data). Data module `n` holds bit `5 - (n-1) % 6` (most-significant first) of
//! codeword `(n-1) / 6`. This is byte-for-byte the same grid used by zint's
//! `maxiGrid` (which stores `n` directly) and ZXing's `BITNR` (which stores `n-1`);
//! the two libraries cross-validate the table used here.
//!
//! The bullseye finder is not part of the data modules and is left light in this
//! representation (the decoder ignores all `0` cells); the encoder additionally sets
//! the fixed [`ORIENTATION`] modules, matching a real symbol's module bitmap.

use std::sync::OnceLock;

/// Grid width in modules.
pub const WIDTH: usize = 30;
/// Grid height in modules.
pub const HEIGHT: usize = 33;

/// ISO/IEC 16023 Figure 5 module sequence: `MAXI_GRID[row][col]` is the 1-based data
/// module number (1..=864), or `0` for a finder/orientation cell.
#[rustfmt::skip]
pub const MAXI_GRID: [[u16; WIDTH]; HEIGHT] = [
    [122,121,128,127,134,133,140,139,146,145,152,151,158,157,164,163,170,169,176,175,182,181,188,187,194,193,200,199,  0,  0],
    [124,123,130,129,136,135,142,141,148,147,154,153,160,159,166,165,172,171,178,177,184,183,190,189,196,195,202,201,817,  0],
    [126,125,132,131,138,137,144,143,150,149,156,155,162,161,168,167,174,173,180,179,186,185,192,191,198,197,204,203,819,818],
    [284,283,278,277,272,271,266,265,260,259,254,253,248,247,242,241,236,235,230,229,224,223,218,217,212,211,206,205,820,  0],
    [286,285,280,279,274,273,268,267,262,261,256,255,250,249,244,243,238,237,232,231,226,225,220,219,214,213,208,207,822,821],
    [288,287,282,281,276,275,270,269,264,263,258,257,252,251,246,245,240,239,234,233,228,227,222,221,216,215,210,209,823,  0],
    [290,289,296,295,302,301,308,307,314,313,320,319,326,325,332,331,338,337,344,343,350,349,356,355,362,361,368,367,825,824],
    [292,291,298,297,304,303,310,309,316,315,322,321,328,327,334,333,340,339,346,345,352,351,358,357,364,363,370,369,826,  0],
    [294,293,300,299,306,305,312,311,318,317,324,323,330,329,336,335,342,341,348,347,354,353,360,359,366,365,372,371,828,827],
    [410,409,404,403,398,397,392,391, 80, 79,  0,  0, 14, 13, 38, 37,  3,  0, 45, 44,110,109,386,385,380,379,374,373,829,  0],
    [412,411,406,405,400,399,394,393, 82, 81, 41,  0, 16, 15, 40, 39,  4,  0,  0, 46,112,111,388,387,382,381,376,375,831,830],
    [414,413,408,407,402,401,396,395, 84, 83, 42,  0,  0,  0,  0,  0,  6,  5, 48, 47,114,113,390,389,384,383,378,377,832,  0],
    [416,415,422,421,428,427,104,103, 56, 55, 17,  0,  0,  0,  0,  0,  0,  0, 21, 20, 86, 85,434,433,440,439,446,445,834,833],
    [418,417,424,423,430,429,106,105, 58, 57,  0,  0,  0,  0,  0,  0,  0,  0, 23, 22, 88, 87,436,435,442,441,448,447,835,  0],
    [420,419,426,425,432,431,108,107, 60, 59,  0,  0,  0,  0,  0,  0,  0,  0,  0, 24, 90, 89,438,437,444,443,450,449,837,836],
    [482,481,476,475,470,469, 49,  0, 31,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  1, 54, 53,464,463,458,457,452,451,838,  0],
    [484,483,478,477,472,471, 50,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,466,465,460,459,454,453,840,839],
    [486,485,480,479,474,473, 52, 51, 32,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  2,  0, 43,468,467,462,461,456,455,841,  0],
    [488,487,494,493,500,499, 98, 97, 62, 61,  0,  0,  0,  0,  0,  0,  0,  0,  0, 27, 92, 91,506,505,512,511,518,517,843,842],
    [490,489,496,495,502,501,100, 99, 64, 63,  0,  0,  0,  0,  0,  0,  0,  0, 29, 28, 94, 93,508,507,514,513,520,519,844,  0],
    [492,491,498,497,504,503,102,101, 66, 65, 18,  0,  0,  0,  0,  0,  0,  0, 19, 30, 96, 95,510,509,516,515,522,521,846,845],
    [560,559,554,553,548,547,542,541, 74, 73, 33,  0,  0,  0,  0,  0,  0, 11, 68, 67,116,115,536,535,530,529,524,523,847,  0],
    [562,561,556,555,550,549,544,543, 76, 75,  0,  0,  8,  7, 36, 35, 12,  0, 70, 69,118,117,538,537,532,531,526,525,849,848],
    [564,563,558,557,552,551,546,545, 78, 77,  0, 34, 10,  9, 26, 25,  0,  0, 72, 71,120,119,540,539,534,533,528,527,850,  0],
    [566,565,572,571,578,577,584,583,590,589,596,595,602,601,608,607,614,613,620,619,626,625,632,631,638,637,644,643,852,851],
    [568,567,574,573,580,579,586,585,592,591,598,597,604,603,610,609,616,615,622,621,628,627,634,633,640,639,646,645,853,  0],
    [570,569,576,575,582,581,588,587,594,593,600,599,606,605,612,611,618,617,624,623,630,629,636,635,642,641,648,647,855,854],
    [728,727,722,721,716,715,710,709,704,703,698,697,692,691,686,685,680,679,674,673,668,667,662,661,656,655,650,649,856,  0],
    [730,729,724,723,718,717,712,711,706,705,700,699,694,693,688,687,682,681,676,675,670,669,664,663,658,657,652,651,858,857],
    [732,731,726,725,720,719,714,713,708,707,702,701,696,695,690,689,684,683,678,677,672,671,666,665,660,659,654,653,859,  0],
    [734,733,740,739,746,745,752,751,758,757,764,763,770,769,776,775,782,781,788,787,794,793,800,799,806,805,812,811,861,860],
    [736,735,742,741,748,747,754,753,760,759,766,765,772,771,778,777,784,783,790,789,796,795,802,801,808,807,814,813,862,  0],
    [738,737,744,743,750,749,756,755,762,761,768,767,774,773,780,779,786,785,792,791,798,797,804,803,810,809,816,815,864,863],
];

/// Fixed dark orientation modules `(row, col)` added on top of the data grid,
/// matching ISO/IEC 16023 (and zint's `z_set_module` orientation markings).
pub const ORIENTATION: [(usize, usize); 13] = [
    (0, 28),
    (0, 29),
    (9, 10),
    (9, 11),
    (10, 11),
    (15, 7),
    (16, 8),
    (16, 20),
    (17, 20),
    (22, 10),
    (23, 10),
    (22, 17),
    (23, 17),
];

/// Map a module sequence number to its `(codeword index, bit)` target, or `None` for
/// a finder/orientation cell (`v == 0`). Bit is most-significant-first within the
/// 6-bit codeword (bit 5 = value 32).
pub fn cell_target(v: u16) -> Option<(usize, u8)> {
    if v == 0 {
        None
    } else {
        let n = (v - 1) as usize;
        Some((n / 6, 5 - (n % 6) as u8))
    }
}

/// One entry of a MaxiCode code set: either a decoded byte or a control operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cw {
    /// A data byte (0..=255) in the active ECI's character set.
    Byte(u8),
    /// Latch to Code Set A.
    LatchA,
    /// Latch to Code Set B.
    LatchB,
    /// Shift the next character to Code Set A.
    ShiftA,
    /// Shift the next character to Code Set B.
    ShiftB,
    /// Shift the next character to Code Set C.
    ShiftC,
    /// Shift the next character to Code Set D.
    ShiftD,
    /// Shift the next character to Code Set E.
    ShiftE,
    /// Shift the next two characters to Code Set A.
    TwoShiftA,
    /// Shift the next three characters to Code Set A.
    ThreeShiftA,
    /// Lock the current (shifted) code set — used to latch into Code Sets C/D/E.
    Lock,
    /// Extended Channel Interpretation prefix.
    Eci,
    /// Numeric-Set prefix: the next five codewords form a 9-digit value.
    Ns,
    /// Padding / no-op.
    Pad,
}

// ZXing-format code-set source strings, transcribed verbatim from the reference
// `DecodedBitStreamParser.SETS`. Private-use code points U+FFF0..U+FFFC mark the
// control operations; all other characters are Latin-1 data bytes. Each string is
// exactly 64 characters.
const SRC_A: &str = "\rABCDEFGHIJKLMNOPQRSTUVWXYZ\u{FFFA}\u{1C}\u{1D}\u{1E}\u{FFFB} \u{FFFC}\"#$%&'()*+,-./0123456789:\u{FFF1}\u{FFF2}\u{FFF3}\u{FFF4}\u{FFF8}";
const SRC_B: &str = "`abcdefghijklmnopqrstuvwxyz\u{FFFA}\u{1C}\u{1D}\u{1E}\u{FFFB}{\u{FFFC}}~\u{7F};<=>?[\\]^_ ,./:@!|\u{FFFC}\u{FFF5}\u{FFF6}\u{FFFC}\u{FFF0}\u{FFF2}\u{FFF3}\u{FFF4}\u{FFF7}";
const SRC_C: &str = "\u{C0}\u{C1}\u{C2}\u{C3}\u{C4}\u{C5}\u{C6}\u{C7}\u{C8}\u{C9}\u{CA}\u{CB}\u{CC}\u{CD}\u{CE}\u{CF}\u{D0}\u{D1}\u{D2}\u{D3}\u{D4}\u{D5}\u{D6}\u{D7}\u{D8}\u{D9}\u{DA}\u{FFFA}\u{1C}\u{1D}\u{1E}\u{FFFB}\u{DB}\u{DC}\u{DD}\u{DE}\u{DF}\u{AA}\u{AC}\u{B1}\u{B2}\u{B3}\u{B5}\u{B9}\u{BA}\u{BC}\u{BD}\u{BE}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{FFF7} \u{FFF9}\u{FFF3}\u{FFF4}\u{FFF8}";
const SRC_D: &str = "\u{E0}\u{E1}\u{E2}\u{E3}\u{E4}\u{E5}\u{E6}\u{E7}\u{E8}\u{E9}\u{EA}\u{EB}\u{EC}\u{ED}\u{EE}\u{EF}\u{F0}\u{F1}\u{F2}\u{F3}\u{F4}\u{F5}\u{F6}\u{F7}\u{F8}\u{F9}\u{FA}\u{FFFA}\u{1C}\u{1D}\u{1E}\u{FFFB}\u{FB}\u{FC}\u{FD}\u{FE}\u{FF}\u{A1}\u{A8}\u{AB}\u{AF}\u{B0}\u{B4}\u{B7}\u{B8}\u{BB}\u{BF}\u{8A}\u{8B}\u{8C}\u{8D}\u{8E}\u{8F}\u{90}\u{91}\u{92}\u{93}\u{94}\u{FFF7} \u{FFF2}\u{FFF9}\u{FFF4}\u{FFF8}";
const SRC_E: &str = "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\n\u{B}\u{C}\r\u{E}\u{F}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1A}\u{FFFA}\u{FFFC}\u{FFFC}\u{1B}\u{FFFB}\u{1C}\u{1D}\u{1E}\u{1F}\u{9F}\u{A0}\u{A2}\u{A3}\u{A4}\u{A5}\u{A6}\u{A7}\u{A9}\u{AD}\u{AE}\u{B6}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9A}\u{9B}\u{9C}\u{9D}\u{9E}\u{FFF7} \u{FFF2}\u{FFF3}\u{FFF9}\u{FFF8}";

fn parse_set(src: &str) -> [Cw; 64] {
    let mut out = [Cw::Pad; 64];
    let mut n = 0;
    for ch in src.chars() {
        let cp = ch as u32;
        out[n] = match cp {
            0xFFF0 => Cw::ShiftA,
            0xFFF1 => Cw::ShiftB,
            0xFFF2 => Cw::ShiftC,
            0xFFF3 => Cw::ShiftD,
            0xFFF4 => Cw::ShiftE,
            0xFFF5 => Cw::TwoShiftA,
            0xFFF6 => Cw::ThreeShiftA,
            0xFFF7 => Cw::LatchA,
            0xFFF8 => Cw::LatchB,
            0xFFF9 => Cw::Lock,
            0xFFFA => Cw::Eci,
            0xFFFB => Cw::Ns,
            0xFFFC => Cw::Pad,
            b if b <= 0xFF => Cw::Byte(b as u8),
            other => panic!("invalid MaxiCode set character U+{other:04X}"),
        };
        n += 1;
    }
    assert_eq!(n, 64, "MaxiCode code set must have 64 entries");
    out
}

/// The five MaxiCode code sets (A..=E), each a 64-entry table indexed by codeword
/// value.
pub fn sets() -> &'static [[Cw; 64]; 5] {
    static SETS: OnceLock<[[Cw; 64]; 5]> = OnceLock::new();
    SETS.get_or_init(|| {
        [
            parse_set(SRC_A),
            parse_set(SRC_B),
            parse_set(SRC_C),
            parse_set(SRC_D),
            parse_set(SRC_E),
        ]
    })
}

/// The codeword value that encodes `byte` in code set `set` (0=A..4=E), if any.
pub fn value_in_set(set: usize, byte: u8) -> Option<u8> {
    sets()[set]
        .iter()
        .position(|&c| c == Cw::Byte(byte))
        .map(|p| p as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn grid_covers_every_data_module_once() {
        let mut seen = HashSet::new();
        for row in &MAXI_GRID {
            for &v in row {
                if v != 0 {
                    assert!(v <= 864, "module {v} out of range");
                    assert!(seen.insert(v), "module {v} appears twice");
                }
            }
        }
        // 144 codewords * 6 bits = 864 data modules.
        assert_eq!(seen.len(), 864);
    }

    #[test]
    fn code_sets_parse_to_64() {
        let s = sets();
        // Spot-check anchor values against the spec / ZXing tables.
        assert_eq!(s[0][1], Cw::Byte(b'A'));
        assert_eq!(s[0][32], Cw::Byte(b' '));
        assert_eq!(s[0][58], Cw::Byte(b':'));
        assert_eq!(s[0][0], Cw::Byte(b'\r'));
        assert_eq!(s[1][0], Cw::Byte(b'`'));
        assert_eq!(s[1][63], Cw::LatchA);
        assert_eq!(s[2][58], Cw::LatchA);
        assert_eq!(s[2][60], Cw::Lock);
        assert_eq!(s[4][0], Cw::Byte(0));
        // Digits live only in set A.
        assert_eq!(value_in_set(0, b'7'), Some(55));
        assert_eq!(value_in_set(1, b'7'), None);
    }
}
