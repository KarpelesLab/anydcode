//! GS1 DataBar reading from scan-line run widths: the path from pixels to a
//! [`Symbol`], for the Omnidirectional family (Omnidirectional/Truncated, Stacked,
//! Stacked Omnidirectional) and the Expanded family (Expanded, Expanded Stacked).
//!
//! A DataBar symbol has no start/stop pattern and need not have a quiet zone, so it
//! cannot be cut out of a scan line the way [`crate::scan1d`] cuts out other linear
//! symbols. What it has instead are *finder patterns*: five elements spanning 15
//! modules, of which two neighbours cover 10–12 and the last two are single modules —
//! a ratio no data character can show. Each finder sits between two data characters,
//! and those three together are the unit this reader works in:
//!
//! - an Omnidirectional **half**: outside character, finder, inside character. The
//!   left half is drawn space-first and the right half is its mirror image drawn
//!   bar-first, so the colour of the finder's first element tells them apart;
//! - an Expanded **block**: character, finder, character (the last block of a symbol
//!   with an odd character count has only its first character). Blocks alternate
//!   between the six left-to-right finders and their mirror images, again told apart
//!   by colour, and no finder occurs twice in a symbol — so a block's finder says where
//!   in the symbol it belongs.
//!
//! Every scan line is searched in both directions for finders, and each unit read
//! beside one is tallied by the lines that read exactly those element widths. A symbol
//! is then assembled from units that at least [`MIN_VOTES`] lines agree on, and must
//! pass the symbology's own checks (the mod 79 checksum carried by the two
//! Omnidirectional finders; the Expanded check character, finder sequence and
//! general-field syntax). Because units are tallied separately and joined afterwards,
//! the rows of a stacked symbol — or the two ends of a long symbol crossed by slanted
//! lines — need never be on one scan line.
//!
//! Element widths are measured per character against that character's own width (16,
//! 15 or 17 modules), which tolerates the pitch change perspective imposes, and are
//! corrected for ink spread: the odd and the even elements of a character are
//! different colours and each must sum to one of a few totals two modules apart, so
//! the bars' gain at the spaces' expense is measured by how far the sums are off.
//!
//! The assembled element widths go through the structural decoders
//! ([`super::decode`], [`super::expanded`]), so the [`Symbol`] is exactly the one they
//! produce and re-encodes to the same symbol. The one liberty taken is with Expanded
//! symbols from generators that write a redundant numeric latch after an FNC1: those
//! are read as meant (`expanded::decode_printed`), and so re-encode without it.

use super::decode::{decode_omni, omn_char_value};
use super::expanded::{self, char_value};
use super::tables::{EXP_FINDER, EXP_FINDER_SEQUENCE, OMN_FINDER};
use super::{DataBarMeta, DataBarVariant};
use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::scan1d::{Analysis, ScanOptions, analyze};
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;
use alloc::vec::Vec;

