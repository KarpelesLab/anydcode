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
//! Symbols rendered at 6 px/module with the standard 1-module quiet zone. For the
//! square sizes, decoding is asserted for:
//!
//! | Transform            | Envelope                                                  |
//! |----------------------|-----------------------------------------------------------|
//! | Rotation             | any angle; ±5°…±45°, 90° all sizes; 180° for dim ≤ 22     |
//! | Scale                | 0.75×, 1.5×, 2.0× all sizes (down to ~4.5 px/module)      |
//! | Gaussian blur        | σ ≤ 1.5 px                                                |
//! | Additive noise       | σ ≤ 25 luminance                                          |
//! | Brightness/contrast  | ±40 brightness, 0.6–1.4 contrast                          |
//! | Perspective tilt     | ≤ 0.30 on either axis, all sizes, at any rotation         |
//! | Combined             | 8° rotation + σ1.0 blur + σ12 noise                       |
//!
//! The rectangular sizes (ISO/IEC 16022 and DMRE) are held to the same transforms in
//! the `rectangles_*` tests, perspective excepted for the five 8-row sizes of 64
//! columns and more.
//!
//! A Data Matrix has no interior alignment anchor, so the fourth (timing-meeting)
//! corner is recovered from the two timing borders alone: seeded where the tangents
//! from the L's ends to the symbol's ink meet, then refined by fitting straight lines
//! to the borders and intersecting them (`steep_perspective`, and amid print in
//! `perspective_amid_print`). Magnitudes are set a comfortable margin inside the
//! sampler's measured failure points; the `known_hard_edge_cases` test pins a few
//! points near the edge.
#![cfg(all(feature = "encode", feature = "scan", feature = "datamatrix"))]

use anyd::codes::datamatrix::{DataMatrixEncoder, DataMatrixScanner, sample_grid, scan};
use anyd::output::{BitMatrix, Encoding};
use anyd::pipeline::Hints;
use anyd::render::render_matrix;
use anyd::traits::{Analyze, Detect, Encode};
use anyd::transform::{self, Rng};
use anyd::{GrayImage, Symbology};

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
    // The historical, size-graded tilt envelope; `steep_perspective` goes further.
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
fn steep_perspective() {
    // A symbol seen well off-axis: one side foreshortened by 15 % and by 30 %, about
    // either axis (`tilt_right` shortens a timing border, `tilt_bottom` an arm of the
    // L, which puts the far corner *beyond* the parallelogram of the other three), at
    // every size, also turned in the frame and through a mildly soft, noisy capture.
    for payload in cases() {
        let (matrix, expected) = encode(&payload);
        let (base, dim) = base_image(&matrix);
        for amount in [0.15f32, 0.30] {
            for (axis, tilted) in [
                ("R", transform::tilt_right(&base, amount)),
                ("B", transform::tilt_bottom(&base, amount)),
            ] {
                for (deg, soft) in [(0.0f32, false), (17.0, true), (45.0, false), (200.0, true)] {
                    let mut img = tilted.clone();
                    if deg != 0.0 {
                        img = transform::rotate(&img, deg.to_radians());
                    }
                    if soft {
                        let mut rng = Rng::new(7);
                        img = transform::add_noise(
                            &transform::gaussian_blur(&img, 1.0),
                            6.0,
                            &mut rng,
                        );
                    }
                    let label = format!("dim{dim} tilt{axis}{amount} rot{deg} soft={soft}");
                    assert_decodes(&img, &expected, &label);
                }
            }
        }
    }
}

/// Paste `sprite` onto a `w × h` page of luminance `paper` with its top-left at
/// `(x0, y0)`, then scatter glyph-sized dark boxes — print — over the rest of the page,
/// right up to the sprite's box.
fn printed_page(sprite: &GrayImage, w: usize, h: usize, (x0, y0): (usize, usize)) -> GrayImage {
    let mut page = GrayImage::filled(w, h, 235);
    let mut rng = Rng::new(42);
    let mut y = 6;
    while y + 14 < h {
        let mut x = 6;
        while x + 10 < w {
            let (gw, gh) = (
                3 + (rng.next_f32() * 6.0) as usize,
                8 + (rng.next_f32() * 5.0) as usize,
            );
            if rng.next_f32() > 0.2 {
                for yy in y..y + gh {
                    for xx in x..x + gw {
                        page.set(xx, yy, 40);
                    }
                }
            }
            x += gw + 3;
        }
        y += 18;
    }
    for y in 0..sprite.height() {
        for x in 0..sprite.width() {
            page.set(x0 + x, y0 + y, sprite.get(x, y));
        }
    }
    page
}

