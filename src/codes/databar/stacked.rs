//! GS1 DataBar stacked variants: [`Symbol`] <-> [`Encoding::Matrix`].
//!
//! These reuse the linear element-width encoders ([`super::encode::omn_total_widths`]
//! and [`super::expanded::expanded_elements`]) and compose their output into a
//! [`BitMatrix`] of data rows interleaved with separator rows.
//!
//! ## Layout convention
//! Each logical row (data or separator) is exactly one module tall, in top-to-bottom
//! order. The printed 5:7 / 34X-per-row height ratios of ISO/IEC 24724:2011 are a
//! rendering concern this abstract module output does not carry. Quiet zone is a
//! 1-module margin on all sides.
//!
//! - **Stacked** (3 rows): top data (left half of the linear symbol), separator,
//!   bottom data (right half).
//! - **Stacked Omnidirectional** (5 rows): top data, top separator, central
//!   alternating separator, bottom separator, bottom data.
//! - **Expanded Stacked** (`4 * stack_rows - 3` rows): `stack_rows` data rows, each
//!   holding up to `columns_per_row` codeblocks, separated by three separator rows
//!   between successive data rows (top separator below the upper row, a central
//!   alternating separator, and a bottom separator above the lower row).
//!
//! The construction mirrors zint's `rss.c` (`BARCODE_DBAR_STK`,
//! `BARCODE_DBAR_OMNSTK`, `BARCODE_DBAR_EXPSTK`), cross-checked module-for-module
//! against BWIPP (`databarstacked` / `databarstackedomni` / `databarexpandedstacked`)
//! in `tests/databar_stacked.rs`.

use super::{DataBarMeta, DataBarVariant};
use crate::error::{Error, Result};
use crate::output::{BitMatrix, Encoding};
use crate::segment::Segment;
use crate::symbol::{Symbol, SymbolMeta};
use crate::symbology::Symbology;

/// Light-module margin framing a stacked symbol (matches the linear encoders).
const QUIET_ZONE: usize = 1;
/// Module width of a DataBar-14 stacked symbol.
const STK_WIDTH: usize = 50;

// ======== shared row expansion ========

/// Expand element widths `widths[start..end]` into row `row` of `m`, beginning at
/// column `writer` with the given colour (`dark`), alternating per element. Returns
/// the column just past the last written module. Zero-width elements flip the colour
/// without emitting a module (matching the linear expander).
fn expand_row(
    m: &mut BitMatrix,
    row: usize,
    mut writer: usize,
    mut dark: bool,
    widths: &[i32],
    start: usize,
    end: usize,
) -> usize {
    for &w in &widths[start..end] {
        for _ in 0..w.max(0) {
            if dark {
                m.set(writer, row, true);
            }
            writer += 1;
        }
        dark = !dark;
    }
    writer
}

/// Run-length decode row `row` (columns `0..width`) into `(dark, run_length)` pairs.
fn row_runs(m: &BitMatrix, row: usize, width: usize) -> Vec<(bool, i32)> {
    let mut out = Vec::new();
    let mut cur = m.get(0, row);
    let mut run = 0i32;
    for x in 0..width {
        let d = m.get(x, row);
        if d == cur {
            run += 1;
        } else {
            out.push((cur, run));
            cur = d;
            run = 1;
        }
    }
    out.push((cur, run));
    out
}

// ======== DataBar-14 Stacked ========

/// Encode a DataBar Stacked symbol from the 46 linear element widths.
pub(super) fn encode_stacked(tw: &[i32; 46]) -> Encoding {
    let mut m = BitMatrix::new(STK_WIDTH, 3, QUIET_ZONE);

    // Top row (row 0): left half, then a dark right guard.
    let writer = expand_row(&mut m, 0, 0, false, tw, 0, 23);
    m.set(writer, 0, true);

    // Bottom row (row 2): dark/light guard, then right half.
    m.set(0, 2, true);
    expand_row(&mut m, 2, 2, true, tw, 23, 46);

    // Separator row (row 1). ISO/IEC 24724:2011 5.3.2.1 (zint interpretation #183).
    for i in 1..46 {
        let top = m.get(i, 0);
        let bot = m.get(i, 2);
        if top == bot {
            if !top {
                m.set(i, 1, true);
            }
        } else if !m.get(i - 1, 1) {
            m.set(i, 1, true);
        }
    }
    m.set(1, 1, false);
    m.set(2, 1, false);
    m.set(3, 1, false);

    Encoding::Matrix(m)
}

