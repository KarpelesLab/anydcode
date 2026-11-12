//! Static Data Matrix ECC 200 specification tables.
//!
//! Three families of symbol size share one row type: the 24 **square** sizes
//! (10×10 … 144×144) and the 6 **rectangular** sizes (8×18 … 16×48) of ISO/IEC 16022
//! Table 7, and the 18 further rectangular sizes of the **DMRE** extension
//! (ISO/IEC 21471, 8×48 … 26×64). Each row records the full symbol dimensions, the
//! data-region geometry (used to lay out finder/timing borders and to split the
//! mapping matrix into regions), and the Reed–Solomon block layout.

/// Fixed attributes of one ECC 200 symbol size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolSpec {
    /// Full symbol width in modules, including finder/timing borders.
    pub width: usize,
    /// Full symbol height in modules, including finder/timing borders.
    pub height: usize,
    /// Rows of a single data region (mapping modules, no border).
    pub region_rows: usize,
    /// Columns of a single data region (mapping modules, no border).
    pub region_cols: usize,
    /// Number of data regions side by side.
    pub regions_h: usize,
    /// Number of data regions stacked vertically.
    pub regions_v: usize,
    /// Total data codewords.
    pub data_cw: usize,
    /// Total error-correction codewords.
    pub ec_cw: usize,
    /// Number of interleaved Reed–Solomon blocks.
    pub blocks: usize,
}

impl SymbolSpec {
    /// Total codewords (data + EC).
    pub const fn total_cw(&self) -> usize {
        self.data_cw + self.ec_cw
    }

    /// EC codewords per interleaved block (uniform across blocks).
    pub const fn ec_per_block(&self) -> usize {
        self.ec_cw / self.blocks
    }

    /// Position of `block`'s codewords within each round of the interleaved EC
    /// stream. Interleaving deals codewords to the blocks in turn and simply carries
    /// on from the data into the EC codewords, so the EC rounds start with whichever
    /// block follows the last data codeword. That is block 0 for every size except
    /// 144×144, whose 1558 data codewords leave blocks 8 and 9 one short: there the
    /// EC codewords run 8, 9, 0, 1, … 7.
    pub const fn ec_slot(&self, block: usize) -> usize {
        (block + self.blocks - self.data_cw % self.blocks) % self.blocks
    }

    /// Rows of the full (multi-region) mapping matrix, i.e. the data area excluding
    /// every region's finder/timing border.
    pub const fn mapping_rows(&self) -> usize {
        self.region_rows * self.regions_v
    }

    /// Columns of the full (multi-region) mapping matrix.
    pub const fn mapping_cols(&self) -> usize {
        self.region_cols * self.regions_h
    }

    /// Whether the symbol is square.
    #[cfg_attr(not(feature = "scan"), allow(dead_code))]
    pub const fn is_square(&self) -> bool {
        self.width == self.height
    }
}

/// How many of [`SPECS`] are the square sizes; they come first.
const SQUARES: usize = 24;

