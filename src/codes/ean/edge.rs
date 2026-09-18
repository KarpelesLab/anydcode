//! EAN/UPC decoding from per-scanline sub-pixel edge widths (the industry-standard
//! way real scanners read the retail family off a camera frame).
//!
//! Where [`decode`](super::decode) consumes a clean, hard-quantized [`LinearPattern`]
//! bool grid, this path consumes the raw run *widths* (in pixels) of one image scanline
//! — as produced by [`crate::scan1d::scan_edges`] — and classifies each digit from its
//! four element widths normalized to that digit's own local 7-module width. Measuring
//! the module scale per digit tolerates the slowly-varying pitch a curved surface
//! imposes, and reading widths (not a global integer grid) recovers the single-module
//! guards and elements that hard quantization merges under optical blur.
//!
//! The recovered digits, variant and check digit are assembled into exactly the
//! [`Symbol`] the structural decoder would produce for the same code, so both paths are
//! interchangeable and the lossless re-encode invariant holds.

use super::tables::{EAN13_FIRST_PARITY, check_digit, l_widths, upce_expand, upce_from_parity};
use super::{EanMeta, EanVariant};
use crate::image::GrayFrame;
use crate::scan1d::{ScanOptions, scan_edges};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use alloc::{format, string::String, vec, vec::Vec};

/// Maximum acceptable per-digit width-match score (sum of squared module-count
/// deviations over the four elements). Clean digits score ~0; this rejects noise.
const MAX_DIGIT_SCORE: f32 = 1.4;
/// Maximum relative deviation of a guard element from the guard's mean element width.
const GUARD_TOL: f32 = 0.55;
/// Light margin (in modules) required on *each* side of a symbol, per variant.
///
/// The margins are what separate a real symbol from a look-alike window inside a longer
/// run sequence, so they scale with how little internal structure the variant has to
/// vouch for itself. EAN-13/UPC-A carry three guards, a parity-encoded digit and a check
/// digit; EAN-8 drops the parity digit. UPC-E is the weak one: its start guard plus six
/// L/G digits are *exactly* the left half of an EAN-13 (every EAN-13 parity pattern is
/// a valid UPC-E one), and its `010101` end guard matches the EAN-13 centre guard plus
/// one more bar — so without a margin check, any scanline that reads only the left half
/// of an EAN-13 (a slanted line leaving the bars early) reports a phantom UPC-E one time
/// in ten. No symbol element is wider than 4 modules, so demanding more than that after
/// the end guard rejects every such interior window. The specified quiet zones are 7–11
/// modules, leaving real symbols ample slack.
const QUIET_EAN13: f32 = 3.5;
const QUIET_EAN8: f32 = 4.5;
const QUIET_UPCE: f32 = 5.0;
/// A digit's total width may deviate this much (relative) from the mean digit width of
/// its symbol. Curvature and perspective change the pitch smoothly — a few tens of
/// percent end to end — while a window over text or another symbology's bars produces
/// "digits" of wildly unequal width that per-digit normalization would otherwise hide.
const DIGIT_WIDTH_TOL: f32 = 0.45;

/// Decode every EAN/UPC main variant from one scanline's run widths, trying both scan
/// directions. Returns the assembled [`Symbol`] for the first variant that validates
/// (guards, parity and check digit), or `None`.
///
/// `edges` must span one scanline from quiet zone to quiet zone: the light margins
/// outside the first and last edge are taken as given (use [`decode_edges_within`] to
/// have them checked). A symbol found *inside* the sequence must still be bracketed by
/// light runs wide enough to be its quiet zones.
pub fn decode_edges(edges: &[f32]) -> Option<Symbol> {
    decode_edges_within(edges, f32::INFINITY, f32::INFINITY)
}

