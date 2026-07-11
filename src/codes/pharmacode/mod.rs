//! Pharmacode (Laetus) — one-track and two-track.
//!
//! # One-track ([`Symbology::Pharmacode`])
//!
//! Encodes a single integer `3..=131070` as a run of thin/thick bars separated by
//! narrow spaces. Reading right-to-left, bar position `n` contributes `2^n` (thin) or
//! `2·2^n` (thick); equivalently each value maps to a bijective base-2 numeral with
//! digits {1, 2}. A thin bar is one module wide, a thick bar is [`WIDE`] modules.
//!
//! # Two-track ([`Symbology::PharmacodeTwoTrack`])
//!
//! Two-track Pharmacode is *height-modulated*: it encodes an integer `4..=64570080`
//! as a bijective base-3 numeral whose digits {1, 2, 3} are drawn as a bottom-half,
//! top-half or full bar. A single-row [`LinearPattern`] (one bit per module) cannot
//! carry three bar heights, so this crate renders each ternary digit *d* as a bar of
//! *d* modules separated by single-module spaces — a lossless linearization, not a
//! directly scannable two-track image. This representation choice is documented here
//! and reflected in [`Symbol::symbology`].
//!
//! Neither variant carries a check digit; the value is self-describing, so
//! [`PharmacodeMeta`] holds no state.
//!
//! # Validation
//!
//! One-track is hand-verified against the documented bounds: `3` → two thin bars and
//! `131070` → sixteen thick bars (Wikipedia). Two-track is hand-verified against its
//! bounds: `4` → `[1,1]` and `64570080` → sixteen `3`s.

use crate::error::{Error, Result};
use crate::output::{Encoding, LinearPattern};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use crate::traits::{Decode, Encode};

/// Module width of a thick (one-track) bar.
const WIDE: u32 = 3;
/// Quiet-zone width in narrow modules on each side.
const QUIET_ZONE: usize = 10;

/// Inclusive value range of one-track Pharmacode.
const ONE_TRACK_RANGE: std::ops::RangeInclusive<u32> = 3..=131070;
/// Inclusive value range of two-track Pharmacode.
const TWO_TRACK_RANGE: std::ops::RangeInclusive<u32> = 4..=64570080;

/// Parameters required to re-encode a Pharmacode symbol identically. The value lives
/// in the numeric segment and the track count in [`Symbol::symbology`]; no extra state
/// is needed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PharmacodeMeta;

/// Parse the numeric payload as a `u32`.
fn parse_value(payload: &[u8]) -> Result<u32> {
    let text = std::str::from_utf8(payload)
        .map_err(|_| Error::invalid_data("Pharmacode payload not UTF-8"))?;
    text.parse::<u32>()
        .map_err(|_| Error::invalid_data("Pharmacode payload is not an integer"))
}

/// Append `width` copies of `bar` to `modules`.
fn push_run(modules: &mut Vec<bool>, bar: bool, width: u32) {
    modules.extend(std::iter::repeat_n(bar, width as usize));
}

/// Render a left-to-right sequence of bar widths, joined by single-module spaces.
fn render_bars(bar_widths: &[u32]) -> Vec<bool> {
    let mut modules = Vec::new();
    for (i, &w) in bar_widths.iter().enumerate() {
        if i > 0 {
            push_run(&mut modules, false, 1);
        }
        push_run(&mut modules, true, w);
    }
    modules
}

/// Run-length encode into element widths, and return only the bar widths (the
/// spaces, at odd indices, must all be narrow).
fn bar_widths(modules: &[bool]) -> Result<Vec<u32>> {
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
    // Bars at even indices, spaces (all narrow) at odd indices.
    if runs.len() % 2 == 0 {
        return Err(Error::undecodable("Pharmacode must end with a bar"));
    }
    for (i, &w) in runs.iter().enumerate() {
        if i % 2 == 1 && w != 1 {
            return Err(Error::undecodable("Pharmacode space not narrow"));
        }
    }
    Ok(runs.iter().step_by(2).copied().collect())
}