// The squares in ascending order, then the ISO/IEC 16022 rectangles, then DMRE.
#[rustfmt::skip]
const SPECS: &[SymbolSpec] = &[
    SymbolSpec { width: 10, height: 10, region_rows: 8, region_cols: 8, regions_h: 1, regions_v: 1, data_cw: 3, ec_cw: 5, blocks: 1 },
    SymbolSpec { width: 12, height: 12, region_rows: 10, region_cols: 10, regions_h: 1, regions_v: 1, data_cw: 5, ec_cw: 7, blocks: 1 },
    SymbolSpec { width: 14, height: 14, region_rows: 12, region_cols: 12, regions_h: 1, regions_v: 1, data_cw: 8, ec_cw: 10, blocks: 1 },
    SymbolSpec { width: 16, height: 16, region_rows: 14, region_cols: 14, regions_h: 1, regions_v: 1, data_cw: 12, ec_cw: 12, blocks: 1 },
    SymbolSpec { width: 18, height: 18, region_rows: 16, region_cols: 16, regions_h: 1, regions_v: 1, data_cw: 18, ec_cw: 14, blocks: 1 },
    SymbolSpec { width: 20, height: 20, region_rows: 18, region_cols: 18, regions_h: 1, regions_v: 1, data_cw: 22, ec_cw: 18, blocks: 1 },
    SymbolSpec { width: 22, height: 22, region_rows: 20, region_cols: 20, regions_h: 1, regions_v: 1, data_cw: 30, ec_cw: 20, blocks: 1 },
    SymbolSpec { width: 24, height: 24, region_rows: 22, region_cols: 22, regions_h: 1, regions_v: 1, data_cw: 36, ec_cw: 24, blocks: 1 },
    SymbolSpec { width: 26, height: 26, region_rows: 24, region_cols: 24, regions_h: 1, regions_v: 1, data_cw: 44, ec_cw: 28, blocks: 1 },
    SymbolSpec { width: 32, height: 32, region_rows: 14, region_cols: 14, regions_h: 2, regions_v: 2, data_cw: 62, ec_cw: 36, blocks: 1 },
    SymbolSpec { width: 36, height: 36, region_rows: 16, region_cols: 16, regions_h: 2, regions_v: 2, data_cw: 86, ec_cw: 42, blocks: 1 },
    SymbolSpec { width: 40, height: 40, region_rows: 18, region_cols: 18, regions_h: 2, regions_v: 2, data_cw: 114, ec_cw: 48, blocks: 1 },
    SymbolSpec { width: 44, height: 44, region_rows: 20, region_cols: 20, regions_h: 2, regions_v: 2, data_cw: 144, ec_cw: 56, blocks: 1 },
    SymbolSpec { width: 48, height: 48, region_rows: 22, region_cols: 22, regions_h: 2, regions_v: 2, data_cw: 174, ec_cw: 68, blocks: 1 },
    SymbolSpec { width: 52, height: 52, region_rows: 24, region_cols: 24, regions_h: 2, regions_v: 2, data_cw: 204, ec_cw: 84, blocks: 2 },
    SymbolSpec { width: 64, height: 64, region_rows: 14, region_cols: 14, regions_h: 4, regions_v: 4, data_cw: 280, ec_cw: 112, blocks: 2 },
    SymbolSpec { width: 72, height: 72, region_rows: 16, region_cols: 16, regions_h: 4, regions_v: 4, data_cw: 368, ec_cw: 144, blocks: 4 },
    SymbolSpec { width: 80, height: 80, region_rows: 18, region_cols: 18, regions_h: 4, regions_v: 4, data_cw: 456, ec_cw: 192, blocks: 4 },
    SymbolSpec { width: 88, height: 88, region_rows: 20, region_cols: 20, regions_h: 4, regions_v: 4, data_cw: 576, ec_cw: 224, blocks: 4 },
    SymbolSpec { width: 96, height: 96, region_rows: 22, region_cols: 22, regions_h: 4, regions_v: 4, data_cw: 696, ec_cw: 272, blocks: 4 },
    SymbolSpec { width: 104, height: 104, region_rows: 24, region_cols: 24, regions_h: 4, regions_v: 4, data_cw: 816, ec_cw: 336, blocks: 6 },
    SymbolSpec { width: 120, height: 120, region_rows: 18, region_cols: 18, regions_h: 6, regions_v: 6, data_cw: 1050, ec_cw: 408, blocks: 6 },
    SymbolSpec { width: 132, height: 132, region_rows: 20, region_cols: 20, regions_h: 6, regions_v: 6, data_cw: 1304, ec_cw: 496, blocks: 8 },
    SymbolSpec { width: 144, height: 144, region_rows: 22, region_cols: 22, regions_h: 6, regions_v: 6, data_cw: 1558, ec_cw: 620, blocks: 10 },
    // ISO/IEC 16022 rectangular sizes.
    SymbolSpec { width: 18, height: 8, region_rows: 6, region_cols: 16, regions_h: 1, regions_v: 1, data_cw: 5, ec_cw: 7, blocks: 1 },
    SymbolSpec { width: 32, height: 8, region_rows: 6, region_cols: 14, regions_h: 2, regions_v: 1, data_cw: 10, ec_cw: 11, blocks: 1 },
    SymbolSpec { width: 26, height: 12, region_rows: 10, region_cols: 24, regions_h: 1, regions_v: 1, data_cw: 16, ec_cw: 14, blocks: 1 },
    SymbolSpec { width: 36, height: 12, region_rows: 10, region_cols: 16, regions_h: 2, regions_v: 1, data_cw: 22, ec_cw: 18, blocks: 1 },
    SymbolSpec { width: 36, height: 16, region_rows: 14, region_cols: 16, regions_h: 2, regions_v: 1, data_cw: 32, ec_cw: 24, blocks: 1 },
    SymbolSpec { width: 48, height: 16, region_rows: 14, region_cols: 22, regions_h: 2, regions_v: 1, data_cw: 49, ec_cw: 28, blocks: 1 },
    // DMRE (ISO/IEC 21471) rectangular sizes.
    SymbolSpec { width: 48, height: 8, region_rows: 6, region_cols: 22, regions_h: 2, regions_v: 1, data_cw: 18, ec_cw: 15, blocks: 1 },
    SymbolSpec { width: 64, height: 8, region_rows: 6, region_cols: 14, regions_h: 4, regions_v: 1, data_cw: 24, ec_cw: 18, blocks: 1 },
    SymbolSpec { width: 80, height: 8, region_rows: 6, region_cols: 18, regions_h: 4, regions_v: 1, data_cw: 32, ec_cw: 22, blocks: 1 },
    SymbolSpec { width: 96, height: 8, region_rows: 6, region_cols: 22, regions_h: 4, regions_v: 1, data_cw: 38, ec_cw: 28, blocks: 1 },
    SymbolSpec { width: 120, height: 8, region_rows: 6, region_cols: 18, regions_h: 6, regions_v: 1, data_cw: 49, ec_cw: 32, blocks: 1 },
    SymbolSpec { width: 144, height: 8, region_rows: 6, region_cols: 22, regions_h: 6, regions_v: 1, data_cw: 63, ec_cw: 36, blocks: 1 },
    SymbolSpec { width: 64, height: 12, region_rows: 10, region_cols: 14, regions_h: 4, regions_v: 1, data_cw: 43, ec_cw: 27, blocks: 1 },
    SymbolSpec { width: 88, height: 12, region_rows: 10, region_cols: 20, regions_h: 4, regions_v: 1, data_cw: 64, ec_cw: 36, blocks: 1 },
    SymbolSpec { width: 64, height: 16, region_rows: 14, region_cols: 14, regions_h: 4, regions_v: 1, data_cw: 62, ec_cw: 36, blocks: 1 },
    SymbolSpec { width: 36, height: 20, region_rows: 18, region_cols: 16, regions_h: 2, regions_v: 1, data_cw: 44, ec_cw: 28, blocks: 1 },
    SymbolSpec { width: 44, height: 20, region_rows: 18, region_cols: 20, regions_h: 2, regions_v: 1, data_cw: 56, ec_cw: 34, blocks: 1 },
    SymbolSpec { width: 64, height: 20, region_rows: 18, region_cols: 14, regions_h: 4, regions_v: 1, data_cw: 84, ec_cw: 42, blocks: 1 },
    SymbolSpec { width: 48, height: 22, region_rows: 20, region_cols: 22, regions_h: 2, regions_v: 1, data_cw: 72, ec_cw: 38, blocks: 1 },
    SymbolSpec { width: 48, height: 24, region_rows: 22, region_cols: 22, regions_h: 2, regions_v: 1, data_cw: 80, ec_cw: 41, blocks: 1 },
    SymbolSpec { width: 64, height: 24, region_rows: 22, region_cols: 14, regions_h: 4, regions_v: 1, data_cw: 108, ec_cw: 46, blocks: 1 },
    SymbolSpec { width: 40, height: 26, region_rows: 24, region_cols: 18, regions_h: 2, regions_v: 1, data_cw: 70, ec_cw: 38, blocks: 1 },
    SymbolSpec { width: 48, height: 26, region_rows: 24, region_cols: 22, regions_h: 2, regions_v: 1, data_cw: 90, ec_cw: 42, blocks: 1 },
    SymbolSpec { width: 64, height: 26, region_rows: 24, region_cols: 14, regions_h: 4, regions_v: 1, data_cw: 118, ec_cw: 50, blocks: 1 },
];

