//! Grid Matrix module layout: the macromodule grid, the alternating frame pattern,
//! codeword placement, and the redundant layer-id metadata.
//!
//! The geometry follows AIMD014 and is implemented to match zint's `gridmtx.c`
//! module-for-module (`gm_place_macromodule`, `gm_macromodule`, `gm_place_layer_id`
//! and the frame-drawing loop), so a rendered symbol is bit-compatible with zint for
//! the same codeword stream.
//!
//! ## Macromodule internal layout (6×6)
//!
//! Every macromodule reserves its interior 4×4 (offset `(1,1)`) for a 2-bit layer-id
//! and 14 data bits (two 7-bit codewords); the 6×6 perimeter is a fixed frame that is
//! dark for macromodules where `(x + y)` is even and light otherwise. The frame is
//! the "grid" that gives the symbology its name and provides the sampling reference.

use super::tables::{EcLevel, Version};
use crate::output::BitMatrix;

/// Quiet-zone width required around a Grid Matrix symbol, in modules.
pub const QUIET_ZONE: usize = 2;

/// Codeword-pair index for the macromodule at grid position `(x, y)`.
///
/// Returns the index of the *first* of the macromodule's two codewords in the
/// interleaved stream (the second is `+1`). Direct port of zint `gm_macromodule`,
/// which numbers macromodules ring-by-ring from the outside in.
pub fn macromodule_index(mpd: usize, x: usize, y: usize) -> usize {
    let m = mpd as isize;
    let x = x as isize;
    let y = y as isize;

    // Ring distance from the nearest edge (0 = outermost).
    let r = if x < y {
        if x < m - y - 1 { x } else { m - y - 1 }
    } else if y < m - x - 1 {
        y
    } else {
        m - x - 1
    };
    let mm = (((m - 1) >> 1) - r) * 2 + 1; // macromodules per side of this ring
    let b = mm * mm - 1; // highest index within this ring

    let idx = if x == r {
        // Left edge
        b - y + r
    } else if y == r {
        // Top edge
        b - (mm - 1) * 4 + (x - r)
    } else if x == m - r - 1 {
        // Right edge
        b - (mm - 1) * 3 + (y - r)
    } else {
        // Bottom edge (y == m - r - 1)
        b - (mm - 1) - (x - r)
    };
    (idx << 1) as usize
}

/// The layer-id values (one per concentric ring, index 0 = centre) for a symbol with
/// `layers` layers at EC level number `ec` (1–5). Port of zint `gm_place_layer_id`.
///
/// The EC level is *encoded* in this ring pattern; decoding recovers it by matching
/// the observed ring ids against every candidate level.
pub fn layer_ids(layers: usize, ec: usize) -> Vec<u8> {
    (0..=layers)
        .map(|i| {
            if ec == 1 {
                (3 - (i as u8 & 0x03)) & 0x03
            } else {
                ((i + 5 - ec) as u8) & 0x03
            }
        })
        .collect()
}

/// Chebyshev ring distance of macromodule `(x, y)` from the centre.
fn ring_of(layers: usize, x: usize, y: usize) -> usize {
    let dx = x.abs_diff(layers);
    let dy = y.abs_diff(layers);
    dx.max(dy)
}

/// Draw the fixed dark frame of the macromodule at grid position `(x, y)`.
fn draw_frame(m: &mut BitMatrix, x: usize, y: usize) {
    let xo = x * 6;
    let yo = y * 6;
    for i in 0..5 {
        m.set(xo + i, yo, true); // top edge
        m.set(xo + i, yo + 5, true); // bottom edge
        m.set(xo, yo + i, true); // left edge
        m.set(xo + 5, yo + i, true); // right edge
    }
    m.set(xo + 5, yo + 5, true); // corner
}

/// Place the 14 data bits of `cw` into the interior of macromodule `(x, y)`.
fn place_macromodule(m: &mut BitMatrix, x: usize, y: usize, cw: u32) {
    let ci = x * 6 + 1;
    let ri = y * 6 + 1;
    if cw & 0x2000 != 0 {
        m.set(ci + 2, ri, true);
    }
    if cw & 0x1000 != 0 {
        m.set(ci + 3, ri, true);
    }
    for r in 1..4usize {
        for c in 0..4usize {
            if (cw >> (15 - r * 4 - c)) & 1 != 0 {
                m.set(ci + c, ri + r, true);
            }
        }
    }
}

/// Read the 14 data bits of macromodule `(x, y)` back into a codeword value.
fn read_macromodule(m: &BitMatrix, x: usize, y: usize) -> u32 {
    let ci = x * 6 + 1;
    let ri = y * 6 + 1;
    let mut cw = 0u32;
    if m.get(ci + 2, ri) {
        cw |= 0x2000;
    }
    if m.get(ci + 3, ri) {
        cw |= 0x1000;
    }
    for r in 1..4usize {
        for c in 0..4usize {
            if m.get(ci + c, ri + r) {
                cw |= 1 << (15 - r * 4 - c);
            }
        }
    }
    cw
}

