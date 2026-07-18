//! Aztec image-sampling tests: render → transform → scan through the bullseye
//! detector and core homography.

use anyd::GrayImage;
use anyd::codes::aztec::{AztecEncoder, scan};
use anyd::render::render;
use anyd::traits::Encode;
use anyd::transform;

/// Render a symbol at `scale` px/module onto a light canvas with a margin.
fn framed(text: &str, scale: usize, margin: usize) -> GrayImage {
    let enc = AztecEncoder::new();
    let sym = enc.build_text(text).unwrap();
    let img = render(&enc.encode(&sym).unwrap(), scale);
    let mut canvas = GrayImage::filled(img.width() + 2 * margin, img.height() + 2 * margin, 255);
    for y in 0..img.height() {
        for x in 0..img.width() {
            canvas.set(x + margin, y + margin, img.get(x, y));
        }
    }
    canvas
}

#[test]
fn scans_upright_compact() {
    let img = framed("AZTEC SCAN", 6, 24);
    let sym = scan(&img.as_frame()).expect("compact Aztec must scan");
    assert_eq!(sym.text().as_deref(), Some("AZTEC SCAN"));
}

#[test]
fn scans_rotated() {
    let img = framed("ROTATED AZTEC", 6, 24);
    for deg in [17.0f32, 90.0, -49.0, 180.0] {
        let r = transform::rotate(&img, deg.to_radians());
        let sym = scan(&r.as_frame()).unwrap_or_else(|e| panic!("rotation {deg}°: {e:?}"));
        assert_eq!(sym.text().as_deref(), Some("ROTATED AZTEC"), "at {deg}°");
    }
}

#[test]
fn scans_scaled_blurred_noisy() {
    let img = framed("DEGRADED", 6, 24);
    let scaled = transform::scale(&img, 1.6);
    assert_eq!(
        anyd::codes::aztec::scan(&scaled.as_frame())
            .unwrap()
            .text()
            .as_deref(),
        Some("DEGRADED"),
        "1.6x scale"
    );
    let blurred = transform::gaussian_blur(&img, 1.0);
    assert_eq!(
        anyd::codes::aztec::scan(&blurred.as_frame())
            .unwrap()
            .text()
            .as_deref(),
        Some("DEGRADED"),
        "sigma 1.0 blur"
    );
    let mut rng = transform::Rng::new(7);
    let noisy = transform::add_noise(&img, 14.0, &mut rng);
    assert_eq!(
        anyd::codes::aztec::scan(&noisy.as_frame())
            .unwrap()
            .text()
            .as_deref(),
        Some("DEGRADED"),
        "sigma 14 noise"
    );
}

#[test]
fn scans_rune() {
    let enc = AztecEncoder::new();
    let img = render(&enc.encode(&enc.build_rune(42)).unwrap(), 6);
    let mut canvas = GrayImage::filled(img.width() + 48, img.height() + 48, 255);
    for y in 0..img.height() {
        for x in 0..img.width() {
            canvas.set(x + 24, y + 24, img.get(x, y));
        }
    }
    let sym = scan(&canvas.as_frame()).expect("rune must scan");
    assert_eq!(sym.symbology, anyd::Symbology::AztecRunes);
}

#[test]
fn scans_full_range() {
    // Long payload forces a full-range symbol (>4 layers worth of data).
    let text = "FULL RANGE AZTEC 0123456789 ".repeat(6);
    let img = framed(&text, 5, 24);
    let sym = scan(&img.as_frame()).expect("full-range Aztec must scan");
    assert_eq!(sym.text().as_deref(), Some(text.as_str()));
}
