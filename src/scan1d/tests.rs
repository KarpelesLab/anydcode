//! Tests for the generic 1D front-end.
//!
//! These render *known* [`LinearPattern`]s to a synthetic [`GrayFrame`] (the tiny
//! renderer below is test-only), run the front-end, and assert the recovered
//! pattern equals the original module-for-module — proving correctness against an
//! independent ground truth, not mere self-consistency.

use super::*;
use crate::error::Result;
use crate::symbology::Symbology;
use crate::traits::Decode;

// --- Test-only rendering & signal helpers ----------------------------------

const DARK: u8 = 30;
const LIGHT: u8 = 225;

/// A small seeded PRNG (SplitMix64-style) for reproducible noise.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Signed noise in `[-amp, amp]`.
    fn noise(&mut self, amp: i32) -> i32 {
        let span = (2 * amp + 1) as u64;
        (self.next_u64() % span) as i32 - amp
    }
}

/// Build a module vector from alternating bar/space widths starting with a bar.
/// The width count must be odd so the pattern also ends with a bar.
fn pattern_from_widths(widths: &[usize], quiet_zone: usize) -> LinearPattern {
    assert!(widths.len() % 2 == 1, "must start and end with a bar");
    let mut modules = Vec::new();
    for (i, &w) in widths.iter().enumerate() {
        let is_bar = i % 2 == 0;
        for _ in 0..w {
            modules.push(is_bar);
        }
    }
    LinearPattern {
        modules,
        quiet_zone,
    }
}

/// Render a pattern full-width: `module_px` pixels per module, dark bars on a light
/// field with `quiet_zone` light modules on each side. Returns `(buf, width, height)`.
fn render(pattern: &LinearPattern, module_px: usize, height: usize) -> (Vec<u8>, usize, usize) {
    render_padded(pattern, module_px, height, 0)
}

/// Like [`render`] but with `pad_px` extra light columns on each side (the barcode
/// then does not span the full frame width).
fn render_padded(
    pattern: &LinearPattern,
    module_px: usize,
    height: usize,
    pad_px: usize,
) -> (Vec<u8>, usize, usize) {
    let qz = pattern.quiet_zone;
    let total_modules = 2 * qz + pattern.modules.len();
    let bar_w = total_modules * module_px;
    let width = bar_w + 2 * pad_px;
    let mut buf = vec![LIGHT; width * height];
    for y in 0..height {
        for x in 0..width {
            let lum = if x < pad_px || x >= pad_px + bar_w {
                LIGHT
            } else {
                let col_module = (x - pad_px) / module_px;
                if col_module < qz || col_module >= qz + pattern.modules.len() {
                    LIGHT
                } else if pattern.modules[col_module - qz] {
                    DARK
                } else {
                    LIGHT
                }
            };
            buf[y * width + x] = lum;
        }
    }
    (buf, width, height)
}

/// Render a pattern rotated by `phi_deg` (barcode extends over the full frame
/// height, so quiet zones appear as slanted light bands). Square frame of `side` px.
fn render_rotated(
    pattern: &LinearPattern,
    module_px: usize,
    phi_deg: f32,
    side: usize,
) -> (Vec<u8>, usize, usize) {
    let qz = pattern.quiet_zone;
    let total_modules = 2 * qz + pattern.modules.len();
    let bar_w = (total_modules * module_px) as f32;
    let phi = phi_deg.to_radians();
    let (sin, cos) = phi.sin_cos();
    let half = (side as f32) / 2.0;
    let mut buf = vec![LIGHT; side * side];
    for y in 0..side {
        for x in 0..side {
            let dx = (x as f32) - half;
            let dy = (y as f32) - half;
            // Inverse-rotate the sample point into the axis-aligned barcode frame.
            let u = dx * cos + dy * sin;
            let uu = u + bar_w / 2.0;
            let lum = if uu < 0.0 || uu >= bar_w {
                LIGHT
            } else {
                let col_module = (uu as usize) / module_px;
                if col_module < qz || col_module >= qz + pattern.modules.len() {
                    LIGHT
                } else if pattern.modules[col_module - qz] {
                    DARK
                } else {
                    LIGHT
                }
            };
            buf[y * side + x] = lum;
        }
    }
    (buf, side, side)
}

