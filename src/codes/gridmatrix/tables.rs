//! Static Grid Matrix specification tables (AIMD014 / GB/T 27766).
//!
//! Grid Matrix has 13 versions. A version-`v` symbol is built from
//! `mpd = 2·v + 1` macromodules per dimension; each macromodule is a 6×6 block of
//! modules, so the symbol is `size = 6·mpd = 6 + 12·v` modules on a side and holds
//! `mpd²` data macromodules of **two 7-bit codewords** each (`total_cw = 2·mpd²`).
//!
//! The error-correction structure (data-codeword capacity per EC level, block split,
//! and per-block EC lengths) is transcribed verbatim from zint's `gridmtx.h`
//! (`gm_data_cws`, `gm_n1`, `gm_b1`, `gm_b2`, `gm_e1b3e2`); those arrays are the
//! independent cross-reference for this implementation.

/// The five Grid Matrix error-correction levels, in ascending order of *data*
/// capacity (descending order of redundancy). `L1` is the strongest protection
/// (least data), `L5` the weakest (most data). The numeric index used by the
/// specification tables is `level as usize + 1` (1–5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EcLevel {
    /// Strongest error correction (roughly half the codewords are EC).
    L1,
    /// Strong error correction.
    L2,
    /// Balanced error correction (the encoder default).
    L3,
    /// Light error correction.
    L4,
    /// Weakest error correction (maximum data capacity).
    L5,
}

impl EcLevel {
    /// The 1-based level number used to index the specification tables.
    pub fn number(self) -> usize {
        self as usize + 1
    }

    /// Recover a level from its 1-based number (1–5).
    pub fn from_number(n: usize) -> Option<Self> {
        match n {
            1 => Some(EcLevel::L1),
            2 => Some(EcLevel::L2),
            3 => Some(EcLevel::L3),
            4 => Some(EcLevel::L4),
            5 => Some(EcLevel::L5),
            _ => None,
        }
    }

    /// All five levels, strongest first.
    pub const ALL: [EcLevel; 5] = [
        EcLevel::L1,
        EcLevel::L2,
        EcLevel::L3,
        EcLevel::L4,
        EcLevel::L5,
    ];
}

/// A Grid Matrix version, 1–13. Determines the symbol size and codeword capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Version(u8);

impl Version {
    /// Construct a version, validating the `1..=13` range.
    pub fn new(v: u8) -> Option<Self> {
        (1..=13).contains(&v).then_some(Version(v))
    }

    /// The version number, 1–13.
    pub fn number(self) -> u8 {
        self.0
    }

    /// Number of macromodules along each side (`2·v + 1`).
    pub fn macromodules_per_dim(self) -> usize {
        1 + (self.0 as usize) * 2
    }

    /// Side length of the module grid (`6 + 12·v`).
    pub fn size(self) -> usize {
        6 * self.macromodules_per_dim()
    }

    /// Total codewords in the symbol (`2 · mpd²`).
    pub fn total_codewords(self) -> usize {
        let mpd = self.macromodules_per_dim();
        2 * mpd * mpd
    }

    /// Data-codeword capacity at the given EC level (zint `gm_data_cws`).
    pub fn data_codewords(self, ec: EcLevel) -> usize {
        GM_DATA_CWS[self.0 as usize - 1][ec.number() - 1] as usize
    }
}

/// Data-codeword capacity per version (row) and EC level 1–5 (column). Verbatim
/// from zint `gm_data_cws`. The trailing comment on each source row is the symbol's
/// total codeword count, which equals `2·(2v+1)²`.
#[rustfmt::skip]
pub const GM_DATA_CWS: [[u16; 5]; 13] = [
    [    0,   15,   13,  11,   9 ], // v1   total 18
    [   45,   40,   35,  30,  25 ], // v2   total 50
    [   89,   79,   69,  59,  49 ], // v3   total 98
    [  146,  130,  114,  98,  81 ], // v4   total 162
    [  218,  194,  170, 146, 121 ], // v5   total 242
    [  305,  271,  237, 203, 169 ], // v6   total 338
    [  405,  360,  315, 270, 225 ], // v7   total 450
    [  521,  463,  405, 347, 289 ], // v8   total 578
    [  650,  578,  506, 434, 361 ], // v9   total 722
    [  794,  706,  618, 530, 441 ], // v10  total 882
    [  953,  847,  741, 635, 529 ], // v11  total 1058
    [ 1125, 1000,  875, 750, 625 ], // v12  total 1250
    [ 1313, 1167, 1021, 875, 729 ], // v13  total 1458
];

/// Size of the larger interleaved RS block per version (zint `gm_n1`).
#[rustfmt::skip]
pub const GM_N1: [u8; 13] = [
    18, 50, 98, 81, 121, 113, 113, 116, 121, 126, 118, 125, 122,
];

/// Number of larger (`n1`) blocks per version (zint `gm_b1`).
#[rustfmt::skip]
pub const GM_B1: [u8; 13] = [
    1, 1, 1, 2, 2, 2, 2, 3, 2, 7, 5, 10, 6,
];

/// Number of smaller (`n2 = n1 - 1`) blocks per version (zint `gm_b2`).
#[rustfmt::skip]
pub const GM_B2: [u8; 13] = [
    0, 0, 0, 0, 0, 1, 2, 2, 4, 0, 4, 0, 6,
];

