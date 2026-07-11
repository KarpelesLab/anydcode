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
//! and stop guards, so its rotation tolerance is deliberately narrower than QR's, and
//! the geometry model is affine — perspective tilt is out of scope and not asserted. The
//! magnitudes above sit a comfortable margin inside the measured failure points.

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
