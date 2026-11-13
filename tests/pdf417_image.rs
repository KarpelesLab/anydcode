//! End-to-end *image* round-trip harness for PDF417.
//!
//! Encode a PDF417 [`Symbol`], render it to a grayscale image, degrade it with the
//! [`transform`] suite, then run the full image sampler (guard detection → edge fitting
//! → homography → module sampling → structural decode) and assert the decoded payload
//! matches. Everything is deterministic: the geometric transforms are analytic and the
//! noise is drawn from a seeded [`Rng`], so a failure always reproduces.
//!
//! ## Working envelope (what the sampler is asserted to handle)
//!
//! Symbols are rendered at 6 px/module with the standard 2-module quiet zone. Within
//! that, decoding is asserted for:
//!
//! | Transform            | Envelope                                    |
//! |----------------------|---------------------------------------------|
//! | Rotation             | ±2°…±10°                                     |
//! | Scale                | 0.8×…2.0×                                    |
//! | Gaussian blur        | σ ≤ 1.5 px                                   |
//! | Additive noise       | σ ≤ 25 luminance                            |
//! | Brightness/contrast  | ±40 brightness, 0.7–1.3 contrast            |
//! | Combined             | 6° rotation + σ1.0 blur + σ12 noise         |
//!
//! PDF417 has no finder patterns; the sampler leans entirely on the full-height start
//! and stop guards, so its rotation tolerance is deliberately narrower than QR's. The
//! magnitudes above sit a comfortable margin inside the measured failure points.
//!
//! A second group of cases, at the end of the file, covers what a camera delivers
//! instead of a render: 3 px modules under soft focus, motion blur, low contrast and
//! grain, perspective tilt, and crops derotated on an estimated axis.
#![cfg(all(feature = "encode", feature = "scan", feature = "pdf417"))]

use anyd::GrayImage;
use anyd::codes::pdf417::{EcLevel, Pdf417Encoder, Pdf417Scanner, sample_grid, scan};
use anyd::output::{BitMatrix, Encoding};
use anyd::pipeline::Hints;
use anyd::render::render_matrix;
use anyd::segment::Segment;
use anyd::traits::{Analyze, Detect, Encode};
use anyd::transform::{self, Rng};

/// Pixels per module for every rendered symbol in this harness.
const SCALE: usize = 6;

fn level(n: u8) -> EcLevel {
    EcLevel::new(n).unwrap()
}

/// Encode `segments` at `level` and return both the canonical matrix and the expected
/// payload bytes.
fn encode(segments: Vec<Segment>, ec: EcLevel) -> (BitMatrix, Vec<u8>) {
    let enc = Pdf417Encoder::new();
    let symbol = enc.build(segments, ec).expect("in-capacity payload");
    let payload = symbol.payload_bytes();
    match enc.encode(&symbol).expect("encode") {
        Encoding::Matrix(m) => (m, payload),
        Encoding::Linear(_) => unreachable!("PDF417 encodes to a matrix"),
    }
}

/// Sample+decode `img` and assert the recovered payload equals `expected`.
fn assert_decodes(img: &GrayImage, expected: &[u8], label: &str) {
    let frame = img.as_frame();
    match scan(&frame) {
        Some(sym) => assert_eq!(
            sym.payload_bytes(),
            expected,
            "{label}: payload mismatch after image round-trip"
        ),
        None => panic!("{label}: sampler failed to decode"),
    }
}

/// Render the matrix and hand back the base (upright) image plus its module dimensions.
fn base_image(matrix: &BitMatrix) -> (GrayImage, usize, usize) {
    (
        render_matrix(matrix, SCALE),
        matrix.width(),
        matrix.height(),
    )
}