/// Scan lines that must read a half or block identically before it is used. The
/// checksums are weak (mod 79, mod 211) and every line is searched at every run in
/// both directions, so one line is not evidence.
const MIN_VOTES: u32 = 2;
/// Share of a finder's last four elements taken by the two wide ones: 10 of 12 to
/// 12 of 14 modules, with slack for edge noise.
const FINDER_RATIO: core::ops::RangeInclusive<f32> = 0.775..=0.905;
/// Largest deviation (modules) of a finder's edge-to-similar-edge distances from the
/// pattern matched. Patterns differ by a module or more in at least one of them.
const FINDER_TOL: f32 = 0.7;
/// Largest deviation (modules) of a single finder element, which unlike the
/// edge-to-similar-edge distances also absorbs ink spread.
const FINDER_ELEMENT_TOL: f32 = 1.1;
/// How far (modules per element) a character's ink spread may differ from that
/// measured on the finder beside it.
const MAX_SPREAD: f32 = 0.3;
/// Largest ink spread (modules per element) corrected at all.
const MAX_TOTAL_SPREAD: f32 = 0.6;
/// Largest deviation (modules) of a character element from the width assigned.
const ELEMENT_TOL: f32 = 0.7;
/// How far the module size may differ between a finder and the characters beside it.
const UNIT_RATIO: core::ops::RangeInclusive<f32> = 0.75..=1.34;
/// A guard element is one module; anything wider than this many is not one.
const MAX_GUARD: f32 = 3.0;
/// Alternative readings tried per symbol position, most-voted first.
const MAX_ALTERNATIVES: usize = 3;
/// Positions recorded per unit for the outline.
const MAX_SPANS: usize = 48;
/// How far the module size may differ between the halves or blocks of one symbol.
const SYMBOL_UNIT_RATIO: core::ops::RangeInclusive<f32> = 0.7..=1.43;
/// Largest gap (modules, along the reading axis) between where one unit ends and the
/// next of the same row begins. They share an edge; the slack is for units read on
/// different, slanted lines.
const SAME_ROW_GAP: f32 = 8.0;
/// Largest distance (modules, across the reading axis) between where the two halves
/// of a one-row Omnidirectional symbol were read.
const OMNI_ROW_SHIFT: f32 = 40.0;
/// Largest offset (modules, along the reading axis) of a stacked row from where the
/// row above puts it. The slack is for scan lines slanted against the rows, along
/// which a lower row appears shifted.
const NEXT_ROW_GAP: f32 = 12.0;
/// Modules in an Expanded block: two characters of 17 and a finder of 15.
const BLOCK_MODULES: usize = 49;
/// Largest distance (modules, across the reading axis) from one stacked row to the
/// next. Rows are specified at 33 or 34 modules tall and separators at 3.
const MAX_ROW_PITCH: f32 = 100.0;
/// A DataBar Stacked row is 5 or 7 modules tall, a Stacked Omnidirectional one at
/// least 33: a half read over a taller band than this is the latter.
const STACKED_ROW_MAX: f32 = 8.0;
/// Height (modules) of a whole DataBar Stacked symbol, with a module of slack.
const STACKED_HEIGHT: f32 = 14.0;

/// Read GS1 DataBar symbols from a frame: at most one of the Omnidirectional family
/// and one of the Expanded family.
///
/// Scan lines follow `opts`, so the symbol's reading axis must be within a few degrees
/// of one of [`ScanOptions::angles_deg`]; [`crate::pipeline::scan_1d`] finds that axis
/// for an arbitrarily rotated symbol and calls this reader among the others.
pub fn scan(frame: &GrayFrame<'_>, opts: &ScanOptions) -> Vec<Symbol> {
    scan_analysis(&analyze(frame, opts))
}

/// [`scan`] over scan lines that have already been sampled.
pub(crate) fn scan_analysis(analysis: &Analysis) -> Vec<Symbol> {
    let mut omni = Tally::default();
    let mut exp = Tally::default();
    for (line, edges) in analysis.line_edges() {
        // 13 runs is a finder with one character.
        if edges.len() < 14 {
            continue;
        }
        let mut pos = edges;
        for _ in 0..2 {
            read_pass(analysis, line, &pos, &mut omni, &mut exp);
            pos.reverse();
        }
    }
    let mut found = Vec::new();
    found.extend(assemble_omni(&omni));
    found.extend(assemble_expanded(&exp));
    found
}

/// A half or block as read: which one, its element widths, and who read it where.
#[derive(Debug, Clone)]
struct Unit {
    /// Omnidirectional: `0` left half, `1` right half. Expanded: the index into
    /// [`EXP_FINDER`], plus [`LEFT_ONLY`] for a block read without its second
    /// character.
    kind: u8,
    /// Element widths in reading order: character, finder, character.
    widths: [u8; 21],
    /// The scan lines that read it, one bit each.
    lines: u128,
    /// Frame positions of its first and last edge, per reading.
    spans: Vec<(Point, Point)>,
}

/// Flag on [`Unit::kind`] for an Expanded block read as first character and finder.
const LEFT_ONLY: u8 = 16;

impl Unit {
    fn votes(&self) -> u32 {
        self.lines.count_ones()
    }

    /// Mean first and last edge positions.
    fn ends(&self) -> (Point, Point) {
        let n = self.spans.len().max(1) as f32;
        let sum = self.spans.iter().fold((0.0, 0.0, 0.0, 0.0), |s, (a, b)| {
            (s.0 + a.x, s.1 + a.y, s.2 + b.x, s.3 + b.y)
        });
        (
            Point::new(sum.0 / n, sum.1 / n),
            Point::new(sum.2 / n, sum.3 / n),
        )
    }

    /// Mean module size, in pixels.
    fn module_px(&self) -> f32 {
        let (a, b) = self.ends();
        a.distance(b) / self.widths.iter().map(|&w| w as f32).sum::<f32>()
    }
}