/// [`decode_edges`] with the scanline's measured outer light margins, in pixels: `lead`
/// before the first edge and `trail` after the last. A symbol that starts at the first
/// edge (or ends at the last) must find its quiet zone in that margin.
pub fn decode_edges_within(edges: &[f32], lead: f32, trail: f32) -> Option<Symbol> {
    if edges.len() < 3 {
        return None;
    }
    let forward: Vec<f32> = edges.windows(2).map(|w| w[1] - w[0]).collect();
    let mut reversed = forward.clone();
    reversed.reverse();
    for (widths, lead, trail) in [(&forward, lead, trail), (&reversed, trail, lead)] {
        if let Some(sym) = decode_widths(widths, lead, trail) {
            return Some(sym);
        }
    }
    None
}

/// Try each main variant against a run-width sequence that starts with a bar, bounded by
/// light margins of `lead` / `trail` pixels.
fn decode_widths(w: &[f32], lead: f32, trail: f32) -> Option<Symbol> {
    let m = Margins { w, lead, trail };
    let (variant, digits) = [try_ean13(&m), try_ean8(&m), try_upce(&m)]
        .into_iter()
        .flatten()
        .next()?;
    Some(Symbol::new(
        symbology_of(variant),
        vec![Segment::numeric(digits)],
        SymbolMeta::Ean(EanMeta {
            variant,
            addon: None,
        }),
    ))
}

fn symbology_of(variant: EanVariant) -> Symbology {
    match variant {
        EanVariant::Ean13 => Symbology::Ean13,
        EanVariant::Ean8 => Symbology::Ean8,
        EanVariant::UpcA => Symbology::UpcA,
        EanVariant::UpcE => Symbology::UpcE,
        EanVariant::Ean2 => Symbology::Ean2,
        EanVariant::Ean5 => Symbology::Ean5,
    }
}

fn to_ascii(vals: &[u8]) -> Vec<u8> {
    vals.iter().map(|&v| b'0' + v).collect()
}

/// Whether `n` guard elements are all within tolerance of their common mean width.
fn guard_ok(elems: &[f32]) -> bool {
    let mean = elems.iter().sum::<f32>() / elems.len() as f32;
    if mean <= 0.0 {
        return false;
    }
    elems.iter().all(|&e| (e - mean).abs() <= GUARD_TOL * mean)
}

/// Classify a 4-element digit from its widths. `left` selects the left half (L/G, parity
/// recovered) versus the right half (R). Returns `(digit, is_even, score)`; even is
/// always `false` for right digits.
fn classify(w: &[f32], left: bool) -> Option<(u8, bool, f32)> {
    let total: f32 = w.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let unit = total / 7.0;
    let norm = [w[0] / unit, w[1] / unit, w[2] / unit, w[3] / unit];
    let score_against = |pat: [u8; 4]| -> f32 {
        (0..4)
            .map(|i| {
                let d = norm[i] - pat[i] as f32;
                d * d
            })
            .sum()
    };
    let mut best: Option<(u8, bool, f32)> = None;
    for d in 0..10u8 {
        let l = l_widths(d);
        // R widths equal L widths; G widths are L reversed. Left digits try L and G;
        // right digits try only R.
        let cands: &[(bool, [u8; 4])] = if left {
            &[(false, l), (true, [l[3], l[2], l[1], l[0]])]
        } else {
            &[(false, l)]
        };
        for &(even, pat) in cands {
            let s = score_against(pat);
            if best.is_none_or(|(_, _, bs)| s < bs) {
                best = Some((d, even, s));
            }
        }
    }
    best.filter(|&(_, _, s)| s <= MAX_DIGIT_SCORE)
}

/// Whether the `n` four-run digits starting at `r[first]` all have a total width within
/// [`DIGIT_WIDTH_TOL`] of their common mean.
fn digits_even(r: &[f32], first: usize, n: usize) -> bool {
    let width = |i: usize| r[first + i * 4..first + i * 4 + 4].iter().sum::<f32>();
    let mean = (0..n).map(width).sum::<f32>() / n as f32;
    mean > 0.0 && (0..n).all(|i| (width(i) - mean).abs() <= DIGIT_WIDTH_TOL * mean)
}

