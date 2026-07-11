//! End-to-end *image* round-trip harness for Data Matrix.
//!
//! Closes the loop through pixels: encode a Data Matrix [`Symbol`], render it to a
//! grayscale image, degrade it with the [`transform`] suite, then run the full image
//! sampler (binarize → corner recovery → L/timing analysis → perspective homography →
//! module sampling → structural decode) and assert the decoded payload matches.
//!
//! Everything is deterministic: the geometric transforms are analytic and the noise is
//! drawn from a seeded [`Rng`], so a failure always reproduces.
//!
//! ## Working envelope (what the sampler is asserted to handle)
//!
//! Square symbols only, rendered at 6 px/module with the standard 1-module quiet zone.
//! Within that, decoding is asserted for:
//!
//! | Transform            | Envelope                                                  |
//! |----------------------|-----------------------------------------------------------|
//! | Rotation             | any angle; ±5°…±45°, 90° all sizes; 180° for dim ≤ 22     |
//! | Scale                | 0.75×, 1.5×, 2.0× all sizes (down to ~4.5 px/module)      |
//! | Gaussian blur        | σ ≤ 1.5 px                                                |
//! | Additive noise       | σ ≤ 25 luminance                                          |
//! | Brightness/contrast  | ±40 brightness, 0.6–1.4 contrast                          |
//! | Perspective tilt     | ≤ 0.20 (dim ≤ 12) / ≤ 0.10 (dim ≤ 22) / ≤ 0.04 (larger)   |
//! | Combined             | 8° rotation + σ1.0 blur + σ12 noise                       |
//!
//! Tilt tolerance shrinks with symbol size: a Data Matrix has no interior alignment
//! anchor, so the fourth (timing-meeting) corner is recovered by fitting straight lines
//! to the two timing borders and intersecting them. That is perspective-correct for a
//! well-resolved edge but degrades as modules-per-edge grows, hence the size-graded
//! tilt column. Magnitudes are set a comfortable margin inside the sampler's measured
//! failure points; the `known_hard_edge_cases` test pins a few points near the edge.

use anydcode::codes::datamatrix::{DataMatrixEncoder, DataMatrixScanner, sample_grid, scan};
use anydcode::output::{BitMatrix, Encoding};
use anydcode::pipeline::Hints;
use anydcode::render::render_matrix;
use anydcode::traits::{Analyze, Detect, Encode};
use anydcode::transform::{self, Rng};
use anydcode::{GrayImage, Symbology};

/// Pixels per module for every rendered symbol in this harness.
const SCALE: usize = 6;

/// Encode raw `payload` bytes and return both the canonical matrix and the payload.
fn encode(payload: &[u8]) -> (BitMatrix, Vec<u8>) {
    let enc = DataMatrixEncoder::new();
    let symbol = enc.build(payload).expect("in-capacity payload");
    let bytes = symbol.payload_bytes();
    assert_eq!(bytes, payload, "encoder must be lossless for the harness");
    match enc.encode(&symbol).expect("encode") {
        Encoding::Matrix(m) => (m, bytes),
        Encoding::Linear(_) => unreachable!("Data Matrix encodes to a matrix"),
    }
}

/// Sample+decode `img` and assert the recovered payload equals `expected`.
fn assert_decodes(img: &GrayImage, expected: &[u8], label: &str) {
    match scan(&img.as_frame()) {
        Ok(sym) => assert_eq!(
            sym.payload_bytes(),
            expected,
            "{label}: payload mismatch after image round-trip"
        ),
        Err(e) => panic!("{label}: sampler failed to decode ({e})"),
    }
}

/// Render the matrix and hand back the base (upright) image plus its module dimension.
fn base_image(matrix: &BitMatrix) -> (GrayImage, usize) {
    (render_matrix(matrix, SCALE), matrix.width())
}

// ===================================================================================
// Individual transform families
// ===================================================================================

#[test]
fn upright_is_pixel_exact() {
    // The upright render must sample back to the *identical* matrix, not merely a
    // decodable one — the strongest possible statement of sampler correctness. This
    // covers single- and multi-region sizes.
    for payload in cases() {
        let (matrix, _) = encode(&payload);
        let (img, dim) = base_image(&matrix);
        let sampled = sample_grid(&img.as_frame()).expect("sample upright");
        assert_eq!(
            sampled, matrix,
            "dim{dim}: upright sampling not pixel-exact"
        );
    }
}

#[test]
fn rotations() {
    for payload in cases() {
        let (matrix, expected) = encode(&payload);
        let (base, dim) = base_image(&matrix);
        for deg in [5.0f32, -5.0, 15.0, -15.0, 30.0, -30.0, 45.0, -45.0, 90.0] {
            let img = transform::rotate(&base, deg.to_radians());
            assert_decodes(&img, &expected, &format!("dim{dim} rot{deg}"));
        }
        // Exact half-turn: reliable for small/medium grids.
        if dim <= 22 {
            let img = transform::rotate(&base, std::f32::consts::PI);
            assert_decodes(&img, &expected, &format!("dim{dim} rot180"));
        }
    }
}

#[test]
fn scaling() {
    for payload in cases() {
        let (matrix, expected) = encode(&payload);
        let (base, dim) = base_image(&matrix);
        for factor in [0.75f32, 1.5, 2.0] {
            let img = transform::scale(&base, factor);
            assert_decodes(&img, &expected, &format!("dim{dim} scale{factor}"));
        }
    }
}

#[test]
fn blur() {
    for payload in cases() {
        let (matrix, expected) = encode(&payload);
        let (base, dim) = base_image(&matrix);
        for sigma in [1.0f32, 1.5] {
            let img = transform::gaussian_blur(&base, sigma);
            assert_decodes(&img, &expected, &format!("dim{dim} blur{sigma}"));
        }
    }
}

