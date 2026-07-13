//! Behavioural tests for the fast frame locator (`anyd::detect`).
//!
//! These run in CI (unlike the benchmark). They assert the locator finds a candidate
//! near every planted code on populated frames and stays quiet on a blank one. Frames
//! are synthesized by rendering real symbols (`anyd::render`) into a blank canvas, so
//! the tests exercise the same path the benchmark times. Everything is small and fast.

use anyd::GrayImage;
use anyd::codes::code128::Code128Encoder;
use anyd::codes::datamatrix::DataMatrixEncoder;
use anyd::codes::qr::{EcLevel, QrEncoder};
use anyd::detect::{Families, LocateOptions, locate};
use anyd::geometry::Point;
use anyd::render::render;
use anyd::traits::Encode;

/// Render a QR symbol to a grayscale image at `scale` px/module.
fn qr_image(text: &str, scale: usize) -> GrayImage {
    let enc = QrEncoder::new();
    let sym = enc.build_text(text, EcLevel::M).unwrap();
    render(&enc.encode(&sym).unwrap(), scale)
}

/// Render a Data Matrix symbol to a grayscale image at `scale` px/module.
fn datamatrix_image(text: &str, scale: usize) -> GrayImage {
    let enc = DataMatrixEncoder::new();
    let sym = enc.build_text(text).unwrap();
    render(&enc.encode(&sym).unwrap(), scale)
}

/// Render a Code 128 symbol to a grayscale image at `scale` px/module.
fn code128_image(text: &str, scale: usize) -> GrayImage {
    let enc = Code128Encoder::new();
    let sym = enc.build_text(text).unwrap();
    render(&enc.encode(&sym).unwrap(), scale)
}

/// A blank light canvas of the given size.
fn blank(width: usize, height: usize) -> GrayImage {
    GrayImage::filled(width, height, 255)
}

/// Copy `sprite` onto `canvas` with its top-left at `(ox, oy)`; return the sprite centre.
fn place(canvas: &mut GrayImage, sprite: &GrayImage, ox: usize, oy: usize) -> Point {
    for y in 0..sprite.height() {
        for x in 0..sprite.width() {
            let cx = ox + x;
            let cy = oy + y;
            if cx < canvas.width() && cy < canvas.height() {
                canvas.set(cx, cy, sprite.get(x, y));
            }
        }
    }
    Point::new(
        (ox + sprite.width() / 2) as f32,
        (oy + sprite.height() / 2) as f32,
    )
}

/// Is there a candidate whose bounding-quad centre is within `tol` px of `target`?
fn found_near(cands: &[anyd::pipeline::Candidate], target: Point, tol: f32) -> bool {
    cands
        .iter()
        .any(|c| c.location.outline.center().distance(target) <= tol)
}

#[test]
fn blank_frame_yields_no_candidates() {
    let canvas = blank(320, 240);
    let cands = locate(&canvas.as_frame(), &LocateOptions::default());
    assert!(
        cands.is_empty(),
        "blank frame should produce no candidates, got {}",
        cands.len()
    );
}

#[test]
fn locates_single_qr() {
    let mut canvas = blank(480, 360);
    let qr = qr_image("HELLO WORLD", 6);
    let center = place(&mut canvas, &qr, 120, 90);

    let cands = locate(&canvas.as_frame(), &LocateOptions::default());
    assert!(
        !cands.is_empty(),
        "expected at least one candidate for a QR"
    );
    assert!(
        found_near(&cands, center, 64.0),
        "no candidate near planted QR at {center:?}; candidates: {:?}",
        cands
            .iter()
            .map(|c| c.location.outline.center())
            .collect::<Vec<_>>()
    );
    // A QR should be reported as a matrix-family guess.
    let near = cands
        .iter()
        .find(|c| c.location.outline.center().distance(center) <= 64.0)
        .unwrap();
    assert_eq!(
        near.symbology.map(|s| s.dimension()),
        Some(anyd::Dimension::Matrix),
        "QR candidate should guess the matrix family"
    );
}

#[test]
fn locates_mixed_codes() {
    let mut canvas = blank(640, 480);
    let qr = qr_image("ABCDEFG", 6);
    let dm = datamatrix_image("DM", 6);
    let c128 = code128_image("CODE128", 2);

    let qr_c = place(&mut canvas, &qr, 40, 40);
    let dm_c = place(&mut canvas, &dm, 380, 60);
    let c128_c = place(&mut canvas, &c128, 60, 320);

    let cands = locate(&canvas.as_frame(), &LocateOptions::default());
    assert!(
        found_near(&cands, qr_c, 72.0),
        "missed QR at {qr_c:?}; centres: {:?}",
        cands
            .iter()
            .map(|c| c.location.outline.center())
            .collect::<Vec<_>>()
    );
    assert!(
        found_near(&cands, dm_c, 72.0),
        "missed Data Matrix at {dm_c:?}"
    );
    assert!(
        found_near(&cands, c128_c, 96.0),
        "missed Code 128 at {c128_c:?}"
    );
}

