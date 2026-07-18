//! Payload codec: 16-byte compressed URL ⇄ the on-ring bit vector.
//!
//! Encoding: pick a version by trimming leading zero bytes (≤14 bytes → v0 with 4
//! gap-parity symbols, else v1 with 2), scramble (reverse + XOR `0xA5`), split into
//! the gaps and arcs blocks, RS-encode each plus a GF(16) metadata block carrying the
//! version and a gap-inversion flag, assemble `[meta 16][gaps 104][template 0x2A]`
//! into 128 bits, spatially permute them with the fixed LUT, then append a separator
//! bit, the 56 arc bits and the extra gap bits. The gap bits are inverted when needed
//! so the visible rings always carry more arcs than gaps.

use super::gf::{Gf, rs_decode, rs_encode};
use super::tables::GAPS_BITS_ORDER_LUT;
use crate::error::{Error, Result};

struct FormatParams {
    gaps_data: usize,
    gaps_parity: usize,
    arcs_data: usize,
    arcs_parity: usize,
}

fn format_params(version: usize) -> FormatParams {
    if version == 0 {
        FormatParams {
            gaps_data: 9,
            gaps_parity: 4,
            arcs_data: 5,
            arcs_parity: 2,
        }
    } else {
        FormatParams {
            gaps_data: 11,
            gaps_parity: 2,
            arcs_data: 5,
            arcs_parity: 2,
        }
    }
}

/// The template byte `0x2A`, stored LSB-first.
const TEMPLATE_BITS: [bool; 8] = [false, true, false, true, false, true, false, false];

/// Encode a compressed-URL payload (≤ 16 bytes) into the final ring bit vector.
pub fn encode_payload(payload: &[u8]) -> Result<Vec<bool>> {
    let trimmed: &[u8] = {
        let start = payload
            .iter()
            .position(|&b| b != 0)
            .unwrap_or(payload.len());
        &payload[start..]
    };
    if trimmed.len() > 16 {
        return Err(Error::capacity("App Clip payload exceeds 16 bytes"));
    }
    let version = usize::from(trimmed.len() > 14);
    let fp = format_params(version);
    let total = fp.gaps_data + fp.arcs_data;

    let mut padded = vec![0u8; total];
    padded[total - trimmed.len()..].copy_from_slice(trimmed);

    // Scramble: reverse byte order and XOR 0xA5.
    let scrambled: Vec<u8> = (0..total).map(|i| padded[total - 1 - i] ^ 0xA5).collect();

    let gf256 = Gf::f256();
    let gaps_syms: Vec<usize> = scrambled[..fp.gaps_data]
        .iter()
        .map(|&b| b as usize)
        .collect();
    let mut gaps_bits = symbols_to_bits(&rs_encode(&gf256, &gaps_syms, fp.gaps_parity), 8);

    // Invert so the gaps ring always holds more zero (visible) than one bits.
    let gap_zeros = gaps_bits.iter().filter(|&&b| !b).count();
    let inverted = gap_zeros <= 51;
    if inverted {
        for b in &mut gaps_bits {
            *b = !*b;
        }
    }

    let gf16 = Gf::f16();
    let meta = [version >> 3, usize::from(inverted) | ((version & 7) << 1)];
    let meta_bits = symbols_to_bits(&rs_encode(&gf16, &meta, 2), 4);

    let arcs_syms: Vec<usize> = scrambled[total - fp.arcs_data..]
        .iter()
        .map(|&b| b as usize)
        .collect();
    let arcs_bits = symbols_to_bits(&rs_encode(&gf256, &arcs_syms, fp.arcs_parity), 8);

    // Assemble [meta 16][gaps 104][template 8] and permute through the LUT.
    let mut pre = [false; 128];
    pre[..16].copy_from_slice(&meta_bits);
    pre[16..120].copy_from_slice(&gaps_bits);
    pre[120..].copy_from_slice(&TEMPLATE_BITS);

    let zero_count = pre.iter().filter(|&&b| !b).count();
    let mut out = vec![false; 129 + zero_count];
    for (i, &b) in pre.iter().enumerate() {
        out[GAPS_BITS_ORDER_LUT[i]] = b;
    }

    // Separator (always 0) + arc bits + extra gap bits.
    out[129..129 + 56].copy_from_slice(&arcs_bits);
    let extra = zero_count.saturating_sub(56);
    if extra > 0 && extra <= gaps_bits.len() {
        out[185..185 + extra].copy_from_slice(&gaps_bits[..extra]);
    }
    Ok(out)
}

