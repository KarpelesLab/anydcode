//! Concentric finder-pattern detection (the QR / Aztec `1:1:3:1:1` signature).
//!
//! A horizontal run-length scan of the reduced dark mask flags any five-run stretch
//! whose widths match the `1:1:3:1:1` finder ratio; each hit is confirmed by a vertical
//! and a diagonal cross-check through its centre, at a consistent module size — a
//! finder is concentric squares, so *every* line through its centre sees the ratio,
//! while bars, text strokes and halftone only line up along one or two. Confirmed
//! centres are clustered so a single physical finder reported on several rows collapses
//! to one hit.
//!
//! This is a *coarse* signal: it does not order finders into a symbol or recover a
//! homography (that is the sampler's job in [`crate::codes`]). It only tells the locator
//! "a matrix-style fiducial sits here, with roughly this module size", which upgrades a
//! region's family guess and seeds its module-size estimate.

use super::grid::DownGrid;
use alloc::vec::Vec;

/// A confirmed finder centre in reduced-pixel coordinates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FinderHit {
    /// Centre x in reduced pixels.
    pub x: f32,
    /// Centre y in reduced pixels.
    pub y: f32,
    /// Estimated module (narrow-run) size in reduced pixels.
    pub module: f32,
    /// How many independent scans landed on this cluster.
    pub count: u32,
}

/// Verify five run lengths against the `1:1:3:1:1` ratio, returning the module size.
fn ratio_module(counts: [i32; 5]) -> Option<f32> {
    let total: i32 = counts.iter().sum();
    if total < 7 {
        return None;
    }
    let module = total as f32 / 7.0;
    let tol = module / 2.0;
    let ok = (counts[0] as f32 - module).abs() < tol
        && (counts[1] as f32 - module).abs() < tol
        && (counts[2] as f32 - 3.0 * module).abs() < 3.0 * tol
        && (counts[3] as f32 - module).abs() < tol
        && (counts[4] as f32 - module).abs() < tol;
    ok.then_some(module)
}

/// Walk a dark-light-dark-light-dark run centred at `start` along an axis of length
/// `len`, returning the five run lengths and the index just past the final dark run.
fn walk(len: i32, start: i32, sample: impl Fn(i32) -> bool) -> Option<([i32; 5], i32)> {
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

/// Refined centre coordinate from run lengths and the walk's end index.
fn center(counts: [i32; 5], end: i32) -> f32 {
    end as f32 - counts[4] as f32 - counts[3] as f32 - counts[2] as f32 / 2.0
}

/// Merge a candidate centre into the cluster list, averaging coincident hits.
fn merge(centers: &mut Vec<FinderHit>, x: f32, y: f32, module: f32) {
    for f in centers.iter_mut() {
        if (f.x - x).abs() <= f.module && (f.y - y).abs() <= f.module {
            let c = f.count as f32;
            f.x = (f.x * c + x) / (c + 1.0);
            f.y = (f.y * c + y) / (c + 1.0);
            f.module = (f.module * c + module) / (c + 1.0);
            f.count += 1;
            return;
        }
    }
    centers.push(FinderHit {
        x,
        y,
        module,
        count: 1,
    });
}

/// Scan `grid` for concentric finder patterns and return their clustered centres.
pub(crate) fn find(grid: &DownGrid) -> Vec<FinderHit> {
    let mut centers: Vec<FinderHit> = Vec::new();
    let w = grid.width;
    let h = grid.height;
    let mut runs: Vec<(bool, usize, i32)> = Vec::new();
    for y in 0..h {
        // Run-length encode this row as (dark, start, len), reusing one buffer.
        runs.clear();
        let row = &grid.dark[y * w..(y + 1) * w];
        let mut cur = row[0];
        let mut start = 0usize;
        for (x, &d) in row.iter().enumerate().skip(1) {
            if d != cur {
                runs.push((cur, start, (x - start) as i32));
                cur = d;
                start = x;
            }
        }
        runs.push((cur, start, (w - start) as i32));
        if runs.len() < 5 {
            continue;
        }
        for i in 0..=runs.len() - 5 {
            if !runs[i].0 {
                continue; // a finder run starts dark
            }
            let counts = [
                runs[i].2,
                runs[i + 1].2,
                runs[i + 2].2,
                runs[i + 3].2,
                runs[i + 4].2,
            ];
            let Some(module) = ratio_module(counts) else {
                continue;
            };
            let mid = &runs[i + 2];
            let cx = mid.1 + (mid.2 as usize) / 2;
            // Vertical cross-check through the candidate centre column.
            let Some((vc, vend)) = walk(h as i32, y as i32, |k| grid.dark(cx, k as usize)) else {
                continue;
            };
            let Some(vmodule) = ratio_module(vc) else {
                continue;
            };
            // A finder is square: both axes must agree on its size (perspective and
            // rotation stretch one against the other, but not by half).
            if vmodule > 1.6 * module || module > 1.6 * vmodule {
                continue;
            }
            let cy = center(vc, vend);
            // Diagonal cross-checks through the refined centre — *both* diagonals. The
            // rings are squares, so along a diagonal each run is √2 longer and the ratio
            // is unchanged. One diagonal is not enough: the bars of a 1D code lying at
            // 45° satisfy the ratio along rows, columns and the diagonal across them,
            // and only fail along the diagonal that runs *with* the bars.
            let (dx0, dy0) = (cx as i32, (cy.round() as i32).clamp(0, h as i32 - 1));
            let both = [1i32, -1].into_iter().all(|dir| {
                // Walk index i along the diagonal (dx = +1, dy = dir), centre at i = off.
                let off = if dir > 0 {
                    dx0.min(dy0)
                } else {
                    dx0.min(h as i32 - 1 - dy0)
                };
                let len = off
                    + if dir > 0 {
                        (w as i32 - dx0).min(h as i32 - dy0)
                    } else {
                        (w as i32 - dx0).min(dy0 + 1)
                    };
                walk(len, off, |i| {
                    grid.dark((dx0 - off + i) as usize, (dy0 + dir * (i - off)) as usize)
                })
                .and_then(|(dc, _)| ratio_module(dc))
                .is_some()
            });
            if !both {
                continue;
            }
            merge(&mut centers, cx as f32, cy, module);
        }
    }
    centers
}