/// EAN-13 / UPC-A: 59 runs (start `101`, six left digits, centre `01010`, six right
/// digits, end `101`), 95 modules.
fn try_ean13(m: &Margins<'_>) -> Option<(EanVariant, Vec<u8>)> {
    m.windows(59, 95.0, QUIET_EAN13, |r| {
        if !guard_ok(&r[0..3]) || !guard_ok(&r[27..32]) || !guard_ok(&r[56..59]) {
            return None;
        }
        if !digits_even(r, 3, 6) || !digits_even(r, 32, 6) {
            return None;
        }
        let mut left = [0u8; 6];
        let mut parity = [false; 6];
        for i in 0..6 {
            let (d, even, _) = classify(&r[3 + i * 4..7 + i * 4], true)?;
            left[i] = d;
            parity[i] = even;
        }
        let first = (0..10u8).find(|&f| EAN13_FIRST_PARITY[f as usize] == parity)?;
        let mut right = [0u8; 6];
        for i in 0..6 {
            let (d, _, _) = classify(&r[32 + i * 4..36 + i * 4], false)?;
            right[i] = d;
        }
        let mut vals = Vec::with_capacity(13);
        vals.push(first);
        vals.extend_from_slice(&left);
        vals.extend_from_slice(&right);
        if check_digit(&vals[..12]) != vals[12] {
            return None;
        }
        if first == 0 {
            Some((EanVariant::UpcA, to_ascii(&vals[1..])))
        } else {
            Some((EanVariant::Ean13, to_ascii(&vals)))
        }
    })
}

/// EAN-8: 43 runs (start `101`, four left L digits, centre `01010`, four right digits,
/// end `101`), 67 modules.
fn try_ean8(m: &Margins<'_>) -> Option<(EanVariant, Vec<u8>)> {
    m.windows(43, 67.0, QUIET_EAN8, |r| {
        if !guard_ok(&r[0..3]) || !guard_ok(&r[19..24]) || !guard_ok(&r[40..43]) {
            return None;
        }
        if !digits_even(r, 3, 4) || !digits_even(r, 24, 4) {
            return None;
        }
        let mut vals = [0u8; 8];
        for i in 0..4 {
            let (d, even, _) = classify(&r[3 + i * 4..7 + i * 4], true)?;
            if even {
                return None; // EAN-8 left digits are all odd (L).
            }
            vals[i] = d;
        }
        for i in 0..4 {
            let (d, _, _) = classify(&r[24 + i * 4..28 + i * 4], false)?;
            vals[4 + i] = d;
        }
        if check_digit(&vals[..7]) != vals[7] {
            return None;
        }
        Some((EanVariant::Ean8, to_ascii(&vals)))
    })
}

/// UPC-E: 33 runs (start `101`, six L/G digits carrying number-system+check parity, end
/// guard `010101`), 51 modules.
fn try_upce(m: &Margins<'_>) -> Option<(EanVariant, Vec<u8>)> {
    m.windows(33, 51.0, QUIET_UPCE, |r| {
        if !guard_ok(&r[0..3]) || !guard_ok(&r[27..33]) {
            return None;
        }
        if !digits_even(r, 3, 6) {
            return None;
        }
        let mut payload = [0u8; 6];
        let mut parity = [false; 6];
        for i in 0..6 {
            let (d, even, _) = classify(&r[3 + i * 4..7 + i * 4], true)?;
            payload[i] = d;
            parity[i] = even;
        }
        let (ns, check) = upce_from_parity(&parity)?;
        if check_digit(&upce_expand(ns, &payload)) != check {
            return None;
        }
        let mut vals = Vec::with_capacity(8);
        vals.push(ns);
        vals.extend_from_slice(&payload);
        vals.push(check);
        Some((EanVariant::UpcE, to_ascii(&vals)))
    })
}

/// One scanline's run widths (starting with a bar) and its outer light margins.
struct Margins<'a> {
    w: &'a [f32],
    lead: f32,
    trail: f32,
}

