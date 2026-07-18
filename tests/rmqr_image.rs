//! rMQR image-sampling tests: render → transform → scan through the finder
//! detector, image-space format read, and sub-dot-refined homography.

use anyd::GrayImage;
use anyd::codes::rmqr::{RmqrEcLevel, RmqrEncoder, SizeStrategy, scan};
use anyd::render::render;
use anyd::traits::Encode;
use anyd::transform;

fn framed(text: &str, strategy: SizeStrategy, scale: usize, margin: usize) -> GrayImage {
    let enc = RmqrEncoder::new();
    let sym = enc.build_text_with(text, RmqrEcLevel::M, strategy).unwrap();
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
    let img = framed("RMQR SCAN", SizeStrategy::Balanced, 6, 24);
    let sym = scan(&img.as_frame()).expect("rMQR must scan");
    assert_eq!(sym.text().as_deref(), Some("RMQR SCAN"));
}

#[test]
fn scans_rotated() {
    let img = framed("ROT", SizeStrategy::Balanced, 6, 24);
    for deg in [19.0f32, 90.0, -64.0, 180.0] {
        let r = transform::rotate(&img, deg.to_radians());
        let sym = scan(&r.as_frame()).unwrap_or_else(|e| panic!("rotation {deg}°: {e:?}"));
        assert_eq!(sym.text().as_deref(), Some("ROT"), "at {deg}°");
    }
}

#[test]
fn scans_wide_flat_symbol() {
    // MinHeight forces the flattest, widest fit — the shape that needs the far
    // sub-dot anchor (a finder-only homography drifts across the width).
    let text = "WIDE RMQR SYMBOL 0123456789 ABCDEFGH";
    let img = framed(text, SizeStrategy::MinHeight, 5, 20);
    let sym = scan(&img.as_frame()).expect("wide rMQR must scan");
    assert_eq!(sym.text().as_deref(), Some(text));
}

#[test]
fn scans_degraded() {
    let img = framed("HARD", SizeStrategy::Balanced, 6, 24);
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
    let mut rng = transform::Rng::new(5);
    let noisy = transform::add_noise(&img, 12.0, &mut rng);
    assert_eq!(
        scan(&noisy.as_frame()).unwrap().text().as_deref(),
        Some("HARD")
    );
}
