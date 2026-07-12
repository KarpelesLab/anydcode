//! Real-world robustness harness for the QR image sampler.
//!
//! `tests/harness_image.rs` proves the sampler on the clean geometric transforms. This
//! file targets the degradations a *camera photo* adds that a single global threshold
//! cannot survive — uneven lighting / glare, contrast loss from blur, and curved
//! surfaces — plus a committed real bottle-label photo.
//!
//! ## Achieved real-world envelope (asserted below)
//!
//! Symbols are rendered at 6 px/module. On top of that the sampler is asserted to decode:
//!
//! | Degradation                          | Envelope                                    |
//! |--------------------------------------|---------------------------------------------|
//! | Uneven lighting (linear gradient)    | 2.5:1 brightness ramp across the symbol,    |
//! |                                      | either direction — a single global Otsu     |
//! |                                      | threshold cannot binarize this (see the     |
//! |                                      | `imgproc::threshold` gradient test)         |
//! | Lighting gradient + Gaussian blur    | ~2.2:1 ramp with σ = 1.5 px                  |
//! | Combined camera capture              | gradient + 6° rotation + σ1.2 blur + σ12    |
//! |                                      | noise together                              |
//! | Perspective tilt + rotation together | tilt 0.12 + 7° rotation (versions ≥ 2)      |
//! | Cylinder curvature (bottle/can)      | half-arc 0.8 (dim 25) / 0.6 (dim 45); 0.7   |
//! |                                      | with σ1.0 blur — well past the 0.45 the     |
//! |                                      | flat homography reached                     |
//! | Non-planar ripple / crease           | a sine ripple (amp ≈ 1.7 modules) and a     |
//! |                                      | dihedral fold that **no** single homography |
//! |                                      | spans — decoded via the alignment-anchor    |
//! |                                      | thin-plate-spline dewarp (versions ≥ 7)     |
//!
//! These are driven by the adaptive (Bradley/Sauvola) binarization ladder, per-module
//! local thresholding, dual-axis + synthesized finder detection, the interior
//! bottom-right anchor sweep, and — for genuinely non-planar surfaces — a multi-anchor
//! thin-plate-spline dewarp (fit through every detected finder and alignment pattern)
//! added to `codes::qr::sample`.
//!
//! ## Real bottle fixture (`testdata/real_qr_bottle.png`)
//!
//! A tight crop of a QR on a curved, glossy bottle sticker, noticeably blurred (module
//! pitch ≈ 4 px with blur on the same order). The hardened front-end **locates** the
//! symbol and recovers a correctly-sized 25×25 (version 2) grid where the previous global
//! sampler could not even find three finder patterns. The multi-anchor thin-plate-spline
//! dewarp now also runs on it and reaches the point of *reading the format information*,
//! but full payload recovery of this specific capture is still out of reach: it fails with
//! `unreadable format information`. That is a **residual-blur** limit, not a curvature one
//! — with module pitch ≈ 4 px and a blur radius on the same order, the format modules bleed
//! into their neighbours below what version 2's error correction can repair, independent of
//! how well the surface is dewarped (the synthetic `strong_cylinder_beyond_flat_envelope`
//! test decodes far tighter *un-blurred* curvature on this same version). The feature-gated
//! test therefore asserts the recovered front-end geometry and deliberately does not assert
//! full decode.

use anyd::GrayImage;
use anyd::codes::qr::{EcLevel, QrEncoder, scan};
use anyd::output::Encoding;
use anyd::render::render_matrix;
use anyd::segment::Segment;
use anyd::traits::Encode;
use anyd::transform::{self, Axis, Rng};

/// Pixels per module for every rendered symbol here.
const SCALE: usize = 6;

/// Encode `segments` at `level`, returning the upright render, the expected payload and
/// the module dimension.
fn encode(segments: Vec<Segment>, level: EcLevel) -> (GrayImage, Vec<u8>, usize) {
    let enc = QrEncoder::new();
    let symbol = enc.build(segments, level).expect("in-capacity payload");
    let payload = symbol.payload_bytes();
    let matrix = match enc.encode(&symbol).expect("encode") {
        Encoding::Matrix(m) => m,
        Encoding::Linear(_) => unreachable!("QR encodes to a matrix"),
    };
    let dim = matrix.width();
    (render_matrix(&matrix, SCALE), payload, dim)
}

