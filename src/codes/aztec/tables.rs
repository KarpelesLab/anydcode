//! Static Aztec tables (ISO/IEC 24778): the five high-level character sets, the
//! per-layer codeword size, and the data capacity of each symbol size.
//!
//! The five modes are Upper, Lower, Mixed, Punct (all 5-bit) and Digit (4-bit). Each
//! table maps a *code* to the byte(s) it produces; codes reserved for latches, shifts
//! and the binary shift are represented as `None` here and interpreted in
//! [`super::highlevel`].

/// Upper mode.
pub const UPPER: usize = 0;
/// Lower mode.
pub const LOWER: usize = 1;
/// Mixed mode.
pub const MIXED: usize = 2;
/// Punct mode.
pub const PUNCT: usize = 3;
/// Digit mode.
pub const DIGIT: usize = 4;

/// Number of bits per code in a given mode.
pub fn mode_bits(mode: usize) -> usize {
    if mode == DIGIT { 4 } else { 5 }
}

/// The plain data byte a `(mode, code)` produces, or `None` for control codes and
/// the multi-byte Punct combinations (handled separately by the decoder).
pub fn char_for_code(mode: usize, code: u16) -> Option<u8> {
    let c = code as usize;
    match mode {
        UPPER => match code {
            1 => Some(b' '),
            2..=27 => Some(b'A' + (code as u8 - 2)),
            _ => None,
        },
        LOWER => match code {
            1 => Some(b' '),
            2..=27 => Some(b'a' + (code as u8 - 2)),
            _ => None,
        },
        MIXED => MIXED_TABLE.get(c).copied().flatten(),
        PUNCT => PUNCT_TABLE.get(c).copied().flatten(),
        DIGIT => match code {
            1 => Some(b' '),
            2..=11 => Some(b'0' + (code as u8 - 2)),
            12 => Some(b','),
            13 => Some(b'.'),
            _ => None,
        },
        _ => None,
    }
}

/// The two-byte string produced by the multi-byte Punct codes (indices 2–5), if any.
pub fn punct_pair_for_code(code: u16) -> Option<[u8; 2]> {
    match code {
        2 => Some([b'\r', b'\n']),
        3 => Some([b'.', b' ']),
        4 => Some([b',', b' ']),
        5 => Some([b':', b' ']),
        _ => None,
    }
}

/// Reverse lookup: the plain data code for `byte` in `mode`, if representable.
pub fn code_for_char(mode: usize, byte: u8) -> Option<u16> {
    let n = if mode == DIGIT { 16 } else { 32 };
    (1..n as u16).find(|&code| char_for_code(mode, code) == Some(byte))
}

/// The Mixed table (code → byte); `None` marks control/reserved codes.
const MIXED_TABLE: [Option<u8>; 28] = [
    None,        // 0  P/S
    Some(b' '),  // 1  space
    Some(1),     // 2  ^A
    Some(2),     // 3
    Some(3),     // 4
    Some(4),     // 5
    Some(5),     // 6
    Some(6),     // 7
    Some(7),     // 8
    Some(8),     // 9  BS
    Some(9),     // 10 TAB
    Some(10),    // 11 LF
    Some(11),    // 12 VT
    Some(12),    // 13 FF
    Some(13),    // 14 CR
    Some(27),    // 15 ESC
    Some(28),    // 16 FS
    Some(29),    // 17 GS
    Some(30),    // 18 RS
    Some(31),    // 19 US
    Some(b'@'),  // 20
    Some(b'\\'), // 21
    Some(b'^'),  // 22
    Some(b'_'),  // 23
    Some(b'`'),  // 24
    Some(b'|'),  // 25
    Some(b'~'),  // 26
    Some(127),   // 27 DEL
];

/// The Punct table (code → byte); indices 2–5 are two-byte combinations handled by
/// [`punct_pair_for_code`], and are `None` here.
const PUNCT_TABLE: [Option<u8>; 31] = [
    None,        // 0  FLG / reserved
    Some(b'\r'), // 1
    None,        // 2  "\r\n"
    None,        // 3  ". "
    None,        // 4  ", "
    None,        // 5  ": "
    Some(b'!'),  // 6
    Some(b'"'),  // 7
    Some(b'#'),  // 8
    Some(b'$'),  // 9
    Some(b'%'),  // 10
    Some(b'&'),  // 11
    Some(b'\''), // 12
    Some(b'('),  // 13
    Some(b')'),  // 14
    Some(b'*'),  // 15
    Some(b'+'),  // 16
    Some(b','),  // 17
    Some(b'-'),  // 18
    Some(b'.'),  // 19
    Some(b'/'),  // 20
    Some(b':'),  // 21
    Some(b';'),  // 22
    Some(b'<'),  // 23
    Some(b'='),  // 24
    Some(b'>'),  // 25
    Some(b'?'),  // 26
    Some(b'['),  // 27
    Some(b']'),  // 28
    Some(b'{'),  // 29
    Some(b'}'),  // 30
];

/// The codeword bit-width for a symbol with `layers` layers.
pub fn word_size(layers: usize) -> usize {
    match layers {
        1..=2 => 6,
        3..=8 => 8,
        9..=22 => 10,
        _ => 12,
    }
}

/// The number of data-region bits (module pairs) available in a symbol.
pub fn total_bits_in_layer(layers: usize, compact: bool) -> usize {
    ((if compact { 88 } else { 112 }) + 16 * layers) * layers
}
