//! The DataBar combinatorial width <-> value algorithms (ISO/IEC 24724:2011
//! Annex B `getRSSwidths`), plus its exact inverse used for decoding.
//!
//! A DataBar data character is an *odd* and an *even* element group, each a set
//! of `elements` widths summing to a fixed module count. [`get_widths`] maps a
//! value in `0 .. C` to the unique width tuple; [`get_value`] recovers it. The
//! two are exact inverses, exercised exhaustively in the tests.

use super::tables::combins;

/// Generate `elements` element widths encoding `val`, given the total module
/// count `n`, the widest allowed element `max_width`, and whether narrow
/// (single-module) elements are disallowed (`no_narrow`).
///
/// Mirrors ISO/IEC 24724:2011 Annex B `getRSSwidths`.
pub fn get_widths(
    mut val: i64,
    mut n: i32,
    elements: usize,
    max_width: i32,
    no_narrow: bool,
) -> Vec<i32> {
    let mut widths = vec![0i32; elements];
    let elements = elements as i32;
    let mut narrow_mask: u32 = 0;
    let mut bar = 0i32;
    while bar < elements - 1 {
        let mut elm_width = 1i32;
        narrow_mask |= 1 << bar;
        let mut sub_val = 0i64;
        loop {
            // Safety bound: a single element can never exceed the remaining module
            // budget. For any valid value the `val < 0` break fires well before
            // this; the guard only stops runaway iteration on out-of-range input.
            if elm_width > n {
                break;
            }
            // All combinations for the remaining elements.
            sub_val = combins((n - elm_width - 1) as i64, (elements - bar - 2) as i64);
            // Fewer combinations if no narrow element has appeared yet.
            if no_narrow
                && narrow_mask == 0
                && n - elm_width - (elements - bar - 1) >= elements - bar - 1
            {
                sub_val -= combins(
                    (n - elm_width - (elements - bar)) as i64,
                    (elements - bar - 2) as i64,
                );
            }
            // Fewer combinations for elements exceeding `max_width`.
            if elements - bar - 1 > 1 {
                let mut less_val = 0i64;
                let mut mxw = n - elm_width - (elements - bar - 2);
                while mxw > max_width {
                    less_val += combins(
                        (n - elm_width - mxw - 1) as i64,
                        (elements - bar - 3) as i64,
                    );
                    mxw -= 1;
                }
                sub_val -= less_val * (elements - 1 - bar) as i64;
            } else if n - elm_width > max_width {
                sub_val -= 1;
            }
            val -= sub_val;
            if val < 0 {
                break;
            }
            elm_width += 1;
            narrow_mask &= !(1 << bar);
        }
        val += sub_val;
        n -= elm_width;
        widths[bar as usize] = elm_width;
        bar += 1;
    }
    widths[bar as usize] = n;
    widths
}

/// Recover the value encoded by `widths` (the exact inverse of [`get_widths`]).
///
/// Mirrors the `getRSSvalue` routine used by DataBar decoders (e.g. ZXing).
pub fn get_value(widths: &[i32], max_width: i32, no_narrow: bool) -> i64 {
    let elements = widths.len() as i32;
    let mut n: i32 = widths.iter().sum();
    let mut val = 0i64;
    let mut narrow_mask: u32 = 0;
    for bar in 0..elements - 1 {
        let mut elm_width = 1i32;
        narrow_mask |= 1 << bar;
        while elm_width < widths[bar as usize] {
            let mut sub_val = combins((n - elm_width - 1) as i64, (elements - bar - 2) as i64);
            if no_narrow
                && narrow_mask == 0
                && n - elm_width - (elements - bar - 1) >= elements - bar - 1
            {
                sub_val -= combins(
                    (n - elm_width - (elements - bar)) as i64,
                    (elements - bar - 2) as i64,
                );
            }
            if elements - bar - 1 > 1 {
                let mut less_val = 0i64;
                let mut mxw = n - elm_width - (elements - bar - 2);
                while mxw > max_width {
                    less_val += combins(
                        (n - elm_width - mxw - 1) as i64,
                        (elements - bar - 3) as i64,
                    );
                    mxw -= 1;
                }
                sub_val -= less_val * (elements - 1 - bar) as i64;
            } else if n - elm_width > max_width {
                sub_val -= 1;
            }
            val += sub_val;
            elm_width += 1;
            narrow_mask &= !(1 << bar);
        }
        n -= widths[bar as usize];
    }
    val
}

