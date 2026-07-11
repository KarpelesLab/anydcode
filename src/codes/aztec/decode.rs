//! Aztec decoding: [`BitMatrix`] → [`Symbol`].
//!
//! The structural decoder reads the mode message to learn the data-word count,
//! reverses the spiral placement, Reed–Solomon-corrects the codewords, removes the
//! bit-stuffing and runs the high-level decoder. The recovered payload is returned as
//! a single byte segment together with the [`AztecMeta`] needed to re-encode
//! identically.

use super::highlevel::{self, unstuff_bits};
use super::layout::Layout;
use super::tables::{total_bits_in_layer, word_size};
use super::{AztecMeta, full_size};
use crate::codes::aztec::gf::{Gf, rs_decode};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// Aztec Code structural decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct AztecDecoder;

impl AztecDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        AztecDecoder
    }

    /// Decode a sampled Aztec module grid into a [`Symbol`].
    pub fn decode_matrix(&self, m: &BitMatrix) -> Result<Symbol> {
        let size = m.width();
        if m.height() != size {
            return Err(Error::undecodable("Aztec grid is not square"));
        }
        // An 11×11 grid is an Aztec Rune: a compact finder with no data layers, the
        // ring around the bullseye carrying a single RS-protected byte.
        if size == super::rune::RUNE_SIZE {
            let value = super::rune::decode_rune(m)?;
            return Ok(super::rune::rune_symbol(value));
        }
        let (compact, layers) = detect_size(size, m)?;
        let layout = Layout::new(compact, layers);

        // Mode message → data-word count (and a layer-count cross-check).
        let mode_bits: Vec<bool> = layout
            .mode_positions()
            .iter()
            .map(|&(x, y)| m.get(x, y))
            .collect();
        let (mode_layers, data_words) = read_mode_message(&mode_bits, compact)?;
        if mode_layers != layers {
            return Err(Error::undecodable(
                "Aztec mode message disagrees with symbol size",
            ));
        }

        let w = word_size(layers);
        let capacity = total_bits_in_layer(layers, compact);
        let total_words = capacity / w;
        let start_pad = capacity % w;

        let raw: Vec<bool> = layout
            .data_positions()
            .iter()
            .map(|&(x, y)| m.get(x, y))
            .collect();
        let message = &raw[start_pad..];
        let mut words: Vec<u16> = message
            .chunks(w)
            .map(|c| c.iter().fold(0u16, |a, &b| (a << 1) | b as u16))
            .collect();
        if words.len() != total_words || data_words >= total_words {
            return Err(Error::undecodable("inconsistent Aztec codeword count"));
        }

        let gf = Gf::for_word_size(w).ok_or(Error::Unsupported {
            what: "Aztec symbols above 22 layers",
        })?;
        let ec = total_words - data_words;
        if !rs_decode(&gf, &mut words, ec) {
            return Err(Error::ErrorCorrectionFailed);
        }

        // First data_words words → stuffed bits → high-level bits → payload.
        let mut stuffed = Vec::with_capacity(data_words * w);
        for &wd in &words[..data_words] {
            for k in (0..w).rev() {
                stuffed.push((wd >> k) & 1 != 0);
            }
        }
        let hl = unstuff_bits(&stuffed, w);
        let payload = highlevel::decode(&hl);

        let meta = AztecMeta {
            compact,
            layers: layers as u8,
            rune: false,
        };
        Ok(Symbol::new(
            Symbology::Aztec,
            vec![Segment::byte(payload)],
            SymbolMeta::Aztec(meta),
        ))
    }
}

impl Decode for AztecDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Matrix(m) => self.decode_matrix(m),
            Encoding::Linear(_) => Err(Error::Unsupported {
                what: "Aztec decode of a linear pattern",
            }),
        }
    }
}

/// Determine `(compact, layers)` from the grid size, disambiguating the overlapping
/// sizes (19/23/27) by probing a compact orientation mark on the dist-5 ring, which
/// is a plain light bullseye ring for full symbols.
fn detect_size(size: usize, m: &BitMatrix) -> Result<(bool, usize)> {
    let compact_layers = (size >= 15 && (size - 11).is_multiple_of(4) && (size - 11) / 4 <= 4)
        .then(|| (size - 11) / 4)
        .filter(|&l| l >= 1);
    let full_layers = (1..=22).find(|&l| full_size(l) == size);

    let center = size / 2;
    match (compact_layers, full_layers) {
        (Some(cl), Some(_)) => {
            // Both sizes exist: a dark module at the compact orientation corner means
            // compact; full symbols leave that dist-5 ring light.
            if m.get(center - 5, center - 5) {
                Ok((true, cl))
            } else {
                Ok((false, full_layers.unwrap()))
            }
        }
        (Some(cl), None) => Ok((true, cl)),
        (None, Some(fl)) => Ok((false, fl)),
        (None, None) => Err(Error::undecodable("grid size is not a valid Aztec symbol")),
    }
}

/// Read the RS-protected mode message, returning `(layers, data_words)`.
pub fn read_mode_message(bits: &[bool], compact: bool) -> Result<(usize, usize)> {
    let expected = if compact { 28 } else { 40 };
    if bits.len() != expected {
        return Err(Error::undecodable("wrong Aztec mode-message length"));
    }
    let gf = Gf::gf16();
    let total_words = if compact { 7 } else { 10 };
    let ec = if compact { 5 } else { 6 };
    let mut words: Vec<u16> = bits
        .chunks(4)
        .map(|c| c.iter().fold(0u16, |a, &b| (a << 1) | b as u16))
        .collect();
    debug_assert_eq!(words.len(), total_words);
    if !rs_decode(&gf, &mut words, ec) {
        return Err(Error::ErrorCorrectionFailed);
    }
    let data_words = total_words - ec;
    let mut value: u32 = 0;
    for &wd in &words[..data_words] {
        value = (value << 4) | wd as u32;
    }
    // Data field is data_words * 4 bits wide.
    if compact {
        // 2 bits layers-1, 6 bits words-1.
        let layers = ((value >> 6) & 0b11) as usize + 1;
        let dwords = (value & 0b11_1111) as usize + 1;
        Ok((layers, dwords))
    } else {
        // 5 bits layers-1, 11 bits words-1.
        let layers = ((value >> 11) & 0b1_1111) as usize + 1;
        let dwords = (value & 0b111_1111_1111) as usize + 1;
        Ok((layers, dwords))
    }
}