/// A spread of payloads spanning Text/Byte/Numeric compaction and several EC levels.
fn cases() -> Vec<(Vec<Segment>, EcLevel)> {
    vec![
        (
            vec![Segment::alphanumeric(b"PDF417 IMAGE 2026".to_vec())],
            level(1),
        ),
        (
            vec![Segment::byte(b"https://example.com/x?q=hi".to_vec())],
            level(2),
        ),
        (
            vec![Segment::numeric(b"0123456789012345678901234".to_vec())],
            level(3),
        ),
        (
            vec![Segment::byte(b"Mixed payload -- test suite".to_vec())],
            level(1),
        ),
    ]
}

// ===================================================================================
// Individual transform families
// ===================================================================================

#[test]
fn upright_is_pixel_exact() {
    // The upright render must sample back to the *identical* matrix, not merely a
    // decodable one — the strongest statement of sampler correctness.
    for (segs, ec) in cases() {
        let (matrix, _) = encode(segs, ec);
        let (img, _, _) = base_image(&matrix);
        let sampled = sample_grid(&img.as_frame()).expect("sample upright");
        assert_eq!(sampled, matrix, "upright sampling not pixel-exact");
    }
}

#[test]
fn rotations() {
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        let (base, w, h) = base_image(&matrix);
        for deg in [2.0f32, -2.0, 5.0, -5.0, 8.0, -8.0, 10.0, -10.0] {
            let img = transform::rotate(&base, deg.to_radians());
            assert_decodes(&img, &expected, &format!("{w}x{h} rot{deg}"));
        }
    }
}

#[test]
fn scaling() {
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        let (base, w, h) = base_image(&matrix);
        for factor in [0.8f32, 1.5, 2.0] {
            let img = transform::scale(&base, factor);
            assert_decodes(&img, &expected, &format!("{w}x{h} scale{factor}"));
        }
    }
}

#[test]
fn blur() {
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        let (base, w, h) = base_image(&matrix);
        for sigma in [0.8f32, 1.5] {
            let img = transform::gaussian_blur(&base, sigma);
            assert_decodes(&img, &expected, &format!("{w}x{h} blur{sigma}"));
        }
    }
}

#[test]
fn noise() {
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        let (base, w, h) = base_image(&matrix);
        for sigma in [15.0f32, 25.0] {
            let mut rng = Rng::new(0xC0FFEE);
            let img = transform::add_noise(&base, sigma, &mut rng);
            assert_decodes(&img, &expected, &format!("{w}x{h} noise{sigma}"));
        }
    }
}

#[test]
fn brightness_and_contrast() {
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        let (base, w, h) = base_image(&matrix);
        for (bright, contrast) in [(-40.0f32, 1.0f32), (40.0, 1.0), (0.0, 0.7), (0.0, 1.3)] {
            let img = transform::brightness_contrast(&base, bright, contrast);
            assert_decodes(&img, &expected, &format!("{w}x{h} bc{bright}/{contrast}"));
        }
    }
}

#[test]
fn combined_pipeline() {
    // A realistic capture: slight rotation, optical blur and sensor noise together.
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        let (base, w, h) = base_image(&matrix);
        let mut rng = Rng::new(1234);
        let rotated = transform::rotate(&base, 6.0f32.to_radians());
        let blurred = transform::gaussian_blur(&rotated, 1.0);
        let noised = transform::add_noise(&blurred, 12.0, &mut rng);
        assert_decodes(&noised, &expected, &format!("{w}x{h} combo"));
    }
}

#[test]
fn detect_and_analyze_traits() {
    // Exercise the Detect/Analyze pipeline entry points, not just the free function.
    let (matrix, expected) = encode(vec![Segment::byte(b"PIPELINE 2026".to_vec())], level(2));
    let (base, _, _) = base_image(&matrix);
    let rotated = transform::rotate(&base, 5.0f32.to_radians());
    let frame = rotated.as_frame();

    let scanner = Pdf417Scanner::new();
    let candidates = scanner.detect(&frame, &Hints::new());
    assert_eq!(candidates.len(), 1, "detector should find one candidate");
    let cand = &candidates[0];
    assert_eq!(cand.symbology, Some(anyd::Symbology::Pdf417));
    assert!(cand.location.module_size.unwrap() > 0.0);

    let symbol = scanner.analyze(&frame, cand).expect("analyze");
    assert_eq!(symbol.payload_bytes(), expected);
    assert_eq!(symbol.symbology, anyd::Symbology::Pdf417);
}

