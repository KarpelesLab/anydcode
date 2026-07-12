//! Fast, symbology-agnostic frame-level code **locator**.
//!
//! This is the cheap first half of the two-stage live pipeline (see [`crate::pipeline`]
//! and [`crate::traits::Detect`]). Given a camera frame it answers one question as fast
//! as possible: *where are the barcodes, and roughly what kind is each?* — a bounding
//! quad plus a coarse family guess per code. It never decodes; the heavier
//! [`Analyze`](crate::traits::Analyze) pass does that, and only for the regions found
//! here.
//!
//! # How it works
//!
//! [`locate`] makes a single reduced-resolution pass:
//!
//! 1. **Downscale + binarize** (`grid`): box-average the frame down by
//!    [`LocateOptions::downscale`] and Otsu-threshold the result once. Everything after
//!    this works on the small image, which is what makes the locator fast enough to run
//!    on every frame.
//! 2. **Concentric finders** (`finder`): a run-length row scan with a vertical
//!    cross-check flags QR/Aztec `1:1:3:1:1` fiducials. These give a high-confidence
//!    matrix classification and a module-size estimate.
//! 3. **Texture tiles** (`tiles`): each dense-edged tile is first *labelled* by its own
//!    horizontal-vs-vertical edge balance — one-directional (a 1D barcode patch) or
//!    balanced (a 2D matrix) — and only same-label tiles are then clustered into regions.
//!    Labelling per tile before clustering is what lets a directional barcode be pulled
//!    out of the isotropic text and artwork it sits amongst, catching every finder-less
//!    symbology without the barcode dissolving into the surrounding scene.
//! 4. **Merge + map back**: finder hits upgrade the region they fall in to a matrix
//!    guess with a real module size; region boxes are scaled back to full resolution and
//!    returned as [`Candidate`]s, ordered strongest-first and capped at
//!    [`LocateOptions::max_candidates`].
//!
//! The locator favours **recall and speed over precision**: it would rather hand
//! `Analyze` a few extra boxes than miss a code. It is fully deterministic; the only
//! randomness available (via a seeded [`Prng`](crate::imgproc::Prng)) is unused by the
//! default path and reserved for future sampling heuristics.
//!
//! # Family guess
//!
//! [`Candidate::symbology`] carries a *representative* symbology for the guessed family,
//! not a decode claim: a finder-backed matrix reports [`Symbology::QrCode`], a
//! finder-less matrix [`Symbology::DataMatrix`], and a linear region [`Symbology::Code128`].
//! Recover just the family with [`Symbology::dimension`].

mod finder;
mod grid;
mod tiles;

use crate::geometry::{Location, Point, Quad};
use crate::image::GrayFrame;
use crate::pipeline::{Candidate, Fingerprint, Hints};
use crate::symbology::Symbology;
use crate::traits::Detect;

use finder::FinderHit;
use grid::DownGrid;
use tiles::{Family, Region};

/// Which layout families the locator should report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Families {
    /// Report 2D matrix regions (QR, Aztec, Data Matrix, …).
    pub matrix: bool,
    /// Report 1D / stacked linear regions.
    pub linear: bool,
}

impl Families {
    /// Report every family.
    pub const ALL: Families = Families {
        matrix: true,
        linear: true,
    };
    /// Report only 2D matrix regions.
    pub const MATRIX: Families = Families {
        matrix: true,
        linear: false,
    };
    /// Report only 1D / stacked regions.
    pub const LINEAR: Families = Families {
        matrix: false,
        linear: true,
    };

    fn allows(self, family: Family) -> bool {
        match family {
            Family::Matrix => self.matrix,
            Family::Linear => self.linear,
        }
    }
}

impl Default for Families {
    fn default() -> Self {
        Families::ALL
    }
}

/// Tuning knobs for [`locate`]. [`Default`] gives sensible live-camera values.
#[derive(Debug, Clone, Copy)]
pub struct LocateOptions {
    /// Integer factor the frame is reduced by before analysis (`>= 1`). Larger is
    /// faster but drops small codes. Default `2`.
    pub downscale: usize,
    /// Which families to report. Default [`Families::ALL`].
    pub families: Families,
    /// Maximum candidates returned, keeping the largest regions. Default `16`.
    pub max_candidates: usize,
    /// Tile edge length in reduced pixels for the texture pass. Default `8`.
    pub tile: usize,
    /// Minimum `(h + v) transitions / pixel` for a tile to count as active. Default `0.12`.
    pub edge_density: f32,
    /// Minimum active tiles for a region to be reported. Default `3`.
    pub min_region_tiles: usize,
    /// `|h - v| / (h + v)` above which a region is classed linear rather than matrix.
    /// Default `0.55`.
    pub anisotropy: f32,
    /// Seed for any randomized heuristic, kept for reproducibility. Default `0x_D0D0_CAFE`.
    pub seed: u64,
}

impl Default for LocateOptions {
    fn default() -> Self {
        LocateOptions {
            downscale: 2,
            families: Families::ALL,
            max_candidates: 16,
            tile: 8,
            edge_density: 0.12,
            min_region_tiles: 3,
            anisotropy: 0.55,
            seed: 0x_D0D0_CAFE,
        }
    }
}

