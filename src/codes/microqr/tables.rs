//! Static Micro QR specification tables (ISO/IEC 18004): codeword/EC counts,
//! mode and character-count field widths, terminator lengths, and the 15-bit
//! format-information BCH code with the Micro QR mask.

use super::{MicroEcLevel, MicroVersion};
use crate::segment::Mode;

/// Data/EC codeword split for one (version, EC level).
#[derive(Debug, Clone, Copy)]
pub struct MicroParams {
    /// Number of data codewords (the final one is 4 bits for M1/M3).
    pub data_cw: usize,
    /// Number of error-correction codewords.
    pub ec_cw: usize,
}

/// The codeword layout for a Micro QR (version, EC level), or `None` for an
/// unsupported combination.
pub fn ec_params(version: MicroVersion, ec: MicroEcLevel) -> Option<MicroParams> {
    use MicroEcLevel::*;
    use MicroVersion::*;
    let (data_cw, ec_cw) = match (version, ec) {
        (M1, Detection) => (3, 2),
        (M2, L) => (5, 5),
        (M2, M) => (4, 6),
        (M3, L) => (11, 6),
        (M3, M) => (9, 8),
        (M4, L) => (16, 8),
        (M4, M) => (14, 10),
        (M4, Q) => (10, 14),
        _ => return None,
    };
    Some(MicroParams { data_cw, ec_cw })
}

/// Whether the final data codeword of this version is only 4 bits (M1 and M3).
pub fn has_short_final_codeword(version: MicroVersion) -> bool {
    matches!(version, MicroVersion::M1 | MicroVersion::M3)
}

/// Number of usable data bits (content + terminator + padding) for a version/level.
pub fn data_bit_capacity(version: MicroVersion, ec: MicroEcLevel) -> Option<usize> {
    let p = ec_params(version, ec)?;
    Some(if has_short_final_codeword(version) {
        (p.data_cw - 1) * 8 + 4
    } else {
        p.data_cw * 8
    })
}

/// Total number of data-region modules (data + EC bits) for a version.
pub fn data_module_count(version: MicroVersion) -> usize {
    match version {
        MicroVersion::M1 => 36,
        MicroVersion::M2 => 80,
        MicroVersion::M3 => 132,
        MicroVersion::M4 => 192,
    }
}

/// The 3-bit "symbol number" S encoding version+EC in the format information.
pub fn symbol_number(version: MicroVersion, ec: MicroEcLevel) -> Option<u8> {
    use MicroEcLevel::*;
    use MicroVersion::*;
    Some(match (version, ec) {
        (M1, Detection) => 0,
        (M2, L) => 1,
        (M2, M) => 2,
        (M3, L) => 3,
        (M3, M) => 4,
        (M4, L) => 5,
        (M4, M) => 6,
        (M4, Q) => 7,
        _ => return None,
    })
}

/// Inverse of [`symbol_number`].
pub fn version_ec_from_symbol_number(s: u8) -> Option<(MicroVersion, MicroEcLevel)> {
    use MicroEcLevel::*;
    use MicroVersion::*;
    Some(match s {
        0 => (M1, Detection),
        1 => (M2, L),
        2 => (M2, M),
        3 => (M3, L),
        4 => (M3, M),
        5 => (M4, L),
        6 => (M4, M),
        7 => (M4, Q),
        _ => return None,
    })
}

/// Number of bits in the mode indicator for a version (M1→0 … M4→3).
pub fn mode_indicator_bits(version: MicroVersion) -> usize {
    version.number() as usize - 1
}

/// The Micro QR mode indicator value for a data mode.
pub fn micro_mode_value(mode: &Mode) -> Option<u8> {
    Some(match mode {
        Mode::Numeric => 0,
        Mode::Alphanumeric => 1,
        Mode::Byte => 2,
        Mode::Kanji => 3,
        Mode::Eci(_) => return None,
    })
}

/// Recover a data mode from a Micro QR mode indicator value.
pub fn mode_from_micro_value(v: u8) -> Option<Mode> {
    Some(match v {
        0 => Mode::Numeric,
        1 => Mode::Alphanumeric,
        2 => Mode::Byte,
        3 => Mode::Kanji,
        _ => return None,
    })
}

/// Character-count indicator width for a mode at a version, or `None` if the mode
/// is not permitted at that version.
pub fn char_count_bits(version: MicroVersion, mode: &Mode) -> Option<usize> {
    use MicroVersion::*;
    let v = match version {
        M1 => 0,
        M2 => 1,
        M3 => 2,
        M4 => 3,
    };
    let bits = match mode {
        Mode::Numeric => [3, 4, 5, 6][v],
        Mode::Alphanumeric => [0, 3, 4, 5][v],
        Mode::Byte => [0, 0, 4, 5][v],
        Mode::Kanji => [0, 0, 3, 4][v],
        Mode::Eci(_) => return None,
    };
    if bits == 0 { None } else { Some(bits) }
}

/// The end-of-message terminator length for a version (M1→3 … M4→9).
pub fn terminator_bits(version: MicroVersion) -> usize {
    2 * version.number() as usize + 1
}

/// Compute the 15-bit Micro QR format code for a symbol number and mask.
///
/// The 5 data bits are `(symbol_number << 2) | mask`; a BCH(15,5) code (generator
/// `0x537`) supplies the 10 check bits, and the whole word is XOR-masked with the
/// Micro QR mask `0x4445`.
pub fn format_bits(symbol_number: u8, mask: u8) -> u16 {
    let data = (((symbol_number & 0x7) as u16) << 2) | (mask & 0x3) as u16;
    let mut rem = data;
    for _ in 0..10 {
        rem = (rem << 1) ^ (((rem >> 9) & 1) * 0x537);
    }
    ((data << 10) | (rem & 0x3FF)) ^ 0x4445
}

/// Decode a 15-bit Micro QR format code to `(symbol_number, mask)` by nearest-codeword
/// search over the 32 valid words (Hamming distance ≤ 3).
pub fn decode_format(raw: u16) -> Option<(u8, u8)> {
    let mut best = None;
    let mut best_dist = u32::MAX;
    for s in 0u8..8 {
        for m in 0u8..4 {
            let candidate = format_bits(s, m);
            let dist = (candidate ^ raw).count_ones();
            if dist < best_dist {
                best_dist = dist;
                best = Some((s, m));
            }
        }
    }
    if best_dist <= 3 { best } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The precomputed format codewords for symbol numbers 0..7, masks 0..3
    /// (32 entries), cross-checked against segno's `FORMAT_INFO_MICRO` table.
    #[test]
    fn format_codes_match_reference() {
        const EXPECTED: [u16; 32] = [
            0x4445, 0x4172, 0x4e2b, 0x4b1c, 0x55ae, 0x5099, 0x5fc0, 0x5af7, 0x6793, 0x62a4, 0x6dfd,
            0x68ca, 0x7678, 0x734f, 0x7c16, 0x7921, 0x06de, 0x03e9, 0x0cb0, 0x0987, 0x1735, 0x1202,
            0x1d5b, 0x186c, 0x2508, 0x203f, 0x2f66, 0x2a51, 0x34e3, 0x31d4, 0x3e8d, 0x3bba,
        ];
        for (i, &want) in EXPECTED.iter().enumerate() {
            let s = (i / 4) as u8;
            let m = (i % 4) as u8;
            assert_eq!(format_bits(s, m), want, "S={s} mask={m}");
            assert_eq!(decode_format(want), Some((s, m)));
        }
    }
}