#[test]
fn perspective_amid_print() {
    // The tilted symbol's far corner has to be found from its timing borders, and
    // print one quiet zone away from them must not be mistaken for more of the same.
    let (matrix, expected) = encode(b"DM-LIVE-2026");
    let (base, dim) = base_image(&matrix);
    for (axis, tilted) in [
        ("R", transform::tilt_right(&base, 0.3)),
        ("B", transform::tilt_bottom(&base, 0.3)),
    ] {
        for deg in [0.0f32, 17.0, 45.0] {
            // The warp's own margin is cropped back to the (foreshortened) quiet zone.
            let turned = if deg == 0.0 {
                tilted.clone()
            } else {
                transform::rotate(&tilted, deg.to_radians())
            };
            let page = printed_page(&turned, 320, 300, (70, 60));
            let img = transform::gaussian_blur(&page, 0.8);
            assert_decodes(
                &img,
                &expected,
                &format!("dim{dim} tilt{axis} rot{deg} in print"),
            );
        }
    }
}

#[test]
fn grain_under_uneven_light() {
    // A dark, grainy capture lit from one side: the frame-wide Otsu threshold is spent
    // on telling the lit half of the page from the shaded one, so the grid has to be
    // read against the symbol's own.
    let (matrix, expected) = encode(b"DM-LIVE-2026");
    let sprite = anyd::render::render_matrix(&matrix, 5);
    let (mut read, mut total) = (0u32, 0u32);
    for (deg, left) in [(0.0f32, 150usize), (0.0, 250), (45.0, 150), (45.0, 250)] {
        let turned = if deg == 0.0 {
            sprite.clone()
        } else {
            transform::rotate(&sprite, deg.to_radians())
        };
        let (w, h) = (640, 360);
        let mut page = GrayImage::filled(w, h, 232);
        for y in 0..turned.height() {
            for x in 0..turned.width() {
                page.set(left + x, 130 + y, turned.get(x, y));
            }
        }
        for y in 0..h {
            for x in 0..w {
                let lit = 0.59 + 0.27 * (x as f32 / w as f32);
                page.set(x, y, (page.get(x, y) as f32 * lit) as u8);
            }
        }
        let dim = transform::brightness_contrast(&transform::gaussian_blur(&page, 0.8), -30.0, 0.6);
        // Noise this heavy leaves some frames unreadable whatever the threshold, so the
        // claim is a read *rate* over independent noise, not one lucky frame. A wrong read is never acceptable.
        for seed in 1..=8u64 {
            let img = transform::add_noise(&dim, 16.0, &mut Rng::new(seed));
            total += 1;
            if let Ok(sym) = scan(&img.as_frame()) {
                assert_eq!(
                    sym.payload_bytes(),
                    expected,
                    "grain + falloff rot{deg} x{left} seed {seed}: wrong payload"
                );
                read += 1;
            }
        }
    }
    assert!(
        read * 100 >= total * 60,
        "grain + falloff: read {read}/{total}"
    );
}

