//! MaxiCode (ISO/IEC 16023) encoder and structural decoder.
//!
//! MaxiCode is a fixed-size 2D symbol: a 33×30 grid of hexagonal modules with a
//! central circular "bullseye" finder, carrying 144 six-bit codewords protected by
//! Reed–Solomon error correction over GF(64).
//!
//! Layout:
//! - [`gf`]      — GF(64) arithmetic (primitive `0x43`, generator base `α^1`) and RS.
//! - `tables`    — the ISO Figure 5 module-placement grid and the code-set tables.
//! - `encode`    — [`Symbol`] → [`BitMatrix`].
//! - `decode`    — [`BitMatrix`] → [`Symbol`].
//!
//! # Structured Carrier Message modes
//!
//! - **Mode 4** (standard symbol, Standard EC) and **Mode 5** (Enhanced EC) carry
//!   free-form data; Mode 6 (reader programming) shares Mode 4's structure.
//! - **Modes 2 and 3** are Structured Carrier Messages whose *primary* message packs
//!   a postal code (numeric for Mode 2, alphanumeric for Mode 3), a 3-digit country
//!   code and a 3-digit service class; the *secondary* message carries the remaining
//!   data.
//!
//! # Losslessness
//!
//! [`MaxiCodeMeta`] stores the symbol mode, the Structured Carrier fields (modes
//! 2/3) and the exact **data codeword body** — the code-set-encoded stream before
//! Reed–Solomon. [`Symbol::segments`] are the human-readable decoding of that body.
//! Because the encoder places the stored body verbatim and appends freshly computed
//! RS, `encode(decode(x)) == x` byte-for-byte. See [`crate::codes::qr`] for the
//! reference round-trip pattern.
//!
//! # Hex-to-cell convention
//!
//! The hexagonal module grid is represented as a plain [`BitMatrix`] of width 30,
//! height 33 — cell `(x, y)` is Figure 5's module at row `y`, column `x`. See
//! [`tables`] for the (reversible) mapping from cells to codeword bits.
//!
//! [`Symbol`]: crate::Symbol
//! [`BitMatrix`]: crate::output::BitMatrix

mod decode;
mod encode;
pub mod gf;
mod tables;

pub use decode::MaxiCodeDecoder;
pub use encode::MaxiCodeEncoder;

use crate::output::BitMatrix;

/// Total number of six-bit codewords in every MaxiCode symbol.
const TOTAL_CW: usize = 144;

/// The Structured Carrier (primary) fields of a Mode 2 or Mode 3 symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carrier {
    /// Postal code: up to 9 digits (Mode 2) or up to 6 Code Set A characters
    /// (Mode 3, space-padded to 6).
    pub postcode: String,
    /// ISO country code, 0..=999.
    pub country: u16,
    /// Service class, 0..=999.
    pub service: u16,
}

/// MaxiCode parameters needed to re-encode a symbol identically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaxiCodeMeta {
    /// Symbol mode: 2, 3, 4, 5 or 6.
    pub mode: u8,
    /// Structured Carrier primary fields, present only for modes 2 and 3.
    pub carrier: Option<Carrier>,
    /// The complete data-codeword body (six-bit values, including code-set
    /// latches/shifts and trailing padding) exactly as placed in the symbol. Its
    /// length is fixed by the mode (93 for modes 4/6, 77 for mode 5, 84 for modes
    /// 2/3).
    pub body: Vec<u8>,
}

impl Default for MaxiCodeMeta {
    fn default() -> Self {
        MaxiCodeMeta {
            mode: 4,
            carrier: None,
            body: vec![0; body_len(4)],
        }
    }
}

/// The length of the data-codeword body for a mode (code-set stream length).
///
/// Modes 4/6 use 9 primary + 84 secondary data codewords; mode 5 uses 9 + 68; modes
/// 2/3 keep the primary for the Structured Carrier, leaving 84 secondary codewords.
pub(crate) fn body_len(mode: u8) -> usize {
    match mode {
        5 => 77,
        2 | 3 => 84,
        _ => 93,
    }
}

