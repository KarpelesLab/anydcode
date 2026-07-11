//! GS1 DataBar decoding: [`LinearPattern`] -> [`Symbol`].
//!
//! Decoding inverts [`super::encode`]: the module row is run-length encoded back
//! into element widths, each data character's combinatorial value is recovered
//! with [`get_value`], the checksum is re-derived and checked against the
//! finder pattern that selected it, and the GTIN is reconstructed.

use super::tables::{
    LTD_CHECK_WEIGHT, LTD_FINDER, LTD_G_SUM, LTD_MODULES, LTD_T_EVEN, LTD_WIDEST, OMN_CHECK_WEIGHT,
    OMN_FINDER, OMN_G_SUM, OMN_MODULES, OMN_T_EVEN_ODD, OMN_WIDEST, gtin_check_digit,
};
use super::widths::get_value;
use super::{DataBarMeta, DataBarVariant};
use crate::error::{Error, Result};
use crate::output::Encoding;
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Decode;

/// GS1 DataBar decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DataBarDecoder;

impl DataBarDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        DataBarDecoder
    }
}

impl Decode for DataBarDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        let pattern = match encoding {
            Encoding::Linear(p) => p,
            // Stacked DataBar (a matrix) is not implemented.
            Encoding::Matrix(_) => {
                return Err(Error::Unsupported {
                    what: "GS1 DataBar stacked decoding",
                });
            }
        };
        let widths = run_lengths(&pattern.modules)?;
        match widths.len() {
            // 46 elements is Omnidirectional or a minimal Expanded symbol; their
            // finder-pattern tables are disjoint, so Omni decoding fails cleanly on
            // an Expanded symbol and we fall through.
            46 => decode_omni(&widths).or_else(|_| super::expanded::decode(&widths)),
            47 => decode_limited(&widths),
            n if super::expanded::is_expanded_width(n) => super::expanded::decode(&widths),
            n => Err(Error::undecodable(format!(
                "unexpected DataBar element count {n} (expected 46, 47, or an Expanded width)"
            ))),
        }
    }
}

/// Run-length encode a module row into element widths.
///
/// DataBar symbols begin with a light guard element, so a canonical row starts
/// light; a row starting dark is rejected rather than silently mis-parsed.
fn run_lengths(modules: &[bool]) -> Result<Vec<i32>> {
    if modules.is_empty() {
        return Err(Error::undecodable("empty DataBar module row"));
    }
    if modules[0] {
        return Err(Error::undecodable(
            "DataBar row must start with a light element",
        ));
    }
    let mut widths = Vec::new();
    let mut current = modules[0];
    let mut run = 0i32;
    for &m in modules {
        if m == current {
            run += 1;
        } else {
            widths.push(run);
            current = m;
            run = 1;
        }
    }
    widths.push(run);
    Ok(widths)
}

/// Reconstruct a GTIN value's canonical 14-digit form (13-digit body + check).
fn format_gtin(val: u64) -> [u8; 14] {
    let mut body = [b'0'; 13];
    let mut v = val;
    for slot in body.iter_mut().rev() {
        *slot = b'0' + (v % 10) as u8;
        v /= 10;
    }
    let check = gtin_check_digit(&body);
    let mut out = [0u8; 14];
    out[..13].copy_from_slice(&body);
    out[13] = check;
    out
}

// ======== Omnidirectional ========

/// Find the Omnidirectional group whose odd-element module total is `n_odd`.
fn omn_group_from_n_odd(n_odd: i32, outside: bool) -> Option<usize> {
    let range = if outside { 0..5 } else { 5..9 };
    range.into_iter().find(|&g| OMN_MODULES[g] == n_odd)
}

/// Recover one Omnidirectional data-character value from its 8 element widths.
fn omn_char_value(dw: &[i32; 8], outside: bool) -> Result<i32> {
    let odd = [dw[0], dw[2], dw[4], dw[6]];
    let even = [dw[1], dw[3], dw[5], dw[7]];
    let n_odd: i32 = odd.iter().sum();
    let group = omn_group_from_n_odd(n_odd, outside)
        .ok_or_else(|| Error::undecodable("invalid DataBar data character (bad module sum)"))?;
    let max_width = OMN_WIDEST[group];
    let no_narrow = !outside;
    let v_odd = get_value(&odd, max_width, no_narrow);
    let v_even = get_value(&even, 9 - max_width, !no_narrow);
    let t = OMN_T_EVEN_ODD[group] as i64;
    let v = if outside {
        v_odd * t + v_even
    } else {
        v_even * t + v_odd
    };
    Ok(OMN_G_SUM[group] + v as i32)
}

/// Match a 5-element pattern against the Omnidirectional finder table.
fn match_omn_finder(pat: &[i32; 5]) -> Option<usize> {
    OMN_FINDER
        .iter()
        .position(|f| f.iter().zip(pat).all(|(&a, &b)| a as i32 == b))
}

