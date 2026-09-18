//! Run-length helpers for the QR-family `1:1:3:1:1` finder pattern, shared by the
//! QR, Micro QR and rMQR samplers.

#[cfg(any(feature = "microqr", feature = "rmqr"))]
use crate::imgproc::binary::BinaryImage;
#[cfg(any(feature = "microqr", feature = "rmqr"))]
use crate::imgproc::cluster::CenterIndex;
#[cfg(any(feature = "microqr", feature = "rmqr"))]
use crate::imgproc::components::{extreme_quad, flood_region_bounded};
use alloc::vec::Vec;

/// Check that five run lengths match the finder 1:1:3:1:1 ratio; return the module
/// size (average narrow-run width) on success.
pub(crate) fn found_pattern_cross(counts: [i32; 5]) -> Option<f32> {
    let total: i32 = counts.iter().sum();
    if total < 7 {
        return None;
    }
    let module = total as f32 / 7.0;
    // Blur softens edges and adjacent dark data modules can bleed into the outer rings,
    // so the ratios are matched with generous slack: ~0.7 module on each narrow ring and
    // ~2.1 modules on the wide centre run. Reed–Solomon plus the geometric triple check
    // reject the extra false positives this admits.
    let max_var = module * 0.7;
    let ok = (counts[0] as f32 - module).abs() < max_var
        && (counts[1] as f32 - module).abs() < max_var
        && (counts[2] as f32 - 3.0 * module).abs() < 3.0 * max_var
        && (counts[3] as f32 - module).abs() < max_var
        && (counts[4] as f32 - module).abs() < max_var;
    ok.then_some(module)
}

/// Walk a dark-light-dark-light-dark run centered at `start` along an axis, returning
/// the five run lengths and the end index just past the final dark run. `sample(k)`
/// returns whether the pixel at axis position `k` is dark; the axis spans `0..len`.
#[cfg(any(feature = "qr", test))]
pub(crate) fn walk_run(
    len: i32,
    start: i32,
    sample: impl Fn(i32) -> bool,
) -> Option<([i32; 5], i32)> {
    walk_run_capped(len, start, i32::MAX, sample)
}

/// [`walk_run`] that gives up (`None`) as soon as any of the five runs exceeds
/// `max_run` pixels — for a caller that already knows the scale it is looking for.
pub(crate) fn walk_run_capped(
    len: i32,
    start: i32,
    max_run: i32,
    sample: impl Fn(i32) -> bool,
) -> Option<([i32; 5], i32)> {
    let mut counts = [0i32; 5];
    let mut i = start;
    while i >= 0 && sample(i) && counts[2] <= max_run {
        counts[2] += 1;
        i -= 1;
    }
    if i < 0 || counts[2] > max_run {
        return None;
    }
    while i >= 0 && !sample(i) && counts[1] <= max_run {
        counts[1] += 1;
        i -= 1;
    }
    if i < 0 || counts[1] == 0 || counts[1] > max_run {
        return None;
    }
    while i >= 0 && sample(i) && counts[0] <= max_run {
        counts[0] += 1;
        i -= 1;
    }
    if counts[0] == 0 || counts[0] > max_run {
        return None;
    }
    let mut j = start + 1;
    while j < len && sample(j) && counts[2] <= max_run {
        counts[2] += 1;
        j += 1;
    }
    if j >= len || counts[2] > max_run {
        return None;
    }
    while j < len && !sample(j) && counts[3] <= max_run {
        counts[3] += 1;
        j += 1;
    }
    if j >= len || counts[3] == 0 || counts[3] > max_run {
        return None;
    }
    while j < len && sample(j) && counts[4] <= max_run {
        counts[4] += 1;
        j += 1;
    }
    if counts[4] == 0 || counts[4] > max_run {
        return None;
    }
    Some((counts, j))
}

/// Refined center from run lengths and the walk's end index.
pub(crate) fn run_center(counts: [i32; 5], end: i32) -> f32 {
    end as f32 - counts[4] as f32 - counts[3] as f32 - counts[2] as f32 / 2.0
}

/// Run-length encode one line, invoking `emit(run_start, [c0..c4])` for every window of
/// five consecutive runs that begins on a dark run — `start` is the pixel index where
/// the middle (centre) run begins.
pub(crate) fn scan_line_runs(
    len: usize,
    dark: impl Fn(usize) -> bool,
    mut emit: impl FnMut(usize, [i32; 5]),
) {
    let mut runs: Vec<(bool, usize, i32)> = Vec::new();
    let mut cur = dark(0);
    let mut start = 0usize;
    for p in 1..len {
        let d = dark(p);
        if d != cur {
            runs.push((cur, start, (p - start) as i32));
            cur = d;
            start = p;
        }
    }
    runs.push((cur, start, (len - start) as i32));
    if runs.len() < 5 {
        return;
    }
    for i in 0..=runs.len() - 5 {
        if !runs[i].0 {
            continue; // pattern must start on a dark run
        }
        let counts = [
            runs[i].2,
            runs[i + 1].2,
            runs[i + 2].2,
            runs[i + 3].2,
            runs[i + 4].2,
        ];
        let center = &runs[i + 2];
        emit(center.1 + (center.2 as usize) / 2, counts);
    }
}

