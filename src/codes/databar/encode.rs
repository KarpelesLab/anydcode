//! GS1 DataBar encoding: [`Symbol`] -> [`LinearPattern`].

use super::tables::{
    LTD_CHECK_WEIGHT, LTD_FINDER, LTD_G_SUM, LTD_MODULES, LTD_T_EVEN, LTD_WIDEST, OMN_CHECK_WEIGHT,
    OMN_FINDER, OMN_G_SUM, OMN_MODULES, OMN_T_EVEN_ODD, OMN_WIDEST, gtin_check_digit,
};
use super::widths::interleave;
use super::{DataBarMeta, DataBarVariant};
use crate::error::{Error, Result};
use crate::output::{Encoding, LinearPattern};
use crate::segment::{Mode, Segment};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::Encode;

/// Light-module margin the encoder frames each symbol with. DataBar does not
/// mandate a specific quiet zone; a 1X margin is used consistently so that
/// re-encoding is stable.
const QUIET_ZONE: usize = 1;

/// Omnidirectional: value multiplier splitting a 14-digit GTIN into two pairs.
const OMN_PAIR_MUL: u64 = 4_537_077;
/// Omnidirectional: multiplier splitting a pair into two data characters.
const OMN_CHAR_MUL: i32 = 1597;
/// Limited: value multiplier splitting the GTIN into two pair values.
const LTD_PAIR_MUL: u64 = 2_013_571;

/// GS1 DataBar encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DataBarEncoder;

impl DataBarEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        DataBarEncoder
    }

    /// Build a GS1 DataBar Omnidirectional symbol from a GTIN (13 or 14 digits).
    ///
    /// A 13-digit body has its GS1 check digit appended; a 14-digit GTIN is
    /// validated against its check digit. The stored symbol carries the canonical
    /// 14-digit GTIN as a single numeric segment.
    pub fn build_omni(&self, gtin: &[u8]) -> Result<Symbol> {
        let canonical = normalize_gtin(gtin, DataBarVariant::Omni)?;
        Ok(Symbol::new(
            Symbology::DataBarOmni,
            vec![Segment::numeric(canonical.to_vec())],
            SymbolMeta::DataBar(DataBarMeta::new(DataBarVariant::Omni)),
        ))
    }

    /// Build a GS1 DataBar Limited symbol from a GTIN (13 or 14 digits).
    ///
    /// Limited only encodes GTINs whose indicator (leading) digit is 0 or 1.
    pub fn build_limited(&self, gtin: &[u8]) -> Result<Symbol> {
        let canonical = normalize_gtin(gtin, DataBarVariant::Limited)?;
        Ok(Symbol::new(
            Symbology::DataBarLimited,
            vec![Segment::numeric(canonical.to_vec())],
            SymbolMeta::DataBar(DataBarMeta::new(DataBarVariant::Limited)),
        ))
    }

    /// Build a GS1 DataBar Expanded symbol from a reduced GS1 element string.
    ///
    /// `gs1` is the Application Identifier data in *reduced* form: AI numbers and
    /// their values concatenated, with a GS1 separator (FNC1, byte `0x1D`) after a
    /// variable-length AI's value when another AI follows. The bytes are validated
    /// by running the encodation and stored verbatim as a single [`Segment::byte`].
    pub fn build_expanded(&self, gs1: &[u8]) -> Result<Symbol> {
        // Validate up front by attempting the encodation.
        super::expanded::element_widths(gs1)?;
        Ok(Symbol::new(
            Symbology::DataBarExpanded,
            vec![Segment::byte(gs1.to_vec())],
            SymbolMeta::DataBar(DataBarMeta::new(DataBarVariant::Expanded)),
        ))
    }
}

impl Encode for DataBarEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        match symbol.symbology {
            Symbology::DataBarOmni => {
                let val = gtin_value(symbol, DataBarVariant::Omni)?;
                Ok(linear(&omn_total_widths(val)))
            }
            Symbology::DataBarLimited => {
                let val = gtin_value(symbol, DataBarVariant::Limited)?;
                Ok(linear(&ltd_total_widths(val)))
            }
            // Expanded is driven by its metadata: a symbol is encodable only when it
            // carries the canonical Expanded metadata that `build_expanded` and
            // `decode` produce. A DataBarExpanded symbology tagged with any other
            // DataBar variant is a malformed/unsupported combination.
            Symbology::DataBarExpanded
                if matches!(
                    symbol.meta,
                    SymbolMeta::DataBar(DataBarMeta {
                        variant: DataBarVariant::Expanded
                    })
                ) =>
            {
                super::expanded::encode(symbol)
            }
            Symbology::DataBarExpanded
            | Symbology::DataBarStacked
            | Symbology::DataBarStackedOmni
            | Symbology::DataBarExpandedStacked => Err(Error::Unsupported {
                what: "GS1 DataBar stacked encoding, or Expanded without Expanded metadata",
            }),
            _ => Err(Error::invalid_parameter(
                "DataBarEncoder given a non-DataBar symbol",
            )),
        }
    }
}