/// `(secondary data codewords, secondary EC codewords)` for a mode.
pub(crate) fn secondary_lengths(mode: u8) -> (usize, usize) {
    if mode == 5 { (68, 56) } else { (84, 40) }
}

/// Compute and store the Reed–Solomon check symbols in place: primary (10 data + 10
/// EC over `cw[0..20]`) and secondary (even/odd interleaved).
pub(crate) fn add_error_correction(cw: &mut [u8; TOTAL_CW], mode: u8) {
    // Primary: always Enhanced EC, 10 data + 10 check codewords.
    let primary_ec = gf::encode(&cw[0..10], 10);
    cw[10..20].copy_from_slice(&primary_ec);

    // Secondary: even and odd codewords form two independent interleaves.
    let (dlen, eclen) = secondary_lengths(mode);
    let half = eclen / 2;
    for parity in 0..2 {
        let data: Vec<u8> = (0..dlen).step_by(2).map(|j| cw[20 + j + parity]).collect();
        let ec = gf::encode(&data, half);
        for (k, &e) in ec.iter().enumerate() {
            cw[20 + dlen + 2 * k + parity] = e;
        }
    }
}

/// Reed–Solomon-correct the primary block (`cw[0..20]`) in place. This recovers the
/// mode codeword before the secondary block can be de-interleaved.
pub(crate) fn correct_primary(cw: &mut [u8; TOTAL_CW]) -> crate::error::Result<()> {
    let fixed = gf::decode(&cw[0..20], 10).ok_or(crate::error::Error::ErrorCorrectionFailed)?;
    cw[0..20].copy_from_slice(&fixed);
    Ok(())
}

/// Reed–Solomon-correct the secondary even/odd interleaves in place.
pub(crate) fn correct_secondary(cw: &mut [u8; TOTAL_CW], mode: u8) -> crate::error::Result<()> {
    let (dlen, eclen) = secondary_lengths(mode);
    let half = eclen / 2;
    for parity in 0..2 {
        let mut block: Vec<u8> = (0..dlen).step_by(2).map(|j| cw[20 + j + parity]).collect();
        block.extend((0..half).map(|k| cw[20 + dlen + 2 * k + parity]));
        let fixed = gf::decode(&block, half).ok_or(crate::error::Error::ErrorCorrectionFailed)?;
        for (k, j) in (0..dlen).step_by(2).enumerate() {
            cw[20 + j + parity] = fixed[k];
        }
    }
    Ok(())
}

/// Render the 144 codewords into the module grid (data modules, orientation
/// modules, light finder region).
pub(crate) fn render_matrix(cw: &[u8; TOTAL_CW]) -> BitMatrix {
    let mut m = BitMatrix::new(tables::WIDTH, tables::HEIGHT, 1);
    for (row, cells) in tables::MAXI_GRID.iter().enumerate() {
        for (col, &v) in cells.iter().enumerate() {
            if let Some((idx, bit)) = tables::cell_target(v)
                && (cw[idx] >> bit) & 1 != 0
            {
                m.set(col, row, true);
            }
        }
    }
    for &(row, col) in &tables::ORIENTATION {
        m.set(col, row, true);
    }
    m
}

/// Read the 144 codewords back out of a module grid.
pub(crate) fn read_codewords(m: &BitMatrix) -> [u8; TOTAL_CW] {
    let mut cw = [0u8; TOTAL_CW];
    for (row, cells) in tables::MAXI_GRID.iter().enumerate() {
        for (col, &v) in cells.iter().enumerate() {
            if let Some((idx, bit)) = tables::cell_target(v)
                && m.get(col, row)
            {
                cw[idx] |= 1 << bit;
            }
        }
    }
    cw
}
