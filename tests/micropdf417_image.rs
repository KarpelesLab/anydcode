//! MicroPDF417 image-sampling tests: render → transform → scan through the
//! dark-cloud corner detector and aspect-ordered variant search.

use anyd::GrayImage;
use anyd::codes::pdf417::{MicroPdf417Encoder, scan_micro};
use anyd::render::render;
use anyd::segment::Segment;
use anyd::traits::Encode;
use anyd::transform;

fn framed(text: &str, columns: Option<usize>, scale: usize, margin: usize) -> GrayImage {
    let enc = MicroPdf417Encoder::new();
    let sym = enc
        .build_sized(vec![Segment::byte(text.as_bytes().to_vec())], columns)
        .unwrap();
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
fn scans_upright() {
    let img = framed("MICROPDF", None, 4, 16);
    let sym = scan_micro(&img.as_frame()).expect("MicroPDF417 must scan");
    assert_eq!(sym.text().as_deref(), Some("MICROPDF"));
}

#[test]
fn scans_all_column_counts() {
    for cols in 1..=4usize {
        let img = framed("COLUMN TEST DATA", Some(cols), 4, 16);
        let sym = scan_micro(&img.as_frame()).unwrap_or_else(|e| panic!("{cols} columns: {e:?}"));
        assert_eq!(
            sym.text().as_deref(),
            Some("COLUMN TEST DATA"),
            "{cols} cols"
        );
    }
}

#[test]
fn scans_rotated() {
    let img = framed("ROT", None, 4, 16);
    for deg in [11.0f32, 90.0, -37.0, 180.0] {
        let r = transform::rotate(&img, deg.to_radians());
        let sym = scan_micro(&r.as_frame()).unwrap_or_else(|e| panic!("rotation {deg}°: {e:?}"));
        assert_eq!(sym.text().as_deref(), Some("ROT"), "at {deg}°");
    }
}

#[test]
fn scans_degraded() {
    let img = framed("HARD", None, 4, 16);
    let scaled = transform::scale(&img, 1.5);
    assert_eq!(
        scan_micro(&scaled.as_frame()).unwrap().text().as_deref(),
        Some("HARD")
    );
    let blurred = transform::gaussian_blur(&img, 0.8);
    assert_eq!(
        scan_micro(&blurred.as_frame()).unwrap().text().as_deref(),
        Some("HARD")
    );
    let mut rng = transform::Rng::new(3);
    let noisy = transform::add_noise(&img, 10.0, &mut rng);
    assert_eq!(
        scan_micro(&noisy.as_frame()).unwrap().text().as_deref(),
        Some("HARD")
    );
}
