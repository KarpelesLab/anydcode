//! Tier B: end-to-end *image* round-trip harness for QR.
//!
//! This closes the loop through pixels: encode a QR [`Symbol`], render it to a
//! grayscale image, degrade it with the [`transform`] suite, then run the full image
//! sampler (finder detection → perspective recovery → module sampling → structural
//! decode) and assert the decoded payload matches.
//!
//! Everything is deterministic: the geometric transforms are analytic and the noise is
//! drawn from a seeded [`Rng`], so a failure always reproduces.
//!
//! ## Working envelope (what the sampler is asserted to handle)
//!
//! Symbols are rendered at 6 px/module with the standard 4-module quiet zone. Within
//! that, decoding is asserted for:
//!
//! | Transform            | Envelope                                             |
//! |----------------------|------------------------------------------------------|
//! | Rotation             | any angle; ±5°…±45°, 90° all versions; 180° ≤ v3     |
//! | Scale                | 1.5×, 2.0× all versions; 0.75× down to v4            |
//! | Gaussian blur        | σ ≤ 2.0 px                                           |
//! | Additive noise       | σ ≤ 25 luminance                                     |
//! | Brightness/contrast  | ±40 brightness, 0.6–1.4 contrast                     |
//! | Perspective tilt     | ≤ 0.15 (v3–v7) / ≤ 0.20 (v2–v4); needs an alignment  |
//! |                      | pattern, so **version 1 has no tilt tolerance**      |
//! | Combined             | 8° rotation + σ1.0 blur + σ12 noise                  |
//!
//! Perspective correction uses the bottom-right alignment pattern (present from
//! version 2 on) for a true 4-point homography; version 1 lacks one and is therefore
//! only sampled affinely, so it is exercised without perspective. Magnitudes are set a
//! comfortable margin inside the sampler's measured failure points — see the
//! `known-hard` test for cases deliberately pushed to the edge of what still passes.
#![cfg(all(feature = "encode", feature = "scan", feature = "qr"))]

use anyd::GrayImage;
use anyd::codes::qr::{EcLevel, QrEncoder, QrScanner, sample_grid, scan};
use anyd::output::{BitMatrix, Encoding};
use anyd::pipeline::Hints;
use anyd::render::render_matrix;
use anyd::segment::Segment;
use anyd::traits::{Analyze, Detect, Encode};
use anyd::transform::{self, Rng};

/// Pixels per module for every rendered symbol in this harness.
const SCALE: usize = 6;

/// Encode `segments` at `level` and return both the symbol's canonical matrix and the
/// expected payload bytes.
fn encode(segments: Vec<Segment>, level: EcLevel) -> (BitMatrix, Vec<u8>) {
    let enc = QrEncoder::new();
    let symbol = enc.build(segments, level).expect("in-capacity payload");
    let payload = symbol.payload_bytes();
    match enc.encode(&symbol).expect("encode") {
        Encoding::Matrix(m) => (m, payload),
        Encoding::Linear(_) => unreachable!("QR encodes to a matrix"),
    }
}