/// The units read so far.
#[derive(Debug, Default)]
struct Tally {
    units: Vec<Unit>,
}

impl Tally {
    fn vote(&mut self, kind: u8, widths: [u8; 21], line: usize, span: (Point, Point)) {
        let bit = 1u128 << (line % 128);
        let unit = match self
            .units
            .iter_mut()
            .find(|u| u.kind == kind && u.widths == widths)
        {
            Some(u) => u,
            None => {
                self.units.push(Unit {
                    kind,
                    widths,
                    lines: 0,
                    spans: Vec::new(),
                });
                self.units.last_mut().expect("just pushed")
            }
        };
        unit.lines |= bit;
        if unit.spans.len() < MAX_SPANS {
            unit.spans.push(span);
        }
    }

    /// Units of `kind` with enough votes, most-voted first, at most
    /// [`MAX_ALTERNATIVES`].
    fn candidates(&self, kind: u8) -> Vec<&Unit> {
        let mut c: Vec<&Unit> = self
            .units
            .iter()
            .filter(|u| u.kind == kind && u.votes() >= MIN_VOTES)
            .collect();
        c.sort_by_key(|u| core::cmp::Reverse(u.votes()));
        c.truncate(MAX_ALTERNATIVES);
        c
    }
}

/// Search one line, read in the order of `pos` (its edge positions, either way
/// round), for finders and tally the units beside them.
fn read_pass(analysis: &Analysis, line: usize, pos: &[f32], omni: &mut Tally, exp: &mut Tally) {
    let w: Vec<f32> = pos.windows(2).map(|p| (p[1] - p[0]).abs()).collect();
    let n = w.len();
    let span = |from: usize, to: usize| {
        (
            analysis.point(line, pos[from]),
            analysis.point(line, pos[to]),
        )
    };
    // A finder at runs `i..i + 5`, its first character in the eight runs before.
    for i in 8..n.saturating_sub(4) {
        let f: [f32; 5] = [w[i], w[i + 1], w[i + 2], w[i + 3], w[i + 4]];
        // Wide pair then single modules, or the mirror image.
        let forward = finder_shape(f[1] + f[2], f[3] + f[4]);
        let mirrored = finder_shape(f[2] + f[3], f[0] + f[1]);
        if !forward && !mirrored {
            continue;
        }
        // Runs alternate from the light margin at index 0, in either reading order.
        let bar_first = i % 2 == 1;
        let before: [f32; 8] = w[i - 8..i].try_into().expect("eight runs");
        let after: Option<[f32; 8]> = w.get(i + 5..i + 13).map(|a| {
            let mut a: [f32; 8] = a.try_into().expect("eight runs");
            // The character after a finder is drawn mirrored.
            a.reverse();
            a
        });
        let unit_px = f.iter().sum::<f32>() / 15.0;
        let f_modules = f.map(|x| x / unit_px);
        let same_scale = |c: &Char| UNIT_RATIO.contains(&(c.unit_px / unit_px));

        // Omnidirectional half: guard, outside character, finder, inside character.
        // The guard is one module wide — unless the frame clips it, when the
        // character starts on the line's first run.
        if let (true, Some(after)) = (forward, after)
            && (i == 8 || w[i - 9] <= MAX_GUARD * unit_px)
            && let Some(finder) = match_finder(&f, OMN_FINDER.iter())
            && let spread = finder_spread(&f_modules, &OMN_FINDER[finder])
            && let Some(outside) = read_char(&before, 16, 4, 12, spread)
            && let Some(inside) = read_char(&after, 15, 5, 11, spread)
            && same_scale(&outside)
            && same_scale(&inside)
            && omn_char_value(&outside.widths, true).is_ok()
            && omn_char_value(&inside.widths, false).is_ok()
        {
            let widths = unit_widths(&outside.widths, &OMN_FINDER[finder], Some(&inside.widths));
            omni.vote(bar_first as u8, widths, line, span(i - 8, i + 13));
        }

        // Expanded block: the left-to-right finders are drawn space-first, their
        // mirror images bar-first.
        let table = match (forward, mirrored, bar_first) {
            (true, _, false) => 0,
            (_, true, true) => 1,
            _ => continue,
        };
        let Some(finder) = match_finder(&f, EXP_FINDER.iter().skip(table).step_by(2)) else {
            continue;
        };
        let finder = 2 * finder + table;
        let spread = finder_spread(&f_modules, &EXP_FINDER[finder]);
        let Some(left) = read_char(&before, 17, 4, 12, spread) else {
            continue;
        };
        if !same_scale(&left) || char_value(&left.widths).is_err() {
            continue;
        }
        let widths = unit_widths(&left.widths, &EXP_FINDER[finder], None);
        exp.vote(finder as u8 | LEFT_ONLY, widths, line, span(i - 8, i + 5));
        if let Some(after) = after
            && let Some(right) = read_char(&after, 17, 4, 12, spread)
            && same_scale(&right)
            && char_value(&right.widths).is_ok()
        {
            let widths = unit_widths(&left.widths, &EXP_FINDER[finder], Some(&right.widths));
            exp.vote(finder as u8, widths, line, span(i - 8, i + 13));
        }
    }
}

