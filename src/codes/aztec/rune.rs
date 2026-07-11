//! Aztec Runes (ISO/IEC 24778 Annex A): the tiny fixed 11×11 symbol encoding a
//! single value 0–255.
//!
//! A Rune reuses the *compact* Aztec finder — the central bullseye and the four
//! orientation modules — with **zero data layers**, so the symbol is exactly 11×11
//! modules (`11 + 4·layers` with `layers = 0`). The single byte is split into two
//! 4-bit words, protected by the same GF(16) Reed–Solomon code the compact mode
//! message uses (2 data + 5 check = 7 words = 28 bits), and every 4-bit word is XORed
//! with `0b1010` before it is written to the ring of modules around the bullseye. The
//! complement is what distinguishes a Rune from a compact Aztec mode message: a reader
//! first tries to RS-decode the ring as-is (a real Aztec symbol) and, only if that
//! fails, XORs each word with `0b1010` and tries again (a Rune).
//!
//! Cross-checked against zxing-cpp `core/src/aztec/AZDetector.cpp` (`word ^= 0b1010`,
//! `GF2nAztec(4)`, 7 codewords / 2 data codewords) — the reference open-source reader.

use super::layout::Layout;
use super::{AztecMeta, QUIET_ZONE};
use crate::codes::aztec::gf::{Gf, rs_decode, rs_encode};
use crate::error::{Error, Result};
use crate::output::BitMatrix;
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;

/// The physical side length of an Aztec Rune, in modules.
pub(crate) const RUNE_SIZE: usize = 11;
/// Each 4-bit ring word is complemented with this mask (ISO/IEC 24778 Annex A).
const RUNE_MASK: u16 = 0b1010;
/// Number of GF(16) codewords in the ring (2 data + 5 check).
const RUNE_WORDS: usize = 7;
/// Number of GF(16) Reed–Solomon check words in the ring.
const RUNE_EC_WORDS: usize = 5;

/// The 7 complemented ring codewords (most-significant word first) for `value`.
fn rune_codewords(value: u8) -> [u16; RUNE_WORDS] {
    let gf = Gf::gf16();
    let data = [(value >> 4) as u16, (value & 0x0F) as u16];
    let check = rs_encode(&gf, &data, RUNE_EC_WORDS);
    let mut words = [0u16; RUNE_WORDS];
    words[0] = data[0];
    words[1] = data[1];
    for (slot, &c) in words[2..].iter_mut().zip(check.iter()) {
        *slot = c;
    }
    for w in &mut words {
        *w ^= RUNE_MASK;
    }
    words
}

/// Render the 11×11 module matrix for a Rune `value`.
pub(super) fn render_rune(value: u8) -> BitMatrix {
    let layout = Layout::new(true, 0);
    let mut m = BitMatrix::new(layout.size, layout.size, QUIET_ZONE);
    layout.draw_fixed(&mut m);

    // `draw_fixed` uses the shared compact-Aztec orientation marks, but an Aztec Rune's
    // reference pattern (ISO/IEC 24778 Annex A) differs on the outer ring's corner
    // modules. Correct those four cells to the documented Rune pattern (cross-checked
    // against zint's `AztecCompactMap`). None of these cells is a ring data position.
    m.set(1, 0, true);
    m.set(10, 1, true);
    m.set(10, 9, true);
    m.set(0, 10, false);

    // Serialize the 7 words to 28 bits, word[0] first, most-significant bit first,
    // then lay them onto the compact mode-message ring positions.
    let words = rune_codewords(value);
    let mut bits = Vec::with_capacity(RUNE_WORDS * 4);
    for &w in &words {
        for k in (0..4).rev() {
            bits.push((w >> k) & 1 != 0);
        }
    }
    for (bit, (x, y)) in bits.iter().zip(layout.mode_positions()) {
        if *bit {
            m.set(x, y, true);
        }
    }
    m
}

/// Recover the byte value from an 11×11 Rune module grid.
pub(super) fn decode_rune(m: &BitMatrix) -> Result<u8> {
    if m.width() != RUNE_SIZE || m.height() != RUNE_SIZE {
        return Err(Error::undecodable("Aztec Rune grid is not 11×11"));
    }
    let layout = Layout::new(true, 0);
    let bits: Vec<bool> = layout
        .mode_positions()
        .iter()
        .map(|&(x, y)| m.get(x, y))
        .collect();
    // Uncomplement each word, then Reed–Solomon-correct as a compact mode message.
    let mut words: Vec<u16> = bits
        .chunks(4)
        .map(|c| c.iter().fold(0u16, |a, &b| (a << 1) | b as u16) ^ RUNE_MASK)
        .collect();
    debug_assert_eq!(words.len(), RUNE_WORDS);
    let gf = Gf::gf16();
    if !rs_decode(&gf, &mut words, RUNE_EC_WORDS) {
        return Err(Error::ErrorCorrectionFailed);
    }
    Ok((((words[0] & 0x0F) << 4) | (words[1] & 0x0F)) as u8)
}

/// The [`AztecMeta`] that pins a Rune symbol.
pub(super) fn rune_meta() -> AztecMeta {
    AztecMeta {
        compact: true,
        layers: 0,
        rune: true,
    }
}

/// Build the reproducible [`Symbol`] for a Rune `value`.
pub(super) fn rune_symbol(value: u8) -> Symbol {
    Symbol::new(
        Symbology::AztecRunes,
        vec![Segment::byte(vec![value])],
        SymbolMeta::Aztec(rune_meta()),
    )
}
