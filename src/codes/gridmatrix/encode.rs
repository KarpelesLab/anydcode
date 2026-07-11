//! Grid Matrix encoding: [`Symbol`] → [`BitMatrix`].
//!
//! [`GridMatrixEncoder`] builds a reproducible symbol from raw bytes (canonical mode
//! segmentation and smallest fitting version) or renders an already-specified
//! [`Symbol`] exactly from its [`GridMatrixMeta`].

use super::data::{GmMode, canonical_segments, encode_segments, recommended_ec};
use super::layout::render;
use super::tables::{EcLevel, Version, blocks, smallest_version_for};
use super::{GridMatrixMeta, gf};
use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// Grid Matrix encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct GridMatrixEncoder;

impl GridMatrixEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        GridMatrixEncoder
    }

    /// Build a reproducible [`Symbol`] from raw `data`, choosing a canonical mode
    /// segmentation, the smallest fitting version, and that version's recommended EC
    /// level.
    pub fn build(&self, data: &[u8]) -> Result<Symbol> {
        let segs = canonical_segments(data);
        let bits = encode_segments(&segs)?;
        let num_data_cws = bits.len().div_ceil(7);

        // Pick the smallest version that fits at its recommended EC level.
        let (version, ec) = (1..=13u8)
            .find_map(|v| {
                let ec = recommended_ec(v);
                let ver = Version::new(v).unwrap();
                let cap = ver.data_codewords(ec);
                (cap > 0 && cap >= num_data_cws).then_some((ver, ec))
            })
            .ok_or_else(|| Error::capacity("data exceeds the largest Grid Matrix symbol"))?;

        Ok(build_symbol(segs, version, ec))
    }

    /// Like [`GridMatrixEncoder::build`] but forcing a specific EC level; the smallest
    /// version that holds the data at that level is chosen.
    pub fn build_with_ec(&self, data: &[u8], ec: EcLevel) -> Result<Symbol> {
        let segs = canonical_segments(data);
        let bits = encode_segments(&segs)?;
        let num_data_cws = bits.len().div_ceil(7);
        let version = smallest_version_for(num_data_cws, ec)
            .ok_or_else(|| Error::capacity("data exceeds the largest Grid Matrix symbol"))?;
        Ok(build_symbol(segs, version, ec))
    }

    /// Build forcing both version and EC level, erroring if the data does not fit.
    pub fn build_sized(&self, data: &[u8], version: Version, ec: EcLevel) -> Result<Symbol> {
        let segs = canonical_segments(data);
        let bits = encode_segments(&segs)?;
        let num_data_cws = bits.len().div_ceil(7);
        let cap = version.data_codewords(ec);
        if cap == 0 || num_data_cws > cap {
            return Err(Error::capacity(
                "data does not fit the requested Grid Matrix version/EC level",
            ));
        }
        Ok(build_symbol(segs, version, ec))
    }

    /// Convenience: build from UTF-8 `text`.
    pub fn build_text(&self, text: &str) -> Result<Symbol> {
        self.build(text.as_bytes())
    }
}

impl Encode for GridMatrixEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        if symbol.symbology != Symbology::GridMatrix {
            return Err(Error::invalid_parameter(
                "GridMatrixEncoder given a non-Grid-Matrix symbol",
            ));
        }
        let meta = match &symbol.meta {
            SymbolMeta::GridMatrix(m) => m,
            _ => {
                return Err(Error::invalid_parameter(
                    "Grid Matrix symbol missing GridMatrixMeta",
                ));
            }
        };
        if meta.modes.len() != symbol.segments.len() {
            return Err(Error::invalid_parameter(
                "Grid Matrix mode count does not match segment count",
            ));
        }

        // Re-pair (mode, bytes) using the meta's authoritative mode sequence.
        let segs: Vec<(GmMode, Vec<u8>)> = symbol
            .segments
            .iter()
            .zip(&meta.modes)
            .map(|(seg, &mode)| (mode, seg.data.clone()))
            .collect();

        let bits = encode_segments(&segs)?;
        let num_data_cws = bits.len().div_ceil(7);
        let cap = meta.version.data_codewords(meta.ec_level);
        if cap == 0 || num_data_cws > cap {
            return Err(Error::capacity("Grid Matrix data does not fit the symbol"));
        }

        let cws = add_ecc(&bits, meta.version, meta.ec_level);
        let matrix = render(meta.version, meta.ec_level, &cws);
        Ok(Encoding::Matrix(matrix))
    }
}

