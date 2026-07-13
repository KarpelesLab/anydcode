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
//! 4. **Gate + merge + map back**: regions that read as scene texture rather than a
//!    code are dropped — near-frame-sized blobs (dense print flood-fills into one giant
//!    region), linear regions without a scannable aspect or with text-like breaks along
//!    the bar axis, finderless matrix guesses outside code-like ink coverage. Finder
//!    hits upgrade the region they fall in to a matrix guess with a real module size
//!    (and exempt it from the size cap — a close-up code legitimately fills the frame).
//!    Survivors are ranked finder-backed first then largest, overlapping duplicates of
//!    an already-accepted box are suppressed, and finder hits claimed by no region
//!    synthesize their own candidate (huge modules leave the texture pass blind, so a
//!    close-up code would otherwise vanish). Boxes are scaled back to full resolution
//!    and returned as [`Candidate`]s, capped at [`LocateOptions::max_candidates`].
//!
//! The locator favours **recall and speed over precision**: it would rather hand
//! `Analyze` a few extra boxes than miss a code — the gates above only reject regions
//! that are structurally *not* scannable codes. It is fully deterministic; the only
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
    /// Largest fraction of the frame area a single region may cover. Textured scenes
    /// (dense print, foliage) flood-fill into one giant blob; a real code is a bounded
    /// object, so anything bigger than this is background and dropped. Default `0.6`.
    pub max_region_frac: f32,
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
            max_region_frac: 0.6,
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
/// Overlap needed (as a fraction of the *smaller* box) for a later, weaker candidate to
/// be suppressed as a duplicate of an accepted one. Real scenes cluster into overlapping
/// fragments of one physical object; without this the same code is reported several times
/// and eats the caller's candidate/crop budget. Two *distinct* codes essentially never
/// overlap at all, so half the smaller box is a safe duplicate signal.
const SUPPRESS_OVERLAP: f32 = 0.5;

/// Dark-pixel fraction bounds for a finderless matrix guess. Every 2D symbology prints
/// close to half ink; a region far outside this band is a solid blob (logo, fill) or
/// sparse print, not a matrix code. Finder-backed and linear regions are exempt — bar
/// widths legitimately skew linear ink coverage.
const MATRIX_DARK_FRAC: std::ops::Range<f32> = 0.22..0.85;

/// A linear region's extent along the reading axis must be at least this fraction of its
/// extent along the bars. A 1D code is read by a scanline crossing *every* bar, so a
/// region much taller than wide (in reading orientation) is not a scannable barcode —
/// it is almost always a column of printed text whose strokes mimic bar anisotropy.
const LINEAR_MIN_ASPECT: f32 = 0.75;

/// Reject a linear region when, walking along the bar axis, the dark mask flips more
/// often than this per reduced pixel (averaged over scanlines). Real bars are continuous
/// — a column through a barcode crosses the band once (~2 flips plus a couple more for
/// the human-readable digits); a row of text flips at every glyph boundary.
const MAX_BAR_FLIPS_PER_PX: f32 = 0.20;

/// Minimum cluster confirmations before an unclaimed finder hit is trusted enough to
/// synthesize a candidate on its own (no texture region backing it).
const FINDER_SYNTH_MIN_COUNT: u32 = 2;

