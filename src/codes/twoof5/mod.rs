//! The non-interleaved 2-of-5 family: Standard/Industrial, IATA and Matrix 2 of 5.
//!
//! All three are discrete numeric symbologies sharing the same five-element
//! "two-of-five" digit patterns (exactly two wide elements, weighted 1-2-4-7-parity).
//! They differ only in layout and start/stop:
//!
//! - **Standard / Industrial** ([`Symbology::Std2of5`]): data in the bars only; each
//!   of the five bars is followed by a narrow space. Start `WnWnn`, stop `WnnnW`.
//! - **IATA** ([`Symbology::Iata2of5`]): identical digit layout to Industrial; start
//!   `nnnn`, stop `Wnn`.
//! - **Matrix** ([`Symbology::Matrix2of5`]): data carried in both bars and spaces
//!   (five elements + a trailing narrow space per digit). Start/stop use a
//!   quadruple-width bar.
//!
//! The variant is fully determined by the start/stop patterns, so it is recovered on
//! decode and stored in [`Symbol::symbology`]; [`TwoOf5Meta`] carries no extra state.
//!
//! # Validation
//!
//! Digit tables and start/stop constants match the reference implementation
//! [zint](https://github.com/zint/zint) (`C25MatrixTable`, `C25IndustTable`,
//! `C25*StartStop`).

use crate::error::{Error, Result};
use crate::output::{Encoding, LinearPattern};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::{Decode, Encode};

/// Quiet-zone width in narrow modules on each side.
const QUIET_ZONE: usize = 10;

/// Five-element two-of-5 bar widths per digit (wide = 3, narrow = 1).
///
/// Matches zint's `C25InterTable`; the wide positions follow the 1-2-4-7-parity
/// weighting shared by the whole 2-of-5 family.
const BAR_WIDTHS: [[u32; 5]; 10] = [
    [1, 1, 3, 3, 1],
    [3, 1, 1, 1, 3],
    [1, 3, 1, 1, 3],
    [3, 3, 1, 1, 1],
    [1, 1, 3, 1, 3],
    [3, 1, 3, 1, 1],
    [1, 3, 3, 1, 1],
    [1, 1, 1, 3, 3],
    [3, 1, 1, 3, 1],
    [1, 3, 1, 3, 1],
];

/// Which 2-of-5 dialect a symbol uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    Standard,
    Iata,
    Matrix,
}

impl Variant {
    fn from_symbology(s: Symbology) -> Result<Self> {
        match s {
            Symbology::Std2of5 => Ok(Variant::Standard),
            Symbology::Iata2of5 => Ok(Variant::Iata),
            Symbology::Matrix2of5 => Ok(Variant::Matrix),
            _ => Err(Error::invalid_parameter("not a 2-of-5 symbology")),
        }
    }

    fn symbology(self) -> Symbology {
        match self {
            Variant::Standard => Symbology::Std2of5,
            Variant::Iata => Symbology::Iata2of5,
            Variant::Matrix => Symbology::Matrix2of5,
        }
    }

    /// Start and stop element-width sequences (alternating bar, space, ...).
    fn start_stop(self) -> (&'static [u32], &'static [u32]) {
        match self {
            // zint C25IndustStartStop
            Variant::Standard => (&[3, 1, 3, 1, 1, 1], &[3, 1, 1, 1, 3]),
            // zint C25IataLogicStartStop
            Variant::Iata => (&[1, 1, 1, 1], &[3, 1, 1]),
            // zint C25MatrixStartStop
            Variant::Matrix => (&[4, 1, 1, 1, 1, 1], &[4, 1, 1, 1, 1]),
        }
    }

    /// Element-width sequence for one digit (alternating bar, space, ...).
    fn digit_widths(self, d: usize) -> Vec<u32> {
        let bars = &BAR_WIDTHS[d];
        match self {
            // Five bars, each followed by a narrow space.
            Variant::Standard | Variant::Iata => {
                let mut v = Vec::with_capacity(10);
                for &b in bars {
                    v.push(b);
                    v.push(1);
                }
                v
            }
            // Five elements (bar,space,bar,space,bar) + trailing narrow space.
            Variant::Matrix => vec![bars[0], bars[1], bars[2], bars[3], bars[4], 1],
        }
    }
}

/// Parameters required to re-encode a 2-of-5 symbol identically. The dialect lives in
/// [`Symbol::symbology`]; no additional state is needed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TwoOf5Meta;

/// Validate that every byte is an ASCII digit.
fn ensure_digits(digits: &[u8]) -> Result<()> {
    if digits.is_empty() {
        return Err(Error::invalid_data("2-of-5 payload is empty"));
    }
    if digits.iter().all(u8::is_ascii_digit) {
        Ok(())
    } else {
        Err(Error::invalid_data("2-of-5 payload must be ASCII digits"))
    }
}

/// Append `width` copies of `bar` to `modules`.
fn push_run(modules: &mut Vec<bool>, bar: bool, width: u32) {
    modules.extend(std::iter::repeat_n(bar, width as usize));
}

/// Render an alternating (bar, space, ...) width sequence into modules.
fn render_widths(modules: &mut Vec<bool>, widths: &[u32]) {
    for (i, &w) in widths.iter().enumerate() {
        push_run(modules, i % 2 == 0, w);
    }
}

/// 2-of-5 encoder (Standard / IATA / Matrix).
#[derive(Debug, Default, Clone, Copy)]
pub struct TwoOf5Encoder;

impl TwoOf5Encoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

    /// Build a symbol for `symbology` (one of the three 2-of-5 dialects) from `digits`.
    pub fn build(&self, symbology: Symbology, digits: &[u8]) -> Result<Symbol> {
        Variant::from_symbology(symbology)?;
        ensure_digits(digits)?;
        Ok(Symbol::new(
            symbology,
            vec![Segment::numeric(digits.to_vec())],
            SymbolMeta::TwoOf5(TwoOf5Meta),
        ))
    }
}

