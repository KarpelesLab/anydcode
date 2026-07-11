//! Static Data Matrix ECC 200 specification tables (ISO/IEC 16022 Table 7).
//!
//! Only the **square** symbol sizes are modelled (10×10 … 144×144); rectangular
//! sizes are intentionally left as future work. Each row records the full symbol
//! dimension, the data-region geometry (used to lay out finder/timing borders and to
//! split the mapping matrix into regions), and the Reed–Solomon block layout.

/// Fixed attributes of one square ECC 200 symbol size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SquareSpec {
    /// Full symbol side length in modules, including finder/timing borders.
    pub symbol_size: usize,
    /// Side length of a single square data region (mapping modules, no border).
    pub region_size: usize,
    /// Number of data regions along each axis (`regions_per_axis²` regions total).
    pub regions_per_axis: usize,
    /// Total data codewords.
    pub data_cw: usize,
    /// Total error-correction codewords.
    pub ec_cw: usize,
    /// Number of interleaved Reed–Solomon blocks.
    pub blocks: usize,
}

impl SquareSpec {
    /// Total codewords (data + EC).
    pub fn total_cw(&self) -> usize {
        self.data_cw + self.ec_cw
    }

    /// EC codewords per interleaved block (uniform across blocks).
    pub fn ec_per_block(&self) -> usize {
        self.ec_cw / self.blocks
    }

    /// Side length of the full (multi-region) mapping matrix, i.e. the data area
    /// excluding every region's finder/timing border.
    pub fn mapping_size(&self) -> usize {
        self.region_size * self.regions_per_axis
    }
}

// symbol, region, regions/axis, data_cw, ec_cw, blocks
#[rustfmt::skip]
const SQUARE: &[SquareSpec] = &[
    SquareSpec { symbol_size: 10,  region_size: 8,  regions_per_axis: 1, data_cw: 3,    ec_cw: 5,   blocks: 1  },
    SquareSpec { symbol_size: 12,  region_size: 10, regions_per_axis: 1, data_cw: 5,    ec_cw: 7,   blocks: 1  },
    SquareSpec { symbol_size: 14,  region_size: 12, regions_per_axis: 1, data_cw: 8,    ec_cw: 10,  blocks: 1  },
    SquareSpec { symbol_size: 16,  region_size: 14, regions_per_axis: 1, data_cw: 12,   ec_cw: 12,  blocks: 1  },
    SquareSpec { symbol_size: 18,  region_size: 16, regions_per_axis: 1, data_cw: 18,   ec_cw: 14,  blocks: 1  },
    SquareSpec { symbol_size: 20,  region_size: 18, regions_per_axis: 1, data_cw: 22,   ec_cw: 18,  blocks: 1  },
    SquareSpec { symbol_size: 22,  region_size: 20, regions_per_axis: 1, data_cw: 30,   ec_cw: 20,  blocks: 1  },
    SquareSpec { symbol_size: 24,  region_size: 22, regions_per_axis: 1, data_cw: 36,   ec_cw: 24,  blocks: 1  },
    SquareSpec { symbol_size: 26,  region_size: 24, regions_per_axis: 1, data_cw: 44,   ec_cw: 28,  blocks: 1  },
    SquareSpec { symbol_size: 32,  region_size: 14, regions_per_axis: 2, data_cw: 62,   ec_cw: 36,  blocks: 1  },
    SquareSpec { symbol_size: 36,  region_size: 16, regions_per_axis: 2, data_cw: 86,   ec_cw: 42,  blocks: 1  },
    SquareSpec { symbol_size: 40,  region_size: 18, regions_per_axis: 2, data_cw: 114,  ec_cw: 48,  blocks: 1  },
    SquareSpec { symbol_size: 44,  region_size: 20, regions_per_axis: 2, data_cw: 144,  ec_cw: 56,  blocks: 1  },
    SquareSpec { symbol_size: 48,  region_size: 22, regions_per_axis: 2, data_cw: 174,  ec_cw: 68,  blocks: 1  },
    SquareSpec { symbol_size: 52,  region_size: 24, regions_per_axis: 2, data_cw: 204,  ec_cw: 84,  blocks: 2  },
    SquareSpec { symbol_size: 64,  region_size: 14, regions_per_axis: 4, data_cw: 280,  ec_cw: 112, blocks: 2  },
    SquareSpec { symbol_size: 72,  region_size: 16, regions_per_axis: 4, data_cw: 368,  ec_cw: 144, blocks: 4  },
    SquareSpec { symbol_size: 80,  region_size: 18, regions_per_axis: 4, data_cw: 456,  ec_cw: 192, blocks: 4  },
    SquareSpec { symbol_size: 88,  region_size: 20, regions_per_axis: 4, data_cw: 576,  ec_cw: 224, blocks: 4  },
    SquareSpec { symbol_size: 96,  region_size: 22, regions_per_axis: 4, data_cw: 696,  ec_cw: 272, blocks: 4  },
    SquareSpec { symbol_size: 104, region_size: 24, regions_per_axis: 4, data_cw: 816,  ec_cw: 336, blocks: 6  },
    SquareSpec { symbol_size: 120, region_size: 18, regions_per_axis: 6, data_cw: 1050, ec_cw: 408, blocks: 6  },
    SquareSpec { symbol_size: 132, region_size: 20, regions_per_axis: 6, data_cw: 1304, ec_cw: 496, blocks: 8  },
    SquareSpec { symbol_size: 144, region_size: 22, regions_per_axis: 6, data_cw: 1558, ec_cw: 620, blocks: 10 },
];

/// All modelled square symbol sizes, in ascending order of capacity.
pub fn all_squares() -> &'static [SquareSpec] {
    SQUARE
}

/// The smallest square symbol whose data capacity holds `data_cw` codewords.
pub fn smallest_square_for(data_cw: usize) -> Option<SquareSpec> {
    all_squares().iter().copied().find(|s| s.data_cw >= data_cw)
}

/// The square symbol of a given full side length, if it exists.
pub fn square_by_size(symbol_size: usize) -> Option<SquareSpec> {
    all_squares()
        .iter()
        .copied()
        .find(|s| s.symbol_size == symbol_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_is_consistent() {
        for s in all_squares() {
            // Full symbol side = mapping side + two border modules per region.
            assert_eq!(
                s.symbol_size,
                s.mapping_size() + 2 * s.regions_per_axis,
                "symbol {} border geometry",
                s.symbol_size
            );
            // The mapping matrix holds exactly total_cw codewords, up to the
            // (region_size % 4 == 2) sizes which reserve 4 fixed corner modules.
            let modules = s.mapping_size() * s.mapping_size();
            let slack = modules - s.total_cw() * 8;
            assert!(
                slack == 0 || slack == 4,
                "symbol {} slack {slack}",
                s.symbol_size
            );
            // EC codewords divide evenly across interleaved blocks.
            assert_eq!(s.ec_cw % s.blocks, 0, "symbol {} ec split", s.symbol_size);
        }
    }
}