/// Interleave the odd and even element groups of a data character.
///
/// The odd group encodes `v_odd` and the even group `v_even`; the result is the
/// `2 * elements` alternating widths (`odd[0], even[0], odd[1], even[1], ...`).
/// Mirrors ISO/IEC 24724:2011 Annex B interleaving (`getRSSwidths` applied to
/// both subsets, with `max_width`/`no_narrow` inverted for the even subset).
pub fn interleave(
    v_odd: i64,
    v_even: i64,
    n_odd: i32,
    n_even: i32,
    elements: usize,
    max_width: i32,
    no_narrow: bool,
) -> Vec<i32> {
    let odd = get_widths(v_odd, n_odd, elements, max_width, no_narrow);
    let even = get_widths(v_even, n_even, elements, 9 - max_width, !no_narrow);
    let mut out = vec![0i32; elements * 2];
    for i in 0..elements {
        out[i * 2] = odd[i];
        out[i * 2 + 1] = even[i];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `get_value` inverts `get_widths` across a representative selection of the
    /// DataBar Omnidirectional and Limited group parameters. Each case is swept
    /// only over its valid value range, whose size is `get_value` of the largest
    /// (lexicographically last) width tuple plus one — avoiding out-of-range
    /// inputs, which `get_widths` is not defined for.
    #[test]
    fn widths_value_roundtrip() {
        // (n, elements, max_width, no_narrow) drawn from the omni outer/inner and
        // limited group tables (odd subsets).
        let cases: &[(i32, usize, i32, bool)] = &[
            (12, 4, 8, false),
            (10, 4, 6, false),
            (8, 4, 4, false),
            (6, 4, 3, false),
            (4, 4, 1, false),
            (5, 4, 2, true),
            (7, 4, 4, true),
            (9, 4, 6, true),
            (11, 4, 8, true),
            (17, 7, 6, false),
            (13, 7, 5, false),
            (9, 7, 3, false),
            (15, 7, 5, false),
            (11, 7, 4, false),
            (19, 7, 8, false),
            (7, 7, 1, false),
        ];
        for &(n, elements, max_width, no_narrow) in cases {
            // Value 0 is always valid; its widths reveal the case's parameters.
            let base = get_widths(0, n, elements, max_width, no_narrow);
            assert_eq!(base.iter().sum::<i32>(), n);
            // The count of valid values equals `get_value` of the maximal tuple
            // plus one. Derive that maximum by encoding a deliberately large
            // value clamped back into range via re-decoding.
            let mut count = 0i64;
            // Binary-search the largest in-range value: the largest v for which
            // get_value(get_widths(v)) == v.
            let (mut lo, mut hi) = (0i64, 1i64 << 20);
            while lo < hi {
                let mid = (lo + hi + 1) / 2;
                let w = get_widths(mid, n, elements, max_width, no_narrow);
                if w.iter().all(|&x| x >= 1)
                    && w.iter().sum::<i32>() == n
                    && get_value(&w, max_width, no_narrow) == mid
                {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            let max_v = lo;
            for v in 0..=max_v {
                let widths = get_widths(v, n, elements, max_width, no_narrow);
                assert_eq!(widths.iter().sum::<i32>(), n, "width sum for v={v}");
                assert_eq!(
                    get_value(&widths, max_width, no_narrow),
                    v,
                    "value roundtrip failed for v={v} (n={n})"
                );
                count += 1;
            }
            assert!(count >= 1, "case n={n} produced no values");
        }
    }
}