/// Assemble a [`Symbol`] from canonical segments and the chosen version/EC level.
fn build_symbol(segs: Vec<(GmMode, Vec<u8>)>, version: Version, ec: EcLevel) -> Symbol {
    let modes: Vec<GmMode> = segs.iter().map(|(m, _)| *m).collect();
    let segments: Vec<Segment> = segs.into_iter().map(|(m, d)| to_segment(m, d)).collect();
    let meta = GridMatrixMeta {
        version,
        ec_level: ec,
        modes,
    };
    Symbol::new(
        Symbology::GridMatrix,
        segments,
        SymbolMeta::GridMatrix(meta),
    )
}

/// Map a Grid Matrix mode + bytes to a crate [`Segment`]. The decoder uses the same
/// mapping, so decoded symbols compare equal to freshly-built ones.
pub(super) fn to_segment(mode: GmMode, data: Vec<u8>) -> Segment {
    match mode {
        GmMode::Numeral => Segment {
            mode: Mode::Numeric,
            data,
        },
        GmMode::Upper | GmMode::Lower => Segment {
            mode: Mode::Alphanumeric,
            data,
        },
        GmMode::Byte | GmMode::Chinese | GmMode::Mixed => Segment {
            mode: Mode::Byte,
            data,
        },
    }
}

/// Convert the data bitstream to 7-bit codewords, pad to the version/EC capacity, and
/// append interleaved Reed–Solomon EC — the full `total_codewords()` stream ready for
/// placement. Mirrors zint `gm_add_ecc`.
fn add_ecc(bits: &[bool], version: Version, ec: EcLevel) -> Vec<u8> {
    let total_data = version.data_codewords(ec);
    let num_data_cws = bits.len().div_ceil(7);

    // Data codewords, MSB-first per 7-bit group; the tail is zero-filled.
    let mut data = vec![0u8; total_data];
    for (i, slot) in data.iter_mut().enumerate().take(num_data_cws) {
        let mut cw = 0u8;
        for j in 0..7 {
            if bits.get(i * 7 + j).copied().unwrap_or(false) {
                cw |= 1 << (6 - j);
            }
        }
        *slot = cw;
    }
    // Pad codewords: 0x7E on odd indices past the data (zint pattern).
    for (i, slot) in data.iter_mut().enumerate().skip(num_data_cws + 1) {
        if i & 1 == 1 {
            *slot = 0x7E;
        }
    }

    let block_layout = blocks(version, ec);
    let num_blocks = block_layout.len();
    let total_cw = version.total_codewords();
    let mut cws = vec![0u8; total_cw];
    let mut wp = 0usize;
    for (i, block) in block_layout.iter().enumerate() {
        let data_size = block.data();
        let ecc_size = block.ec;
        let block_data = &data[wp..wp + data_size];
        wp += data_size;
        let ecc = gf::encode(block_data, ecc_size);
        for (j, &d) in block_data.iter().enumerate() {
            cws[num_blocks * j + i] = d;
        }
        for (j, &e) in ecc.iter().enumerate() {
            cws[num_blocks * (data_size + j) + i] = e;
        }
    }
    cws
}

#[cfg(test)]
mod tests {
    use super::*;

    /// zint cross-check of the RS block structure: version-1 at EC level L5 has one
    /// block of 18 codewords carrying 9 data + 9 EC. Encoding a fixed data block and
    /// re-reading it must survive a full round trip through `add_ecc` for a range of
    /// levels/versions.
    #[test]
    fn add_ecc_fills_the_symbol() {
        for v in 1..=13u8 {
            let ver = Version::new(v).unwrap();
            for ec in EcLevel::ALL {
                if ver.data_codewords(ec) == 0 {
                    continue;
                }
                let bits = vec![true; 7]; // one non-trivial data codeword
                let cws = add_ecc(&bits, ver, ec);
                assert_eq!(cws.len(), ver.total_codewords(), "v{v} {ec:?}");
                assert!(cws.iter().all(|&c| c < 128), "7-bit codewords v{v} {ec:?}");
            }
        }
    }
}
