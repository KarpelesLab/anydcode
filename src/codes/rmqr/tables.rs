//! Static rMQR specification tables (ISO/IEC 23941): the 32 sizes with their
//! dimensions, remainder bits, character-count field widths, and per-EC-level
//! Reed–Solomon block layout. Transcribed and cross-checked against the reference
//! `rmqrcode` implementation.

use super::{RmqrEcLevel, RmqrSize};
use crate::segment::Mode;

/// One Reed–Solomon block group: `num` blocks of `total` codewords each, `data` of
/// which are data codewords.
#[derive(Debug, Clone, Copy)]
pub struct BlockGroup {
    /// Number of blocks in this group.
    pub num: usize,
    /// Total codewords per block (data + EC).
    pub total: usize,
    /// Data codewords per block.
    pub data: usize,
}

/// Everything the encoder/decoder needs for one rMQR size.
pub struct SizeInfo {
    /// Symbol height in modules.
    pub height: usize,
    /// Symbol width in modules.
    pub width: usize,
    /// Trailing remainder bits after the last codeword.
    pub remainder: usize,
    /// Character-count field widths `[numeric, alphanumeric, byte, kanji]`.
    pub cc: [u8; 4],
    /// RS block layout for EC level M.
    pub blocks_m: &'static [BlockGroup],
    /// RS block layout for EC level H.
    pub blocks_h: &'static [BlockGroup],
}

const fn bg(num: usize, total: usize, data: usize) -> BlockGroup {
    BlockGroup { num, total, data }
}