/// Encode a DataBar Stacked Omnidirectional symbol from the 46 linear element widths.
pub(super) fn encode_stacked_omni(tw: &[i32; 46]) -> Encoding {
    let c_right = super::encode::omn_right_finder_index(tw);
    let mut m = BitMatrix::new(STK_WIDTH, 5, QUIET_ZONE);

    // Top data row (row 0) and bottom data row (row 4).
    let writer = expand_row(&mut m, 0, 0, false, tw, 0, 23);
    m.set(writer, 0, true);
    m.set(0, 4, true);
    expand_row(&mut m, 4, 2, true, tw, 23, 46);

    // Central separator (row 2): alternating dark modules from column 5.
    let mut i = 5;
    while i < 46 {
        m.set(i, 2, true);
        i += 2;
    }

    // Top separator (row 1, reads the row above it = row 0).
    omn_separator(&mut m, STK_WIDTH, 1, -1, 18, 0, false);
    // Bottom separator (row 3, reads the row below it = row 4), with the
    // finder-value-3 special case of ISO/IEC 24724:2011 5.3.2.2.
    omn_separator(&mut m, STK_WIDTH, 3, 1, 19, 0, c_right == 3);

    Encoding::Matrix(m)
}

/// The omnidirectional separator pattern (ISO/IEC 24724:2011 5.3.2.2). `above_below`
/// is `-1` when the referenced data row is below `sep_row`, `+1` when above.
fn omn_separator(
    m: &mut BitMatrix,
    width: usize,
    sep_row: usize,
    above_below: i32,
    finder_start: usize,
    finder2_start: usize,
    bottom_finder_value_3: bool,
) {
    let module_row = (sep_row as i32 + above_below) as usize;
    for i in 4..(width - 4) {
        if !m.get(i, module_row) {
            m.set(i, sep_row, true);
        }
    }
    if bottom_finder_value_3 {
        let set_at = finder_start + 10;
        for i in finder_start..finder_start + 13 {
            m.set(i, sep_row, i == set_at);
        }
    } else {
        if finder_start != 0 {
            omn_finder_adjust(m, sep_row, above_below, finder_start);
        }
        if finder2_start != 0 {
            omn_finder_adjust(m, sep_row, above_below, finder2_start);
        }
    }
}

/// Adjust the 13 separator modules over an omnidirectional finder pattern.
fn omn_finder_adjust(m: &mut BitMatrix, sep_row: usize, above_below: i32, finder_start: usize) {
    let module_row = (sep_row as i32 + above_below) as usize;
    let mut latch = true;
    for i in finder_start..finder_start + 13 {
        if !m.get(i, module_row) {
            m.set(i, sep_row, latch);
            latch = !latch;
        } else {
            m.set(i, sep_row, false);
            latch = true;
        }
    }
}

// ======== DataBar-14 Stacked decoding ========

/// Reconstruct the 46 linear element widths from the two data rows of a
/// DataBar-14 stacked symbol (`top_row`/`bottom_row` module-row indices).
fn stacked_total_widths(m: &BitMatrix, top_row: usize, bottom_row: usize) -> Result<[i32; 46]> {
    let top = row_runs(m, top_row, STK_WIDTH);
    // Top row starts light (the left guard) and holds elements 0..23 followed by the
    // extra dark right guard, so its first 23 runs are the left half's widths.
    if top.len() < 23 || top[0].0 {
        return Err(Error::undecodable("DataBar stacked: malformed top row"));
    }
    let bottom = row_runs(m, bottom_row, STK_WIDTH);
    // Bottom row starts with a dark then light guard; elements 23..46 follow.
    if bottom.len() < 25 || !bottom[0].0 {
        return Err(Error::undecodable("DataBar stacked: malformed bottom row"));
    }
    let mut tw = [0i32; 46];
    for (k, run) in top.iter().take(23).enumerate() {
        tw[k] = run.1;
    }
    for k in 0..23 {
        tw[23 + k] = bottom[2 + k].1;
    }
    Ok(tw)
}