/// Separable box blur of the given radius (defocus model).
fn box_blur(buf: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    // Horizontal pass.
    let mut tmp = vec![0u16; width * height];
    for y in 0..height {
        for x in 0..width {
            let lo = x.saturating_sub(radius);
            let hi = (x + radius).min(width - 1);
            let mut sum = 0u32;
            for xx in lo..=hi {
                sum += buf[y * width + xx] as u32;
            }
            tmp[y * width + x] = (sum / ((hi - lo + 1) as u32)) as u16;
        }
    }
    // Vertical pass.
    let mut out = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let lo = y.saturating_sub(radius);
            let hi = (y + radius).min(height - 1);
            let mut sum = 0u32;
            for yy in lo..=hi {
                sum += tmp[yy * width + x] as u32;
            }
            out[y * width + x] = (sum / ((hi - lo + 1) as u32)) as u8;
        }
    }
    out
}

/// Add reproducible signed noise to every pixel.
fn add_noise(buf: &mut [u8], amp: i32, seed: u64) {
    let mut rng = Rng::new(seed);
    for p in buf.iter_mut() {
        let v = (*p as i32 + rng.noise(amp)).clamp(0, 255);
        *p = v as u8;
    }
}

/// Whether any returned candidate reproduces `pattern.modules` exactly.
fn recovers_modules(cands: &[LinearCandidate], pattern: &LinearPattern) -> bool {
    cands.iter().any(|c| c.pattern.modules == pattern.modules)
}

/// Whether any candidate reproduces the whole pattern (modules and quiet zone).
fn recovers_exact(cands: &[LinearCandidate], pattern: &LinearPattern) -> bool {
    cands.iter().any(|c| &c.pattern == pattern)
}

/// A selection of representative patterns (each starts and ends with a bar).
fn sample_patterns() -> Vec<LinearPattern> {
    vec![
        pattern_from_widths(&[1, 1, 1, 1, 1], 10),
        pattern_from_widths(&[2, 1, 1, 3, 1, 1, 2], 10),
        pattern_from_widths(&[3, 1, 2, 2, 1, 1, 1, 1, 3], 12),
        pattern_from_widths(&[1, 2, 3, 2, 1, 2, 1, 3, 1, 1, 2], 10),
    ]
}

// --- Tests -----------------------------------------------------------------

#[test]
fn recovers_clean_patterns_across_scales() {
    for pattern in sample_patterns() {
        for &module_px in &[2usize, 3, 4, 6] {
            let (buf, w, h) = render(&pattern, module_px, 40);
            let frame = GrayFrame::new(&buf, w, h).unwrap();
            let cands = scan_lines(&frame, &ScanOptions::default());
            assert!(
                recovers_exact(&cands, &pattern),
                "exact recovery failed at module_px={module_px} for {pattern:?}",
            );
        }
    }
}

#[test]
fn module_size_estimate_is_close() {
    let pattern = pattern_from_widths(&[2, 1, 1, 3, 1, 1, 2], 10);
    let module_px = 5usize;
    let (buf, w, h) = render(&pattern, module_px, 30);
    let frame = GrayFrame::new(&buf, w, h).unwrap();
    let cands = scan_lines(&frame, &ScanOptions::default());
    let best = cands
        .iter()
        .find(|c| c.pattern.modules == pattern.modules)
        .expect("pattern recovered");
    let est = best.location.module_size.unwrap();
    assert!(
        (est - module_px as f32).abs() < 0.5,
        "module size estimate {est} far from {module_px}",
    );
    assert!(
        best.confidence > 0.8,
        "confidence too low: {}",
        best.confidence
    );
}

#[test]
fn recovers_under_blur() {
    for pattern in sample_patterns() {
        // Larger modules so a few-pixel blur stays sub-module.
        let module_px = 6usize;
        for &radius in &[1usize, 2, 3] {
            let (buf, w, h) = render(&pattern, module_px, 40);
            let blurred = box_blur(&buf, w, h, radius);
            let frame = GrayFrame::new(&blurred, w, h).unwrap();
            let cands = scan_lines(&frame, &ScanOptions::default());
            assert!(
                recovers_modules(&cands, &pattern),
                "blur radius {radius} broke recovery for {pattern:?}",
            );
        }
    }
}

#[test]
fn recovers_under_mild_rotation() {
    // The default angle set includes ±3 and ±6 degrees; a scan aligned to the
    // barcode's tilt recovers it cleanly.
    for &phi in &[3.0f32, 6.0, -3.0, -6.0] {
        let pattern = pattern_from_widths(&[2, 1, 1, 3, 1, 1, 2, 1, 3], 12);
        let module_px = 4usize;
        let side = 320usize;
        let (buf, w, h) = render_rotated(&pattern, module_px, phi, side);
        let frame = GrayFrame::new(&buf, w, h).unwrap();
        let cands = scan_lines(&frame, &ScanOptions::default());
        assert!(
            recovers_modules(&cands, &pattern),
            "rotation {phi} deg broke recovery",
        );
    }
}