// ---------- one-track ----------

/// Left-to-right bar widths for a one-track value (thin = 1, thick = [`WIDE`]).
fn one_track_bars(mut n: u32) -> Vec<u32> {
    let mut bars = Vec::new(); // least-significant first
    while n > 0 {
        if n % 2 == 1 {
            bars.push(1); // thin, value 1
            n = (n - 1) / 2;
        } else {
            bars.push(WIDE); // thick, value 2
            n = (n - 2) / 2;
        }
    }
    bars.reverse(); // most-significant (leftmost) first
    bars
}

/// Recover a one-track value from its left-to-right bar widths.
fn one_track_value(bars: &[u32]) -> Result<u32> {
    let mut value: u32 = 0;
    for &w in bars {
        let digit = match w {
            1 => 1,
            _ if w > 1 => 2,
            _ => return Err(Error::undecodable("invalid Pharmacode bar")),
        };
        value = value
            .checked_mul(2)
            .and_then(|v| v.checked_add(digit))
            .ok_or_else(|| Error::undecodable("Pharmacode value overflow"))?;
    }
    Ok(value)
}

// ---------- two-track ----------

/// Left-to-right ternary digits (bar widths 1/2/3) for a two-track value.
fn two_track_bars(mut n: u32) -> Vec<u32> {
    let mut digits = Vec::new();
    while n > 0 {
        let r = n % 3;
        if r == 0 {
            digits.push(3);
            n = n / 3 - 1;
        } else {
            digits.push(r);
            n /= 3;
        }
    }
    digits.reverse();
    digits
}

/// Recover a two-track value from its left-to-right bar widths (1/2/3).
fn two_track_value(bars: &[u32]) -> Result<u32> {
    let mut value: u32 = 0;
    for &w in bars {
        if !(1..=3).contains(&w) {
            return Err(Error::undecodable("invalid two-track bar width"));
        }
        value = value
            .checked_mul(3)
            .and_then(|v| v.checked_add(w))
            .ok_or_else(|| Error::undecodable("two-track value overflow"))?;
    }
    Ok(value)
}

/// Pharmacode encoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct PharmacodeEncoder;

impl PharmacodeEncoder {
    /// A new encoder.
    pub fn new() -> Self {
        Self
    }

    /// Build a one-track symbol for `value` (`3..=131070`).
    pub fn build(&self, value: u32) -> Result<Symbol> {
        if !ONE_TRACK_RANGE.contains(&value) {
            return Err(Error::capacity(
                "one-track Pharmacode value out of range 3..=131070",
            ));
        }
        Ok(Symbol::new(
            Symbology::Pharmacode,
            vec![Segment::numeric(value.to_string().into_bytes())],
            SymbolMeta::Pharmacode(PharmacodeMeta),
        ))
    }

    /// Build a two-track symbol for `value` (`4..=64570080`).
    pub fn build_two_track(&self, value: u32) -> Result<Symbol> {
        if !TWO_TRACK_RANGE.contains(&value) {
            return Err(Error::capacity(
                "two-track Pharmacode value out of range 4..=64570080",
            ));
        }
        Ok(Symbol::new(
            Symbology::PharmacodeTwoTrack,
            vec![Segment::numeric(value.to_string().into_bytes())],
            SymbolMeta::Pharmacode(PharmacodeMeta),
        ))
    }
}

