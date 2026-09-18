//! A coarse spatial index for clustering detector hits.
//!
//! The finder / bullseye row scans report one hit per matching row and merge each into
//! the cluster it lands on. A real symbol yields a handful of clusters, but a periodic
//! texture (a halftone, a fabric, a checkerboard) yields one hit every few pixels and
//! tens of thousands of clusters — and testing every hit against every cluster so far
//! is quadratic: seconds on a VGA frame, minutes at camera resolution. [`CenterIndex`]
//! buckets the clusters on a grid so a hit is only tested against the clusters that
//! can reach it, without changing which cluster it merges into.

use alloc::{vec, vec::Vec};

/// Bucket side in pixels.
const CELL: usize = 8;

/// Grid of cluster ids, each registered in every bucket its *reach* — the box around
/// its centre inside which it absorbs a hit — touches.
pub(crate) struct CenterIndex {
    cols: usize,
    rows: usize,
    cells: Vec<Vec<u32>>,
    /// Per cluster id: the inclusive bucket box `[x0, y0, x1, y1]` it is registered in.
    covered: Vec<[usize; 4]>,
}

impl CenterIndex {
    /// An empty index over a `width × height` image.
    pub(crate) fn new(width: usize, height: usize) -> Self {
        let cols = width.div_ceil(CELL).max(1);
        let rows = height.div_ceil(CELL).max(1);
        CenterIndex {
            cols,
            rows,
            cells: vec![Vec::new(); cols * rows],
            covered: Vec::new(),
        }
    }

    /// Bucket coordinate of pixel coordinate `v` along an axis of `n` buckets
    /// (out-of-image and non-finite coordinates clamp to the border buckets).
    fn bucket(v: f32, n: usize) -> usize {
        ((v.max(0.0) as usize) / CELL).min(n - 1)
    }

    /// Ids of every cluster whose reach may contain `(x, y)`, in ascending order — so
    /// the first one that accepts the hit is the one a scan of all clusters in creation
    /// order would have found.
    pub(crate) fn candidates(&self, x: f32, y: f32) -> &[u32] {
        &self.cells[Self::bucket(y, self.rows) * self.cols + Self::bucket(x, self.cols)]
    }

    /// Record that cluster `id` now sits at `(x, y)` and absorbs hits within `reach` of
    /// it on each axis. Ids are dense: a new cluster's id is the number of clusters so
    /// far. Call again whenever a merge moves the centre or changes the reach.
    pub(crate) fn cover(&mut self, id: u32, x: f32, y: f32, reach: f32) {
        let reach = reach.max(0.0);
        let want = [
            Self::bucket(x - reach, self.cols),
            Self::bucket(y - reach, self.rows),
            Self::bucket(x + reach, self.cols),
            Self::bucket(y + reach, self.rows),
        ];
        let idx = id as usize;
        let have = self.covered.get(idx).copied();
        if let Some(h) = have
            && h[0] <= want[0]
            && h[1] <= want[1]
            && h[2] >= want[2]
            && h[3] >= want[3]
        {
            return;
        }
        let grown = match have {
            Some(h) => [
                h[0].min(want[0]),
                h[1].min(want[1]),
                h[2].max(want[2]),
                h[3].max(want[3]),
            ],
            None => want,
        };
        for by in grown[1]..=grown[3] {
            for bx in grown[0]..=grown[2] {
                if let Some(h) = have
                    && (h[0]..=h[2]).contains(&bx)
                    && (h[1]..=h[3]).contains(&by)
                {
                    continue; // already registered here
                }
                let cell = &mut self.cells[by * self.cols + bx];
                // Keep ids ascending; a new cluster's id is the largest so far.
                let at = cell.partition_point(|&other| other < id);
                cell.insert(at, id);
            }
        }
        if idx < self.covered.len() {
            self.covered[idx] = grown;
        } else {
            debug_assert_eq!(idx, self.covered.len(), "cluster ids must be dense");
            self.covered.push(grown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_clusters_within_reach_in_creation_order() {
        let mut index = CenterIndex::new(100, 60);
        index.cover(0, 50.0, 30.0, 4.0);
        index.cover(1, 52.0, 31.0, 4.0);
        index.cover(2, 5.0, 5.0, 1.0);
        assert_eq!(index.candidates(51.0, 30.0), &[0, 1]);
        assert_eq!(index.candidates(5.5, 5.5), &[2]);
        assert!(index.candidates(90.0, 50.0).is_empty());
    }

    #[test]
    fn growing_reach_keeps_ids_ascending() {
        let mut index = CenterIndex::new(200, 200);
        index.cover(0, 100.0, 100.0, 2.0);
        index.cover(1, 150.0, 100.0, 2.0);
        assert!(index.candidates(149.0, 100.0) == [1]);
        // Cluster 0's module estimate jumps: it now reaches cluster 1's bucket, and must
        // be listed before it there.
        index.cover(0, 100.0, 100.0, 60.0);
        assert_eq!(index.candidates(149.0, 100.0), &[0, 1]);
        assert_eq!(index.candidates(100.0, 45.0), &[0]);
        // Shrinking back never unregisters (a superset of candidates is harmless).
        index.cover(0, 100.0, 100.0, 1.0);
        assert_eq!(index.candidates(149.0, 100.0), &[0, 1]);
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let mut index = CenterIndex::new(1, 1);
        index.cover(0, f32::NAN, f32::INFINITY, f32::NAN);
        index.cover(1, -1e30, 1e30, 1e30);
        assert_eq!(index.candidates(f32::NAN, -5.0), &[0, 1]);
        let mut index = CenterIndex::new(17, 3);
        index.cover(0, 16.9, 2.9, 0.0);
        assert_eq!(index.candidates(1e9, 1e9), &[0]);
    }
}