/// The 32 rMQR sizes, indexed by version indicator (0..=31), matching the
/// ISO/IEC 23941 ordering R7x43, R7x59, …, R17x139.
pub static SIZES: [SizeInfo; 32] = [
    // R7x43
    SizeInfo {
        height: 7,
        width: 43,
        remainder: 0,
        cc: [4, 3, 3, 2],
        blocks_m: &[bg(1, 13, 6)],
        blocks_h: &[bg(1, 13, 3)],
    },
    // R7x59
    SizeInfo {
        height: 7,
        width: 59,
        remainder: 3,
        cc: [5, 5, 4, 3],
        blocks_m: &[bg(1, 21, 12)],
        blocks_h: &[bg(1, 21, 7)],
    },
    // R7x77
    SizeInfo {
        height: 7,
        width: 77,
        remainder: 5,
        cc: [6, 5, 5, 4],
        blocks_m: &[bg(1, 32, 20)],
        blocks_h: &[bg(1, 32, 10)],
    },
    // R7x99
    SizeInfo {
        height: 7,
        width: 99,
        remainder: 6,
        cc: [7, 6, 5, 5],
        blocks_m: &[bg(1, 44, 28)],
        blocks_h: &[bg(1, 44, 14)],
    },
    // R7x139
    SizeInfo {
        height: 7,
        width: 139,
        remainder: 1,
        cc: [7, 6, 6, 5],
        blocks_m: &[bg(1, 68, 44)],
        blocks_h: &[bg(2, 34, 12)],
    },
    // R9x43
    SizeInfo {
        height: 9,
        width: 43,
        remainder: 2,
        cc: [5, 5, 4, 3],
        blocks_m: &[bg(1, 21, 12)],
        blocks_h: &[bg(1, 21, 7)],
    },
    // R9x59
    SizeInfo {
        height: 9,
        width: 59,
        remainder: 3,
        cc: [6, 5, 5, 4],
        blocks_m: &[bg(1, 33, 21)],
        blocks_h: &[bg(1, 33, 11)],
    },
    // R9x77
    SizeInfo {
        height: 9,
        width: 77,
        remainder: 1,
        cc: [7, 6, 5, 5],
        blocks_m: &[bg(1, 49, 31)],
        blocks_h: &[bg(1, 24, 8), bg(1, 25, 9)],
    },
    // R9x99
    SizeInfo {
        height: 9,
        width: 99,
        remainder: 4,
        cc: [7, 6, 6, 5],
        blocks_m: &[bg(1, 66, 42)],
        blocks_h: &[bg(2, 33, 11)],
    },
    // R9x139
    SizeInfo {
        height: 9,
        width: 139,
        remainder: 5,
        cc: [8, 7, 6, 6],
        blocks_m: &[bg(1, 49, 31), bg(1, 50, 32)],
        blocks_h: &[bg(3, 33, 11)],
    },
    // R11x27
    SizeInfo {
        height: 11,
        width: 27,
        remainder: 2,
        cc: [4, 4, 3, 2],
        blocks_m: &[bg(1, 15, 7)],
        blocks_h: &[bg(1, 15, 5)],
    },
    // R11x43
    SizeInfo {
        height: 11,
        width: 43,
        remainder: 1,
        cc: [6, 5, 5, 4],
        blocks_m: &[bg(1, 31, 19)],
        blocks_h: &[bg(1, 31, 11)],
    },
    // R11x59
    SizeInfo {
        height: 11,
        width: 59,
        remainder: 0,
        cc: [7, 6, 5, 5],
        blocks_m: &[bg(1, 47, 31)],
        blocks_h: &[bg(1, 23, 7), bg(1, 24, 8)],
    },
    // R11x77
    SizeInfo {
        height: 11,
        width: 77,
        remainder: 2,
        cc: [7, 6, 6, 5],
        blocks_m: &[bg(1, 67, 43)],
        blocks_h: &[bg(1, 33, 11), bg(1, 34, 12)],
    },
    // R11x99
    SizeInfo {
        height: 11,
        width: 99,
        remainder: 7,
        cc: [8, 7, 6, 6],
        blocks_m: &[bg(1, 44, 28), bg(1, 45, 29)],
        blocks_h: &[bg(1, 44, 14), bg(1, 45, 15)],
    },
    // R11x139
    SizeInfo {
        height: 11,
        width: 139,
        remainder: 6,
        cc: [8, 7, 7, 6],
        blocks_m: &[bg(2, 66, 42)],
        blocks_h: &[bg(3, 44, 14)],
    },
    // R13x27 (rmqrcode lists k=14 for M, but ISO/IEC 23941 Table 8 and zxing-cpp
    // give k=12 — matching this size's 96 data-bit capacity).
    SizeInfo {
        height: 13,
        width: 27,
        remainder: 4,
        cc: [5, 5, 4, 3],
        blocks_m: &[bg(1, 21, 12)],
        blocks_h: &[bg(1, 21, 7)],
    },
    // R13x43
    SizeInfo {
        height: 13,
        width: 43,
        remainder: 1,
        cc: [6, 6, 5, 5],
        blocks_m: &[bg(1, 41, 27)],
        blocks_h: &[bg(1, 41, 13)],
    },
    // R13x59
    SizeInfo {
        height: 13,
        width: 59,
        remainder: 6,
        cc: [7, 6, 6, 5],
        blocks_m: &[bg(1, 60, 38)],
        blocks_h: &[bg(2, 30, 10)],
    },
    // R13x77
    SizeInfo {
        height: 13,
        width: 77,
        remainder: 4,
        cc: [7, 7, 6, 6],
        blocks_m: &[bg(1, 42, 26), bg(1, 43, 27)],
        blocks_h: &[bg(1, 42, 14), bg(1, 43, 15)],
    },
    // R13x99
    SizeInfo {
        height: 13,
        width: 99,
        remainder: 3,
        cc: [8, 7, 7, 6],
        blocks_m: &[bg(1, 56, 36), bg(1, 57, 37)],
        blocks_h: &[bg(1, 37, 11), bg(2, 38, 12)],
    },
    // R13x139
    SizeInfo {
        height: 13,
        width: 139,
        remainder: 0,
        cc: [8, 8, 7, 7],
        blocks_m: &[bg(2, 55, 35), bg(1, 56, 36)],
        blocks_h: &[bg(2, 41, 13), bg(2, 42, 14)],
    },
    // R15x43
    SizeInfo {
        height: 15,
        width: 43,
        remainder: 1,
        cc: [7, 6, 6, 5],
        blocks_m: &[bg(1, 51, 33)],
        blocks_h: &[bg(1, 25, 7), bg(1, 26, 8)],
    },
    // R15x59
    SizeInfo {
        height: 15,
        width: 59,
        remainder: 4,
        cc: [7, 7, 6, 5],
        blocks_m: &[bg(1, 74, 48)],
        blocks_h: &[bg(2, 37, 13)],
    },
    // R15x77
    SizeInfo {
        height: 15,
        width: 77,
        remainder: 6,
        cc: [8, 7, 7, 6],
        blocks_m: &[bg(1, 51, 33), bg(1, 52, 34)],
        blocks_h: &[bg(2, 34, 10), bg(1, 35, 11)],
    },
    // R15x99
    SizeInfo {
        height: 15,
        width: 99,
        remainder: 7,
        cc: [8, 7, 7, 6],
        blocks_m: &[bg(2, 68, 44)],
        blocks_h: &[bg(4, 34, 12)],
    },
    // R15x139
    SizeInfo {
        height: 15,
        width: 139,
        remainder: 2,
        cc: [9, 8, 7, 7],
        blocks_m: &[bg(2, 66, 42), bg(1, 67, 43)],
        blocks_h: &[bg(1, 39, 13), bg(4, 40, 14)],
    },
    // R17x43 (rmqrcode lists c=60 for M; ISO/IEC 23941 and zxing-cpp give 22 EC
    // codewords → c=61, matching the R17x43 codeword total of 61).
    SizeInfo {
        height: 17,
        width: 43,
        remainder: 1,
        cc: [7, 6, 6, 5],
        blocks_m: &[bg(1, 61, 39)],
        blocks_h: &[bg(1, 30, 10), bg(1, 31, 11)],
    },
    // R17x59
    SizeInfo {
        height: 17,
        width: 59,
        remainder: 2,
        cc: [8, 7, 6, 6],
        blocks_m: &[bg(2, 44, 28)],
        blocks_h: &[bg(2, 44, 14)],
    },
    // R17x77
    SizeInfo {
        height: 17,
        width: 77,
        remainder: 0,
        cc: [8, 7, 7, 6],
        blocks_m: &[bg(2, 61, 39)],
        blocks_h: &[bg(1, 40, 12), bg(2, 41, 13)],
    },
    // R17x99
    SizeInfo {
        height: 17,
        width: 99,
        remainder: 3,
        cc: [8, 8, 7, 6],
        blocks_m: &[bg(2, 53, 33), bg(1, 54, 34)],
        blocks_h: &[bg(4, 40, 14)],
    },
    // R17x139
    SizeInfo {
        height: 17,
        width: 139,
        remainder: 4,
        cc: [9, 8, 8, 7],
        blocks_m: &[bg(4, 58, 38)],
        blocks_h: &[bg(2, 38, 12), bg(4, 39, 13)],
    },
];