#[test]
fn other_ink_is_not_read() {
    // The sampler runs on every frame, most of which show something else: neither a
    // solid block, nor print, nor noise may come back as a symbol.
    let mut rng = Rng::new(3);
    let block = {
        let mut img = GrayImage::filled(200, 160, 240);
        for y in 30..130 {
            for x in 40..150 {
                img.set(x, y, 20);
            }
        }
        img
    };
    let print = printed_page(&GrayImage::filled(1, 1, 235), 320, 240, (0, 0));
    let noise = transform::add_noise(&GrayImage::filled(160, 160, 128), 60.0, &mut rng);
    for (label, img) in [("block", block), ("print", print), ("noise", noise)] {
        assert!(
            scan(&img.as_frame()).is_err(),
            "{label} read as a Data Matrix"
        );
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

// ===================================================================================
// Rectangular symbols
// ===================================================================================

/// Every rectangular size, ISO/IEC 16022 and DMRE, as `(width, height)`.
const RECTANGLES: &[(usize, usize)] = &[
    (18, 8),
    (32, 8),
    (26, 12),
    (36, 12),
    (36, 16),
    (48, 16),
    (48, 8),
    (64, 8),
    (80, 8),
    (96, 8),
    (120, 8),
    (144, 8),
    (64, 12),
    (88, 12),
    (64, 16),
    (36, 20),
    (44, 20),
    (64, 20),
    (48, 22),
    (48, 24),
    (64, 24),
    (40, 26),
    (48, 26),
    (64, 26),
];

/// A `width × height` symbol about three-quarters full of mixed text.
fn rectangle(width: usize, height: usize) -> (BitMatrix, Vec<u8>) {
    let enc = DataMatrixEncoder::new();
    let text = b"Rect 8x18 DMRE: 0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    // The longest prefix that fits, less a quarter.
    let fits = (1..=text.len())
        .take_while(|&n| enc.build_rectangular(&text[..n], width, height).is_ok())
        .last()
        .expect("a rectangle holds something");
    let payload = text[..(fits * 3).div_ceil(4)].to_vec();
    let symbol = enc.build_rectangular(&payload, width, height).unwrap();
    match enc.encode(&symbol).expect("encode") {
        Encoding::Matrix(m) => (m, payload),
        Encoding::Linear(_) => unreachable!("Data Matrix encodes to a matrix"),
    }
}

#[test]
fn rectangles_upright_are_pixel_exact() {
    for &(w, h) in RECTANGLES {
        let (matrix, _) = rectangle(w, h);
        assert_eq!((matrix.width(), matrix.height()), (w, h));
        let img = render_matrix(&matrix, SCALE);
        let sampled = sample_grid(&img.as_frame()).unwrap_or_else(|e| panic!("{w}x{h}: {e}"));
        assert_eq!(sampled, matrix, "{w}x{h}: upright sampling not pixel-exact");
    }
}

#[test]
fn rectangles_rotated_scaled_and_degraded() {
    for &(w, h) in RECTANGLES {
        let (matrix, expected) = rectangle(w, h);
        let base = render_matrix(&matrix, SCALE);
        for deg in [5.0f32, -15.0, 30.0, -45.0, 90.0, 180.0, 250.0] {
            let img = transform::rotate(&base, deg.to_radians());
            assert_decodes(&img, &expected, &format!("{w}x{h} rot{deg}"));
        }
        for factor in [0.75f32, 1.5] {
            let img = transform::scale(&base, factor);
            assert_decodes(&img, &expected, &format!("{w}x{h} scale{factor}"));
        }
        let img = transform::gaussian_blur(&base, 1.2);
        assert_decodes(&img, &expected, &format!("{w}x{h} blur"));
        let mut rng = Rng::new(7);
        let img = transform::add_noise(&base, 20.0, &mut rng);
        assert_decodes(&img, &expected, &format!("{w}x{h} noise"));
        let img = transform::rotate(&transform::gaussian_blur(&base, 1.0), 8f32.to_radians());
        let img = transform::add_noise(&img, 12.0, &mut rng);
        assert_decodes(&img, &expected, &format!("{w}x{h} combined"));
    }
}

/// A tilt foreshortens a symbol in proportion to the depth it spans: its width for
/// a yaw, its height for a pitch. The pitch is scaled to the rectangle's height, so
/// that both are the tilts the squares are held to. The five 8-row DMRE sizes of 64
/// columns and more are left out: over such a length the four corners no longer pin
/// down the foreshortening along the row.
#[test]
fn rectangles_under_perspective() {
    for &(w, h) in RECTANGLES.iter().filter(|&&(w, h)| w < 8 * h) {
        let (matrix, expected) = rectangle(w, h);
        let base = render_matrix(&matrix, SCALE);
        for amount in [0.1f32, 0.2, 0.3] {
            let pitch = amount * h as f32 / w as f32;
            let tilts = [
                transform::tilt_right(&base, amount),
                transform::tilt_bottom(&base, pitch),
            ];
            for (axis, img) in tilts.iter().enumerate() {
                assert_decodes(img, &expected, &format!("{w}x{h} tilt{axis} {amount}"));
                let img = transform::rotate(img, 20f32.to_radians());
                assert_decodes(
                    &img,
                    &expected,
                    &format!("{w}x{h} tilt{axis} {amount} rot20"),
                );
            }
        }
    }
}

/// A symbol as software writes it to a file: two pixels to the module, with the
/// one-module quiet zone. Every rectangle and a spread of squares.
#[test]
fn two_pixel_modules() {
    for &(w, h) in RECTANGLES {
        let (matrix, expected) = rectangle(w, h);
        let img = render_matrix(&matrix, 2);
        assert_decodes(&img, &expected, &format!("{w}x{h} at 2 px"));
        let sampled = sample_grid(&img.as_frame()).unwrap();
        assert_eq!(sampled, matrix, "{w}x{h} at 2 px");
    }
    for payload in cases() {
        let (matrix, expected) = encode(&payload);
        let img = render_matrix(&matrix, 2);
        if img.width() * img.height() <= 320 * 200 {
            assert_decodes(&img, &expected, &format!("dim{} at 2 px", matrix.width()));
        }
    }
    // The reported outline is in the frame's own pixels.
    let (matrix, _) = rectangle(48, 16);
    let img = render_matrix(&matrix, 2);
    let location = scan(&img.as_frame()).unwrap().location.unwrap();
    for p in location.outline.corners {
        assert!(p.x >= 0.0 && p.x <= img.width() as f32, "{p:?}");
        assert!(p.y >= 0.0 && p.y <= img.height() as f32, "{p:?}");
    }
    let xs = location.outline.corners.map(|p| p.x);
    let span =
        xs.iter().fold(f32::MIN, |a, &b| a.max(b)) - xs.iter().fold(f32::MAX, |a, &b| a.min(b));
    assert!((span - 96.0).abs() < 3.0, "{span}");
}