/// Decode a ring bit vector back to the right-aligned 16-byte payload.
pub fn decode_payload(bits: &[bool]) -> Result<[u8; 16]> {
    if bits.len() < 128 {
        return Err(Error::undecodable("App Clip bit vector shorter than 128"));
    }

    // Reverse the LUT permutation.
    let mut pre = [false; 128];
    for (i, p) in pre.iter_mut().enumerate() {
        *p = bits[GAPS_BITS_ORDER_LUT[i]];
    }

    // Metadata: four GF(16) symbols, RS(4,2).
    let gf16 = Gf::f16();
    let meta_cw: Vec<usize> = (0..4)
        .map(|i| bits_to_symbol(&pre[i * 4..i * 4 + 4]))
        .collect();
    let meta = rs_decode(&gf16, &meta_cw, 2)
        .ok_or_else(|| Error::undecodable("App Clip metadata RS failed"))?;
    let version = (meta[0] << 3) | (meta[1] >> 1);
    let inverted = meta[1] & 1 == 1;
    if version > 1 {
        return Err(Error::undecodable("invalid App Clip codec version"));
    }
    let fp = format_params(version);

    // Gaps: thirteen GF(256) symbols.
    let mut gap_bits: Vec<bool> = pre[16..120].to_vec();
    if inverted {
        for b in &mut gap_bits {
            *b = !*b;
        }
    }
    let gap_cw: Vec<usize> = (0..13)
        .map(|i| bits_to_symbol(&gap_bits[i * 8..i * 8 + 8]))
        .collect();
    let gf256 = Gf::f256();
    let gap_syms = rs_decode(&gf256, &gap_cw, fp.gaps_parity)
        .ok_or_else(|| Error::undecodable("App Clip gaps RS failed"))?;

    // Arcs, when the color stream is present.
    let total = fp.gaps_data + fp.arcs_data;
    let mut scrambled = vec![0u8; total];
    for i in 0..fp.gaps_data {
        scrambled[i] = gap_syms[i] as u8;
    }
    if bits.len() >= 128 + 57 {
        if bits[128] {
            return Err(Error::undecodable("invalid App Clip separator bit"));
        }
        let arc_cw: Vec<usize> = (0..7)
            .map(|i| bits_to_symbol(&bits[129 + i * 8..129 + i * 8 + 8]))
            .collect();
        let arc_syms = rs_decode(&gf256, &arc_cw, fp.arcs_parity)
            .ok_or_else(|| Error::undecodable("App Clip arcs RS failed"))?;
        for i in 0..fp.arcs_data {
            scrambled[fp.gaps_data + i] = arc_syms[i] as u8;
        }
    }

    // Unscramble and right-align into 16 bytes.
    let mut payload = [0u8; 16];
    for i in 0..total {
        payload[16 - total + (total - 1 - i)] = scrambled[i] ^ 0xA5;
    }
    Ok(payload)
}

/// Symbols to bits, MSB-first per symbol.
fn symbols_to_bits(symbols: &[usize], bits_per: usize) -> Vec<bool> {
    let mut out = Vec::with_capacity(symbols.len() * bits_per);
    for &s in symbols {
        for j in (0..bits_per).rev() {
            out.push((s >> j) & 1 == 1);
        }
    }
    out
}

fn bits_to_symbol(bits: &[bool]) -> usize {
    bits.iter()
        .fold(0usize, |acc, &b| (acc << 1) | usize::from(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_roundtrip_both_versions() {
        // ≤14 significant bytes → version 0; 15-16 → version 1.
        let mut short = [0u8; 16];
        short[14] = 0x42;
        short[15] = 0xAA;
        let mut long = [0u8; 16];
        for (i, b) in long.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(17).wrapping_add(1);
        }
        for payload in [short, long] {
            let bits = encode_payload(&payload).unwrap();
            assert!(bits.len() >= 129, "vector too short: {}", bits.len());
            let back = decode_payload(&bits).unwrap();
            assert_eq!(back, payload);
        }
    }

    #[test]
    fn gap_ring_majority_zero() {
        // The inversion step guarantees more zero (arc) than one (gap) bits among the
        // 104 gap bits, whatever the payload.
        for seed in 0u8..16 {
            let payload = [seed.wrapping_mul(37); 16];
            let bits = encode_payload(&payload).unwrap();
            // Count zeros in the whole 128-bit permuted block: ≥ the guarantee on the
            // gaps portion alone minus meta/template variance; the strict per-spec
            // check is on the pre-permutation gaps, so re-derive them.
            let mut pre = [false; 128];
            for (i, p) in pre.iter_mut().enumerate() {
                *p = bits[GAPS_BITS_ORDER_LUT[i]];
            }
            let zeros = pre[16..120].iter().filter(|&&b| !b).count();
            assert!(zeros >= 52, "gap zeros {zeros} < 52 for seed {seed}");
        }
    }
}
