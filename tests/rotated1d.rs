//! Rotated linear-barcode decoding through the shared [`anyd::pipeline::scan_1d`] path.
//!
//! The scanline sweep only covers a few degrees around horizontal; these tests pin the
//! two layers that lift that to full rotation coverage:
//!
//! - **mirrored patterns** — a 180°-rotated code presents its module sequence in
//!   reverse; every candidate is now also tried mirrored, and
//! - **orientation-estimated derotation** — for any other angle the crop's dominant
//!   gradient orientation ([`anyd::imgproc::orient`]) locates the reading axis, the
//!   crop is rotated upright and rescanned, and the reported outline is mapped back
//!   into the original frame.

use anyd::codes::code128::Code128Encoder;
use anyd::codes::ean::EanEncoder;
use anyd::pipeline::scan_1d;
use anyd::render::render;
use anyd::traits::Encode;
use anyd::transform::rotate;
use anyd::{GrayImage, Symbology};

/// Render an EAN-13 at `scale` px/module with a tall bar band.
fn ean(scale: usize) -> GrayImage {
    let enc = EanEncoder::new();
    render(&enc.encode(&enc.build_ean13("4901777300521").unwrap()).unwrap(), scale)
}

fn code128(scale: usize) -> GrayImage {
    let enc = Code128Encoder::new();
    render(&enc.encode(&enc.build_text("ROT-128").unwrap()).unwrap(), scale)
}

/// Scan a rotated render and assert the expected symbology + payload comes back.
fn assert_rotated_decodes(img: &GrayImage, deg: f32, sym: Symbology, text: &str) {
    let rotated = rotate(img, deg.to_radians());
    let found = scan_1d(&rotated.as_frame());
    let hit = found.iter().find(|s| s.symbology == sym);
    assert!(
        hit.is_some_and(|s| s.text().as_deref() == Some(text)),
        "{sym} at {deg}°: expected {text:?}, got {:?}",
        found
            .iter()
            .map(|s| (s.symbology, s.text()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn ean13_decodes_at_ninety_degrees() {
    let img = ean(3);
    assert_rotated_decodes(&img, 90.0, Symbology::Ean13, "4901777300521");
    assert_rotated_decodes(&img, -90.0, Symbology::Ean13, "4901777300521");
}

#[test]
fn ean13_decodes_upside_down() {
    assert_rotated_decodes(&ean(3), 180.0, Symbology::Ean13, "4901777300521");
}

#[test]
fn ean13_decodes_at_oblique_angles() {
    let img = ean(4);
    for deg in [30.0, -37.0, 60.0, 135.0] {
        assert_rotated_decodes(&img, deg, Symbology::Ean13, "4901777300521");
    }
}

#[test]
fn code128_decodes_rotated_and_mirrored() {
    let img = code128(3);
    assert_rotated_decodes(&img, 90.0, Symbology::Code128, "ROT-128");
    assert_rotated_decodes(&img, 180.0, Symbology::Code128, "ROT-128");
    assert_rotated_decodes(&img, -50.0, Symbology::Code128, "ROT-128");
}

/// The derotated outline must land back on the code in original-frame coordinates.
#[test]
fn rotated_outline_maps_back_into_frame() {
    let img = ean(4);
    let rotated = rotate(&img, 45.0_f32.to_radians());
    let found = scan_1d(&rotated.as_frame());
    let sym = found
        .iter()
        .find(|s| s.symbology == Symbology::Ean13)
        .expect("EAN at 45° must decode");
    let loc = sym.location.as_ref().expect("decoded symbol carries a location");
    let c = loc.outline.center();
    let (w, h) = (rotated.width() as f32, rotated.height() as f32);
    // The symbol fills the rotated canvas's centre; its mapped-back centre must land
    // well inside the frame, near the middle.
    assert!(
        (c.x - w / 2.0).abs() < w * 0.2 && (c.y - h / 2.0).abs() < h * 0.2,
        "outline centre {c:?} far from frame centre ({w}x{h})"
    );
}