// ===================================================================================
// Low-quality captures (the grey-level scan-line reader)
// ===================================================================================
//
// What a phone camera actually delivers: small modules, soft focus, motion blur, low
// contrast, sensor grain, and a symbol that is not parallel to the sensor. None of
// these binarize into a clean module grid, so they exercise the sampler's second
// stage — edge-to-edge codeword reading along the rows, voted per codeword, with
// blurred-template matching behind it. Each case is rendered at 3 px/module.

/// Pixels per module for the low-quality cases.
const SMALL: usize = 3;

/// Render `matrix` at [`SMALL`] px/module.
fn small_image(matrix: &BitMatrix) -> GrayImage {
    render_matrix(matrix, SMALL)
}

/// A horizontal box blur `len` pixels long — camera shake along the reading direction,
/// the direction that hurts.
fn motion_blur(img: &GrayImage, len: usize) -> GrayImage {
    let (w, h) = (img.width(), img.height());
    let mut out = GrayImage::filled(w, h, 255);
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0u32;
            for k in 0..len {
                let xx = (x + k).saturating_sub(len / 2).min(w - 1);
                sum += u32::from(img.get(xx, y));
            }
            out.set(x, y, (sum / len as u32) as u8);
        }
    }
    out
}

#[test]
fn small_modules_soft_focus() {
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        let mut rng = Rng::new(0x50F7);
        let img = transform::gaussian_blur(&small_image(&matrix), 1.6);
        let img = transform::add_noise(&img, 8.0, &mut rng);
        assert_decodes(&img, &expected, "3px soft focus");
    }
}

#[test]
fn small_modules_motion_blur() {
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        let mut rng = Rng::new(0x5A4E);
        let img = motion_blur(&transform::gaussian_blur(&small_image(&matrix), 0.6), 4);
        let img = transform::add_noise(&img, 4.0, &mut rng);
        assert_decodes(&img, &expected, "3px motion blur");
    }
}

#[test]
fn small_modules_low_contrast() {
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        let mut rng = Rng::new(0xD1A);
        let img = transform::gaussian_blur(&small_image(&matrix), 0.8);
        let img = transform::brightness_contrast(&img, -20.0, 0.33);
        let img = transform::add_noise(&img, 6.0, &mut rng);
        assert_decodes(&img, &expected, "3px low contrast");
    }
}

#[test]
fn small_modules_grain() {
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        let mut rng = Rng::new(0x6A1);
        let img = transform::gaussian_blur(&small_image(&matrix), 0.8);
        let img = transform::brightness_contrast(&img, -30.0, 0.6);
        let img = transform::add_noise(&img, 16.0, &mut rng);
        assert_decodes(&img, &expected, "3px grain");
    }
}

#[test]
fn perspective_tilt() {
    // Yaw: the far (right) edge is 30% shorter and its modules correspondingly
    // narrower, so neither the outline nor the module pitch is affine.
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        for scale in [SMALL, SCALE] {
            let mut rng = Rng::new(0x7117);
            let img = transform::tilt_right(&render_matrix(&matrix, scale), 0.3);
            let img = transform::add_noise(&transform::gaussian_blur(&img, 0.8), 4.0, &mut rng);
            assert_decodes(&img, &expected, &format!("{scale}px tilt"));
        }
    }
}

#[test]
fn tilt_with_rotation_and_gradient() {
    // The lot at once, as a hand-held capture has it: yaw, a few degrees of roll and
    // a 2:1 illumination falloff across the symbol.
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        let img = transform::tilt_right(&render_matrix(&matrix, 4), 0.2);
        let mut img = transform::rotate(&img, 5.0f32.to_radians());
        let w = img.width();
        for y in 0..img.height() {
            for x in 0..w {
                let f = 0.5 + 0.5 * (x as f32 / w as f32);
                img.set(x, y, (f32::from(img.get(x, y)) * f) as u8);
            }
        }
        let mut rng = Rng::new(0x6AD);
        let img = transform::add_noise(&transform::gaussian_blur(&img, 0.8), 4.0, &mut rng);
        assert_decodes(&img, &expected, "tilt + roll + gradient");
    }
}