pub fn locate(frame: &GrayFrame<'_>, opts: &LocateOptions) -> Vec<Candidate> {
    // Too small to hold any code worth locating.
    if frame.width() < 8 || frame.height() < 8 {
        return Vec::new();
    }

    let grid = DownGrid::build(frame, opts.downscale);
    let finders = finder::find(&grid);
    let regions = tiles::regions(
        &grid,
        opts.tile,
        opts.edge_density,
        opts.min_region_tiles,
        opts.anisotropy,
    );

    // Gate and classify regions before ranking. A region bigger than the cap is scene
    // texture (dense print flood-fills into one giant blob), not a bounded code.
    let max_area = (opts.max_region_frac * (grid.width * grid.height) as f32) as usize;
    struct Scored<'a> {
        region: Region,
        family: Family,
        hit: Option<&'a FinderHit>,
    }
    let mut scored: Vec<Scored<'_>> = Vec::new();
    for region in regions {
        // A finder falling inside the region upgrades it to a matrix guess and lends
        // its module size.
        let hit = enclosing_finder(&finders, &region);
        let finder_backed = hit.is_some_and(|h| h.count >= FINDER_SYNTH_MIN_COUNT);
        // A region bigger than the cap is scene texture — dense print flood-fills into
        // one giant blob — *unless* a confirmed finder backs it: a code held close to
        // the camera legitimately fills the frame.
        if region.area() > max_area && !finder_backed {
            continue;
        }
        let family = if hit.is_some() {
            Family::Matrix
        } else {
            region.family
        };
        if !opts.families.allows(family) {
            continue;
        }
        match family {
            Family::Linear if !linear_plausible(&grid, &region) => continue,
            // A finderless matrix guess must at least hold code-like ink coverage.
            Family::Matrix if hit.is_none() && !matrix_plausible(&grid, &region) => continue,
            _ => {}
        }
        scored.push(Scored {
            region,
            family,
            hit,
        });
    }

    // Rank: finder-backed regions first (a concentric fiducial is a far stronger signal
    // than raw texture), then largest first, so max_candidates keeps the best.
    scored.sort_by_key(|s| {
        (
            std::cmp::Reverse(u8::from(s.hit.is_some())),
            std::cmp::Reverse(s.region.area()),
        )
    });

    let scale = grid.scale as f32;
    let mut accepted: Vec<[usize; 4]> = Vec::new();
    let mut out: Vec<Candidate> = Vec::new();
    for s in &scored {
        if out.len() >= opts.max_candidates {
            break;
        }
        let core = [s.region.x0, s.region.y0, s.region.x1, s.region.y1];
        // Suppress duplicates: fragments of an object already reported.
        if accepted
            .iter()
            .any(|a| overlap_min_frac(*a, core) > SUPPRESS_OVERLAP)
        {
            continue;
        }
        accepted.push(core);

        let symbology = match (s.family, s.hit.is_some()) {
            (Family::Matrix, true) => Symbology::QrCode,
            (Family::Matrix, false) => Symbology::DataMatrix,
            (Family::Linear, _) => Symbology::Code128,
        };
        let module_size = s.hit.map(|h| h.module * scale);

        // Barcodes read across their bars, so a linear box needs the quiet zone on each
        // side of the bars to survive downstream scanning. Grow it one tile out (matrix
        // codes carry their own quiet zone inside the finder search).
        let [mut x0, mut y0, mut x1, mut y1] = core;
        if s.family == Family::Linear {
            let t = opts.tile.max(1);
            x0 = x0.saturating_sub(t);
            y0 = y0.saturating_sub(t);
            x1 = (x1 + t).min(grid.width);
            y1 = (y1 + t).min(grid.height);
        }

        let location = Location {
            outline: box_quad([x0, y0, x1, y1], scale),
            rotation: None,
            module_size,
        };
        out.push(Candidate {
            location,
            symbology: Some(symbology),
            fingerprint: Some(fingerprint(&grid, core, s.family)),
            known: None,
        });
    }

    // Finder hits not claimed by any region become candidates of their own. Close-up
    // codes defeat the texture pass entirely — at a large module size a tile sees too
    // few transitions to go active — but their finder rings still register, so without
    // this a code filling the view is never reported at all.
    if opts.families.matrix {
        for f in &finders {
            if out.len() >= opts.max_candidates {
                break;
            }
            if f.count < FINDER_SYNTH_MIN_COUNT {
                continue;
            }
            let (fx, fy) = (f.x as usize, f.y as usize);
            if accepted
                .iter()
                .any(|&[x0, y0, x1, y1]| fx >= x0 && fx < x1 && fy >= y0 && fy < y1)
            {
                continue;
            }
            // The version is unknown here, so size the box for a small symbol (25
            // modules ≈ version 2) around the finder; the decode pass self-localizes
            // within whatever crop the caller derives from it.
            let half = f.module * 12.5;
            let x0 = (f.x - half).max(0.0) as usize;
            let y0 = (f.y - half).max(0.0) as usize;
            let x1 = ((f.x + half).max(0.0) as usize).min(grid.width);
            let y1 = ((f.y + half).max(0.0) as usize).min(grid.height);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let core = [x0, y0, x1, y1];
            accepted.push(core);
            out.push(Candidate {
                location: Location {
                    outline: box_quad(core, scale),
                    rotation: None,
                    module_size: Some(f.module * scale),
                },
                symbology: Some(Symbology::QrCode),
                fingerprint: Some(fingerprint(&grid, core, Family::Matrix)),
                known: None,
            });
        }
    }
    out
}