impl Encode for PharmacodeEncoder {
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        let value = parse_value(&symbol.payload_bytes())?;
        let bars = match symbol.symbology {
            Symbology::Pharmacode => {
                if !ONE_TRACK_RANGE.contains(&value) {
                    return Err(Error::capacity("one-track Pharmacode value out of range"));
                }
                one_track_bars(value)
            }
            Symbology::PharmacodeTwoTrack => {
                if !TWO_TRACK_RANGE.contains(&value) {
                    return Err(Error::capacity("two-track Pharmacode value out of range"));
                }
                two_track_bars(value)
            }
            _ => return Err(Error::invalid_parameter("not a Pharmacode symbology")),
        };
        if !matches!(symbol.meta, SymbolMeta::Pharmacode(_)) {
            return Err(Error::invalid_parameter(
                "Pharmacode symbol missing PharmacodeMeta",
            ));
        }
        Ok(Encoding::Linear(LinearPattern {
            modules: render_bars(&bars),
            quiet_zone: QUIET_ZONE,
        }))
    }
}

/// Pharmacode decoder.
#[derive(Debug, Default, Clone, Copy)]
pub struct PharmacodeDecoder {
    two_track: bool,
}

impl PharmacodeDecoder {
    /// A new one-track decoder.
    pub fn new() -> Self {
        Self { two_track: false }
    }

    /// A two-track decoder (interprets bar widths 1/2/3 as ternary digits).
    pub fn two_track() -> Self {
        Self { two_track: true }
    }
}

impl Decode for PharmacodeDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        let pattern = match encoding {
            Encoding::Linear(p) => p,
            Encoding::Matrix(_) => {
                return Err(Error::invalid_parameter(
                    "Pharmacode expects a linear pattern",
                ));
            }
        };
        let bars = bar_widths(&pattern.modules)?;
        let (value, symbology, range) = if self.two_track {
            (
                two_track_value(&bars)?,
                Symbology::PharmacodeTwoTrack,
                &TWO_TRACK_RANGE,
            )
        } else {
            (
                one_track_value(&bars)?,
                Symbology::Pharmacode,
                &ONE_TRACK_RANGE,
            )
        };
        if !range.contains(&value) {
            return Err(Error::undecodable("Pharmacode value out of range"));
        }
        Ok(Symbol::new(
            symbology,
            vec![Segment::numeric(value.to_string().into_bytes())],
            SymbolMeta::Pharmacode(PharmacodeMeta),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_track_bounds() {
        // Documented bounds: 3 = two thin bars, 131070 = sixteen thick bars.
        assert_eq!(one_track_bars(3), vec![1, 1]);
        assert_eq!(one_track_bars(131070), vec![WIDE; 16]);
    }

    #[test]
    fn two_track_bounds() {
        assert_eq!(two_track_bars(4), vec![1, 1]);
        assert_eq!(two_track_bars(64570080), vec![3; 16]);
    }

    #[test]
    fn one_track_roundtrip() {
        let enc = PharmacodeEncoder::new();
        for value in [3u32, 4, 42, 1234, 65535, 131070] {
            let sym = enc.build(value).unwrap();
            let encoding = enc.encode(&sym).unwrap();
            let back = PharmacodeDecoder::new().decode(&encoding).unwrap();
            assert_eq!(back.segments, sym.segments);
            assert_eq!(enc.encode(&back).unwrap(), encoding);
        }
    }

    #[test]
    fn two_track_roundtrip() {
        let enc = PharmacodeEncoder::new();
        for value in [4u32, 5, 100, 117480, 64570080] {
            let sym = enc.build_two_track(value).unwrap();
            let encoding = enc.encode(&sym).unwrap();
            let back = PharmacodeDecoder::two_track().decode(&encoding).unwrap();
            assert_eq!(back.symbology, Symbology::PharmacodeTwoTrack);
            assert_eq!(back.segments, sym.segments);
            assert_eq!(enc.encode(&back).unwrap(), encoding);
        }
    }

    #[test]
    fn range_checks() {
        let enc = PharmacodeEncoder::new();
        assert!(enc.build(2).is_err());
        assert!(enc.build(131071).is_err());
        assert!(enc.build_two_track(3).is_err());
        assert!(enc.build_two_track(64570081).is_err());
    }
}