/// Decode `img` the way the live pipeline does for a located linear region whose
/// reading axis it estimated as `axis_deg`: derotate, then sample.
fn assert_decodes_at(img: &GrayImage, axis_deg: f32, expected: &[u8], label: &str) {
    let found = anyd::pipeline::scan_stacked_at(&img.as_frame(), axis_deg.to_radians());
    assert!(
        found.iter().any(|s| s.payload_bytes() == expected),
        "{label}: not read after derotation"
    );
}

#[test]
fn rotated_low_contrast_capture_reads_after_derotation() {
    // A locator's axis is good to a couple of degrees, and derotating resamples the
    // crop — so what reaches the sampler is slightly skewed, slightly softer, on a
    // blank canvas, and here at a third of the contrast under a lighting falloff.
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        let mut img = transform::rotate(&small_image(&matrix), 17.0f32.to_radians());
        let w = img.width();
        for y in 0..img.height() {
            for x in 0..w {
                let f = 0.45 + 0.55 * (x as f32 / w as f32);
                img.set(x, y, (f32::from(img.get(x, y)) * f) as u8);
            }
        }
        let mut rng = Rng::new(0xD1A2);
        let img = transform::gaussian_blur(&img, 0.8);
        let img = transform::brightness_contrast(&img, -20.0, 0.33);
        let img = transform::add_noise(&img, 6.0, &mut rng);
        assert_decodes_at(&img, 14.5, &expected, "3px dim, 17° read at 14.5°");
    }
}

#[test]
fn rotated_grainy_capture_reads_after_derotation() {
    // As above with heavy sensor grain, which derotation turns into correlated noise
    // that a per-pixel estimate badly underrates.
    for (segs, ec) in cases() {
        let (matrix, expected) = encode(segs, ec);
        let img = transform::rotate(&small_image(&matrix), 45.0f32.to_radians());
        let mut rng = Rng::new(0x6A12);
        let img = transform::gaussian_blur(&img, 0.8);
        let img = transform::brightness_contrast(&img, -30.0, 0.6);
        let img = transform::add_noise(&img, 16.0, &mut rng);
        assert_decodes_at(&img, 45.3, &expected, "3px grain, 45° read at 45.3°");
    }
}

/// A symbol as software writes it to a file: a pixel or two to the module.
#[test]
fn tiny_modules() {
    let enc = Pdf417Encoder::new();
    let symbol = enc
        .build_text(
            "-ActiveBarcode-ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            EcLevel::new(2).unwrap(),
        )
        .unwrap();
    let Encoding::Matrix(matrix) = enc.encode(&symbol).unwrap() else {
        panic!("PDF417 encodes to a matrix");
    };
    for scale in [1usize, 2] {
        let img = render_matrix(&matrix, scale);
        let read = scan(&img.as_frame()).unwrap_or_else(|| panic!("{scale} px: not read"));
        assert_eq!(read.payload_bytes(), symbol.payload_bytes(), "{scale} px");
        // The outline is reported in the frame's own pixels.
        let location = read.location.expect("a location");
        for p in location.outline.corners {
            assert!(p.x >= -1.0 && p.x <= img.width() as f32 + 1.0, "{p:?}");
            assert!(p.y >= -1.0 && p.y <= img.height() as f32 + 1.0, "{p:?}");
        }
    }
}

// ===================================================================================
// Symbols cut short by the frame
// ===================================================================================
//
// A crop (or a camera held too close) may leave only one guard in the frame. The
// reader then works from that guard and its row indicators alone, and Reed–Solomon
// fills in the columns the frame does not hold.