fn decode_stacked_family(
    m: &BitMatrix,
    top_row: usize,
    bottom_row: usize,
    symbology: Symbology,
    variant: DataBarVariant,
) -> Result<Symbol> {
    let tw = stacked_total_widths(m, top_row, bottom_row)?;
    let sym = super::decode::decode_omni(&tw)?;
    Ok(Symbol::new(
        symbology,
        sym.segments,
        SymbolMeta::DataBar(DataBarMeta::new(variant)),
    ))
}

// ======== DataBar Expanded Stacked ========

/// A codeblock spans 49 modules (17 + 15 finder + 17); guards add 4 modules per row.
const CODEBLOCK_MODULES: usize = 49;

/// Encode an Expanded Stacked symbol from its `Symbol` (payload + column metadata).
pub(super) fn encode_expanded_stacked(symbol: &Symbol) -> Result<Encoding> {
    let cols_per_row = match symbol.meta {
        SymbolMeta::DataBar(DataBarMeta {
            variant: DataBarVariant::ExpandedStacked,
            columns_per_row: Some(c),
        }) => c,
        _ => {
            return Err(Error::Unsupported {
                what: "GS1 DataBar Expanded Stacked without Expanded Stacked metadata",
            });
        }
    };
    let reduced = super::expanded::extract_reduced(&symbol.segments)?;
    expanded_stacked_matrix(&reduced, cols_per_row)
}

/// The effective column count for a reduced element string and a requested column
/// count: `min(requested, codeblocks)`, so that a symbol with fewer codeblocks than
/// requested columns canonicalises to a single row of exactly its codeblocks. Also
/// validates the payload by running the encodation.
pub(super) fn effective_columns(reduced: &[u8], cols_per_row: usize) -> Result<usize> {
    if !(1..=11).contains(&cols_per_row) {
        return Err(Error::invalid_parameter(
            "DataBar Expanded Stacked columns_per_row must be in 1..=11",
        ));
    }
    let els = super::expanded::expanded_elements(reduced, cols_per_row * 2)?;
    Ok(cols_per_row.min(els.codeblocks))
}

/// Build the Expanded Stacked module matrix for a reduced element string.
pub(super) fn expanded_stacked_matrix(reduced: &[u8], cols_per_row: usize) -> Result<Encoding> {
    if !(1..=11).contains(&cols_per_row) {
        return Err(Error::invalid_parameter(
            "DataBar Expanded Stacked columns_per_row must be in 1..=11",
        ));
    }
    let els = super::expanded::expanded_elements(reduced, cols_per_row * 2)?;
    let elements = &els.elements;
    let pattern_width = els.pattern_width;
    let codeblocks = els.codeblocks;
    let data_chars = els.data_chars;

    let stack_rows = codeblocks.div_ceil(cols_per_row);
    let height = stack_rows * 4 - 3;
    let num_columns_row1 = cols_per_row.min(codeblocks);
    let width = num_columns_row1 * CODEBLOCK_MODULES + 4;
    let mut m = BitMatrix::new(width, height, QUIET_ZONE);

    let mut current_block = 0usize;
    let mut v2_latch = false;
    for current_row in 1..=stack_rows {
        let data_row = (current_row - 1) * 4;
        let num_columns = if current_row * cols_per_row > codeblocks {
            codeblocks - current_block
        } else {
            cols_per_row
        };
        let special_case_row = current_row == stack_rows
            && num_columns != cols_per_row
            && current_row.is_multiple_of(2)
            && cols_per_row.is_multiple_of(2)
            && num_columns % 2 == 1;
        let left_to_right = cols_per_row % 2 == 1 || current_row % 2 == 1 || special_case_row;

        // Assemble the row's element widths: guards, codeblock region, guards.
        let mut sub: Vec<i32> = Vec::with_capacity(num_columns * 21 + 4);
        sub.push(if special_case_row { 2 } else { 1 });
        sub.push(1);
        let mut region = vec![0i32; num_columns * 21];
        let mut reader = 0usize;
        loop {
            let base = 2 + current_block * 21;
            for j in 0..21 {
                if base + j < pattern_width {
                    let v = elements[base + j];
                    if left_to_right {
                        region[reader * 21 + j] = v;
                    } else {
                        region[(20 - j) + (num_columns - 1 - reader) * 21] = v;
                    }
                }
            }
            reader += 1;
            current_block += 1;
            if reader >= cols_per_row || current_block >= codeblocks {
                break;
            }
        }
        sub.extend_from_slice(&region);
        sub.push(1);
        sub.push(1);

        let latch = !(current_row % 2 == 1 || special_case_row);
        let writer = expand_row(&mut m, data_row, 0, latch, &sub, 0, sub.len());

        if current_row != 1 {
            let odd_last_row = current_row == stack_rows && data_chars.is_multiple_of(2);
            // Central separator (alternating dark modules) two rows up.
            let mut j = 5;
            while j < CODEBLOCK_MODULES * cols_per_row {
                m.set(j, data_row - 2, true);
                j += 2;
            }
            // Bottom separator, one row up, reading the current row.
            exp_separator(
                &mut m,
                writer,
                reader,
                data_row - 1,
                1,
                special_case_row,
                left_to_right,
                odd_last_row,
                &mut v2_latch,
            );
        }
        if current_row != stack_rows {
            // Top separator, one row down, reading the current row.
            exp_separator(
                &mut m,
                writer,
                reader,
                data_row + 1,
                -1,
                false,
                left_to_right,
                false,
                &mut v2_latch,
            );
        }
    }

    Ok(Encoding::Matrix(m))
}