/// Whether two wide elements summing to `wide` and two single modules summing to
/// `narrow` have a finder's proportions.
fn finder_shape(wide: f32, narrow: f32) -> bool {
    let total = wide + narrow;
    total > 0.0 && FINDER_RATIO.contains(&(wide / total))
}

/// The pattern of `patterns` that five runs match best, if any is within tolerance.
///
/// Patterns are compared by the distances between edges of the same polarity (the sum
/// of two neighbouring elements), which ink spread does not change.
fn match_finder<'a>(f: &[f32; 5], patterns: impl Iterator<Item = &'a [u16; 5]>) -> Option<usize> {
    let unit = f.iter().sum::<f32>() / 15.0;
    if unit <= 0.0 {
        return None;
    }
    let mut best: Option<(usize, f32)> = None;
    for (idx, p) in patterns.enumerate() {
        let (mut sum, mut worst) = (0.0f32, 0.0f32);
        for k in 0..4 {
            let d = ((f[k] + f[k + 1]) / unit - (p[k] + p[k + 1]) as f32).abs();
            sum += d;
            worst = worst.max(d);
        }
        let element = (0..5)
            .map(|k| (f[k] / unit - p[k] as f32).abs())
            .fold(0.0, f32::max);
        if worst <= FINDER_TOL && element <= FINDER_ELEMENT_TOL && best.is_none_or(|(_, s)| sum < s)
        {
            best = Some((idx, sum));
        }
    }
    best.map(|(idx, _)| idx)
}

/// How much wider (modules) the finder's first, third and fifth elements were measured
/// than `pattern` draws them, the second and fourth being as much narrower: the ink
/// spread, signed for the colour of the finder's first element.
fn finder_spread(f: &[f32; 5], pattern: &[u16; 5]) -> f32 {
    let d = |k: usize| f[k] - pattern[k] as f32;
    (d(0) + d(2) + d(4) - d(1) - d(3)) / 5.0
}

/// A data character as read: its element widths in modules and its module size.
struct Char {
    widths: [i32; 8],
    unit_px: f32,
}

/// Read a data character of `modules` modules from its eight runs (pixels, in the
/// character's own element order), whose odd elements must sum to `odd_min`,
/// `odd_min + 2`, … `odd_max` modules. `spread` is the ink spread measured on the
/// finder beside it ([`finder_spread`]): in either character of a half or block the
/// odd elements are the colour of the finder's first element.
fn read_char(
    runs: &[f32; 8],
    modules: i32,
    odd_min: i32,
    odd_max: i32,
    finder_spread: f32,
) -> Option<Char> {
    let total: f32 = runs.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let unit_px = total / modules as f32;
    let f = runs.map(|r| r / unit_px);
    // The odd elements are one colour and the even ones the other, so ink spread
    // moves width between the two sums. The permitted splits are two modules apart:
    // the one nearest the sums once the finder's spread is taken off is the split,
    // and what then remains to be moved is this character's own spread.
    let odd_sum = f[0] + f[2] + f[4] + f[6];
    let expected = odd_sum - 4.0 * finder_spread;
    let steps = ((expected - odd_min as f32) / 2.0).round() as i32;
    let odd_total = (odd_min + 2 * steps).clamp(odd_min, odd_max);
    let spread = (odd_total as f32 - odd_sum) / 4.0;
    if (spread + finder_spread).abs() > MAX_SPREAD || spread.abs() > MAX_TOTAL_SPREAD {
        return None;
    }
    let odd = fit(
        [f[0] + spread, f[2] + spread, f[4] + spread, f[6] + spread],
        odd_total,
    )?;
    let even = fit(
        [f[1] - spread, f[3] - spread, f[5] - spread, f[7] - spread],
        modules - odd_total,
    )?;
    Some(Char {
        widths: [
            odd[0], even[0], odd[1], even[1], odd[2], even[2], odd[3], even[3],
        ],
        unit_px,
    })
}

