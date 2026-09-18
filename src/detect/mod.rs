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
//!    [`LocateOptions::downscale`] and threshold the result against its *local*
//!    neighbourhood mean, so a lighting falloff or a code on a dark surface still
//!    binarizes cleanly. Everything after this works on the small image, which is what
//!    makes the locator fast enough to run on every frame.
//! 2. **Concentric finders** (`finder`): a run-length row scan with a vertical
//!    cross-check flags QR/Aztec `1:1:3:1:1` fiducials. These give a high-confidence
//!    matrix classification and a module-size estimate.
//! 3. **Texture tiles** (`tiles`): each dense-edged tile is first *labelled* by the
//!    coherence of its own gradient field — one-directional (a 1D barcode patch, at
//!    *any* rotation) or not (a 2D matrix, text, texture) — and only compatible tiles
//!    are then clustered into regions. Labelling per tile before clustering is what
//!    lets a barcode be pulled out of the text and artwork it sits amongst, catching
//!    every finder-less symbology without the barcode dissolving into the surrounding
//!    scene. A linear region carries its reading axis and a box aligned with it.
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
//!
//! # Linear candidates are oriented
//!
//! A linear candidate's [`Location::outline`] is a box aligned with the code, not with
//! the frame, and its [`Location::rotation`] is the code's reading axis (radians,
//! clockwise from +x, in `(-π/2, π/2]` — an axis, so a code and its upside-down self
//! report the same value). Crop the outline's bounds and hand the axis to
//! [`crate::pipeline::scan_1d_at`] to read it without an orientation search.

use alloc::vec::Vec;
use std::eprintln;
use std::sync::OnceLock;

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
    /// Minimum fraction of a tile's pixels that must be edge pixels (strong local
    /// gradient) for the tile to count as active. Default `0.12`.
    pub edge_density: f32,
    /// Minimum active tiles for a region to be reported. Default `3`.
    pub min_region_tiles: usize,
    /// Gradient coherence (`0` = edges in every direction, `1` = all edges parallel) at
    /// or above which a tile is classed linear rather than matrix. Rotation-invariant.
    /// Default `0.7`.
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
            anisotropy: 0.7,
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

/// Dark-pixel fraction bounds (under the locally thresholded mask) for a finderless
/// matrix guess. Every 2D symbology prints close to half ink; a region far outside this
/// band is a solid blob (logo, fill) or sparse print, not a matrix code. Finder-backed and linear regions are exempt — bar
/// widths legitimately skew linear ink coverage.
const MATRIX_DARK_FRAC: core::ops::Range<f32> = 0.22..0.85;

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

/// Minimum cluster confirmations (scan rows agreeing on one centre) before a finder hit
/// is trusted — to vouch for the region it sits in, or to synthesize a candidate on its
/// own. A real finder's centre is three modules tall, so even at the smallest module
/// the locator resolves it is crossed by several rows; dense print throws up the odd
/// `1:1:3:1:1` run with a matching column, but not on row after row.
const FINDER_SYNTH_MIN_COUNT: u32 = 3;

/// Least luminance spread (90th − 10th percentile) inside a finderless matrix guess. A
/// 2D code is two well-separated tones; the local threshold will happily split the
/// grain of a dark surface or a shadow into "half ink" too, and only the spread tells
/// them apart.
const MATRIX_MIN_SPREAD: u32 = 56;

/// Least share of its oriented box a linear region's tiles must cover (see
/// `OrientedBox::fill`). Gap bridging and the wide runs of a coarse code leave holes in
/// a genuine barcode's tile set too, hence well under one half.
const LINEAR_MIN_FILL: f32 = 0.35;

/// Fewest dark/light flips a line along the reading axis must cross inside a linear
/// region. The shortest linear symbols have well over a dozen bars; an icon outline, a
/// rule, a box border or a letter or two have a handful of parallel strokes at most.
const MIN_BARS_FLIPS: u32 = 10;

/// The same spread floor for a linear region. Lower, because bars are easier to confirm
/// than a matrix (the scan that follows is cheap and self-validating) and thin bars
/// blurred into their spaces genuinely lose contrast — but grain on a dark surface with
/// a brushed direction is "coherent" too, and has almost none.
const LINEAR_MIN_SPREAD: u32 = 40;