impl Encode for TwoOf5Encoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        let variant = Variant::from_symbology(symbol.symbology)?;
        if !matches!(symbol.meta, SymbolMeta::TwoOf5(_)) {
            return Err(Error::invalid_parameter("2-of-5 symbol missing TwoOf5Meta"));
        }
        let digits = symbol.payload_bytes();
        ensure_digits(&digits)?;

        let (start, stop) = variant.start_stop();
        let mut modules = Vec::new();
        render_widths(&mut modules, start);
        for &d in &digits {
            render_widths(&mut modules, &variant.digit_widths((d - b'0') as usize));
        }
        render_widths(&mut modules, stop);

        Ok(Encoding::Linear(LinearPattern {
            modules,
            quiet_zone: QUIET_ZONE,
        }))
    }
}

/// 2-of-5 decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct TwoOf5Decoder;

impl TwoOf5Decoder {
    /// A new decoder.
    pub fn new() -> Self {
        Self
    }
}

/// Run-length encode `modules` into element widths, starting with a bar.
fn rle(modules: &[bool]) -> Result<Vec<u32>> {
    if modules.is_empty() || !modules[0] {
        return Err(Error::undecodable("linear pattern must start with a bar"));
    }
    let mut runs = Vec::new();
    let mut cur = modules[0];
    let mut len = 0u32;
    for &m in modules {
        if m == cur {
            len += 1;
        } else {
            runs.push(len);
            cur = m;
            len = 1;
        }
    }
    runs.push(len);
    Ok(runs)
}

/// Compare a run slice against an expected width pattern by wide/narrow class.
fn matches_widths(runs: &[u32], pattern: &[u32]) -> bool {
    runs.len() == pattern.len() && runs.iter().zip(pattern).all(|(&r, &p)| (r > 1) == (p > 1))
}

/// Recover a digit from its five bar widths (wide = width > 1).
fn digit_from_bars(bars: &[u32]) -> Result<u8> {
    for (d, pat) in BAR_WIDTHS.iter().enumerate() {
        if (0..5).all(|i| (bars[i] > 1) == (pat[i] > 1)) {
            return Ok(d as u8);
        }
    }
    Err(Error::undecodable("invalid 2-of-5 digit pattern"))
}

impl Decode for TwoOf5Decoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        let pattern = match encoding {
            Encoding::Linear(p) => p,
            Encoding::Matrix(_) => {
                return Err(Error::invalid_parameter("2-of-5 expects a linear pattern"));
            }
        };
        let runs = rle(&pattern.modules)?;

        // Identify the variant by its start pattern (the first `4` bar is unique to
        // Matrix; Standard vs IATA differ in the leading bar width).
        for variant in [Variant::Standard, Variant::Iata, Variant::Matrix] {
            let (start, stop) = variant.start_stop();
            if runs.len() < start.len() + stop.len() {
                continue;
            }
            // Matrix uses a quadruple-width start/stop bar; distinguish it exactly so
            // it is not confused with the wide (3) bar of the other dialects.
            if variant == Variant::Matrix && runs[0] < 4 {
                continue;
            }
            if variant != Variant::Matrix && runs[0] >= 4 {
                continue;
            }
            if !matches_widths(&runs[..start.len()], start) {
                continue;
            }
            let stop_at = runs.len() - stop.len();
            if !matches_widths(&runs[stop_at..], stop) {
                continue;
            }
            let body = &runs[start.len()..stop_at];
            let per_digit = match variant {
                Variant::Standard | Variant::Iata => 10,
                Variant::Matrix => 6,
            };
            if body.is_empty() || body.len() % per_digit != 0 {
                continue;
            }
            let mut digits = Vec::new();
            let mut ok = true;
            for chunk in body.chunks(per_digit) {
                let bars: Vec<u32> = match variant {
                    Variant::Standard | Variant::Iata => chunk.iter().step_by(2).copied().collect(),
                    Variant::Matrix => chunk[..5].to_vec(),
                };
                match digit_from_bars(&bars) {
                    Ok(d) => digits.push(b'0' + d),
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
            return Ok(Symbol::new(
                variant.symbology(),
                vec![Segment::numeric(digits)],
                SymbolMeta::TwoOf5(TwoOf5Meta),
            ));
        }
        Err(Error::undecodable("no matching 2-of-5 variant"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(sym: Symbology, digits: &[u8]) {
        let enc = TwoOf5Encoder::new();
        let symbol = enc.build(sym, digits).unwrap();
        let encoding = enc.encode(&symbol).unwrap();
        let back = TwoOf5Decoder::new().decode(&encoding).unwrap();
        assert_eq!(back.symbology, sym);
        assert_eq!(back.segments, symbol.segments);
        assert_eq!(enc.encode(&back).unwrap(), encoding);
    }

    #[test]
    fn roundtrip_all_variants() {
        for sym in [
            Symbology::Std2of5,
            Symbology::Iata2of5,
            Symbology::Matrix2of5,
        ] {
            roundtrip(sym, b"0123456789");
            roundtrip(sym, b"42");
        }
    }

    #[test]
    fn variant_is_recovered_from_start_stop() {
        // The three dialects must not be confused for one another.
        let enc = TwoOf5Encoder::new();
        for sym in [
            Symbology::Std2of5,
            Symbology::Iata2of5,
            Symbology::Matrix2of5,
        ] {
            let encoding = enc.encode(&enc.build(sym, b"1234").unwrap()).unwrap();
            assert_eq!(
                TwoOf5Decoder::new().decode(&encoding).unwrap().symbology,
                sym
            );
        }
    }
}