/// Columns `x0..x1` of `img`.
fn crop_columns(img: &GrayImage, x0: usize, x1: usize) -> GrayImage {
    let (w, h) = (x1 - x0, img.height());
    let mut out = GrayImage::filled(w, h, 255);
    for y in 0..h {
        for x in 0..w {
            out.set(x, y, img.get(x0 + x, y));
        }
    }
    out
}

/// A 4-column symbol at level 4 (32 check codewords, 17 rows): a missing column is 17
/// erasures, well inside what the level fills in.
fn croppable() -> (anyd::Symbol, BitMatrix) {
    let enc = Pdf417Encoder::new();
    let symbol = enc
        .build_sized(
            vec![Segment::alphanumeric(
                b"CUT SHORT BY THE FRAME 0123456789".to_vec(),
            )],
            level(4),
            Some(4),
        )
        .expect("in-capacity payload");
    let Encoding::Matrix(matrix) = enc.encode(&symbol).expect("encode") else {
        panic!("PDF417 encodes to a matrix");
    };
    assert_eq!(matrix.width(), 17 * 4 + 69);
    (symbol, matrix)
}

#[test]
fn one_guard_cropped_away() {
    let (symbol, matrix) = croppable();
    let enc = Pdf417Encoder::new();
    for (scale, sigma) in [(2usize, 0.0f32), (3, 0.8), (6, 1.2)] {
        let full = render_matrix(&matrix, scale);
        let full = if sigma > 0.0 {
            let mut rng = Rng::new(0xC0FF);
            transform::add_noise(&transform::gaussian_blur(&full, sigma), 4.0, &mut rng)
        } else {
            full
        };
        let w = full.width();
        // Modules cut off one side: half a guard; the guard and its row indicator;
        // those and a data column besides. (The render adds a 2-module quiet zone.)
        for cut in [10usize, 2 + 17 + 17 + 3, 2 + 17 + 17 + 17 + 5] {
            for from_left in [true, false] {
                let label = format!("{scale} px, {cut} modules off, left={from_left}");
                let img = if from_left {
                    crop_columns(&full, cut * scale, w)
                } else {
                    crop_columns(&full, 0, w - cut * scale)
                };
                let read = scan(&img.as_frame()).unwrap_or_else(|| panic!("{label}: not read"));
                // The whole symbol, exactly: what was cut off is restored.
                assert_eq!(read.segments, symbol.segments, "{label}");
                assert_eq!(read.meta, symbol.meta, "{label}");
                assert_eq!(
                    enc.encode(&read).unwrap(),
                    Encoding::Matrix(matrix.clone()),
                    "{label}"
                );
                let location = read.location.expect("a location");
                for p in location.outline.corners {
                    assert!(
                        p.x >= -1.0 && p.x <= img.width() as f32 + 1.0,
                        "{label}: {p:?}"
                    );
                    assert!(
                        p.y >= -1.0 && p.y <= img.height() as f32 + 1.0,
                        "{label}: {p:?}"
                    );
                }
            }
        }
    }
}

/// What the frame holds must still vouch for the rest: once the missing columns leave
/// Reed–Solomon no check codewords to spare, there is no read — and never a wrong one.
#[test]
fn too_little_of_a_symbol_is_not_read() {
    let (symbol, matrix) = croppable();
    let full = render_matrix(&matrix, 3);
    let w = full.width();
    // One column missing is 17 of 32 check codewords; two are 34.
    let one = crop_columns(&full, 0, w - (2 + 18 + 17 + 17 + 2) * 3);
    let read = scan(&one.as_frame()).expect("one column short reads");
    assert_eq!(read.segments, symbol.segments);
    for cut in [18 + 17 + 2 * 17 + 4, 18 + 17 + 3 * 17 + 4] {
        let two = crop_columns(&full, 0, w - (2 + cut) * 3);
        assert!(scan(&two.as_frame()).is_none(), "{cut} modules off");
        let two = crop_columns(&full, (2 + cut - 1) * 3, w);
        assert!(
            scan(&two.as_frame()).is_none(),
            "{cut} modules off the left"
        );
    }
}