pub fn locate(frame: &GrayFrame<'_>, opts: &LocateOptions) -> Vec<Candidate> {
    // Too small to hold any code worth locating.
    if frame.width() < 8 || frame.height() < 8 {
        return Vec::new();
    }

    let grid = DownGrid::build(frame, opts.downscale);
    let finders = finder::find(&grid);
    // Read once: an environment lookup takes a process-wide lock, and this runs on
    // every frame.
    static DEBUG: OnceLock<bool> = OnceLock::new();
    let debug = *DEBUG.get_or_init(|| std::env::var_os("ANYD_LOC_DEBUG").is_some());
    let regions = tiles::regions(
        &grid,
        opts.tile,
        opts.edge_density,
        opts.min_region_tiles,
        opts.anisotropy,
        debug,
    );

    // Gate and classify regions before ranking. A region bigger than the cap is scene
    // texture (dense print flood-fills into one giant blob), not a bounded code.
    let max_area = (opts.max_region_frac * (grid.width * grid.height) as f32) as usize;
    struct Scored<'a> {
        region: Region,
        family: Family,
        hit: Option<&'a FinderHit>,
    }
    if debug {
        for f in &finders {
            eprintln!(
                "finder ({},{}) module={:.2} count={}",
                f.x as usize * grid.scale,
                f.y as usize * grid.scale,
                f.module,
                f.count
            );
        }
    }
    let mut scored: Vec<Scored<'_>> = Vec::new();
    for region in regions {
        if debug {
            eprintln!(
                "region ({},{})-({},{}) {:?} axis={:?} area={} lin_ok={} mat_ok={}",
                region.x0 * grid.scale,
                region.y0 * grid.scale,
                region.x1 * grid.scale,
                region.y1 * grid.scale,
                region.family,
                region.oriented.map(|o| o.angle.to_degrees()),
                region.area(),
                linear_plausible(&grid, &region),
                matrix_plausible(&grid, &region),
            );
        }
        // A confirmed finder inside a matrix-textured region vouches for it and lends
        // its module size. A *linear* region keeps its family: its bars are coherent
        // evidence of their own, and bar patterns throw up finder-like runs.
        let hit = enclosing_finder(&finders, &region)
            .filter(|h| h.count >= FINDER_SYNTH_MIN_COUNT && region.family == Family::Matrix);
        let finder_backed = hit.is_some();
        // A region bigger than the cap is scene texture — dense print flood-fills into
        // one giant blob — *unless* a confirmed finder backs it: a code held close to
        // the camera legitimately fills the frame.
        if region.area() > max_area && !finder_backed {
            continue;
        }
        let family = region.family;
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

    // Rank by strength of evidence, then largest first, so max_candidates (and a
    // caller's decode budget) keeps the best: a concentric fiducial is the strongest
    // signal there is; a coherent, structurally plausible bar field is next — and the
    // cheapest to confirm; a finderless matrix guess is only "textured like a code".
    scored.sort_by_key(|s| {
        let class = match (s.hit.is_some(), s.family) {
            (true, _) => 0u8,
            (false, Family::Linear) => 1,
            (false, Family::Matrix) => 2,
        };
        (class, core::cmp::Reverse(s.region.area()))
    });

    let scale = grid.scale as f32;
    // Accepted core boxes with their family and whether a finder backs them.
    let mut accepted: Vec<([usize; 4], Family, bool)> = Vec::new();
    let mut out: Vec<Candidate> = Vec::new();
    for s in &scored {
        if out.len() >= opts.max_candidates {
            break;
        }
        let core = [s.region.x0, s.region.y0, s.region.x1, s.region.y1];
        // Suppress duplicates: fragments of an object already reported. A bar field is
        // not a fragment of a *finderless* matrix box, though — a barcode printed amid
        // text sits inside the text's texture blob, and is the one thing in it worth
        // reading. Inside a finder-backed box it is: a QR's finder rings and timing
        // tracks are coherent stripes too.
        if accepted.iter().any(|&(a, family, finder)| {
            !(s.family == Family::Linear && family == Family::Matrix && !finder)
                && overlap_min_frac(a, core) > SUPPRESS_OVERLAP
        }) {
            continue;
        }
        accepted.push((core, s.family, s.hit.is_some()));

        let symbology = match (s.family, s.hit.is_some()) {
            (Family::Matrix, true) => Symbology::QrCode,
            (Family::Matrix, false) => Symbology::DataMatrix,
            (Family::Linear, _) => Symbology::Code128,
        };
        let module_size = s.hit.map(|h| h.module * scale);

        // Barcodes read across their bars, so a linear box needs the quiet zone on each
        // side of the bars to survive downstream scanning: grow it along the reading
        // axis (matrix codes carry their own quiet zone inside the finder search).
        let location = match (s.family, s.region.oriented) {
            (Family::Linear, Some(mut o)) => {
                let t = opts.tile.max(1) as f32;
                o.half_read += 1.5 * t;
                o.half_bars += 0.25 * t;
                let (w, h) = (grid.width as f32, grid.height as f32);
                let corner = |(x, y): (f32, f32)| {
                    Point::new(x.clamp(0.0, w) * scale, y.clamp(0.0, h) * scale)
                };
                let cs = o.corners();
                Location {
                    outline: Quad::new([
                        corner(cs[0]),
                        corner(cs[1]),
                        corner(cs[2]),
                        corner(cs[3]),
                    ]),
                    rotation: Some(o.angle),
                    module_size,
                }
            }
            _ => {
                // The texture cluster stops at the last edge-dense tile, which on a
                // rotated or small symbol cuts its corners and quiet zone off; a
                // sampler needs both. One tile out restores them.
                let t = opts.tile.max(1);
                let grown = [
                    core[0].saturating_sub(t),
                    core[1].saturating_sub(t),
                    (core[2] + t).min(grid.width),
                    (core[3] + t).min(grid.height),
                ];
                Location {
                    outline: box_quad(grown, scale),
                    rotation: None,
                    module_size,
                }
            }
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
                .any(|&([x0, y0, x1, y1], _, _)| fx >= x0 && fx < x1 && fy >= y0 && fy < y1)
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
            accepted.push((core, Family::Matrix, true));
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

/// Ink gate for a finderless matrix guess: the dark fraction of the region under the
/// frame's local binarization must sit in the code-like band [`MATRIX_DARK_FRAC`], and
/// its two tones must actually be apart ([`MATRIX_MIN_SPREAD`]).
fn matrix_plausible(grid: &DownGrid, region: &Region) -> bool {
    let area = region.area();
    if area == 0 {
        return false;
    }
    let mut dark = 0usize;
    let mut hist = [0u32; 256];
    for y in region.y0..region.y1 {
        for x in region.x0..region.x1 {
            dark += usize::from(grid.dark(x, y));
            hist[usize::from(grid.luma(x, y))] += 1;
        }
    }
    MATRIX_DARK_FRAC.contains(&(dark as f32 / area as f32))
        && spread(&hist, area as u32) >= MATRIX_MIN_SPREAD
}

/// Distance between the 10th and 90th percentiles of a luminance histogram of `count`
/// samples.
fn spread(hist: &[u32; 256], count: u32) -> u32 {
    let percentile = |frac: f32| {
        let target = (frac * count as f32) as u32;
        let mut seen = 0u32;
        hist.iter()
            .position(|&n| {
                seen += n;
                seen > target
            })
            .unwrap_or(255) as u32
    };
    percentile(0.9) - percentile(0.1)
}

/// Structural gate for a linear-family region: scannable aspect and coherent bars.
///
/// Printed text shares a barcode's one-directional strokes but not its structure: a
/// column of text is taller than a scanline can use, and a row of text breaks up along
/// the bar axis where real bars run unbroken. Both checks are made in the region's own
/// frame, so they hold at any rotation.
fn linear_plausible(grid: &DownGrid, region: &Region) -> bool {
    let Some(o) = region.oriented else {
        return false;
    };
    if o.half_read <= 0.0 || o.half_bars <= 0.0 {
        return false;
    }
    if o.half_read < LINEAR_MIN_ASPECT * o.half_bars || o.fill < LINEAR_MIN_FILL {
        return false;
    }
    // Flips of the dark mask walking along the bars, averaged per pixel walked, on
    // lines spread across the reading axis.
    let (sin, cos) = o.angle.sin_cos();
    let (mut flips, mut walked) = (0u32, 0u32);
    let mut hist = [0u32; 256];
    let lines = ((2.0 * o.half_read) as usize / 2).clamp(4, 96);
    let steps = (2.0 * o.half_bars) as usize;
    for l in 0..lines {
        let u = -o.half_read + (l as f32 + 0.5) / lines as f32 * 2.0 * o.half_read;
        let mut prev: Option<bool> = None;
        for k in 0..steps {
            let v = -o.half_bars + k as f32 + 0.5;
            let x = o.cx + cos * u - sin * v;
            let y = o.cy + sin * u + cos * v;
            if x < 0.0 || y < 0.0 || x >= grid.width as f32 || y >= grid.height as f32 {
                prev = None;
                continue;
            }
            let d = grid.dark(x as usize, y as usize);
            if prev == Some(!d) {
                flips += 1;
            }
            prev = Some(d);
            hist[usize::from(grid.luma(x as usize, y as usize))] += 1;
            walked += 1;
        }
    }
    if walked == 0
        || flips as f32 / walked as f32 > MAX_BAR_FLIPS_PER_PX
        || spread(&hist, walked) < LINEAR_MIN_SPREAD
    {
        return false;
    }

    // Enough bars: the best of three lines along the reading axis must cross a
    // barcode's worth of edges.
    let steps = (2.0 * o.half_read) as usize;
    let most = [-0.4f32, 0.0, 0.4]
        .iter()
        .map(|&f| {
            let v = f * o.half_bars;
            let mut prev: Option<bool> = None;
            let mut n = 0u32;
            for k in 0..steps {
                let u = -o.half_read + k as f32 + 0.5;
                let x = o.cx + cos * u - sin * v;
                let y = o.cy + sin * u + cos * v;
                if x < 0.0 || y < 0.0 || x >= grid.width as f32 || y >= grid.height as f32 {
                    continue;
                }
                let d = grid.dark(x as usize, y as usize);
                n += u32::from(prev == Some(!d));
                prev = Some(d);
            }
            n
        })
        .max()
        .unwrap_or(0);
    most >= MIN_BARS_FLIPS
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

/// A cheap signature of a region: which of its `4×4` cells are darker than the region's
/// mean, plus the family tag in the top bits.
///
/// Each bit compares a whole cell's mean with the region's, so sensor noise and a pixel
/// or two of box jitter rarely flip one — but "rarely" is not "never", which is why
/// [`FrameDetector`] matches fingerprints by [`fingerprint_distance`] rather than by
/// equality.
fn fingerprint(grid: &DownGrid, b: [usize; 4], family: Family) -> Fingerprint {
    let b = ink_bounds(grid, b);
    let w = (b[2] - b[0]).max(1);
    let h = (b[3] - b[1]).max(1);
    let mut cell = [0u64; 16];
    let mut count = [0u64; 16];
    for y in b[1]..b[3].min(grid.height) {
        let gy = ((y - b[1]) * 4 / h).min(3);
        for x in b[0]..b[2].min(grid.width) {
            let gx = ((x - b[0]) * 4 / w).min(3);
            cell[gy * 4 + gx] += u64::from(grid.luma(x, y));
            count[gy * 4 + gx] += 1;
        }
    }
    let total: u64 = cell.iter().sum();
    let n: u64 = count.iter().sum();
    let mut bits: u64 = 0;
    for i in 0..16 {
        // cell mean <= region mean, cross-multiplied to stay in integers.
        if count[i] > 0 && cell[i] * n <= total * count[i] {
            bits |= 1 << i;
        }
    }
    let tag: u64 = match family {
        Family::Matrix => 0x1,
        Family::Linear => 0x2,
    };
    Fingerprint(bits | (tag << 62))
}

/// Shrink (or grow, by up to a tile) a tile-snapped box to the extent of the ink in it.
///
/// Region boxes snap to the tile grid, so a code that moves three pixels can move its
/// box by a whole tile — and a darkness grid laid over the *box* then samples different
/// parts of the code. Anchoring the grid to the ink itself makes the signature follow
/// the code instead of the tiling.
fn ink_bounds(grid: &DownGrid, b: [usize; 4]) -> [usize; 4] {
    const SLACK: usize = 8;
    let x0 = b[0].saturating_sub(SLACK);
    let y0 = b[1].saturating_sub(SLACK);
    let x1 = (b[2] + SLACK).min(grid.width);
    let y1 = (b[3] + SLACK).min(grid.height);
    if x1 <= x0 || y1 <= y0 {
        return b;
    }
    let mut cols = alloc::vec![0u32; x1 - x0];
    let mut rows = alloc::vec![0u32; y1 - y0];
    for y in y0..y1 {
        for x in x0..x1 {
            if grid.dark(x, y) {
                cols[x - x0] += 1;
                rows[y - y0] += 1;
            }
        }
    }
    // A row/column belongs to the code when more than a stray speck of it is dark.
    let span = |counts: &[u32], across: usize| {
        let need = (across as u32 / 16).max(2);
        let first = counts.iter().position(|&n| n >= need)?;
        let last = counts.iter().rposition(|&n| n >= need)?;
        Some((first, last + 1))
    };
    match (span(&cols, y1 - y0), span(&rows, x1 - x0)) {
        (Some((cx0, cx1)), Some((ry0, ry1))) => [x0 + cx0, y0 + ry0, x0 + cx1, y0 + ry1],
        _ => b,
    }
}

/// How far apart two [`locate`] fingerprints are: the number of differing darkness
/// cells, or `None` when they are of different families (never the same code).
pub fn fingerprint_distance(a: Fingerprint, b: Fingerprint) -> Option<u32> {
    ((a.0 >> 62) == (b.0 >> 62)).then(|| ((a.0 ^ b.0) & 0xFFFF).count_ones())
}

/// A ready-to-use [`Detect`] wrapping [`locate`] with fixed options, reusing prior-frame
/// [`Hints`] to short-circuit re-analysis: a candidate that is evidently the same
/// physical code as a known symbol from a previous frame is returned with that symbol
/// attached.
///
/// "Evidently the same" means **both** that it sits where the known symbol was (their
/// boxes overlap by [`KNOWN_MIN_IOU`]) **and** that it looks like it (fingerprints within
/// [`KNOWN_MAX_DISTANCE`] cells). Neither alone will do on a camera stream: an exact
/// fingerprint match almost never survives sensor noise and hand shake from one frame
/// to the next, so nothing would ever be reused; position alone would keep reporting
/// the old value after a different code is slid into the same spot.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameDetector {
    /// Locator options applied to every frame.
    pub options: LocateOptions,
}

/// Least box overlap (intersection over union) between a candidate and a known symbol's
/// last location for them to be the same code. Hand-held video moves a code a fraction
/// of its size per frame.
pub const KNOWN_MIN_IOU: f32 = 0.4;

/// Most fingerprint cells (of 16) that may differ between a candidate and a known symbol.
pub const KNOWN_MAX_DISTANCE: u32 = 2;

/// Axis-aligned bounds `[x0, y0, x1, y1]` of a quad.
fn bounds(q: &Quad) -> [f32; 4] {
    let cs = q.corners;
    [
        cs.iter().map(|p| p.x).fold(f32::MAX, f32::min),
        cs.iter().map(|p| p.y).fold(f32::MAX, f32::min),
        cs.iter().map(|p| p.x).fold(f32::MIN, f32::max),
        cs.iter().map(|p| p.y).fold(f32::MIN, f32::max),
    ]
}

/// Intersection over union of two boxes.
fn iou(a: [f32; 4], b: [f32; 4]) -> f32 {
    let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let union = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - ix * iy;
    if union > 0.0 { ix * iy / union } else { 0.0 }
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
            let Some(fp) = c.fingerprint else { continue };
            let here = bounds(&c.location.outline);
            let best = hints
                .previous
                .iter()
                .filter_map(|k| {
                    let distance = fingerprint_distance(fp, k.fingerprint?)?;
                    let there = bounds(&k.symbol.location.as_ref()?.outline);
                    (distance <= KNOWN_MAX_DISTANCE && iou(here, there) >= KNOWN_MIN_IOU)
                        .then_some((distance, k))
                })
                .min_by_key(|&(distance, _)| distance);
            if let Some((_, known)) = best {
                c.known = Some(known.symbol.clone());
            }
        }
        candidates
    }
}
