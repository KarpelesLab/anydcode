//! Aztec encoding: [`Symbol`] → [`BitMatrix`].
//!
//! The encoder flattens a symbol's data segments to a byte payload, runs the
//! deterministic high-level encodation, bit-stuffs to the codeword size, adds
//! Reed–Solomon check words, and lays the mode message and spiral data around the
//! bullseye. Size selection prefers the smallest compact symbol, falling back to
//! full-range symbols for larger payloads.

use super::highlevel::{self, BitVec, stuff_bits};
use super::layout::Layout;
use super::tables::{total_bits_in_layer, word_size};
use super::{AztecMeta, QUIET_ZONE};
use crate::codes::aztec::gf::{Gf, rs_encode};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// Aztec Code encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct AztecEncoder;

impl AztecEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        AztecEncoder
    }

    /// Build a reproducible [`Symbol`] from `segments`, choosing the smallest symbol
    /// that fits. The payload is flattened to bytes; the returned symbol carries a
    /// single byte segment plus the [`AztecMeta`] that pins the chosen size.
    pub fn build(&self, segments: Vec<Segment>) -> Result<Symbol> {
        let data = flatten(&segments)?;
        let (compact, layers, _) = choose_size(&data)?;
        let meta = AztecMeta {
            compact,
            layers: layers as u8,
        };
        Ok(Symbol::new(
            Symbology::Aztec,
            vec![Segment::byte(data)],
            SymbolMeta::Aztec(meta),
        ))
    }

    /// Convenience: build a symbol from UTF-8 `text` as a single byte segment.
    pub fn build_text(&self, text: &str) -> Result<Symbol> {
        self.build(vec![Segment::byte(text.as_bytes().to_vec())])
    }
}

impl Encode for AztecEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::Aztec {
            return Err(Error::invalid_parameter(
                "AztecEncoder given a non-Aztec symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::Aztec(m) => m,
            _ => return Err(Error::invalid_parameter("Aztec symbol missing AztecMeta")),
        };
        let data = flatten(&symbol.segments)?;
        let matrix = render(&data, meta.compact, meta.layers as usize)?;
        Ok(Encoding::Matrix(matrix))
    }
}

/// Concatenate the bytes of all data segments (ECI/FLG segments are unsupported).
fn flatten(segments: &[Segment]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for seg in segments {
        match seg.mode {
            Mode::Eci(_) => {
                return Err(Error::Unsupported {
                    what: "Aztec ECI/FLG segments",
                });
            }
            _ => out.extend_from_slice(&seg.data),
        }
    }
    Ok(out)
}

/// Pick the smallest `(compact, layers, word_size)` whose data region fits the
/// payload together with a reasonable amount of error correction (~23% + 11 bits, per
/// the Aztec default), preferring compact symbols.
pub fn choose_size(data: &[u8]) -> Result<(bool, usize, usize)> {
    let hl = highlevel::encode(data);
    let ecc_bits = hl.len() * 23 / 100 + 11;

    let mut candidates: Vec<(bool, usize)> = (1..=4).map(|l| (true, l)).collect();
    candidates.extend((1..=22).map(|l| (false, l)));

    for (compact, layers) in candidates {
        let w = word_size(layers);
        let stuffed = stuff_bits(&hl, w);
        let capacity = total_bits_in_layer(layers, compact);
        let total_words = capacity / w;
        let data_words = stuffed.len() / w;
        // Reed–Solomon needs total_words < field size, and at least one check word.
        let field_size = 1usize << w;
        if total_words >= field_size {
            continue;
        }
        if data_words == 0 || data_words >= total_words {
            continue;
        }
        // Mode message field must hold data_words - 1.
        let max_words = if compact { 1 << 6 } else { 1 << 11 };
        if data_words > max_words {
            continue;
        }
        if stuffed.len() + ecc_bits <= total_words * w {
            return Ok((compact, layers, w));
        }
    }
    Err(Error::capacity(
        "data does not fit in any supported Aztec size",
    ))
}

