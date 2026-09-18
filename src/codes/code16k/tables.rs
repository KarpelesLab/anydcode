//! Code 16K symbol tables.
//!
//! Code 16K (BS EN 12323:2005) is built on the Code 128 character set: every data
//! symbol value `0..=106` renders as the same six-element (bar/space) width pattern
//! Code 128 uses. This table is transcribed from the Code 128 module and cross-checked
//! against the zint reference implementation (`backend/code128.h` `zint_C128Table`).
//!
//! Code 16K adds its own row framing: eight four-element start/stop delimiter patterns
//! (EN 12323 Table 3/4) and two lookup tables (Table 5) that map a row number to the
//! start and stop delimiter used, encoding the row's position in the stack.

use crate::error::{Error, Result};
use crate::segment::Segment;
use alloc::{vec, vec::Vec};

/// The 107 Code 128 symbol patterns, indexed by symbol value `0..=106`.
///
/// Each string is a run of element widths starting with a bar; every entry sums to
/// 11 modules. Identical to the Code 128 table for values `0..=105` (`103`/`104`/`105`
/// are the Start A/B/C patterns, reused by Code 16K for its mode/pad characters).
/// Value `106` — which Code 16K only ever produces as a modulo-107 check character —
/// is `211133`, not Code 128's 13-module Stop.
pub const C128_PATTERNS: [&str; 107] = [
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
    "113141", "114131", "311141", "411131", "211412", "211214", "211232", "211133",
];

/// EN 12323 Table 3/4: the eight start/stop delimiter patterns, four element widths
/// each (summing to 7 modules).
pub const START_STOP: [[u8; 4]; 8] = [
    [3, 2, 1, 1],
    [2, 2, 2, 1],
    [2, 1, 2, 2],
    [1, 4, 1, 1],
    [1, 1, 3, 2],
    [1, 2, 3, 1],
    [1, 1, 1, 4],
    [3, 1, 1, 2],
];

/// EN 12323 Table 5: row number -> index into [`START_STOP`] for the start delimiter.
pub const START_VALUES: [usize; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 0, 1, 2, 3, 4, 5, 6, 7];

/// EN 12323 Table 5: row number -> index into [`START_STOP`] for the stop delimiter.
pub const STOP_VALUES: [usize; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 4, 5, 6, 7, 0, 1, 2, 3];

/// Switch-to-Code-C symbol value.
pub const CODE_C: u8 = 99;
/// Switch-to-Code-B symbol value.
pub const CODE_B: u8 = 100;
/// Switch-to-Code-A symbol value.
pub const CODE_A: u8 = 101;
/// Shift symbol value (shift the next character to the other of A/B).
pub const SHIFT: u8 = 98;
/// FNC1 symbol value.
pub const FNC1: u8 = 102;
/// Pad symbol value.
pub const PAD: u8 = 103;

/// Look up the Code 128 symbol value whose pattern matches the six element widths.
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

/// The two modulo-107 check characters for a Code 16K data-value sequence (mode char +
/// data + pads, without the checks). Ported from EN 12323 / zint.
pub(crate) fn checksum(values: &[u8]) -> (u8, u8) {
    let mut first: u32 = 0;
    let mut second: u32 = 0;
    for (i, &v) in values.iter().enumerate() {
        first += (i as u32 + 2) * v as u32;
        second += (i as u32 + 1) * v as u32;
    }
    let first_check = (first % 107) as u8;
    second += first_check as u32 * (values.len() as u32 + 1);
    let second_check = (second % 107) as u8;
    (first_check, second_check)
}

/// Fixed symbol width in modules: 7 (start) + 1 (guard) + 5*11 (chars) + 7 (stop).
pub(crate) const ROW_WIDTH: usize = 70;

/// Reconstruct the payload [`Segment`]s from a Code 16K data-value sequence (the mode
/// character, data and padding, without the check characters). Shared by the decoder and
/// the encoder's `build` path so both produce identical payload segments.
///
/// Mode values `0..=4` start in set A/B/C (optionally GS1); modes `5`/`6` start in
/// set C with an implied Shift B on the first one / two data characters. Because
/// re-encoding renders from the stored symbol values rather than these segments,
/// segment fidelity never affects the round-trip identity.
pub(crate) fn reconstruct_segments(values: &[u8]) -> Result<Vec<Segment>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }
    let m = values[0] % 7;
    let mut set = match m {
        0 => Set::A,
        2 | 4..=6 => Set::C,
        _ => Set::B,
    };
    // Leading data characters read in set B before the implied set C takes effect.
    let mut implied_b = match m {
        5 => 1,
        6 => 2,
        _ => 0,
    };
    let mut bytes = Vec::new();
    let mut shift: Option<Set> = None;

    for &v in &values[1..] {
        if v == PAD {
            continue;
        }
        let active = if implied_b > 0 {
            implied_b -= 1;
            Set::B
        } else {
            shift.take().unwrap_or(set)
        };
        match active {
            Set::C => match v {
                0..=99 => {
                    bytes.push(b'0' + v / 10);
                    bytes.push(b'0' + v % 10);
                }
                CODE_B => set = Set::B,
                CODE_A => set = Set::A,
                FNC1 => {}
                _ => return Err(Error::undecodable("invalid symbol in Code 16K set C")),
            },
            Set::A => match v {
                0..=95 => bytes.push(if v < 64 { v + 32 } else { v - 64 }),
                96 | 97 => {} // FNC3 / FNC2
                SHIFT => shift = Some(Set::B),
                CODE_C => set = Set::C,
                CODE_B => set = Set::B,
                101 => {} // FNC4 in Set A
                FNC1 => {}
                _ => return Err(Error::undecodable("invalid symbol in Code 16K set A")),
            },
            Set::B => match v {
                0..=95 => bytes.push(v + 32),
                96 | 97 => {} // FNC3 / FNC2
                SHIFT => shift = Some(Set::A),
                CODE_C => set = Set::C,
                100 => {} // FNC4 in Set B
                CODE_A => set = Set::A,
                FNC1 => {}
                _ => return Err(Error::undecodable("invalid symbol in Code 16K set B")),
            },
        }
    }

    Ok(if bytes.is_empty() {
        Vec::new()
    } else {
        vec![Segment::byte(bytes)]
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    A,
    B,
    C,
}
