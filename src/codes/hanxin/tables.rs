//! Static Han Xin Code specification tables (ISO/IEC 20830), mirrored from zint
//! `backend/hanxin.h` / `hanxin.c`.
//!
//! **Scope:** this implementation covers **versions 1–3** (23×23, 25×25, 27×27),
//! which have no alignment/assistant patterns (`hx_module_m == 0` for these
//! versions, so zint's `hx_setup_grid` skips the `version > 3` grid entirely). The
//! per-version tables below therefore only carry those three versions. Larger
//! versions and the alignment-grid geometry are out of scope; see the module docs.

use super::{EcLevel, Version};

/// Number of supported versions.
pub const MAX_VERSION: u8 = 3;

/// Total codewords (data + EC) per version, `hx_total_codewords` in zint.
const TOTAL_CODEWORDS: [usize; 3] = [25, 37, 50];

/// Reed–Solomon block layout for one (version, EC level): a list of up to three
/// batches, each `(block_count, data_per_block, ec_per_block)`.
#[derive(Debug, Clone, Copy)]
pub struct EcBlocks {
    /// Up to three batches `(blocks, data, ec)`; zero-block batches are ignored.
    pub batches: [(usize, usize, usize); 3],
}

impl EcBlocks {
    /// Total data codewords across all batches.
    pub fn total_data(&self) -> usize {
        self.batches.iter().map(|(b, d, _)| b * d).sum()
    }
}

// `hx_table_d1`: for each version, four EC levels (L1..L4), each `{blocks,k,2t}` for
// up to three batches. Versions 1–3 use a single batch. Values cross-checked against
// `hx_data_codewords` (data) and `hx_total_codewords` (data + EC) in zint hanxin.h.
#[rustfmt::skip]
const TABLE_D1: [[[(usize, usize, usize); 3]; 4]; 3] = [
    // Version 1 (total 25)
    [
        [(1, 21, 4), (0, 0, 0), (0, 0, 0)],
        [(1, 17, 8), (0, 0, 0), (0, 0, 0)],
        [(1, 13, 12), (0, 0, 0), (0, 0, 0)],
        [(1, 9, 16), (0, 0, 0), (0, 0, 0)],
    ],
    // Version 2 (total 37)
    [
        [(1, 31, 6), (0, 0, 0), (0, 0, 0)],
        [(1, 25, 12), (0, 0, 0), (0, 0, 0)],
        [(1, 19, 18), (0, 0, 0), (0, 0, 0)],
        [(1, 15, 22), (0, 0, 0), (0, 0, 0)],
    ],
    // Version 3 (total 50)
    [
        [(1, 42, 8), (0, 0, 0), (0, 0, 0)],
        [(1, 34, 16), (0, 0, 0), (0, 0, 0)],
        [(1, 26, 24), (0, 0, 0), (0, 0, 0)],
        [(1, 20, 30), (0, 0, 0), (0, 0, 0)],
    ],
];

fn level_index(level: EcLevel) -> usize {
    match level {
        EcLevel::L1 => 0,
        EcLevel::L2 => 1,
        EcLevel::L3 => 2,
        EcLevel::L4 => 3,
    }
}

/// The RS block layout for a version and EC level.
pub fn ec_blocks(version: Version, level: EcLevel) -> EcBlocks {
    EcBlocks {
        batches: TABLE_D1[version.number() as usize - 1][level_index(level)],
    }
}

/// Total codewords (data + EC) for a version.
pub fn total_codewords(version: Version) -> usize {
    TOTAL_CODEWORDS[version.number() as usize - 1]
}

/// Total data codewords for a version and EC level.
pub fn data_codewords(version: Version, level: EcLevel) -> usize {
    ec_blocks(version, level).total_data()
}

/// The four 7×7 corner finder patterns, one byte per row, bit `0x40 >> col` = dark.
/// Straight from zint (`hx_place_finder_top_left`, `hx_place_finder`,
/// `hx_place_finder_bottom_right`).
pub const FINDER_TOP_LEFT: [u8; 7] = [0x7F, 0x40, 0x5F, 0x50, 0x57, 0x57, 0x57];
/// Finder used at top-right and bottom-left corners.
pub const FINDER_SIDE: [u8; 7] = [0x7F, 0x01, 0x7D, 0x05, 0x75, 0x75, 0x75];
/// Finder used at the bottom-right corner.
pub const FINDER_BOTTOM_RIGHT: [u8; 7] = [0x75, 0x75, 0x75, 0x05, 0x7D, 0x01, 0x7F];