/// Impose a horizontal illumination gradient: luminance is scaled by a factor ramping
/// linearly from `lo` (left) to `hi` (right). This is the degradation a global threshold
/// cannot handle but an adaptive/local one can.
fn lighting_gradient(img: &GrayImage, lo: f32, hi: f32) -> GrayImage {
    let (w, h) = (img.width(), img.height());
    let mut out = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let t = x as f32 / (w.max(2) - 1) as f32;
            let g = lo + (hi - lo) * t;
            let v = (img.get(x, y) as f32 * g).round().clamp(0.0, 255.0) as u8;
            out.set(x, y, v);
        }
    }
    out
}

/// Decode `img` and assert the recovered payload equals `expected`.
fn assert_decodes(img: &GrayImage, expected: &[u8], label: &str) {
    match scan(&img.as_frame()) {
        Ok(sym) => assert_eq!(
            sym.payload_bytes(),
            expected,
            "{label}: payload mismatch after real-world degradation"
        ),
        Err(e) => panic!("{label}: sampler failed to decode ({e})"),
    }
}

/// Payloads spanning QR versions 1–7 (dims 21, 25, 29, 33, 45), mirroring the geometric
/// harness so the real-world envelope is exercised across sizes.
fn cases() -> Vec<(Vec<Segment>, EcLevel)> {
    vec![
        (vec![Segment::byte(b"HI".to_vec())], EcLevel::M), // v1 (21)
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

#[test]
fn uneven_lighting_gradient() {
    // A 2.5:1 brightness ramp across the symbol, both directions. No single global
    // threshold can binarize both the bright and dark halves — the adaptive ladder does.
    for (payload, level) in cases() {
        let (base, expected, dim) = encode(payload, level);
        let dark_right = lighting_gradient(&base, 1.0, 0.4);
        assert_decodes(
            &dark_right,
            &expected,
            &format!("dim{dim} gradient→dark-right"),
        );
        let dark_left = lighting_gradient(&base, 0.4, 1.0);
        assert_decodes(
            &dark_left,
            &expected,
            &format!("dim{dim} gradient→dark-left"),
        );
    }
}

#[test]
fn lighting_gradient_with_blur() {
    // Glare-like gradient plus optical blur — the pairing that collapses a global
    // threshold on real photos.
    for (payload, level) in cases() {
        let (base, expected, dim) = encode(payload, level);
        let img = transform::gaussian_blur(&lighting_gradient(&base, 1.0, 0.45), 1.5);
        assert_decodes(&img, &expected, &format!("dim{dim} gradient+blur"));
    }
}

#[test]
fn combined_capture_degradations() {
    // A realistic hand-held capture: uneven lighting, a small rotation, optical blur and
    // sensor noise, all at once.
    for (payload, level) in cases() {
        let (base, expected, dim) = encode(payload, level);
        let mut img = lighting_gradient(&base, 1.0, 0.5);
        img = transform::rotate(&img, 6.0f32.to_radians());
        img = transform::gaussian_blur(&img, 1.2);
        let mut rng = Rng::new(7);
        img = transform::add_noise(&img, 12.0, &mut rng);
        assert_decodes(&img, &expected, &format!("dim{dim} combined-capture"));
    }
}

#[test]
fn tilt_and_rotation_together() {
    // Perspective tilt and in-plane rotation combined. Perspective recovery needs the
    // bottom-right alignment pattern, present from version 2 (dim 25) up.
    for (payload, level) in cases() {
        let (base, expected, dim) = encode(payload, level);
        if dim < 25 {
            continue;
        }
        let mut img = transform::rotate(&base, 7.0f32.to_radians());
        img = transform::tilt_right(&img, 0.12);
        assert_decodes(&img, &expected, &format!("dim{dim} tilt+rotation"));
    }
}

#[test]
fn cylinder_curvature() {
    // A label wrapped on a bottle/can: the flat symbol is projected onto a cylinder. The
    // interior bottom-right anchor lets the homography bow to follow the curve.
    for (payload, level) in cases() {
        let (base, expected, dim) = encode(payload, level);
        let curved = transform::cylinder(&base, 0.45, Axis::Vertical);
        assert_decodes(&curved, &expected, &format!("dim{dim} cylinder0.45"));
        if dim >= 25 {
            let curved_blur =
                transform::gaussian_blur(&transform::cylinder(&base, 0.35, Axis::Vertical), 1.0);
            assert_decodes(&curved_blur, &expected, &format!("dim{dim} cylinder+blur"));
        }
    }
}

/// Byte payloads that render at versions carrying a genuine interior alignment mesh
/// (dims 45, 61, 81 — versions 7, 12, 16). The non-planar dewarp needs those interior
/// anchors, so it is exercised where the mesh actually exists.
fn mesh_cases() -> Vec<(Vec<Segment>, EcLevel)> {
    vec![
        (vec![Segment::byte(vec![b'B'; 120])], EcLevel::M), // dim 45 (v7)
        (vec![Segment::byte(vec![b'C'; 220])], EcLevel::M), // dim 61 (v12)
        (vec![Segment::byte(vec![b'D'; 440])], EcLevel::M), // dim 81 (v16)
    ]
}

#[test]
fn strong_cylinder_beyond_flat_envelope() {
    // Half-arc well past the 0.45 the flat/homography path reached. Version 2 (dim 25) is
    // small enough that even the interior bottom-right anchor carries it a long way; the
    // larger dim-45 mesh follows a 0.6 arc.
    let (base2, exp2, _) = encode(vec![Segment::byte(vec![b'A'; 20])], EcLevel::M); // dim 25
    for &c in &[0.6f32, 0.8] {
        let curved = transform::cylinder(&base2, c, Axis::Vertical);
        assert_decodes(&curved, &exp2, &format!("dim25 cylinder{c}"));
    }
    // And a 0.7 arc that additionally carries σ1.0 blur.
    let cb = transform::gaussian_blur(&transform::cylinder(&base2, 0.7, Axis::Vertical), 1.0);
    assert_decodes(&cb, &exp2, "dim25 cylinder0.7+blur");

    let (base7, exp7, _) = encode(vec![Segment::byte(vec![b'B'; 120])], EcLevel::M); // dim 45
    let curved7 = transform::cylinder(&base7, 0.6, Axis::Vertical);
    assert_decodes(&curved7, &exp7, "dim45 cylinder0.6");
}

#[test]
fn nonplanar_ripple_and_fold_dewarp() {
    // The headline non-planar cases: surfaces a *single* homography — even the corner-swept
    // one — provably cannot sample, so they decode only through the multi-anchor thin-plate
    // spline. (Verified out-of-band by disabling the TPS path: with it off, none of the
    // asserts below decode; with it on, all do.)

    // Rippled paper: roughly one sine cycle across the symbol, amplitude 10 px (~1.7
    // modules) — a bend the finder-triangle homography cannot track, but the interior
    // alignment anchors pin the spline to.
    for (payload, level) in mesh_cases() {
        let (base, expected, dim) = encode(payload, level);
        let wl = (dim * SCALE) as f32 * 1.5;
        let waved = transform::wave(&base, 10.0, wl, Axis::Vertical);
        assert_decodes(&waved, &expected, &format!("dim{dim} ripple"));
    }

    // A dihedral crease through the middle: two planar halves meeting at an angle no one
    // homography spans. The spline bridges the fold because it has anchors on both faces.
    let (base7, exp7, _) = encode(vec![Segment::byte(vec![b'B'; 120])], EcLevel::M); // dim 45
    let folded7 = transform::fold(&base7, 0.5, 40.0, Axis::Vertical);
    assert_decodes(&folded7, &exp7, "dim45 fold40");

    let (base16, exp16, _) = encode(vec![Segment::byte(vec![b'D'; 440])], EcLevel::M); // dim 81
    let folded16 = transform::fold(&base16, 0.5, 25.0, Axis::Vertical);
    assert_decodes(&folded16, &exp16, "dim81 fold25");
}

/// Real camera photo of a QR on a curved bottle sticker. Gated on the `cli` feature so it
/// can borrow that feature's PNG decoder without adding a library dependency; CI's
/// `cargo test --all-features` runs it, plain `cargo test` skips it.
#[cfg(feature = "cli")]
#[test]
fn real_bottle_photo_front_end() {
    use anyd::GrayFrame;
    use anyd::codes::qr::{QrScanner, sample_grid};
    use anyd::pipeline::Hints;
    use anyd::traits::Detect;

    let bytes = std::fs::read("testdata/real_qr_bottle.png").expect("read fixture");
    let rgba = oxideav_png::decode_png_to_rgba(&bytes).expect("decode PNG");
    let (w, h) = (rgba.width as usize, rgba.height as usize);
    // ITU-R BT.601 luma, as the CLI does.
    let luma: Vec<u8> = rgba
        .data
        .chunks_exact(4)
        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
        .collect();
    let frame = GrayFrame::new(&luma, w, h).expect("valid frame");

    // The hardened front-end locates the symbol on this real, blurred, curved capture —
    // where the previous global-Otsu sampler reported "fewer than three finder patterns
    // found". Detection yields a candidate with a plausible module size...
    let candidates = QrScanner::new().detect(&frame, &Hints::new());
    assert_eq!(candidates.len(), 1, "front-end should locate the bottle QR");
    let module_size = candidates[0]
        .location
        .module_size
        .expect("candidate carries a module size");
    assert!(
        (2.0..8.0).contains(&module_size),
        "module size {module_size} implausible for the fixture"
    );

    // ...and grid recovery produces a valid, square QR module matrix of the correct
    // version-2 size (25×25).
    let grid = sample_grid(&frame).expect("front-end recovers a module grid");
    assert_eq!(grid.width(), grid.height(), "recovered grid must be square");
    assert!(
        grid.width() >= 21 && (grid.width() - 21).is_multiple_of(4),
        "recovered dimension {} is not a valid QR size",
        grid.width()
    );
    assert_eq!(grid.width(), 25, "fixture is a version-2 (25×25) symbol");

    // Full payload decode of this specific heavy-blur + curvature capture is documented
    // as not yet achieved; exercise the path without asserting success so a future
    // dewarping improvement can light it up without editing this test.
    let _ = scan(&frame);
}

/// End-to-end locate → crop → decode on a real captured photo of a 1D barcode, mirroring
/// exactly what the live demo does per frame. `testdata/real_scene_ean.png` is a
/// native-resolution region of a camera capture: a gold PET-bottle label with an EAN-13
/// ("4901085663356") amid dense Japanese text and artwork.
///
/// This is the regression for the locator's per-tile labelling fix. Previously the whole
/// edge-dense scene flood-filled into one frame-spanning matrix blob whose averaged
/// anisotropy read as 2D, so the locator emitted **no** linear box on the barcode and the
/// demo had nothing to crop-decode — even though the decoder reads the barcode fine when
/// handed the right crop. Labelling each tile by its own edge balance before clustering
/// keeps the barcode's directionally-coherent tiles out of the isotropic text, isolating
/// it as a linear candidate.
#[cfg(feature = "cli")]
#[test]
fn real_scene_locate_then_decode_ean() {
    use anyd::GrayFrame;
    use anyd::detect::{LocateOptions, locate};

    let bytes = std::fs::read("testdata/real_scene_ean.png").expect("read fixture");
    let rgba = oxideav_png::decode_png_to_rgba(&bytes).expect("decode PNG");
    let (w, h) = (rgba.width as usize, rgba.height as usize);
    let luma: Vec<u8> = rgba
        .data
        .chunks_exact(4)
        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
        .collect();
    let frame = GrayFrame::new(&luma, w, h).expect("valid frame");

    // Axis-aligned bounds of a candidate's outline, clamped to the frame.
    let bounds = |c: &anyd::pipeline::Candidate| {
        let cs = c.location.outline.corners;
        let x0 = cs.iter().map(|p| p.x).fold(f32::MAX, f32::min).max(0.0) as usize;
        let y0 = cs.iter().map(|p| p.y).fold(f32::MAX, f32::min).max(0.0) as usize;
        let x1 = (cs.iter().map(|p| p.x).fold(0.0, f32::max) as usize).min(w);
        let y1 = (cs.iter().map(|p| p.y).fold(0.0, f32::max) as usize).min(h);
        (x0, y0, x1, y1)
    };

    // The barcode occupies roughly (250,205)-(790,405); its centre is ~(512,295).
    let (bx, by) = (512usize, 295usize);
    let cands = locate(&frame, &LocateOptions::default());
    let bar = cands
        .iter()
        .find(|c| {
            c.symbology.map(|s| s.dimension()) == Some(anyd::Dimension::Linear) && {
                let (x0, y0, x1, y1) = bounds(c);
                x0 <= bx && bx < x1 && y0 <= by && by < y1
            }
        })
        .expect("locate() must isolate the barcode as a linear candidate over its centre");

    // Crop the located box and decode it end-to-end, exactly as the demo's worker does.
    let (x0, y0, x1, y1) = bounds(bar);
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut crop = vec![0u8; cw * ch];
    for y in 0..ch {
        for x in 0..cw {
            crop[y * cw + x] = frame.get_unchecked(x0 + x, y0 + y);
        }
    }
    let cframe = GrayFrame::new(&crop, cw, ch).expect("valid crop");

    let lines = anyd::scan1d::scan_lines(&cframe, &anyd::scan1d::ScanOptions::default());
    let dec = anyd::codes::ean::EanDecoder::new();
    let text = lines
        .iter()
        .find_map(|cand| anyd::scan1d::try_decode(cand, &dec).and_then(|s| s.text()))
        .expect("the located region must decode as EAN-13");
    assert_eq!(
        text, "4901085663356",
        "wrong payload from the located barcode"
    );
}