/// Round four element widths to whole modules (at least one each) summing to `total`.
fn fit(v: [f32; 4], total: i32) -> Option<[i32; 4]> {
    let mut out = v.map(|x| (x.round() as i32).max(1));
    let mut diff = total - out.iter().sum::<i32>();
    // The sum of `v` is `total`, so rounding is off by one at most on real data: give
    // it to the element that was rounded the furthest the other way.
    if diff.abs() > 1 {
        return None;
    }
    if diff != 0 {
        let k = (0..4)
            .filter(|&k| diff > 0 || out[k] > 1)
            .max_by(|&a, &b| {
                let ra = (v[a] - out[a] as f32) * diff as f32;
                let rb = (v[b] - out[b] as f32) * diff as f32;
                ra.total_cmp(&rb)
            })?;
        out[k] += diff;
        diff = 0;
    }
    debug_assert_eq!(diff, 0);
    (0..4)
        .all(|k| (v[k] - out[k] as f32).abs() <= ELEMENT_TOL)
        .then_some(out)
}

/// A unit's element widths in reading order: the first character, the finder, and
/// the second character mirrored (zeros when absent).
fn unit_widths(first: &[i32; 8], finder: &[u16; 5], second: Option<&[i32; 8]>) -> [u8; 21] {
    let mut out = [0u8; 21];
    for k in 0..8 {
        out[k] = first[k] as u8;
    }
    for k in 0..5 {
        out[8 + k] = finder[k] as u8;
    }
    if let Some(second) = second {
        for k in 0..8 {
            out[13 + k] = second[7 - k] as u8;
        }
    }
    out
}

/// The best-supported Omnidirectional-family symbol: a left and a right half whose
/// characters reproduce the checksum their two finders carry, and which lie where the
/// two halves of one symbol do.
fn assemble_omni(tally: &Tally) -> Option<Symbol> {
    let mut best: Option<(u32, Symbol)> = None;
    for left in tally.candidates(0) {
        for right in tally.candidates(1) {
            let mut tw = [1i32; 46];
            for k in 0..21 {
                tw[2 + k] = left.widths[k] as i32;
                // The right half was read from the outside in: mirror it.
                tw[23 + k] = right.widths[20 - k] as i32;
            }
            let Ok(sym) = decode_omni(&tw) else {
                continue;
            };
            let score = left.votes().min(right.votes());
            if best.as_ref().is_some_and(|(s, _)| score <= *s) {
                continue;
            }
            if let Some(sym) = omni_symbol(sym, left, right) {
                best = Some((score, sym));
            }
        }
    }
    best.map(|(_, sym)| sym)
}

/// Classify an Omnidirectional-family reading by where its halves are, and locate it.
/// `None` if they are not laid out as the halves of one symbol.
fn omni_symbol(sym: Symbol, left: &Unit, right: &Unit) -> Option<Symbol> {
    let ((l0, l1), (r0, r1)) = (left.ends(), right.ends());
    let axis = Axis::along(l0, l1);
    let (lm, rm) = (left.module_px(), right.module_px());
    if !SYMBOL_UNIT_RATIO.contains(&(rm / lm)) {
        return None;
    }
    let module = (lm + rm) / 2.0;
    let u = |p: Point| axis.u(p) / module;
    let v = |p: Point| axis.v(p) / module;
    // Each half is read from its guard inwards, so the two run opposite ways.
    if u(r1) >= u(r0) {
        return None;
    }
    let (symbology, variant) =
        if (u(l1) - u(r1)).abs() <= SAME_ROW_GAP && (v(l1) - v(r1)).abs() <= OMNI_ROW_SHIFT {
            // One row: both end at its middle.
            (Symbology::DataBarOmni, DataBarVariant::Omni)
        } else if (u(l0) - u(r1)).abs() <= NEXT_ROW_GAP
            && (u(l1) - u(r0)).abs() <= NEXT_ROW_GAP
            && (1.0..=MAX_ROW_PITCH).contains(&(v(r1) - v(l0)))
        {
            // Two rows: the right half lies under the left one, end under start.
            // DataBar Stacked is 13 modules tall in all; a scan line can only cross
            // its rows where a taller symbol's rows could also have been read, but
            // the taller symbol is read further apart or over a wider band as well.
            if axis.height(left).max(axis.height(right)) <= STACKED_ROW_MAX * module
                && v(r1) - v(l0) <= STACKED_HEIGHT
            {
                (Symbology::DataBarStacked, DataBarVariant::Stacked)
            } else {
                (Symbology::DataBarStackedOmni, DataBarVariant::StackedOmni)
            }
        } else {
            return None;
        };
    let mut out = Symbol::new(
        symbology,
        sym.segments,
        SymbolMeta::DataBar(DataBarMeta::new(variant)),
    );
    out.location = Some(axis.locate(&[left, right], module));
    Some(out)
}