/// Expand an element-width sequence into modules, starting with a light element
/// and alternating (ISO/IEC 24724 symbols begin with a light guard element).
fn expand(widths: &[i32]) -> Vec<bool> {
    let mut modules = Vec::with_capacity(widths.iter().map(|&w| w.max(0) as usize).sum());
    let mut dark = false;
    for &w in widths {
        for _ in 0..w {
            modules.push(dark);
        }
        dark = !dark;
    }
    modules
}

/// Wrap an element-width sequence as a linear encoding.
pub(super) fn linear(widths: &[i32]) -> Encoding {
    Encoding::Linear(LinearPattern {
        modules: expand(widths),
        quiet_zone: QUIET_ZONE,
    })
}

/// Read the canonical GTIN value (its 13-digit body as an integer) from a symbol.
fn gtin_value(symbol: &Symbol, variant: DataBarVariant) -> Result<u64> {
    let digits = extract_digits(&symbol.segments)?;
    let canonical = normalize_gtin(&digits, variant)?;
    Ok(body_value(&canonical))
}

/// Collect the digit bytes from a symbol's numeric segments.
fn extract_digits(segments: &[Segment]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for seg in segments {
        match seg.mode {
            Mode::Numeric => {
                for &b in &seg.data {
                    if !b.is_ascii_digit() {
                        return Err(Error::invalid_data("DataBar payload must be digits"));
                    }
                    out.push(b);
                }
            }
            _ => {
                return Err(Error::invalid_data(
                    "DataBar payload must be a numeric GTIN",
                ));
            }
        }
    }
    Ok(out)
}

/// Validate `digits` (13 or 14) and return the canonical 14-digit GTIN.
fn normalize_gtin(digits: &[u8], variant: DataBarVariant) -> Result<[u8; 14]> {
    if digits.len() != 13 && digits.len() != 14 {
        return Err(Error::capacity(format!(
            "DataBar GTIN must be 13 or 14 digits, got {}",
            digits.len()
        )));
    }
    if let Some(&b) = digits.iter().find(|b| !b.is_ascii_digit()) {
        return Err(Error::invalid_data(format!(
            "DataBar GTIN contains non-digit byte {b:#04x}"
        )));
    }
    let mut body = [0u8; 13];
    body.copy_from_slice(&digits[..13]);
    let check = gtin_check_digit(&body);
    if digits.len() == 14 && digits[13] != check {
        return Err(Error::invalid_data(format!(
            "DataBar GTIN check digit mismatch: expected '{}', got '{}'",
            check as char, digits[13] as char
        )));
    }
    if variant == DataBarVariant::Limited && body[0] != b'0' && body[0] != b'1' {
        return Err(Error::capacity(
            "DataBar Limited requires a GTIN indicator digit of 0 or 1",
        ));
    }
    let mut canonical = [0u8; 14];
    canonical[..13].copy_from_slice(&body);
    canonical[13] = check;
    Ok(canonical)
}

/// The integer value of a canonical GTIN's 13-digit body.
fn body_value(canonical: &[u8; 14]) -> u64 {
    canonical[..13]
        .iter()
        .fold(0u64, |acc, &b| acc * 10 + (b - b'0') as u64)
}

// ======== Omnidirectional ========

/// Select the Omnidirectional group for a data character value.
///
/// `outside` chooses the outer (groups 0-4) versus inner (groups 5-8) subset.
fn omn_group(val: i32, outside: bool) -> usize {
    let end = if outside { 4 } else { 8 };
    let mut i = if outside { 0 } else { 5 };
    while i < end {
        if val < OMN_G_SUM[i + 1] {
            return i;
        }
        i += 1;
    }
    i
}