#[test]
fn detects_quiet_zone() {
    // Full-width render: measured quiet zone equals the rendered one.
    let pattern = pattern_from_widths(&[1, 1, 2, 1, 1, 1, 3], 10);
    let (buf, w, h) = render(&pattern, 4, 30);
    let frame = GrayFrame::new(&buf, w, h).unwrap();
    let cands = scan_lines(&frame, &ScanOptions::default());
    let best = cands
        .iter()
        .find(|c| c.pattern.modules == pattern.modules)
        .expect("pattern recovered");
    assert_eq!(best.pattern.quiet_zone, pattern.quiet_zone);
}

#[test]
fn tolerates_barcode_not_spanning_full_width() {
    // Extra light padding on both sides: the modules still recover exactly and a
    // quiet zone at least as large as the rendered one is reported.
    let pattern = pattern_from_widths(&[2, 1, 1, 3, 1, 1, 2], 10);
    let module_px = 4usize;
    let (buf, w, h) = render_padded(&pattern, module_px, 30, 37);
    let frame = GrayFrame::new(&buf, w, h).unwrap();
    let cands = scan_lines(&frame, &ScanOptions::default());
    let best = cands
        .iter()
        .find(|c| c.pattern.modules == pattern.modules)
        .expect("pattern recovered despite side padding");
    assert!(
        best.pattern.quiet_zone >= pattern.quiet_zone,
        "quiet zone {} should be >= {}",
        best.pattern.quiet_zone,
        pattern.quiet_zone,
    );
}

#[test]
fn recovers_from_noisy_profile() {
    let pattern = pattern_from_widths(&[2, 1, 1, 3, 1, 1, 2, 1, 3], 10);
    let module_px = 6usize;
    let (mut buf, w, h) = render(&pattern, module_px, 40);
    add_noise(&mut buf, 30, 0xC0FF_EE12);
    let frame = GrayFrame::new(&buf, w, h).unwrap();
    let opts = ScanOptions {
        smooth_radius: 2,
        ..ScanOptions::default()
    };
    let cands = scan_lines(&frame, &opts);
    assert!(
        recovers_modules(&cands, &pattern),
        "noisy profile broke recovery",
    );
}

#[test]
fn flat_frame_yields_no_candidates() {
    let buf = vec![200u8; 100 * 20];
    let frame = GrayFrame::new(&buf, 100, 20).unwrap();
    let cands = scan_lines(&frame, &ScanOptions::default());
    assert!(cands.is_empty(), "flat frame must not produce candidates");
}

/// A stand-in linear decoder proving generic `&dyn Decode` dispatch: it accepts any
/// linear pattern with an even number of dark modules and echoes a fixed symbol.
struct EvenBarDecoder;

impl Decode for EvenBarDecoder {
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        match encoding {
            Encoding::Linear(p) => {
                let bars = p.modules.iter().filter(|&&b| b).count();
                if bars % 2 == 0 {
                    Ok(Symbol::new(
                        Symbology::Code39,
                        Vec::new(),
                        crate::symbol::SymbolMeta::Generic,
                    ))
                } else {
                    Err(crate::error::Error::undecodable("odd bar count"))
                }
            }
            _ => Err(crate::error::Error::Unsupported { what: "not linear" }),
        }
    }
}

#[test]
fn try_decode_dispatches_to_generic_decoder() {
    // Craft a pattern with an even number of dark modules so the stub accepts it.
    let pattern = pattern_from_widths(&[2, 1, 2, 1, 2], 10); // bars: 2+2+2 = 6 dark
    let (buf, w, h) = render(&pattern, 4, 30);
    let frame = GrayFrame::new(&buf, w, h).unwrap();
    let cands = scan_lines(&frame, &ScanOptions::default());
    let cand = cands
        .iter()
        .find(|c| c.pattern.modules == pattern.modules)
        .expect("pattern recovered");

    let decoder = EvenBarDecoder;
    let symbol = try_decode(cand, &decoder).expect("decoder accepts even-bar pattern");
    assert_eq!(symbol.symbology, Symbology::Code39);
    // The front-end attaches its scanline location.
    assert!(symbol.location.is_some());
}

// --- Low-quality captures: blur, faint print, perspective ---------------------