impl SizeInfo {
    /// The RS block layout for an EC level.
    pub fn blocks(&self, ec: RmqrEcLevel) -> &'static [BlockGroup] {
        match ec {
            RmqrEcLevel::M => self.blocks_m,
            RmqrEcLevel::H => self.blocks_h,
        }
    }

    /// Total codewords (data + EC) across all blocks.
    pub fn total_codewords(&self, ec: RmqrEcLevel) -> usize {
        self.blocks(ec).iter().map(|b| b.num * b.total).sum()
    }

    /// Total data codewords across all blocks.
    pub fn total_data_codewords(&self, ec: RmqrEcLevel) -> usize {
        self.blocks(ec).iter().map(|b| b.num * b.data).sum()
    }

    /// Usable data-bit capacity (content + terminator + padding), i.e. data
    /// codewords × 8.
    pub fn data_bit_capacity(&self, ec: RmqrEcLevel) -> usize {
        self.total_data_codewords(ec) * 8
    }
}

/// Look up a size's parameters.
pub fn info(size: RmqrSize) -> &'static SizeInfo {
    &SIZES[size.indicator() as usize]
}

/// Character-count indicator width for a mode at a size, or `None` for ECI.
pub fn char_count_bits(size: RmqrSize, mode: &Mode) -> Option<usize> {
    let cc = info(size).cc;
    Some(match mode {
        Mode::Numeric => cc[0] as usize,
        Mode::Alphanumeric => cc[1] as usize,
        Mode::Byte => cc[2] as usize,
        Mode::Kanji => cc[3] as usize,
        Mode::Eci(_) => return None,
    })
}