impl Margins<'_> {
    /// Try `f` on every `len`-run window that starts on a bar and is bracketed by light
    /// runs of at least `quiet` modules (the window spans `modules` modules), returning
    /// the first success. Searching the whole line, not just its start, is what reads a
    /// symbol with print or a label edge beside it in the crop; the quiet-zone bracket
    /// is what keeps that search from matching windows *inside* some other pattern.
    fn windows<F>(
        &self,
        len: usize,
        modules: f32,
        quiet: f32,
        f: F,
    ) -> Option<(EanVariant, Vec<u8>)>
    where
        F: Fn(&[f32]) -> Option<(EanVariant, Vec<u8>)>,
    {
        let w = self.w;
        if w.len() < len {
            return None;
        }
        (0..=w.len() - len).step_by(2).find_map(|o| {
            let r = &w[o..o + len];
            let need = quiet * r.iter().sum::<f32>() / modules;
            let before = if o == 0 { self.lead } else { w[o - 1] };
            let after = w.get(o + len).copied().unwrap_or(self.trail);
            if before < need || after < need {
                return None;
            }
            f(r)
        })
    }
}

/// Minimum scanlines that must agree on a value before it is reported. A genuine
/// barcode is crossed by dozens of clean scanlines; unstructured texture (printed text,
/// artwork) yields only a stray line or two that happen to slip past a checksum — most
/// dangerously as UPC-E, whose eight digits are cheap to satisfy by chance.
const MIN_CONSENSUS_VOTES: usize = 3;
/// The floor for a UPC-E reading. A slanted scanline that leaves an EAN-13's bars just
/// past its centre guard — into the white above or below them — yields a run sequence
/// that *is* a UPC-E, quiet zone included; only the couple of lines exiting at exactly
/// that module see it (each counted once per prominence level), whereas a real UPC-E is
/// read by every line crossing it.
const MIN_UPCE_VOTES: usize = 8;
/// The winning value must additionally out-poll the runner-up by this factor, so a
/// scanline set split between two readings is treated as unresolved rather than guessed.
const DOMINANCE: usize = 2;

/// Robustly read an EAN/UPC symbol from a frame by decoding every fine scanline
/// ([`scan_edges`]) and taking the value the most scanlines agree on — provided that
/// value clears a consensus floor and clearly out-polls any competitor. Consensus across
/// scanlines is what makes a curved, blurred capture — where each individual line
/// corrupts different thin features — decode reliably; the floor and dominance test are
/// what stop a lone checksum-passing misread on non-barcode texture from being reported.
pub fn scan(frame: &GrayFrame<'_>, opts: &ScanOptions) -> Option<Symbol> {
    let mut tally: Vec<(String, usize, Symbol)> = Vec::new();
    for cand in scan_edges(frame, opts) {
        let Some(sym) = decode_edges_within(&cand.edges, cand.lead_px, cand.trail_px) else {
            continue;
        };
        let key = format!("{:?}|{}", sym.symbology, sym.text().unwrap_or_default());
        if let Some(entry) = tally.iter_mut().find(|(k, _, _)| *k == key) {
            entry.1 += 1;
        } else {
            let mut sym = sym;
            sym.location.get_or_insert(cand.location);
            tally.push((key, 1, sym));
        }
    }
    tally.sort_by_key(|(_, n, _)| core::cmp::Reverse(*n));
    let top = tally.first().map(|(_, n, _)| *n).unwrap_or(0);
    let runner_up = tally.get(1).map(|(_, n, _)| *n).unwrap_or(0);
    let floor = match tally.first() {
        Some((_, _, sym)) if sym.symbology == Symbology::UpcE => MIN_UPCE_VOTES,
        _ => MIN_CONSENSUS_VOTES,
    };
    if top < floor || top < DOMINANCE * runner_up {
        return None;
    }
    tally.into_iter().next().map(|(_, _, s)| s)
}