// ---- Micro QR / rMQR single-finder helpers -------------------------------------

/// A clustered finder-pattern centre.
#[cfg(any(feature = "microqr", feature = "rmqr"))]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Finder {
    pub x: f32,
    pub y: f32,
    pub module: f32,
    pub count: u32,
}

/// Signed area (doubled) of a quad via the shoelace formula.
#[cfg(any(feature = "microqr", feature = "rmqr"))]
pub(crate) fn shoelace(q: &[(f32, f32); 4]) -> f32 {
    let mut sum = 0.0;
    for i in 0..4 {
        let (x0, y0) = q[i];
        let (x1, y1) = q[(i + 1) % 4];
        sum += x0 * y1 - x1 * y0;
    }
    sum
}

/// Row sweep for the 1:1:3:1:1 finder ratio with a vertical cross-check.
#[cfg(any(feature = "microqr", feature = "rmqr"))]
pub(crate) fn find_finders(bin: &BinaryImage) -> Vec<Finder> {
    let (w, h) = (bin.width(), bin.height());
    let mut out: Vec<Finder> = Vec::new();
    let mut index = CenterIndex::new(w, h);
    for y in 0..h {
        scan_line_runs(
            w,
            |x| bin.get(x, y),
            |mid, counts| {
                let Some(module) = found_pattern_cross(counts) else {
                    return;
                };
                // The column must repeat the row's pattern at a comparable scale: its
                // widest run is ~3 modules, so 20 of the row's modules is ample even for
                // a strongly foreshortened capture. Uncapped, every row of a vertically
                // striped texture walks the full image height for each of its hits.
                let max_run = (module * 20.0) as i32 + 2;
                let Some((vc, vend)) =
                    walk_run_capped(h as i32, y as i32, max_run, |k| bin.get(mid, k as usize))
                else {
                    return;
                };
                if found_pattern_cross(vc).is_none() {
                    return;
                }
                let cy = run_center(vc, vend);
                merge(&mut out, &mut index, mid as f32, cy, module);
            },
        );
    }
    out.sort_by_key(|f| core::cmp::Reverse(f.count));
    out
}

#[cfg(any(feature = "microqr", feature = "rmqr"))]
fn merge(finders: &mut Vec<Finder>, index: &mut CenterIndex, x: f32, y: f32, module: f32) {
    // The first finder (in creation order) within one module of the hit absorbs it.
    // The index narrows the search to the finders near the hit: a texture that matches
    // the ratio everywhere makes tens of thousands of them.
    let near = index.candidates(x, y).iter().copied().find(|&i| {
        let f = &finders[i as usize];
        (f.x - x).abs() <= f.module && (f.y - y).abs() <= f.module
    });
    if let Some(i) = near {
        let f = &mut finders[i as usize];
        let c = f.count as f32;
        f.x = (f.x * c + x) / (c + 1.0);
        f.y = (f.y * c + y) / (c + 1.0);
        f.module = (f.module * c + module) / (c + 1.0);
        f.count += 1;
        index.cover(i, f.x, f.y, f.module);
        return;
    }
    index.cover(finders.len() as u32, x, y, module);
    finders.push(Finder {
        x,
        y,
        module,
        count: 1,
    });
}