/// The best-supported Expanded-family symbol: the block with the check character,
/// which fixes the character count and with it the finder sequence, then the
/// most-voted block for every further finder of that sequence.
fn assemble_expanded(tally: &Tally) -> Option<Symbol> {
    for first in tally.candidates(0) {
        let check: [i32; 8] = core::array::from_fn(|k| first.widths[k] as i32);
        let Ok(value) = char_value(&check) else {
            continue;
        };
        // The check character carries 211 × (symbol characters − 4) + checksum.
        let chars = (value / 211 + 4) as usize;
        if !(4..=22).contains(&chars) {
            continue;
        }
        let blocks = chars.div_ceil(2);
        let sequence = &EXP_FINDER_SEQUENCE[(chars - 1) / 2 - 1];
        let mut options: Vec<Vec<&Unit>> = alloc::vec![alloc::vec![first]];
        for (k, &finder) in sequence.iter().enumerate().take(blocks).skip(1) {
            let half = k == blocks - 1 && chars % 2 == 1;
            let kind = (finder - 1) | if half { LEFT_ONLY } else { 0 };
            options.push(tally.candidates(kind));
        }
        if options.iter().any(Vec::is_empty) {
            continue;
        }
        // The most-voted reading of every block, then each alternative in turn.
        let mut pick: Vec<usize> = alloc::vec![0; blocks];
        let mut trial = 0;
        loop {
            let units: Vec<&Unit> = options.iter().zip(&pick).map(|(o, &p)| o[p]).collect();
            if let Some(sym) = expanded_symbol(&units, chars) {
                return Some(sym);
            }
            // Next single substitution: block `b`, alternative `a`.
            let next = (1..blocks)
                .flat_map(|b| (1..options[b].len()).map(move |a| (b, a)))
                .nth(trial);
            let Some((b, a)) = next else {
                break;
            };
            pick.fill(0);
            pick[b] = a;
            trial += 1;
        }
    }
    None
}

/// Decode `units` (one per block, in symbol order) as an Expanded symbol of `chars`
/// symbol characters; classify it by how its blocks are laid out, and locate it.
fn expanded_symbol(units: &[&Unit], chars: usize) -> Option<Symbol> {
    let blocks = units.len();
    let mut el: Vec<i32> = Vec::with_capacity(blocks * 5 + chars * 8 + 4);
    el.extend([1, 1]);
    for (k, unit) in units.iter().enumerate() {
        let len = if k == blocks - 1 && chars % 2 == 1 {
            13
        } else {
            21
        };
        el.extend(unit.widths[..len].iter().map(|&w| w as i32));
    }
    el.extend([1, 1]);
    let sym = expanded::decode_printed(&el).ok()?;

    let (a, b) = units[0].ends();
    let axis = Axis::along(a, b);
    let first_module = units[0].module_px();
    if units
        .iter()
        .any(|u| !SYMBOL_UNIT_RATIO.contains(&(u.module_px() / first_module)))
    {
        return None;
    }
    let module = units.iter().map(|u| u.module_px()).sum::<f32>() / blocks as f32;
    let columns = expanded_columns(units, &axis, module, chars % 2 == 1)?;
    let mut out = if columns >= blocks {
        sym
    } else {
        Symbol::new(
            Symbology::DataBarExpandedStacked,
            sym.segments,
            SymbolMeta::DataBar(DataBarMeta::expanded_stacked(columns)),
        )
    };
    out.location = Some(axis.locate(units, module));
    Some(out)
}