fn decode_omni(tw: &[i32]) -> Result<Symbol> {
    // Reconstruct the four data characters (some stored reversed) and finders.
    let mut dw = [[0i32; 8]; 4];
    for i in 0..8 {
        dw[0][i] = tw[i + 2];
        dw[1][7 - i] = tw[i + 15];
        dw[3][i] = tw[i + 23];
        dw[2][7 - i] = tw[i + 36];
    }
    let finder_left: [i32; 5] = tw[10..15].try_into().unwrap();
    let mut finder_right: [i32; 5] = tw[31..36].try_into().unwrap();
    finder_right.reverse();

    let c_left = match_omn_finder(&finder_left)
        .ok_or_else(|| Error::undecodable("unrecognized DataBar left finder pattern"))?;
    let c_right = match_omn_finder(&finder_right)
        .ok_or_else(|| Error::undecodable("unrecognized DataBar right finder pattern"))?;

    let dc = [
        omn_char_value(&dw[0], true)?,
        omn_char_value(&dw[1], false)?,
        omn_char_value(&dw[2], true)?,
        omn_char_value(&dw[3], false)?,
    ];

    // Re-derive the checksum and confirm it selects the observed finders.
    let mut checksum = 0i32;
    for (i, dw_i) in dw.iter().enumerate() {
        for (j, &w) in dw_i.iter().enumerate() {
            checksum += OMN_CHECK_WEIGHT[i][j] * w;
        }
    }
    checksum %= 79;
    if checksum >= 8 {
        checksum += 1;
    }
    if checksum >= 72 {
        checksum += 1;
    }
    if (checksum / 9) as usize != c_left || (checksum % 9) as usize != c_right {
        return Err(Error::undecodable(
            "DataBar checksum does not match finder patterns",
        ));
    }

    let left_pair = dc[0] as u64 * 1597 + dc[1] as u64;
    let right_pair = dc[2] as u64 * 1597 + dc[3] as u64;
    let val = left_pair * 4_537_077 + right_pair;

    Ok(gtin_symbol(
        Symbology::DataBarOmni,
        DataBarVariant::Omni,
        val,
    ))
}

// ======== Limited ========

/// Recover one Limited pair value from its 14 element widths.
fn ltd_pair_value(pw: &[i32; 14]) -> Result<i64> {
    let odd = [pw[0], pw[2], pw[4], pw[6], pw[8], pw[10], pw[12]];
    let even = [pw[1], pw[3], pw[5], pw[7], pw[9], pw[11], pw[13]];
    let n_odd: i32 = odd.iter().sum();
    let group = LTD_MODULES
        .iter()
        .position(|&m| m == n_odd)
        .ok_or_else(|| Error::undecodable("invalid DataBar Limited character (bad module sum)"))?;
    let max_width = LTD_WIDEST[group];
    let odd_val = get_value(&odd, max_width, false);
    let even_val = get_value(&even, 9 - max_width, true);
    Ok(LTD_G_SUM[group] as i64 + odd_val * LTD_T_EVEN[group] as i64 + even_val)
}

fn decode_limited(tw: &[i32]) -> Result<Symbol> {
    let pw0: [i32; 14] = tw[2..16].try_into().unwrap();
    let finder: [i32; 14] = tw[16..30].try_into().unwrap();
    let pw1: [i32; 14] = tw[30..44].try_into().unwrap();

    let checksum_index = LTD_FINDER
        .iter()
        .position(|f| f.iter().zip(&finder).all(|(&a, &b)| a as i32 == b))
        .ok_or_else(|| Error::undecodable("unrecognized DataBar Limited finder pattern"))?;

    let pair0 = ltd_pair_value(&pw0)?;
    let pair1 = ltd_pair_value(&pw1)?;

    // Re-derive and verify the checksum.
    let mut checksum = 0i32;
    for i in 0..14 {
        checksum += LTD_CHECK_WEIGHT[0][i] * pw0[i];
        checksum += LTD_CHECK_WEIGHT[1][i] * pw1[i];
    }
    checksum %= 89;
    if checksum as usize != checksum_index {
        return Err(Error::undecodable(
            "DataBar Limited checksum does not match finder pattern",
        ));
    }

    let val = pair0 as u64 * 2_013_571 + pair1 as u64;

    Ok(gtin_symbol(
        Symbology::DataBarLimited,
        DataBarVariant::Limited,
        val,
    ))
}

/// Assemble a decoded symbol from a recovered GTIN value.
fn gtin_symbol(symbology: Symbology, variant: DataBarVariant, val: u64) -> Symbol {
    let gtin = format_gtin(val);
    Symbol::new(
        symbology,
        vec![Segment::numeric(gtin.to_vec())],
        SymbolMeta::DataBar(DataBarMeta::new(variant)),
    )
}