/// Per version and EC level: `[e1, b3, e2]` where `e1` is the EC length of the first
/// `b3` blocks and `e2` the EC length of the remaining blocks (zint `gm_e1b3e2`).
#[rustfmt::skip]
pub const GM_E1B3E2: [[[u8; 3]; 5]; 13] = [
    [ [  0, 0,  0 ], [  3,  1,  0 ], [  5, 1,  0 ], [  7,  1,  0 ], [  9, 1,  0 ] ], // 1
    [ [  5, 1,  0 ], [ 10,  1,  0 ], [ 15, 1,  0 ], [ 20,  1,  0 ], [ 25, 1,  0 ] ], // 2
    [ [  9, 1,  0 ], [ 19,  1,  0 ], [ 29, 1,  0 ], [ 39,  1,  0 ], [ 49, 1,  0 ] ], // 3
    [ [  8, 2,  0 ], [ 16,  2,  0 ], [ 24, 2,  0 ], [ 32,  2,  0 ], [ 41, 1, 40 ] ], // 4
    [ [ 12, 2,  0 ], [ 24,  2,  0 ], [ 36, 2,  0 ], [ 48,  2,  0 ], [ 61, 1, 60 ] ], // 5
    [ [ 11, 3,  0 ], [ 23,  1, 22 ], [ 34, 2, 33 ], [ 45,  3,  0 ], [ 57, 1, 56 ] ], // 6
    [ [ 12, 1, 11 ], [ 23,  2, 22 ], [ 34, 3, 33 ], [ 45,  4,  0 ], [ 57, 1, 56 ] ], // 7
    [ [ 12, 2, 11 ], [ 23,  5,  0 ], [ 35, 3, 34 ], [ 47,  1, 46 ], [ 58, 4, 57 ] ], // 8
    [ [ 12, 6,  0 ], [ 24,  6,  0 ], [ 36, 6,  0 ], [ 48,  6,  0 ], [ 61, 1, 60 ] ], // 9
    [ [ 13, 4, 12 ], [ 26,  1, 25 ], [ 38, 5, 37 ], [ 51,  2, 50 ], [ 63, 7,  0 ] ], // 10
    [ [ 12, 6, 11 ], [ 24,  4, 23 ], [ 36, 2, 35 ], [ 47,  9,  0 ], [ 59, 7, 58 ] ], // 11
    [ [ 13, 5, 12 ], [ 25, 10,  0 ], [ 38, 5, 37 ], [ 50, 10,  0 ], [ 63, 5, 62 ] ], // 12
    [ [ 13, 1, 12 ], [ 25,  3, 24 ], [ 37, 5, 36 ], [ 49,  7, 48 ], [ 61, 9, 60 ] ], // 13
];

/// One interleaved Reed–Solomon block: how many total and data codewords it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    /// Total codewords in the block (data + EC).
    pub total: usize,
    /// EC codewords in the block.
    pub ec: usize,
}

impl Block {
    /// Data codewords in the block.
    pub fn data(self) -> usize {
        self.total - self.ec
    }
}

/// The ordered list of interleaved RS blocks for `version` at EC level `ec`.
///
/// Mirrors zint `gm_add_ecc`: the first `b1` blocks have size `n1`, the remaining
/// `b2` have size `n2 = n1 - 1`; the first `b3` blocks carry `e1` EC codewords and
/// the rest carry `e2`.
pub fn blocks(version: Version, ec: EcLevel) -> Vec<Block> {
    let idx = version.number() as usize - 1;
    let eidx = ec.number() - 1;
    let n1 = GM_N1[idx] as usize;
    let n2 = n1 - 1;
    let b1 = GM_B1[idx] as usize;
    let b2 = GM_B2[idx] as usize;
    let e1 = GM_E1B3E2[idx][eidx][0] as usize;
    let b3 = GM_E1B3E2[idx][eidx][1] as usize;
    let e2 = GM_E1B3E2[idx][eidx][2] as usize;

    let num_blocks = b1 + b2;
    let mut out = Vec::with_capacity(num_blocks);
    for i in 0..num_blocks {
        let total = if i < b1 { n1 } else { n2 };
        let ec = if i < b3 { e1 } else { e2 };
        out.push(Block { total, ec });
    }
    out
}

/// The smallest version whose data capacity at `ec` holds `data_cw` codewords.
pub fn smallest_version_for(data_cw: usize, ec: EcLevel) -> Option<Version> {
    (1..=13u8).find_map(|v| {
        let ver = Version(v);
        let cap = ver.data_codewords(ec);
        (cap > 0 && cap >= data_cw).then_some(ver)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The geometry-derived total codeword count matches the value implied by the
    /// zint capacity table, and the block split accounts for exactly those codewords.
    #[test]
    fn geometry_matches_zint_tables() {
        // Total codewords implied by the annotated zint rows.
        let totals = [
            18, 50, 98, 162, 242, 338, 450, 578, 722, 882, 1058, 1250, 1458,
        ];
        for v in 1..=13u8 {
            let ver = Version::new(v).unwrap();
            assert_eq!(
                ver.total_codewords(),
                totals[v as usize - 1],
                "total codewords for v{v}"
            );
            assert_eq!(ver.size(), 6 + 12 * v as usize, "size for v{v}");

            // For every EC level the blocks tile the full codeword budget and the
            // data codewords equal the zint gm_data_cws capacity.
            for ec in EcLevel::ALL {
                let blocks = blocks(ver, ec);
                if ver.data_codewords(ec) == 0 {
                    continue; // v1/L1 is unusable (0 data codewords)
                }
                let total: usize = blocks.iter().map(|b| b.total).sum();
                let data: usize = blocks.iter().map(|b| b.data()).sum();
                assert_eq!(total, ver.total_codewords(), "block total v{v} {ec:?}");
                assert_eq!(data, ver.data_codewords(ec), "block data v{v} {ec:?}");
                assert!(
                    blocks.iter().all(|b| b.ec <= b.total),
                    "ec exceeds block v{v} {ec:?}"
                );
            }
        }
    }
}