/// Intersection area of two boxes as a fraction of the smaller box's area.
fn overlap_min_frac(a: [usize; 4], b: [usize; 4]) -> f32 {
    let ix = a[2].min(b[2]).saturating_sub(a[0].max(b[0]));
    let iy = a[3].min(b[3]).saturating_sub(a[1].max(b[1]));
    let inter = (ix * iy) as f32;
    let area_a = ((a[2] - a[0]) * (a[3] - a[1])) as f32;
    let area_b = ((b[2] - b[0]) * (b[3] - b[1])) as f32;
    let min = area_a.min(area_b);
    if min > 0.0 { inter / min } else { 0.0 }
}

/// Ink-coverage gate for a finderless matrix guess: the dark fraction of the region
/// under the frame's binarization must sit in the code-like band [`MATRIX_DARK_FRAC`].
fn matrix_plausible(grid: &DownGrid, region: &Region) -> bool {
    let area = region.area();
    if area == 0 {
        return false;
    }
    let mut dark = 0usize;
    for y in region.y0..region.y1 {
        for x in region.x0..region.x1 {
            dark += usize::from(grid.dark(x, y));
        }
    }
    MATRIX_DARK_FRAC.contains(&(dark as f32 / area as f32))
}

/// Structural gate for a linear-family region: scannable aspect and coherent bars.
///
/// Printed text shares a barcode's edge anisotropy but not its structure: a column of
/// text is taller than a scanline can use, and a row of text breaks up along the bar
/// axis where real bars run unbroken. Both checks are orientation-normalized via
/// [`Region::reads_horizontal`].
fn linear_plausible(grid: &DownGrid, region: &Region) -> bool {
    let w = region.x1 - region.x0;
    let h = region.y1 - region.y0;
    if w == 0 || h == 0 {
        return false;
    }
    // Extent along the reading axis vs along the bars.
    let (read, bars) = if region.reads_horizontal {
        (w, h)
    } else {
        (h, w)
    };
    if (read as f32) < LINEAR_MIN_ASPECT * bars as f32 {
        return false;
    }
    // Flips of the dark mask along the bar axis, averaged per scanline, per pixel.
    let mut flips = 0u32;
    for u in 0..read {
        let mut prev: Option<bool> = None;
        for v in 0..bars {
            let (x, y) = if region.reads_horizontal {
                (region.x0 + u, region.y0 + v)
            } else {
                (region.x0 + v, region.y0 + u)
            };
            let d = grid.dark(x, y);
            if prev == Some(!d) {
                flips += 1;
            }
            prev = Some(d);
        }
    }
    let per_px = flips as f32 / (read * bars) as f32;
    per_px <= MAX_BAR_FLIPS_PER_PX
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

/// A full-resolution bounding quad (clockwise from top-left) for a reduced-pixel box.
fn box_quad(b: [usize; 4], scale: f32) -> Quad {
    let x0 = b[0] as f32 * scale;
    let y0 = b[1] as f32 * scale;
    let x1 = b[2] as f32 * scale;
    let y1 = b[3] as f32 * scale;
    Quad::new([
        Point::new(x0, y0),
        Point::new(x1, y0),
        Point::new(x1, y1),
        Point::new(x0, y1),
    ])
}

/// A cheap, position-tolerant signature: a coarse `4×4` darkness grid sampled over the
/// reduced-pixel box, packed with the family tag. Robust enough to re-match a code
/// across frames.
fn fingerprint(grid: &DownGrid, b: [usize; 4], family: Family) -> Fingerprint {
    let mut bits: u64 = 0;
    let w = (b[2] - b[0]).max(1);
    let h = (b[3] - b[1]).max(1);
    // Mean darkness of the region for a per-region threshold.
    let mut sum = 0u64;
    let mut n = 0u64;
    for gy in 0..4 {
        for gx in 0..4 {
            let x = b[0] + gx * w / 4 + w / 8;
            let y = b[1] + gy * h / 4 + h / 8;
            sum += u64::from(grid.luma(x, y));
            n += 1;
        }
    }
    let mean = (sum / n.max(1)) as u8;
    let mut i = 0;
    for gy in 0..4 {
        for gx in 0..4 {
            let x = b[0] + gx * w / 4 + w / 8;
            let y = b[1] + gy * h / 4 + h / 8;
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