#[test]
fn families_filter_restricts_output() {
    let mut canvas = blank(640, 480);
    let qr = qr_image("MATRIXONLY", 6);
    let c128 = code128_image("LINEARONLY", 2);
    place(&mut canvas, &qr, 40, 40);
    let c128_c = place(&mut canvas, &c128, 60, 320);

    // Matrix-only must not report the 1D region.
    let opts = LocateOptions {
        families: Families::MATRIX,
        ..LocateOptions::default()
    };
    let cands = locate(&canvas.as_frame(), &opts);
    assert!(
        !found_near(&cands, c128_c, 96.0),
        "matrix-only filter unexpectedly reported the linear code"
    );
    assert!(
        cands
            .iter()
            .all(|c| c.symbology.map(|s| s.dimension()) == Some(anyd::Dimension::Matrix)),
        "matrix-only filter returned a non-matrix candidate"
    );
}

/// A code held close to the camera fills most of the frame. That defeats the texture
/// pass twice over — huge modules leave tiles below the edge-density floor, and the
/// region (if any) trips the scene-blob size cap — so this used to yield *no* candidate.
/// Finder hits must rescue it: a finder-backed region is exempt from the size cap, and
/// unclaimed finder hits synthesize a candidate on their own.
#[test]
fn locates_close_up_qr_filling_the_frame() {
    // 25×25 modules at 16 px/module ≈ 464 px incl. quiet zone, in a 480×480 frame.
    let qr = qr_image("CLOSE-UP", 16);
    let mut canvas = blank(qr.width() + 16, qr.height() + 16);
    let center = place(&mut canvas, &qr, 8, 8);

    let cands = locate(&canvas.as_frame(), &LocateOptions::default());
    assert!(
        found_near(&cands, center, 0.4 * qr.width() as f32),
        "close-up QR must still be located; candidates: {:?}",
        cands
            .iter()
            .map(|c| c.location.outline.center())
            .collect::<Vec<_>>()
    );
}

/// One physical code must not be reported as a pile of overlapping fragments: after
/// duplicate suppression no two candidates may cover the same ground.
#[test]
fn candidates_do_not_duplicate_one_code() {
    let mut canvas = blank(480, 360);
    let qr = qr_image("NO DUPLICATES PLEASE", 6);
    place(&mut canvas, &qr, 120, 90);

    let cands = locate(&canvas.as_frame(), &LocateOptions::default());
    let boxes: Vec<(f32, f32, f32, f32)> = cands
        .iter()
        .map(|c| {
            let cs = c.location.outline.corners;
            (
                cs.iter().map(|p| p.x).fold(f32::MAX, f32::min),
                cs.iter().map(|p| p.y).fold(f32::MAX, f32::min),
                cs.iter().map(|p| p.x).fold(f32::MIN, f32::max),
                cs.iter().map(|p| p.y).fold(f32::MIN, f32::max),
            )
        })
        .collect();
    for (i, a) in boxes.iter().enumerate() {
        for b in &boxes[i + 1..] {
            let ix = (a.2.min(b.2) - a.0.max(b.0)).max(0.0);
            let iy = (a.3.min(b.3) - a.1.max(b.1)).max(0.0);
            let min_area = ((a.2 - a.0) * (a.3 - a.1)).min((b.2 - b.0) * (b.3 - b.1));
            assert!(
                ix * iy <= 0.65 * min_area,
                "candidates {a:?} and {b:?} overlap like duplicates"
            );
        }
    }
}

#[test]
fn respects_max_candidates() {
    let mut canvas = blank(640, 480);
    // Several codes scattered around.
    place(&mut canvas, &qr_image("ONE", 5), 20, 20);
    place(&mut canvas, &qr_image("TWO", 5), 300, 20);
    place(&mut canvas, &qr_image("THREE", 5), 20, 260);
    place(&mut canvas, &qr_image("FOUR", 5), 300, 260);

    let opts = LocateOptions {
        max_candidates: 2,
        ..LocateOptions::default()
    };
    let cands = locate(&canvas.as_frame(), &opts);
    assert!(
        cands.len() <= 2,
        "max_candidates not honoured: {}",
        cands.len()
    );
}