/// The 46 element widths of a DataBar Omnidirectional symbol for GTIN value `val`.
///
/// Mirrors `zint_dbar_omn` (zint `rss.c`): four data characters (outer, inner,
/// inner, outer), two finder patterns chosen by the checksum, and guards.
pub(super) fn omn_total_widths(val: u64) -> [i32; 46] {
    let left_pair = (val / OMN_PAIR_MUL) as i32;
    let right_pair = (val % OMN_PAIR_MUL) as i32;
    let dc = [
        left_pair / OMN_CHAR_MUL,
        left_pair % OMN_CHAR_MUL,
        right_pair / OMN_CHAR_MUL,
        right_pair % OMN_CHAR_MUL,
    ];

    let mut dw = [[0i32; 8]; 4];
    for (i, dw_i) in dw.iter_mut().enumerate() {
        let outside = i % 2 == 0;
        let group = omn_group(dc[i], outside);
        let v = dc[i] - OMN_G_SUM[group];
        let v_div = v / OMN_T_EVEN_ODD[group];
        let v_mod = v % OMN_T_EVEN_ODD[group];
        let (v_odd, v_even) = if outside {
            (v_div, v_mod)
        } else {
            (v_mod, v_div)
        };
        let w = interleave(
            v_odd as i64,
            v_even as i64,
            OMN_MODULES[group],
            OMN_MODULES[group + 9],
            4,
            OMN_WIDEST[group],
            !outside,
        );
        dw_i.copy_from_slice(&w);
    }

    // Checksum -> two finder-pattern indices (Table 5).
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
    let c_left = (checksum / 9) as usize;
    let c_right = (checksum % 9) as usize;

    let mut tw = [0i32; 46];
    tw[0] = 1;
    tw[1] = 1;
    tw[44] = 1;
    tw[45] = 1;
    for i in 0..8 {
        tw[i + 2] = dw[0][i];
        tw[i + 15] = dw[1][7 - i];
        tw[i + 23] = dw[3][i];
        tw[i + 36] = dw[2][7 - i];
    }
    for i in 0..5 {
        tw[i + 10] = OMN_FINDER[c_left][i] as i32;
        tw[i + 31] = OMN_FINDER[c_right][4 - i] as i32;
    }
    tw
}

// ======== Limited ========

/// Select the Limited group for a pair value, returning `(group, remainder)`.
fn ltd_group(val: i32) -> (usize, i32) {
    for i in (1..=6).rev() {
        if val >= LTD_G_SUM[i] {
            return (i, val - LTD_G_SUM[i]);
        }
    }
    (0, val)
}

/// The 47 element widths of a DataBar Limited symbol for GTIN value `val`.
///
/// Mirrors `zint_dbar_ltd` (zint `rss.c`): two pair characters flanking a
/// checksum-selected finder pattern, plus guards (including the wide right guard).
pub(super) fn ltd_total_widths(val: u64) -> [i32; 47] {
    let pair_vals = [(val / LTD_PAIR_MUL) as i32, (val % LTD_PAIR_MUL) as i32];

    let mut pw = [[0i32; 14]; 2];
    for (i, pw_i) in pw.iter_mut().enumerate() {
        let (group, v) = ltd_group(pair_vals[i]);
        let odd = v / LTD_T_EVEN[group];
        let even = v % LTD_T_EVEN[group];
        let w = interleave(
            odd as i64,
            even as i64,
            LTD_MODULES[group],
            26 - LTD_MODULES[group],
            7,
            LTD_WIDEST[group],
            false,
        );
        pw_i.copy_from_slice(&w);
    }

    let mut checksum = 0i32;
    for i in 0..14 {
        checksum += LTD_CHECK_WEIGHT[0][i] * pw[0][i];
        checksum += LTD_CHECK_WEIGHT[1][i] * pw[1][i];
    }
    checksum %= 89;
    let cfp = &LTD_FINDER[checksum as usize];

    let mut tw = [0i32; 47];
    tw[0] = 1;
    tw[1] = 1;
    tw[44] = 1;
    tw[45] = 1;
    tw[46] = 5;
    for i in 0..14 {
        tw[i + 2] = pw[0][i];
        tw[i + 16] = cfp[i] as i32;
        tw[i + 30] = pw[1][i];
    }
    tw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omni_symbol_is_96_modules() {
        let Encoding::Linear(p) = linear(&omn_total_widths(0)) else {
            panic!("expected linear");
        };
        assert_eq!(p.modules.len(), 96);
    }

    #[test]
    fn limited_symbol_is_79_modules() {
        let Encoding::Linear(p) = linear(&ltd_total_widths(0)) else {
            panic!("expected linear");
        };
        assert_eq!(p.modules.len(), 79);
    }

    #[test]
    fn check_digit_matches_known_gtins() {
        // Independently computed GS1 mod-10 check digits.
        assert_eq!(gtin_check_digit(b"2001234567890"), b'9');
        assert_eq!(gtin_check_digit(b"0095011015300"), b'7');
        assert_eq!(gtin_check_digit(b"0000000000000"), b'0');
    }
}