/// A Code 128-like pattern: every element width 1..=4, many single modules between
/// wide neighbours — the elements blur displaces most.
fn dense_pattern() -> LinearPattern {
    pattern_from_widths(
        &[
            2, 1, 1, 2, 1, 4, 1, 1, 1, 3, 2, 3, 1, 1, 3, 3, 2, 1, 3, 1, 2, 1, 1, 3, 1, 1, 2, 3, 1,
            3, 1, 2, 2, 1, 3, 2, 1, 2, 3, 1, 1, 1, 4, 1, 2, 1, 1, 2, 3, 3, 1, 1, 1, 2, 3,
        ],
        10,
    )
}

/// Gaussian blur + sensor noise, as a camera would add them.
fn degrade(buf: &[u8], w: usize, h: usize, sigma: f32, noise: f32, seed: u64) -> Vec<u8> {
    let img = crate::image::GrayImage::from_raw(w, h, buf.to_vec());
    let mut rng = crate::transform::Rng::new(seed);
    let out = crate::transform::add_noise(
        &crate::transform::gaussian_blur(&img, sigma),
        noise,
        &mut rng,
    );
    out.pixels().to_vec()
}

#[test]
fn normal_cdf_matches_known_values() {
    for (u, want) in [
        (-3.0f32, 0.001_349_9f32),
        (-1.0, 0.158_655_3),
        (0.0, 0.5),
        (0.5, 0.691_462_5),
        (2.0, 0.977_249_9),
    ] {
        assert!((normal_cdf(u) - want).abs() < 1e-5, "Φ({u})");
    }
    assert!(normal_cdf(-40.0) >= 0.0 && normal_cdf(40.0) <= 1.0);
}