/// Locate candidate code regions in `frame` without decoding them.
///
/// Returns a [`Candidate`] per region, each with a full-resolution bounding [`Quad`], a
/// coarse family guess in [`Candidate::symbology`], a cheap [`Fingerprint`], and a
/// module-size estimate in [`Location::module_size`] when a finder pattern backed the
/// region. Candidates are ordered largest-region first and capped at
/// [`LocateOptions::max_candidates`]. Deterministic for a given frame and options.
pub fn locate(frame: &GrayFrame<'_>, opts: &LocateOptions) -> Vec<Candidate> {
    // Too small to hold any code worth locating.
    if frame.width() < 8 || frame.height() < 8 {
        return Vec::new();
    }

    let grid = DownGrid::build(frame, opts.downscale);
    let finders = finder::find(&grid);
    let mut regions = tiles::regions(
        &grid,
        opts.tile,
        opts.edge_density,
        opts.min_region_tiles,
        opts.anisotropy,
    );

    // Sort strongest (largest) first so max_candidates keeps the best.
    regions.sort_by_key(|r| std::cmp::Reverse(r.area()));

    let scale = grid.scale as f32;
    let mut out: Vec<Candidate> = Vec::new();
    for region in &regions {
        if out.len() >= opts.max_candidates {
            break;
        }
        // A finder falling inside the region upgrades it to a matrix guess and lends
        // its module size.
        let hit = enclosing_finder(&finders, region);
        let family = if hit.is_some() {
            Family::Matrix
        } else {
            region.family
        };
        if !opts.families.allows(family) {
            continue;
        }

        let symbology = match (family, hit.is_some()) {
            (Family::Matrix, true) => Symbology::QrCode,
            (Family::Matrix, false) => Symbology::DataMatrix,
            (Family::Linear, _) => Symbology::Code128,
        };
        let module_size = hit.map(|h| h.module * scale);

        let outline = region_quad(region, scale);
        let location = Location {
            outline,
            rotation: None,
            module_size,
        };
        out.push(Candidate {
            location,
            symbology: Some(symbology),
            fingerprint: Some(fingerprint(&grid, region, family)),
            known: None,
        });
    }
    out
}

/// The finder hit whose centre lies inside `region`, if any (highest count wins).
fn enclosing_finder<'a>(finders: &'a [FinderHit], region: &Region) -> Option<&'a FinderHit> {
    finders
        .iter()
        .filter(|f| {
            let x = f.x as usize;
            let y = f.y as usize;
            x >= region.x0 && x < region.x1 && y >= region.y0 && y < region.y1
        })
        .max_by_key(|f| f.count)
}

/// A full-resolution bounding quad (clockwise from top-left) for a reduced-pixel region.
fn region_quad(region: &Region, scale: f32) -> Quad {
    let x0 = region.x0 as f32 * scale;
    let y0 = region.y0 as f32 * scale;
    let x1 = region.x1 as f32 * scale;
    let y1 = region.y1 as f32 * scale;
    Quad::new([
        Point::new(x0, y0),
        Point::new(x1, y0),
        Point::new(x1, y1),
        Point::new(x0, y1),
    ])
}

/// A cheap, position-tolerant signature: a coarse `4×4` darkness grid sampled over the
/// region, packed with the family tag. Robust enough to re-match a code across frames.
fn fingerprint(grid: &DownGrid, region: &Region, family: Family) -> Fingerprint {
    let mut bits: u64 = 0;
    let w = (region.x1 - region.x0).max(1);
    let h = (region.y1 - region.y0).max(1);
    // Mean darkness of the region for a per-region threshold.
    let mut sum = 0u64;
    let mut n = 0u64;
    for gy in 0..4 {
        for gx in 0..4 {
            let x = region.x0 + gx * w / 4 + w / 8;
            let y = region.y0 + gy * h / 4 + h / 8;
            sum += u64::from(grid.luma(x, y));
            n += 1;
        }
    }
    let mean = (sum / n.max(1)) as u8;
    let mut i = 0;
    for gy in 0..4 {
        for gx in 0..4 {
            let x = region.x0 + gx * w / 4 + w / 8;
            let y = region.y0 + gy * h / 4 + h / 8;
            if grid.luma(x, y) <= mean {
                bits |= 1 << i;
            }
            i += 1;
        }
    }
    let tag: u64 = match family {
        Family::Matrix => 0x1,
        Family::Linear => 0x2,
    };
    // Mix the 16 darkness bits with the family tag via a xorshift-style hash.
    let mut v = bits | (tag << 62);
    v ^= v >> 33;
    v = v.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    v ^= v >> 33;
    Fingerprint(v)
}

/// A ready-to-use [`Detect`] wrapping [`locate`] with fixed options, reusing prior-frame
/// [`Hints`] to short-circuit re-analysis: any candidate whose fingerprint matches a
/// known symbol from a previous frame is returned with that symbol attached.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameDetector {
    /// Locator options applied to every frame.
    pub options: LocateOptions,
}

impl FrameDetector {
    /// A detector with default options.
    pub fn new() -> Self {
        FrameDetector::default()
    }

    /// A detector with explicit options.
    pub fn with_options(options: LocateOptions) -> Self {
        FrameDetector { options }
    }
}

impl Detect for FrameDetector {
    fn detect(&self, frame: &GrayFrame<'_>, hints: &Hints) -> Vec<Candidate> {
        let mut candidates = locate(frame, &self.options);
        for c in &mut candidates {
            if let Some(fp) = c.fingerprint
                && let Some(known) = hints.find(fp)
            {
                c.known = Some(known.symbol.clone());
            }
        }
        candidates
    }
}