#[test]
fn noise() {
    for payload in cases() {
        let (matrix, expected) = encode(&payload);
        let (base, dim) = base_image(&matrix);
        for sigma in [15.0f32, 25.0] {
            let mut rng = Rng::new(0xC0FFEE);
            let img = transform::add_noise(&base, sigma, &mut rng);
            assert_decodes(&img, &expected, &format!("dim{dim} noise{sigma}"));
        }
    }
}

#[test]
fn brightness_and_contrast() {
    for payload in cases() {
        let (matrix, expected) = encode(&payload);
        let (base, dim) = base_image(&matrix);
        for (bright, contrast) in [(-40.0f32, 1.0f32), (40.0, 1.0), (0.0, 0.6), (0.0, 1.4)] {
            let img = transform::brightness_contrast(&base, bright, contrast);
            assert_decodes(&img, &expected, &format!("dim{dim} bc{bright}/{contrast}"));
        }
    }
}

#[test]
fn perspective_tilt() {
    // Tilt tolerance is size-graded (see the module header): the fourth corner is
    // recovered from the timing-border lines, which resolve more coarsely as the
    // module count grows.
    for payload in cases() {
        let (matrix, expected) = encode(&payload);
        let (base, dim) = base_image(&matrix);
        let amount = if dim <= 12 {
            0.20
        } else if dim <= 22 {
            0.10
        } else {
            0.04
        };
        let r = transform::tilt_right(&base, amount);
        assert_decodes(&r, &expected, &format!("dim{dim} tiltR{amount}"));
        let b = transform::tilt_bottom(&base, amount);
        assert_decodes(&b, &expected, &format!("dim{dim} tiltB{amount}"));
    }
}

#[test]
fn combined_pipeline() {
    // A realistic capture: slight rotation, optical blur, and sensor noise together.
    for payload in cases() {
        let (matrix, expected) = encode(&payload);
        let (base, dim) = base_image(&matrix);
        let mut rng = Rng::new(1234);
        let rotated = transform::rotate(&base, 8.0f32.to_radians());
        let blurred = transform::gaussian_blur(&rotated, 1.0);
        let noised = transform::add_noise(&blurred, 12.0, &mut rng);
        assert_decodes(&noised, &expected, &format!("dim{dim} combo"));
    }
}

#[test]
fn detect_and_analyze_traits() {
    // Exercise the Detect/Analyze pipeline entry points, not just the free function.
    let (matrix, expected) = encode(b"PIPELINE 2026");
    let (base, _) = base_image(&matrix);
    let rotated = transform::rotate(&base, 12.0f32.to_radians());
    let frame = rotated.as_frame();

    let scanner = DataMatrixScanner::new();
    let candidates = scanner.detect(&frame, &Hints::new());
    assert_eq!(candidates.len(), 1, "detector should find one candidate");
    let cand = &candidates[0];
    assert_eq!(cand.symbology, Some(Symbology::DataMatrix));
    assert!(cand.location.module_size.unwrap() > 0.0);

    let symbol = scanner.analyze(&frame, cand).expect("analyze");
    assert_eq!(symbol.payload_bytes(), expected);
    assert_eq!(symbol.symbology, Symbology::DataMatrix);
}

#[test]
fn binary_base256_payload() {
    // Bytes ≥ 128 force Base256 encodation; confirm the sampler round-trips raw binary.
    let payload: Vec<u8> = vec![0x00, 0xFF, 0x80, 0x41, 0x90, 0x7F, 0xC0, 0x01, 0xAA, 0x55];
    let (matrix, expected) = encode(&payload);
    let (base, dim) = base_image(&matrix);
    let rotated = transform::rotate(&base, 20.0f32.to_radians());
    assert_decodes(&rotated, &expected, &format!("dim{dim} base256"));
}

#[test]
fn known_hard_edge_cases() {
    // Cases deliberately pushed toward the sampler's limits but kept inside the
    // passing envelope, so a regression that narrows robustness is caught here.

    // 1) Steep tilt on a small symbol, near the documented 0.25 edge.
    let (matrix, expected) = encode(b"HELLO");
    let (base, dim) = base_image(&matrix);
    assert!(dim <= 12, "expected a small symbol, got dim {dim}");
    let steep = transform::tilt_right(&base, 0.25);
    assert_decodes(&steep, &expected, "known-hard steep-tilt");

    // 2) Rotation combined with noise near the noise ceiling.
    let mut rng = Rng::new(555);
    let rot = transform::rotate(&base, 22.0f32.to_radians());
    let noisy = transform::add_noise(&rot, 30.0, &mut rng);
    assert_decodes(&noisy, &expected, "known-hard rot+noise");

    // 3) Downscale a mid-size symbol to ~3.3 px/module.
    let (m2, exp2) = encode(b"Data Matrix 2026 test payload");
    let (b2, _) = base_image(&m2);
    let small = transform::scale(&b2, 0.55);
    assert_decodes(&small, &exp2, "known-hard downscale");
}

/// A spread of payloads chosen to span square sizes 10, 12, 16, 22, 32 and 36
/// (the last two are multi-region symbols).
fn cases() -> Vec<Vec<u8>> {
    vec![
        b"AB".to_vec(),                                                      // dim 10
        b"HELLO".to_vec(),                                                   // dim 12
        b"Data Matrix".to_vec(),                                             // dim 16
        b"Data Matrix 2026 test payload".to_vec(),                           // dim 22
        b"0123456789 abcdefghij KLMNOPQRSTUV extra content here!!".to_vec(), // dim 32
        b"The quick brown fox jumps over the lazy dog 0123456789 AAAAAAAAAAAAAAAAAAAA".to_vec(), // dim 36
    ]
}