/// The Expanded Stacked separator pattern (ISO/IEC 24724:2011 7.2.8), a port of
/// zint's `dbar_exp_separator`. `above_below` is `+1` when the referenced data row
/// is above `sep_row`, `-1` when below.
#[allow(clippy::too_many_arguments)]
fn exp_separator(
    m: &mut BitMatrix,
    width: usize,
    cols: usize,
    sep_row: usize,
    above_below: i32,
    special_case_row: bool,
    left_to_right: bool,
    odd_last_row: bool,
    v2_latch: &mut bool,
) {
    let module_row = (sep_row as i32 + above_below) as usize;
    let mut v2 = *v2_latch;
    let mut space_latch = false;
    let sp = if special_case_row { 1usize } else { 0 };

    for j in (4 + sp)..(width - 4) {
        m.set(j, sep_row, !m.get(j, module_row));
    }

    for col in 0..cols {
        let k = CODEBLOCK_MODULES * col + 19 + sp;
        if left_to_right {
            let (i_start, i_end) = if v2 {
                (2usize, 15usize)
            } else {
                (0usize, 13usize)
            };
            for i in i_start..i_end {
                exp_separator_step(m, sep_row, module_row, i + k, &mut space_latch);
            }
        } else {
            let base = if odd_last_row {
                k as i32 - 17
            } else {
                k as i32
            };
            let (i_start, i_end) = if v2 { (14i32, 2i32) } else { (12i32, 0i32) };
            let mut i = i_start;
            while i >= i_end {
                exp_separator_step(
                    m,
                    sep_row,
                    module_row,
                    (i + base) as usize,
                    &mut space_latch,
                );
                i -= 1;
            }
        }
        v2 = !v2;
    }

    if above_below == -1 {
        *v2_latch = v2;
    }
}

/// One finder-adjustment step of [`exp_separator`].
fn exp_separator_step(
    m: &mut BitMatrix,
    sep_row: usize,
    module_row: usize,
    idx: usize,
    space_latch: &mut bool,
) {
    if m.get(idx, module_row) || *space_latch {
        // Over a dark module, or right after setting a separator module, leave this
        // separator module light.
        m.set(idx, sep_row, false);
        *space_latch = false;
    } else {
        m.set(idx, sep_row, true);
        *space_latch = true;
    }
}

// ======== Expanded Stacked decoding ========