/// Corners of the finder's outer 7×7 dark ring: march from the centre out of the
/// solid 3×3 core, across the light ring, into the outer ring, and flood it. The
/// light separator row/column isolates the ring from the data region, so the flood
/// cannot leak.
#[cfg(any(feature = "microqr", feature = "rmqr"))]
pub(crate) fn finder_ring_corners(bin: &BinaryImage, finder: &Finder) -> Option<[(f32, f32); 4]> {
    let w = bin.width();
    let mut x = finder.x as usize;
    let y = finder.y as usize;
    let mut seen_light = false;
    let mut seed = None;
    let limit = ((finder.module * 4.5) as usize).max(4);
    for _ in 0..limit {
        if x + 1 >= w {
            break;
        }
        x += 1;
        let dark = bin.get(x, y);
        if !dark {
            seen_light = true;
        } else if seen_light {
            seed = Some((x, y));
            break;
        }
    }
    // Sanity: the ring must span ~7 modules. A flood that outgrows that (twice over
    // in y, where the row-measured module says less) is abandoned on the spot: on a
    // noisy frame it would otherwise collect every connected dark pixel of the image.
    let span = 7.0 * finder.module;
    let pixels = flood_region_bounded(
        bin,
        seed?,
        true,
        (span * 1.5) as usize,
        (span * 3.0) as usize,
    );
    if pixels.is_empty() {
        return None;
    }
    let (min_x, max_x) = pixels.iter().fold((usize::MAX, 0), |(lo, hi), &(px, _)| {
        (lo.min(px), hi.max(px))
    });
    let width = (max_x - min_x) as f32;
    if width < span * 0.7 || width > span * 1.5 {
        return None;
    }
    extreme_quad(&pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_walk_matches_uncapped_within_the_cap() {
        // 2:2:6:2:2 dark/light/dark/light/dark with light margins.
        let line: alloc::vec::Vec<bool> = [(false, 3), (true, 2), (false, 2), (true, 6)]
            .into_iter()
            .chain([(false, 2), (true, 2), (false, 3)])
            .flat_map(|(dark, n)| core::iter::repeat_n(dark, n))
            .collect();
        let sample = |k: i32| line[k as usize];
        let len = line.len() as i32;
        let full = walk_run(len, 9, sample).expect("pattern walks");
        assert_eq!(full.0, [2, 2, 6, 2, 2]);
        assert_eq!(walk_run_capped(len, 9, 6, sample), Some(full));
        assert_eq!(walk_run_capped(len, 9, 5, sample), None, "centre run is 6");
        // A solid column gives up after the cap instead of walking to the border.
        let steps = core::cell::Cell::new(0usize);
        let solid = |_: i32| {
            steps.set(steps.get() + 1);
            true
        };
        assert_eq!(walk_run_capped(1_000_000, 500_000, 10, solid), None);
        assert!(steps.get() < 50, "walked {} pixels", steps.get());
        // Degenerate axes.
        assert_eq!(walk_run(0, 0, |_| true), None);
        assert_eq!(walk_run(1, 0, |_| true), None);
        assert_eq!(walk_run(1, 0, |_| false), None);
    }

    #[test]
    fn scan_line_runs_on_degenerate_lines() {
        let mut hits = 0;
        scan_line_runs(1, |_| true, |_, _| hits += 1);
        scan_line_runs(4, |p| p % 2 == 0, |_, _| hits += 1);
        assert_eq!(hits, 0, "fewer than five runs is never a window");
        scan_line_runs(
            5,
            |p| p % 2 == 0,
            |mid, counts| {
                hits += 1;
                assert_eq!((mid, counts), (2, [1; 5]));
            },
        );
        assert_eq!(hits, 1);
        assert!(found_pattern_cross([0; 5]).is_none());
        assert!(found_pattern_cross([1, 1, 3, 1, 1]).is_some());
    }

    /// The indexed merge must cluster exactly as the plain scan over every finder did.
    #[cfg(any(feature = "microqr", feature = "rmqr"))]
    #[test]
    fn indexed_merge_matches_linear_scan() {
        fn merge_linear(finders: &mut alloc::vec::Vec<Finder>, x: f32, y: f32, module: f32) {
            for f in finders.iter_mut() {
                if (f.x - x).abs() <= f.module && (f.y - y).abs() <= f.module {
                    let c = f.count as f32;
                    f.x = (f.x * c + x) / (c + 1.0);
                    f.y = (f.y * c + y) / (c + 1.0);
                    f.module = (f.module * c + module) / (c + 1.0);
                    f.count += 1;
                    return;
                }
            }
            finders.push(Finder {
                x,
                y,
                module,
                count: 1,
            });
        }

        let (w, h) = (300usize, 200usize);
        let mut rng = crate::imgproc::rng::Prng::new(99);
        let mut indexed = alloc::vec::Vec::new();
        let mut linear = alloc::vec::Vec::new();
        let mut index = CenterIndex::new(w, h);
        for _ in 0..20_000 {
            let x = rng.below(w) as f32;
            let y = rng.below(h) as f32;
            // Mostly small modules, now and then a huge one that makes a cluster's reach
            // jump across many index cells.
            let module = if rng.below(50) == 0 {
                5.0 + rng.below(60) as f32
            } else {
                1.0 + rng.next_f64() as f32 * 3.0
            };
            merge(&mut indexed, &mut index, x, y, module);
            merge_linear(&mut linear, x, y, module);
        }
        assert_eq!(indexed.len(), linear.len());
        for (a, b) in indexed.iter().zip(&linear) {
            assert_eq!((a.x, a.y, a.module, a.count), (b.x, b.y, b.module, b.count));
        }
    }
}