/// The square symbol sizes, in ascending order of capacity.
pub fn all_squares() -> &'static [SymbolSpec] {
    &SPECS[..SQUARES]
}

/// Every modelled symbol size: squares, then rectangles.
#[cfg_attr(not(feature = "scan"), allow(dead_code))]
pub fn all_specs() -> &'static [SymbolSpec] {
    SPECS
}

/// The largest symbol (144×144): the most codewords and the most modules of any
/// size, square or not.
pub const LARGEST: SymbolSpec = SPECS[SQUARES - 1];

/// The smallest square symbol whose data capacity holds `data_cw` codewords.
pub fn smallest_square_for(data_cw: usize) -> Option<SymbolSpec> {
    all_squares().iter().copied().find(|s| s.data_cw >= data_cw)
}

/// The symbol of the given full dimensions, if such a size exists.
pub const fn spec_by_size(width: usize, height: usize) -> Option<SymbolSpec> {
    let mut i = 0;
    while i < SPECS.len() {
        if SPECS[i].width == width && SPECS[i].height == height {
            return Some(SPECS[i]);
        }
        i += 1;
    }
    None
}

#[cfg(all(test, feature = "encode", feature = "decode"))]
mod tests {
    use super::*;

    #[test]
    fn geometry_is_consistent() {
        assert_eq!(all_squares().len(), 24);
        assert!(all_squares().iter().all(SymbolSpec::is_square));
        assert_eq!(all_specs().len(), 48);
        for s in all_specs() {
            // Full symbol side = mapping side + two border modules per region.
            assert_eq!(s.width, s.mapping_cols() + 2 * s.regions_h, "{s:?}");
            assert_eq!(s.height, s.mapping_rows() + 2 * s.regions_v, "{s:?}");
            // The mapping matrix holds exactly total_cw codewords, up to the sizes
            // which reserve 4 fixed corner modules.
            let modules = s.mapping_rows() * s.mapping_cols();
            let slack = modules - s.total_cw() * 8;
            assert!(slack == 0 || slack == 4, "{s:?} slack {slack}");
            // EC codewords divide evenly across interleaved blocks.
            assert_eq!(s.ec_cw % s.blocks, 0, "{s:?}");
            assert!(s.total_cw() <= LARGEST.total_cw() && s.width * s.height <= 144 * 144);
            assert_eq!(spec_by_size(s.width, s.height), Some(*s));
        }
        assert_eq!(spec_by_size(18, 8).map(|s| s.data_cw), Some(5));
        assert_eq!(spec_by_size(8, 18), None);
    }
}