/// Encode the mode message (layers, data-word count) with its GF(16) BCH/RS check
/// words. Returns 28 bits (compact) or 40 bits (full).
pub fn generate_mode_message(compact: bool, layers: usize, data_words: usize) -> Vec<bool> {
    let mut m = BitVec::default();
    if compact {
        m.push((layers - 1) as u32, 2);
        m.push((data_words - 1) as u32, 6);
    } else {
        m.push((layers - 1) as u32, 5);
        m.push((data_words - 1) as u32, 11);
    }
    let bits = m.into_bits();
    let gf = Gf::gf16();
    let total_words = if compact { 7 } else { 10 };
    let mut words: Vec<u16> = bits
        .chunks(4)
        .map(|c| c.iter().fold(0u16, |a, &b| (a << 1) | b as u16))
        .collect();
    let ec = total_words - words.len();
    let check = rs_encode(&gf, &words, ec);
    words.extend(check);
    words_to_bits(&words, 4)
}

/// Assemble the full data-region bit stream: leading pad, data words, RS check words.
pub fn build_message_bits(stuffed: &[bool], compact: bool, layers: usize, w: usize) -> Vec<bool> {
    let capacity = total_bits_in_layer(layers, compact);
    let total_words = capacity / w;
    let data_words = stuffed.len() / w;
    let ec = total_words - data_words;
    let gf = Gf::for_word_size(w).expect("supported word size");
    let mut words: Vec<u16> = stuffed
        .chunks(w)
        .map(|c| c.iter().fold(0u16, |a, &b| (a << 1) | b as u16))
        .collect();
    let check = rs_encode(&gf, &words, ec);
    words.extend(check);

    let start_pad = capacity % w;
    let mut out = vec![false; start_pad];
    out.extend(words_to_bits(&words, w));
    out
}

/// Serialize `words` to bits, `w` bits each, most-significant first.
fn words_to_bits(words: &[u16], w: usize) -> Vec<bool> {
    let mut out = Vec::with_capacity(words.len() * w);
    for &wd in words {
        for k in (0..w).rev() {
            out.push((wd >> k) & 1 != 0);
        }
    }
    out
}

/// Render the module matrix for `data` at a fixed `(compact, layers)` size.
pub fn render(data: &[u8], compact: bool, layers: usize) -> Result<BitMatrix> {
    let w = word_size(layers);
    let hl = highlevel::encode(data);
    let stuffed = stuff_bits(&hl, w);
    let data_words = stuffed.len() / w;

    let capacity = total_bits_in_layer(layers, compact);
    let total_words = capacity / w;
    if data_words == 0 || data_words >= total_words {
        return Err(Error::capacity(
            "payload does not fit the selected Aztec size",
        ));
    }
    let max_words = if compact { 1 << 6 } else { 1 << 11 };
    if data_words > max_words {
        return Err(Error::capacity("too many data words for the mode message"));
    }

    let layout = Layout::new(compact, layers);
    let mut m = BitMatrix::new(layout.size, layout.size, QUIET_ZONE);
    layout.draw_fixed(&mut m);

    let mode_message = generate_mode_message(compact, layers, data_words);
    for (bit, (x, y)) in mode_message.iter().zip(layout.mode_positions()) {
        if *bit {
            m.set(x, y, true);
        }
    }

    let message = build_message_bits(&stuffed, compact, layers, w);
    for (bit, (x, y)) in message.iter().zip(layout.data_positions()) {
        if *bit {
            m.set(x, y, true);
        }
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_message_roundtrips_via_rs() {
        // Independent check: the mode message is GF(16) RS-protected; decoding the
        // generated 28/40 bits must recover the (layers, data_words) we put in.
        for (compact, layers, words) in [(true, 1, 2), (true, 4, 40), (false, 3, 100)] {
            let bits = generate_mode_message(compact, layers, words);
            assert_eq!(bits.len(), if compact { 28 } else { 40 });
            let (l, dw) = super::super::decode::read_mode_message(&bits, compact).unwrap();
            assert_eq!((l, dw), (layers, words));
        }
    }
}