/// The 3-bit rMQR mode indicator value for a data mode.
pub fn mode_value(mode: &Mode) -> Option<u8> {
    Some(match mode {
        Mode::Numeric => 0b001,
        Mode::Alphanumeric => 0b010,
        Mode::Byte => 0b011,
        Mode::Kanji => 0b100,
        Mode::Eci(_) => return None,
    })
}

/// Recover a data mode from a 3-bit rMQR mode indicator value.
pub fn mode_from_value(v: u8) -> Option<Mode> {
    Some(match v {
        0b001 => Mode::Numeric,
        0b010 => Mode::Alphanumeric,
        0b011 => Mode::Byte,
        0b100 => Mode::Kanji,
        _ => return None,
    })
}

/// Alignment-pattern center columns for a symbol width.
pub fn alignment_columns(width: usize) -> &'static [usize] {
    match width {
        27 => &[],
        43 => &[21],
        59 => &[19, 39],
        77 => &[25, 51],
        99 => &[23, 49, 75],
        139 => &[27, 55, 83, 111],
        _ => &[],
    }
}

/// The 18-bit rMQR format code: 6 data bits (5-bit version indicator plus the H
/// flag in bit 5) followed by a 12-bit BCH remainder (generator `0x1F25`). The
/// per-copy XOR masks are applied at placement time.
pub fn format_bits(size: RmqrSize, ec_h: bool) -> u32 {
    let data = size.indicator() as u32 | if ec_h { 1 << 5 } else { 0 };
    let mut rem = data;
    for _ in 0..12 {
        rem = (rem << 1) ^ (((rem >> 11) & 1) * 0x1F25);
    }
    (data << 12) | (rem & 0xFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacities_are_consistent() {
        // number_of_data_bits from the ISO tables must equal data codewords × 8.
        let data_bits_m: [usize; 32] = [
            48, 96, 160, 224, 352, 96, 168, 248, 336, 504, 56, 152, 248, 344, 456, 672, 96, 216,
            304, 424, 584, 848, 264, 384, 536, 704, 1016, 312, 448, 624, 800, 1216,
        ];
        let data_bits_h: [usize; 32] = [
            24, 56, 80, 112, 192, 56, 88, 136, 176, 264, 40, 88, 120, 184, 232, 336, 56, 104, 160,
            232, 280, 432, 120, 208, 248, 384, 552, 168, 224, 304, 448, 608,
        ];
        for (i, s) in SIZES.iter().enumerate() {
            let size = RmqrSize::from_indicator(i as u8).unwrap();
            assert_eq!(
                s.data_bit_capacity(RmqrEcLevel::M),
                data_bits_m[i],
                "M vi={i}"
            );
            assert_eq!(
                s.data_bit_capacity(RmqrEcLevel::H),
                data_bits_h[i],
                "H vi={i}"
            );
            // The data region must exactly hold all codeword bits plus remainder.
            let modules = s.total_codewords(RmqrEcLevel::M) * 8 + s.remainder;
            let modules_h = s.total_codewords(RmqrEcLevel::H) * 8 + s.remainder;
            assert_eq!(modules, modules_h, "codeword bits differ by level vi={i}");
            let _ = size;
        }
    }
}
