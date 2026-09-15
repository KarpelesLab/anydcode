//! Run-length helpers for the QR-family `1:1:3:1:1` finder pattern, shared by the
//! QR, Micro QR and rMQR samplers.

#[cfg(any(feature = "microqr", feature = "rmqr"))]
use crate::imgproc::binary::BinaryImage;
#[cfg(any(feature = "microqr", feature = "rmqr"))]
use crate::imgproc::components::{extreme_quad, flood_region};
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
pub(crate) fn walk_run(
    len: i32,
    start: i32,
    sample: impl Fn(i32) -> bool,
) -> Option<([i32; 5], i32)> {
    let mut counts = [0i32; 5];
    let mut i = start;
    while i >= 0 && sample(i) {
        counts[2] += 1;
        i -= 1;
    }
    if i < 0 {
        return None;
    }
    while i >= 0 && !sample(i) {
        counts[1] += 1;
        i -= 1;
    }
    if i < 0 || counts[1] == 0 {
        return None;
    }
    while i >= 0 && sample(i) {
        counts[0] += 1;
        i -= 1;
    }
    if counts[0] == 0 {
        return None;
    }
    let mut j = start + 1;
    while j < len && sample(j) {
        counts[2] += 1;
        j += 1;
    }
    if j >= len {
        return None;
    }
    while j < len && !sample(j) {
        counts[3] += 1;
        j += 1;
    }
    if j >= len || counts[3] == 0 {
        return None;
    }
    while j < len && sample(j) {
        counts[4] += 1;
        j += 1;
    }
    if counts[4] == 0 {
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
    for y in 0..h {
        scan_line_runs(
            w,
            |x| bin.get(x, y),
            |mid, counts| {
                let Some(module) = found_pattern_cross(counts) else {
                    return;
                };
                let Some((vc, vend)) = walk_run(h as i32, y as i32, |k| bin.get(mid, k as usize))
                else {
                    return;
                };
                if found_pattern_cross(vc).is_none() {
                    return;
                }
                let cy = run_center(vc, vend);
                merge(&mut out, mid as f32, cy, module);
            },
        );
    }
    out.sort_by_key(|f| core::cmp::Reverse(f.count));
    out
}

#[cfg(any(feature = "microqr", feature = "rmqr"))]
fn merge(finders: &mut Vec<Finder>, x: f32, y: f32, module: f32) {
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
    let pixels = flood_region(bin, seed?, true);
    if pixels.is_empty() {
        return None;
    }
    // Sanity: the ring must span ~7 modules.
    let span = 7.0 * finder.module;
    let (min_x, max_x) = pixels.iter().fold((usize::MAX, 0), |(lo, hi), &(px, _)| {
        (lo.min(px), hi.max(px))
    });
    let width = (max_x - min_x) as f32;
    if width < span * 0.7 || width > span * 1.5 {
        return None;
    }
    extreme_quad(&pixels)
}