/// Recover the reduced element string from an Expanded Stacked matrix by rebuilding
/// the linear element array and delegating to [`super::expanded::decode`].
fn decode_expanded_stacked(m: &BitMatrix) -> Result<Symbol> {
    let width = m.width();
    let height = m.height();
    let stack_rows = height.div_ceil(4);
    let cols_per_row = (width - 4) / CODEBLOCK_MODULES;
    if cols_per_row == 0 {
        return Err(Error::undecodable("DataBar Expanded Stacked: bad width"));
    }

    // Extract the codeblocks of the full (non-final) rows once.
    let mut full_blocks: Vec<Vec<i32>> = Vec::new();
    for current_row in 1..stack_rows {
        let data_row = (current_row - 1) * 4;
        let left_to_right = cols_per_row % 2 == 1 || current_row % 2 == 1;
        let sizes = vec![21usize; cols_per_row];
        let mut blocks =
            row_global_blocks(m, data_row, width, cols_per_row, left_to_right, &sizes)?;
        full_blocks.append(&mut blocks);
    }

    let last_data_row = (stack_rows - 1) * 4;
    // Brute-force the final row's column count and last-codeblock size; the correct
    // reconstruction is the one whose payload passes the Expanded checksum.
    for num_columns_last in 1..=cols_per_row {
        for &last_block in &[21usize, 13usize] {
            let special_case_row = num_columns_last != cols_per_row
                && stack_rows.is_multiple_of(2)
                && cols_per_row.is_multiple_of(2)
                && num_columns_last % 2 == 1;
            let left_to_right = cols_per_row % 2 == 1 || stack_rows % 2 == 1 || special_case_row;
            let mut sizes = vec![21usize; num_columns_last];
            if left_to_right {
                *sizes.last_mut().unwrap() = last_block;
            } else {
                sizes[0] = last_block;
            }
            let last_blocks = match row_global_blocks(
                m,
                last_data_row,
                width,
                num_columns_last,
                left_to_right,
                &sizes,
            ) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if let Some(sym) = try_reconstruct(&full_blocks, &last_blocks, cols_per_row) {
                return Ok(sym);
            }
        }
    }
    Err(Error::undecodable(
        "DataBar Expanded Stacked: could not reconstruct payload",
    ))
}

/// Attempt to decode the linear element array assembled from the row codeblocks.
fn try_reconstruct(
    full_blocks: &[Vec<i32>],
    last_blocks: &[Vec<i32>],
    cols_per_row: usize,
) -> Option<Symbol> {
    let mut el = vec![1i32, 1];
    for b in full_blocks {
        el.extend_from_slice(b);
    }
    for b in last_blocks {
        el.extend_from_slice(b);
    }
    el.push(1);
    el.push(1);
    let sym = super::expanded::decode(&el).ok()?;
    let reduced = sym.segments.into_iter().next()?.data;
    Some(Symbol::new(
        Symbology::DataBarExpandedStacked,
        vec![Segment::byte(reduced)],
        SymbolMeta::DataBar(DataBarMeta::expanded_stacked(cols_per_row)),
    ))
}

/// Extract one data row's codeblocks in global (reader) order. `sizes` are the
/// physical (left-to-right) codeblock element counts.
fn row_global_blocks(
    m: &BitMatrix,
    data_row: usize,
    width: usize,
    num_columns: usize,
    left_to_right: bool,
    sizes: &[usize],
) -> Result<Vec<Vec<i32>>> {
    let runs = row_runs(m, data_row, width);
    let total: usize = sizes.iter().sum();
    // Skip the two leading guard runs; the codeblock element runs follow.
    if runs.len() < 2 + total {
        return Err(Error::undecodable("DataBar Expanded Stacked: short row"));
    }
    let region: Vec<i32> = runs[2..2 + total].iter().map(|r| r.1).collect();

    // Segment into physical codeblocks.
    let mut phys: Vec<Vec<i32>> = Vec::with_capacity(num_columns);
    let mut p = 0usize;
    for &sz in sizes {
        phys.push(region[p..p + sz].to_vec());
        p += sz;
    }

    let mut global: Vec<Vec<i32>> = Vec::with_capacity(num_columns);
    if left_to_right {
        for b in phys {
            global.push(b);
        }
    } else {
        for r in 0..num_columns {
            let mut b = phys[num_columns - 1 - r].clone();
            b.reverse();
            global.push(b);
        }
    }
    Ok(global)
}

// ======== dispatch ========

/// Decode any stacked DataBar matrix, dispatching on its dimensions.
pub(super) fn decode(m: &BitMatrix) -> Result<Symbol> {
    let (w, h) = (m.width(), m.height());
    if w == STK_WIDTH && h == 3 {
        decode_stacked_family(m, 0, 2, Symbology::DataBarStacked, DataBarVariant::Stacked)
    } else if w == STK_WIDTH && h == 5 {
        decode_stacked_family(
            m,
            0,
            4,
            Symbology::DataBarStackedOmni,
            DataBarVariant::StackedOmni,
        )
    } else if w >= CODEBLOCK_MODULES + 4 && (w - 4) % CODEBLOCK_MODULES == 0 && (h + 3) % 4 == 0 {
        decode_expanded_stacked(m)
    } else {
        Err(Error::undecodable(format!(
            "unexpected DataBar stacked matrix dimensions {w}x{h}"
        )))
    }
}