/// How many blocks per row `units` (in symbol order) are laid out in — all of them
/// for a symbol that is not stacked — or `None` if they are not laid out as the
/// blocks of one symbol. `half_last` says the last block has one character only.
///
/// Within a row each block starts where the one before it ends. A row ends after
/// the same number of blocks every time, and the next one lies below it, flush with
/// it at the side the first row starts from. It is read the same way, or (every
/// other row of a symbol with an even number of columns) the other way — from under
/// the end of the row above if it is as long, from its own far end if it is the
/// last row and shorter.
fn expanded_columns(units: &[&Unit], axis: &Axis, module: f32, half_last: bool) -> Option<usize> {
    let u = |p: Point| axis.u(p) / module;
    let v = |p: Point| axis.v(p) / module;
    let mut columns: Option<usize> = None;
    // The side of the symbol the first row starts from — every row is flush with it
    // — and the way the first row runs from there.
    let side = u(units[0].ends().0);
    let outwards = (u(units[0].ends().1) - side).signum();
    let mut in_row = 1;
    for (k, pair) in units.windows(2).enumerate() {
        let ((p0, p1), (n0, n1)) = (pair[0].ends(), pair[1].ends());
        let way = (u(p1) - u(p0)).signum();
        let same_way = way * (u(n1) - u(n0)) > 0.0;
        let follows = same_way && (u(n0) - u(p1)).abs() <= SAME_ROW_GAP;
        if follows && columns.is_none_or(|c| in_row < c) {
            in_row += 1;
            continue;
        }
        if columns.is_some_and(|c| c != in_row) {
            return None;
        }
        // How far from that side the next row starts: not at all if it runs
        // outwards, its own length if it runs back.
        let remaining = units.len() - (k + 1);
        let length = if same_way || way != outwards {
            0.0
        } else if remaining <= in_row && half_last {
            (remaining * BLOCK_MODULES) as f32 - 17.0
        } else {
            (remaining.min(in_row) * BLOCK_MODULES) as f32
        };
        let flush = (u(n0) - (side + outwards * length)).abs() <= NEXT_ROW_GAP;
        let below = (1.0..=MAX_ROW_PITCH).contains(&(v(n0) - v(p0)));
        if !flush || !below {
            return None;
        }
        columns = Some(in_row);
        in_row = 1;
    }
    // The last row may be short, never long.
    match columns {
        Some(c) if in_row > c => None,
        Some(c) => Some(c),
        None => Some(units.len()),
    }
}

/// A symbol's reading axis: a unit direction and its perpendicular.
struct Axis {
    ux: f32,
    uy: f32,
}

impl Axis {
    /// The axis from `a` towards `b`.
    fn along(a: Point, b: Point) -> Axis {
        let d = a.distance(b).max(f32::MIN_POSITIVE);
        Axis {
            ux: (b.x - a.x) / d,
            uy: (b.y - a.y) / d,
        }
    }

    /// Position of `p` along the axis.
    fn u(&self, p: Point) -> f32 {
        p.x * self.ux + p.y * self.uy
    }

    /// Position of `p` across the axis (clockwise of it).
    fn v(&self, p: Point) -> f32 {
        p.y * self.ux - p.x * self.uy
    }

    /// Extent across the axis of the places `unit` was read at.
    fn height(&self, unit: &Unit) -> f32 {
        let (lo, hi) = self.range(unit.spans.iter(), |p| self.v(p));
        hi - lo
    }

    fn range<'a>(
        &self,
        spans: impl Iterator<Item = &'a (Point, Point)>,
        f: impl Fn(Point) -> f32,
    ) -> (f32, f32) {
        spans
            .flat_map(|&(a, b)| [a, b])
            .fold((f32::MAX, f32::MIN), |(lo, hi), p| {
                (lo.min(f(p)), hi.max(f(p)))
            })
    }

    /// The outline of everywhere `units` were read: a box along the axis.
    fn locate(&self, units: &[&Unit], module: f32) -> Location {
        let spans = || units.iter().flat_map(|u| u.spans.iter());
        let (u0, u1) = self.range(spans(), |p| self.u(p));
        let (v0, v1) = self.range(spans(), |p| self.v(p));
        let at = |u: f32, v: f32| Point::new(u * self.ux - v * self.uy, u * self.uy + v * self.ux);
        Location {
            outline: Quad::new([at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1)]),
            rotation: Some(self.uy.atan2(self.ux)),
            module_size: Some(module),
        }
    }
}