/// Render a full codeword stream (`version.total_codewords()` 7-bit values) into a
/// [`BitMatrix`]: frame, interleaved data macromodules, and layer-id metadata.
pub fn render(version: Version, ec: EcLevel, cws: &[u8]) -> BitMatrix {
    let layers = version.number() as usize;
    let mpd = version.macromodules_per_dim();
    let size = version.size();
    debug_assert_eq!(cws.len(), version.total_codewords());

    let mut m = BitMatrix::new(size, size, QUIET_ZONE);

    // Alternating macromodule frames.
    for y in 0..mpd {
        for x in 0..mpd {
            if (x + y) % 2 == 0 {
                draw_frame(&mut m, x, y);
            }
        }
    }

    // Two codewords per macromodule, ordered by the ring numbering.
    for y in 0..mpd {
        for x in 0..mpd {
            let idx = macromodule_index(mpd, x, y);
            let cw = ((cws[idx + 1] as u32) << 7) | (cws[idx] as u32);
            place_macromodule(&mut m, x, y, cw);
        }
    }

    // Layer-id metadata (encodes the EC level in concentric rings).
    let ids = layer_ids(layers, ec.number());
    for y in 0..mpd {
        for x in 0..mpd {
            let id = ids[ring_of(layers, x, y)];
            let ci = x * 6 + 1;
            let ri = y * 6 + 1;
            if id & 0x02 != 0 {
                m.set(ci, ri, true);
            }
            if id & 0x01 != 0 {
                m.set(ci + 1, ri, true);
            }
        }
    }

    m
}

/// Read the interleaved codeword stream back out of a rendered matrix.
pub fn read_codewords(version: Version, m: &BitMatrix) -> Vec<u8> {
    let mpd = version.macromodules_per_dim();
    let mut cws = vec![0u8; version.total_codewords()];
    for y in 0..mpd {
        for x in 0..mpd {
            let idx = macromodule_index(mpd, x, y);
            let cw = read_macromodule(m, x, y);
            cws[idx] = (cw & 0x7F) as u8;
            cws[idx + 1] = ((cw >> 7) & 0x7F) as u8;
        }
    }
    cws
}

/// Recover the EC level from the layer-id rings by majority vote per ring, then
/// matching the observed ring ids against every candidate level.
pub fn read_ec_level(version: Version, m: &BitMatrix) -> Option<EcLevel> {
    let layers = version.number() as usize;
    let mpd = version.macromodules_per_dim();

    // Vote for each ring's 2-bit id.
    let mut counts = vec![[0usize; 4]; layers + 1];
    for y in 0..mpd {
        for x in 0..mpd {
            let ci = x * 6 + 1;
            let ri = y * 6 + 1;
            let mut id = 0usize;
            if m.get(ci, ri) {
                id |= 0x02;
            }
            if m.get(ci + 1, ri) {
                id |= 0x01;
            }
            counts[ring_of(layers, x, y)][id] += 1;
        }
    }
    let observed: Vec<u8> = counts
        .iter()
        .map(|c| {
            let mut best = 0usize;
            for v in 1..4 {
                if c[v] > c[best] {
                    best = v;
                }
            }
            best as u8
        })
        .collect();

    // Pick the level whose ring pattern best matches the observed ids.
    let mut best_ec = None;
    let mut best_score = 0usize;
    for ec in EcLevel::ALL {
        let expected = layer_ids(layers, ec.number());
        let score = expected
            .iter()
            .zip(&observed)
            .filter(|(a, b)| a == b)
            .count();
        if score > best_score {
            best_score = score;
            best_ec = Some(ec);
        }
    }
    best_ec
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every macromodule maps to a distinct codeword pair covering the whole stream.
    #[test]
    fn macromodule_indices_are_a_permutation() {
        for v in 1..=13u8 {
            let ver = Version::new(v).unwrap();
            let mpd = ver.macromodules_per_dim();
            let mut seen = vec![false; ver.total_codewords()];
            for y in 0..mpd {
                for x in 0..mpd {
                    let idx = macromodule_index(mpd, x, y);
                    assert!(idx + 1 < seen.len(), "index out of range v{v}");
                    assert!(!seen[idx], "duplicate pair index v{v}");
                    assert!(!seen[idx + 1], "duplicate pair index v{v}");
                    seen[idx] = true;
                    seen[idx + 1] = true;
                }
            }
            assert!(seen.iter().all(|&s| s), "not all codewords covered v{v}");
        }
    }

    /// Codeword placement round-trips through the matrix for every version.
    #[test]
    fn codewords_round_trip() {
        for v in 1..=13u8 {
            let ver = Version::new(v).unwrap();
            let total = ver.total_codewords();
            let cws: Vec<u8> = (0..total).map(|i| ((i * 37 + 5) % 128) as u8).collect();
            let m = render(ver, EcLevel::L3, &cws);
            assert_eq!(read_codewords(ver, &m), cws, "codeword round trip v{v}");
            assert_eq!(read_ec_level(ver, &m), Some(EcLevel::L3), "ec v{v}");
        }
    }

    /// The EC level survives its own round trip for every usable (version, level).
    #[test]
    fn ec_level_round_trips() {
        for v in 1..=13u8 {
            let ver = Version::new(v).unwrap();
            let total = ver.total_codewords();
            let cws: Vec<u8> = vec![0u8; total];
            for ec in EcLevel::ALL {
                if ver.data_codewords(ec) == 0 {
                    continue;
                }
                let m = render(ver, ec, &cws);
                assert_eq!(read_ec_level(ver, &m), Some(ec), "v{v} {ec:?}");
            }
        }
    }
}