#[test]
fn solve3_solves_and_detects_singular() {
    let x = solve3(
        [[2.0, 1.0, -1.0], [-3.0, -1.0, 2.0], [-2.0, 1.0, 2.0]],
        [8.0, -11.0, -3.0],
    )
    .expect("regular system");
    for (got, want) in x.iter().zip([2.0, 3.0, -1.0]) {
        assert!((got - want).abs() < 1e-9);
    }
    assert!(
        solve3(
            [[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [1.0, 0.0, 1.0]],
            [1.0, 2.0, 3.0]
        )
        .is_none()
    );
}

/// Blur as wide as half a module displaces every edge that has a close neighbour. The
/// threshold-crossing extractor alone then mis-sizes the narrow elements (it reads the
/// module a third too large); the gradient extractor with the blur model recovers the
/// pattern exactly.
#[test]
fn recovers_under_gaussian_blur_half_a_module_wide() {
    let pattern = dense_pattern();
    let (buf, w, h) = render(&pattern, 3, 40);
    for (sigma, seed) in [(1.2f32, 1u64), (1.6, 2)] {
        let img = degrade(&buf, w, h, sigma, 6.0, seed);
        let frame = GrayFrame::new(&img, w, h).unwrap();
        let cands = scan_spans(&frame, &ScanOptions::default());
        assert!(
            recovers_modules(&cands, &pattern),
            "sigma {sigma}: pattern not recovered"
        );
    }
}

/// The blur model puts edges back where they were printed, to a fraction of a pixel.
#[test]
fn deblurred_edges_are_within_a_third_of_a_pixel() {
    let pattern = dense_pattern();
    let (buf, w, h) = render(&pattern, 3, 40);
    let img = degrade(&buf, w, h, 1.6, 0.0, 3);
    let frame = GrayFrame::new(&img, w, h).unwrap();
    let line = scan_lines_of(
        w,
        h,
        &ScanOptions {
            scan_count: 1,
            angles_deg: vec![0.0],
            ..ScanOptions::default()
        },
    )[0];
    let runs = extract_runs_grad(&sample_profile(&frame, &line, 0)).expect("edges");
    // True edge k sits on the boundary between two pixels: (30 + cumulative modules·3)
    // − 0.5 in sample coordinates.
    let mut truth = vec![30.0f32 - 0.5];
    for (run, pair) in (1..).zip(pattern.modules.windows(2)) {
        if pair[0] != pair[1] {
            truth.push(truth[0] + 3.0 * run as f32);
        }
    }
    truth.push(truth[0] + 3.0 * pattern.modules.len() as f32);
    assert_eq!(runs.edges.len(), truth.len(), "every edge found once");
    for (got, want) in runs.edges.iter().zip(&truth) {
        assert!(
            (got - want).abs() < 0.34,
            "edge at {got}, printed at {want}"
        );
    }
}

/// Faint print: 25 grey levels between bar and space, below the absolute contrast the
/// threshold extractor demands. Contrast is only meaningful relative to noise.
#[test]
fn recovers_faint_print() {
    let pattern = dense_pattern();
    let (buf, w, h) = render(&pattern, 3, 40);
    // Remap 30..225 onto 110..135.
    let faint: Vec<u8> = buf
        .iter()
        .map(|&v| if v == DARK { 110 } else { 135 })
        .collect();
    let img = degrade(&faint, w, h, 0.7, 1.5, 4);
    let frame = GrayFrame::new(&img, w, h).unwrap();
    let cands = scan_spans(&frame, &ScanOptions::default());
    assert!(recovers_modules(&cands, &pattern), "faint pattern lost");
}

/// A flat field of sensor noise holds no edges for the gradient extractor either: its
/// floor is set by the noise it measures across the band.
#[test]
fn noise_alone_yields_no_long_candidates() {
    let (w, h) = (400usize, 40usize);
    for (level, seed) in [(128u8, 5u64), (40, 6), (230, 7)] {
        let img = degrade(&vec![level; w * h], w, h, 0.0, 14.0, seed);
        let frame = GrayFrame::new(&img, w, h).unwrap();
        let longest = scan_spans(&frame, &ScanOptions::default())
            .iter()
            .map(|c| c.pattern.modules.len())
            .max()
            .unwrap_or(0);
        assert!(
            longest < 20,
            "level {level}: {longest}-module span from noise"
        );
    }
}

/// A symbol turned away from the camera: the module pitch shrinks steadily along it
/// (here from 4.6 to 3.0 px). A single pitch mis-counts the wide runs at both ends.
#[test]
fn recovers_a_foreshortened_pattern() {
    let pattern = dense_pattern();
    let n = pattern.modules.len();
    let (p0, p1) = (4.6f32, 3.0f32);
    // Position of module boundary j: pitch falls linearly along the symbol.
    let edge = |j: usize| {
        let t = j as f32 / n as f32;
        40.0 + n as f32 * (p0 * t + (p1 - p0) * t * t / 2.0)
    };
    let w = edge(n) as usize + 40;
    let h = 30usize;
    let mut buf = vec![LIGHT; w * h];
    for (j, &dark) in pattern.modules.iter().enumerate() {
        if !dark {
            continue;
        }
        // Anti-aliased bar from edge(j) to edge(j + 1).
        let (a, b) = (edge(j), edge(j + 1));
        for x in a.floor() as usize..=b.floor() as usize {
            let cover = (b.min(x as f32 + 1.0) - a.max(x as f32)).clamp(0.0, 1.0);
            for y in 0..h {
                let v = buf[y * w + x] as f32 - cover * (LIGHT - DARK) as f32;
                buf[y * w + x] = v.max(DARK as f32) as u8;
            }
        }
    }
    let img = degrade(&buf, w, h, 0.6, 3.0, 8);
    let frame = GrayFrame::new(&img, w, h).unwrap();
    let cands = scan_spans(&frame, &ScanOptions::default());
    assert!(
        recovers_modules(&cands, &pattern),
        "foreshortened pattern not recovered"
    );
}

/// `scan_lines` puts a pattern many lines agree on ahead of the cleanly quantized
/// fragments single lines produce, so the truncated list still holds the symbol.
#[test]
fn scan_lines_ranks_by_line_agreement() {
    let pattern = dense_pattern();
    let (buf, w, h) = render(&pattern, 3, 60);
    let img = degrade(&buf, w, h, 1.0, 5.0, 9);
    let frame = GrayFrame::new(&img, w, h).unwrap();
    let top = scan_lines(
        &frame,
        &ScanOptions {
            max_candidates: 1,
            ..ScanOptions::default()
        },
    );
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].pattern.modules, pattern.modules);
}

/// Every candidate of the tagged scans names a real line, and one line contributes
/// candidates from more than one extractor.
#[test]
fn tagged_scans_report_their_line() {
    let pattern = dense_pattern();
    let (buf, w, h) = render(&pattern, 3, 40);
    let frame = GrayFrame::new(&buf, w, h).unwrap();
    let opts = ScanOptions::default();
    let lines = opts.scan_count * opts.angles_deg.len();
    let analysis = analyze(&frame, &opts);
    for tagged in [analysis.spans(), analysis.edge_candidates()] {
        assert!(!tagged.is_empty());
        assert!(tagged.iter().all(|(line, _)| *line < lines));
        let on_first = tagged
            .iter()
            .filter(|(line, _)| *line == tagged[0].0)
            .count();
        assert!(on_first >= 2, "one extractor per line only");
    }
}
