//! Micro QR image-sampling tests: render → transform → scan through the
//! single-finder detector and corner homography.

use anyd::GrayImage;
use anyd::codes::microqr::MicroEcLevel;
use anyd::codes::microqr::{MicroQrEncoder, scan};
use anyd::render::render;
use anyd::segment::Segment;
use anyd::traits::Encode;
use anyd::transform;

fn framed(data: &[u8], ec: MicroEcLevel, scale: usize, margin: usize) -> GrayImage {
    let enc = MicroQrEncoder::new();
    let sym = enc.build(vec![Segment::byte(data.to_vec())], ec).unwrap();
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
    let img = framed(b"MICRO", MicroEcLevel::M, 6, 24);
    let sym = scan(&img.as_frame()).expect("Micro QR must scan");
    assert_eq!(sym.text().as_deref(), Some("MICRO"));
}

#[test]
fn scans_rotated() {
    let img = framed(b"ROT", MicroEcLevel::M, 6, 24);
    for deg in [23.0f32, 90.0, -71.0, 180.0] {
        let r = transform::rotate(&img, deg.to_radians());
        let sym = scan(&r.as_frame()).unwrap_or_else(|e| panic!("rotation {deg}°: {e:?}"));
        assert_eq!(sym.text().as_deref(), Some("ROT"), "at {deg}°");
    }
}

#[test]
fn scans_degraded() {
    let img = framed(b"HARD", MicroEcLevel::M, 6, 24);
    let scaled = transform::scale(&img, 1.5);
    assert_eq!(
        scan(&scaled.as_frame()).unwrap().text().as_deref(),
        Some("HARD")
    );
    let blurred = transform::gaussian_blur(&img, 0.9);
    assert_eq!(
        scan(&blurred.as_frame()).unwrap().text().as_deref(),
        Some("HARD")
    );
    let mut rng = transform::Rng::new(11);
    let noisy = transform::add_noise(&img, 12.0, &mut rng);
    assert_eq!(
        scan(&noisy.as_frame()).unwrap().text().as_deref(),
        Some("HARD")
    );
}

#[test]
fn scans_all_versions() {
    // Payload sizes that land in M2..M4 (byte mode; M1 is numeric-only).
    for data in [&b"AB"[..], b"MICROQR", b"MICRO QR M4::"] {
        let img = framed(data, MicroEcLevel::L, 5, 20);
        let sym = scan(&img.as_frame()).unwrap_or_else(|e| panic!("{} bytes: {e:?}", data.len()));
        assert_eq!(sym.text().as_deref().map(str::as_bytes), Some(data));
    }
}