/// Sample+decode `img` and assert the recovered payload equals `expected`.
fn assert_decodes(img: &GrayImage, expected: &[u8], label: &str) {
    let frame = img.as_frame();
    match scan(&frame) {
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
    // decodable one — the strongest possible statement of sampler correctness.
    for payload in ["HI", "https://example.com/x?q=hello", "MIXED 2026 test"] {
        let (matrix, _) = encode(vec![Segment::byte(payload.as_bytes().to_vec())], EcLevel::Q);
        let (img, _) = base_image(&matrix);
        let sampled = sample_grid(&img.as_frame()).expect("sample upright");
        assert_eq!(
            sampled, matrix,
            "{payload}: upright sampling not pixel-exact"
        );
    }
}

#[test]
fn rotations() {
    for (payload, level) in cases() {
        let (matrix, expected) = encode(payload, level);
        let (base, dim) = base_image(&matrix);
        for deg in [5.0f32, -5.0, 15.0, -15.0, 30.0, -30.0, 45.0, -45.0, 90.0] {
            let img = transform::rotate(&base, deg.to_radians());
            assert_decodes(&img, &expected, &format!("dim{dim} rot{deg}"));
        }
        // Exact half-turn: reliable for small/medium grids.
        if dim <= 29 {
            let img = transform::rotate(&base, std::f32::consts::PI);
            assert_decodes(&img, &expected, &format!("dim{dim} rot180"));
        }
    }
}

#[test]
fn scaling() {
    for (payload, level) in cases() {
        let (matrix, expected) = encode(payload, level);
        let (base, dim) = base_image(&matrix);
        for factor in [1.5f32, 2.0] {
            let img = transform::scale(&base, factor);
            assert_decodes(&img, &expected, &format!("dim{dim} scale{factor}"));
        }
        // Downscaling shrinks modules toward the detector's limit; safe through v4.
        if dim <= 33 {
            let img = transform::scale(&base, 0.75);
            assert_decodes(&img, &expected, &format!("dim{dim} scale0.75"));
        }
    }
}

#[test]
fn blur() {
    for (payload, level) in cases() {
        let (matrix, expected) = encode(payload, level);
        let (base, dim) = base_image(&matrix);
        for sigma in [1.0f32, 1.5, 2.0] {
            let img = transform::gaussian_blur(&base, sigma);
            assert_decodes(&img, &expected, &format!("dim{dim} blur{sigma}"));
        }
    }
}

#[test]
fn noise() {
    for (payload, level) in cases() {
        let (matrix, expected) = encode(payload, level);
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
    for (payload, level) in cases() {
        let (matrix, expected) = encode(payload, level);
        let (base, dim) = base_image(&matrix);
        for (bright, contrast) in [(-40.0f32, 1.0f32), (40.0, 1.0), (0.0, 0.6), (0.0, 1.4)] {
            let img = transform::brightness_contrast(&base, bright, contrast);
            assert_decodes(&img, &expected, &format!("dim{dim} bc{bright}/{contrast}"));
        }
    }
}

#[test]
fn perspective_tilt() {
    // Perspective recovery depends on the bottom-right alignment pattern, which exists
    // from version 2 (dim 25) upward. Version 1 (dim 21) is intentionally excluded.
    for (payload, level) in cases() {
        let (matrix, expected) = encode(payload, level);
        let (base, dim) = base_image(&matrix);
        if !(25..=45).contains(&dim) {
            continue;
        }
        let r = transform::tilt_right(&base, 0.15);
        assert_decodes(&r, &expected, &format!("dim{dim} tiltR0.15"));
        let b = transform::tilt_bottom(&base, 0.15);
        assert_decodes(&b, &expected, &format!("dim{dim} tiltB0.15"));
        // Larger tilt is reliable on the smaller (denser-anchored) versions.
        if dim <= 33 {
            let r = transform::tilt_right(&base, 0.2);
            assert_decodes(&r, &expected, &format!("dim{dim} tiltR0.2"));
            let b = transform::tilt_bottom(&base, 0.2);
            assert_decodes(&b, &expected, &format!("dim{dim} tiltB0.2"));
        }
    }
}

#[test]
fn combined_pipeline() {
    // A realistic capture: slight rotation, optical blur, and sensor noise together.
    for (payload, level) in cases() {
        let (matrix, expected) = encode(payload, level);
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
    let (matrix, expected) = encode(vec![Segment::byte(b"PIPELINE 2026".to_vec())], EcLevel::Q);
    let (base, _) = base_image(&matrix);
    let rotated = transform::rotate(&base, 12.0f32.to_radians());
    let frame = rotated.as_frame();

    let scanner = QrScanner::new();
    let candidates = scanner.detect(&frame, &Hints::new());
    assert_eq!(candidates.len(), 1, "detector should find one QR candidate");
    let cand = &candidates[0];
    assert_eq!(cand.symbology, Some(anyd::Symbology::QrCode));
    assert!(cand.location.module_size.unwrap() > 0.0);

    let symbol = scanner.analyze(&frame, cand).expect("analyze");
    assert_eq!(symbol.payload_bytes(), expected);
    assert_eq!(symbol.symbology, anyd::Symbology::QrCode);
}

#[test]
fn known_hard_edge_cases() {
    // Cases deliberately pushed toward the sampler's limits but kept inside the
    // passing envelope, so a regression that narrows robustness is caught here.

    // 1) Steep tilt on a mid-size symbol (v3), right at the documented 0.25 edge.
    let (matrix, expected) = encode(vec![Segment::byte(vec![b'Q'; 40])], EcLevel::L);
    let (base, dim) = base_image(&matrix);
    assert!((25..=33).contains(&dim), "expected v2..v4, got dim {dim}");
    let steep = transform::tilt_right(&base, 0.25);
    assert_decodes(&steep, &expected, "known-hard steep-tilt");

    // 2) Rotation combined with noise near the noise ceiling.
    let mut rng = Rng::new(555);
    let rot = transform::rotate(&base, 22.0f32.to_radians());
    let noisy = transform::add_noise(&rot, 22.0, &mut rng);
    assert_decodes(&noisy, &expected, "known-hard rot+noise");

    // 3) Downscale to ~4 px/module on a small symbol.
    let (m2, exp2) = encode(vec![Segment::alphanumeric(b"EDGE 42".to_vec())], EcLevel::H);
    let (b2, _) = base_image(&m2);
    let small = transform::scale(&b2, 0.7);
    assert_decodes(&small, &exp2, "known-hard downscale");
}

/// A spread of payloads chosen to span QR versions 1–7 (dims 21, 25, 29, 33, 45).
fn cases() -> Vec<(Vec<Segment>, EcLevel)> {
    vec![
        (vec![Segment::byte(b"HI".to_vec())], EcLevel::M), // v1  (21)
        (
            vec![Segment::alphanumeric(b"ABCDEFGH IJKLMN".to_vec())],
            EcLevel::M,
        ), // v2 (25)
        (
            vec![Segment::byte(b"https://example.com/x?q=hello".to_vec())],
            EcLevel::Q,
        ), // v3 (29)
        (
            vec![Segment::byte(b"MIXED payload 2026 test-suite".to_vec())],
            EcLevel::H,
        ), // v4 (33)
        (vec![Segment::byte(vec![b'B'; 120])], EcLevel::M), // v7 (45)
    ]
}

// ===================================================================================
// Phone-camera captures
// ===================================================================================

/// A symbol of the given version, at level L, filled with URL-like text.
fn of_version(version: usize) -> (BitMatrix, Vec<u8>) {
    let enc = QrEncoder::new();
    let text = "http://example.com/a/rather/long/path?with=a&query=string&and=more&of=it&";
    let fitting = |n: usize| {
        let s: String = text.chars().cycle().take(n).collect();
        enc.build_text(&s, EcLevel::L)
            .ok()
            .and_then(|sym| match enc.encode(&sym) {
                Ok(Encoding::Matrix(m)) => Some(m.width() <= 17 + 4 * version),
                _ => None,
            })
            .unwrap_or(false)
    };
    // The longest text that still fits this version, by bisection.
    let (mut fits, mut over) = (1usize, 3000usize);
    while over - fits > 1 {
        let mid = (fits + over) / 2;
        if fitting(mid) {
            fits = mid;
        } else {
            over = mid;
        }
    }
    let s: String = text.chars().cycle().take(fits).collect();
    let symbol = enc.build_text(&s, EcLevel::L).unwrap();
    match enc.encode(&symbol).unwrap() {
        Encoding::Matrix(m) => {
            assert_eq!(m.width(), 17 + 4 * version);
            (m, symbol.payload_bytes())
        }
        Encoding::Linear(_) => unreachable!("QR encodes to a matrix"),
    }
}

/// What a phone hands over: a symbol four pixels to the module, out of focus and
/// seen at an angle. Blur greys every dark module that stands alone, and the tilt
/// puts the corner opposite the finders modules away from where they point; at level
/// L a handful of misread modules is all the symbol forgives.
#[test]
fn small_blurred_and_tilted() {
    for version in [3, 5, 7] {
        let (matrix, expected) = of_version(version);
        let base = render_matrix(&matrix, 4);
        // A quarter foreshortening is read at version 3; the larger ones to 15 %.
        let tilts: &[(u8, f32)] = if version == 3 {
            &[(0, 0.15), (1, 0.15), (0, 0.25), (1, 0.25)]
        } else {
            &[(0, 0.15), (1, 0.15)]
        };
        for &(tilt, amount) in tilts {
            let img = if tilt == 0 {
                transform::tilt_right(&base, amount)
            } else {
                transform::tilt_bottom(&base, amount)
            };
            for deg in [0.0f32, 12.0] {
                let img = transform::rotate(&img, deg.to_radians());
                let img = transform::gaussian_blur(&img, 1.5);
                assert_decodes(
                    &img,
                    &expected,
                    &format!("v{version} tilt{tilt} {amount} rot{deg}"),
                );
            }
        }
    }
}

/// A symbol as software writes it to a file: a pixel or two to the module and no
/// quiet zone at all, up to the largest version.
#[test]
fn tiny_modules_without_a_quiet_zone() {
    for (version, scales) in [(2, &[1usize, 2][..]), (10, &[2]), (25, &[2]), (40, &[1, 2])] {
        let (matrix, expected) = of_version(version);
        let dim = matrix.width();
        for &scale in scales {
            let mut img = GrayImage::new(dim * scale, dim * scale);
            for y in 0..dim * scale {
                for x in 0..dim * scale {
                    let dark = matrix.get(x / scale, y / scale);
                    img.set(x, y, if dark { 0 } else { 255 });
                }
            }
            assert_decodes(&img, &expected, &format!("v{version} at {scale} px"));
        }
    }
}
