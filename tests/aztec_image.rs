//! Aztec image-sampling tests: render → transform → scan through the bullseye
//! detector and core homography.
#![cfg(all(feature = "encode", feature = "scan", feature = "aztec"))]

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

// ---------------------------------------------------------------------------
// Low-quality captures: small modules, blur, noise, low contrast, motion, tilt.
// ---------------------------------------------------------------------------

/// Text of `len` characters (longer texts force larger symbols).
fn text_of(len: usize) -> String {
    "AZTEC SIZE SWEEP 0123456789 abcdefghij "
        .chars()
        .cycle()
        .take(len)
        .collect()
}

/// A mild, realistic capture: slight optical blur and sensor noise.
fn capture(img: &GrayImage, seed: u64) -> GrayImage {
    let mut rng = transform::Rng::new(seed);
    transform::add_noise(&transform::gaussian_blur(img, 0.7), 3.0, &mut rng)
}

/// 2×2 box downscale: what a half-resolution whole-frame pass sees.
fn half(img: &GrayImage) -> GrayImage {
    let (w, h) = (img.width() / 2, img.height() / 2);
    let mut out = GrayImage::filled(w, h, 0);
    for y in 0..h {
        for x in 0..w {
            let s = img.get(2 * x, 2 * y) as u32
                + img.get(2 * x + 1, 2 * y) as u32
                + img.get(2 * x, 2 * y + 1) as u32
                + img.get(2 * x + 1, 2 * y + 1) as u32;
            out.set(x, y, (s / 4) as u8);
        }
    }
    out
}

/// Linear motion blur: the mean of `len` samples along direction `deg`.
fn motion_blur(img: &GrayImage, len: usize, deg: f32) -> GrayImage {
    let (s, c) = deg.to_radians().sin_cos();
    let (w, h) = (img.width(), img.height());
    let mut out = GrayImage::filled(w, h, 0);
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f32;
            for i in 0..len {
                let t = i as f32 - (len as f32 - 1.0) / 2.0;
                let sx = (x as f32 + c * t).clamp(0.0, (w - 1) as f32);
                let sy = (y as f32 + s * t).clamp(0.0, (h - 1) as f32);
                acc += img.sample_bilinear(sx, sy).unwrap_or(255.0);
            }
            out.set(x, y, (acc / len as f32).round() as u8);
        }
    }
    out
}

/// Illumination falling off to `floor` of full brightness across the image.
fn shade(img: &GrayImage, floor: f32) -> GrayImage {
    let (w, h) = (img.width(), img.height());
    let mut out = GrayImage::filled(w, h, 0);
    for y in 0..h {
        for x in 0..w {
            let f = floor + (1.0 - floor) * (x + y / 2) as f32 / (w + h / 2) as f32;
            out.set(x, y, (img.get(x, y) as f32 * f) as u8);
        }
    }
    out
}

#[track_caller]
fn assert_scans(img: &GrayImage, text: &str, what: &str) {
    match scan(&img.as_frame()) {
        Ok(sym) => assert_eq!(sym.text().as_deref(), Some(text), "{what}"),
        Err(e) => panic!("{what}: {e:?}"),
    }
}

#[test]
fn scans_small_modules_rotated_and_tilted() {
    // 3 px per module: the bullseye rings are too thin for their binarized corners
    // to anchor a homography; the fit has to come from the grey levels.
    let text = text_of(30);
    let img = framed(&text, 3, 30);
    for deg in [17.0f32, 45.0, -63.0] {
        let r = capture(&transform::rotate(&img, deg.to_radians()), 5);
        assert_scans(&r, &text, &format!("3 px/module at {deg}°"));
    }
    assert_scans(
        &capture(&transform::tilt_right(&img, 0.3), 6),
        &text,
        "3 px/module, 30% tilt right",
    );
    assert_scans(
        &capture(&transform::tilt_bottom(&img, 0.3), 7),
        &text,
        "3 px/module, 30% tilt bottom",
    );
    let both = transform::rotate(&transform::tilt_right(&img, 0.25), 28f32.to_radians());
    assert_scans(&capture(&both, 8), &text, "3 px/module, tilted and rotated");
}

#[test]
fn scans_soft_small_modules() {
    // Out of focus at 2.5 px per module: the rings smear into each other and the
    // centre dot all but vanishes, so no single global threshold separates them.
    let text = "AZTEC LIVE";
    let img = framed(text, 5, 40);
    for deg in [0.0f32, 17.0, 45.0, 90.0] {
        let mut rng = transform::Rng::new(11);
        let r = transform::rotate(&img, deg.to_radians());
        let soft = transform::add_noise(&transform::gaussian_blur(&r, 1.6), 8.0, &mut rng);
        assert_scans(&half(&soft), text, &format!("soft, half-res, {deg}°"));
        assert_scans(&soft, text, &format!("soft, full-res, {deg}°"));
    }
    // And a denser symbol, blurred at 3 px per module.
    let text = text_of(60);
    let mut rng = transform::Rng::new(12);
    let img = framed(&text, 3, 30);
    let soft = transform::add_noise(&transform::gaussian_blur(&img, 1.2), 4.0, &mut rng);
    assert_scans(&soft, &text, "sigma 1.2 blur at 3 px/module");
}

#[test]
fn scans_dim_and_grainy() {
    let text = "AZTEC LIVE";
    let img = framed(text, 5, 40);
    for deg in [0.0f32, 45.0] {
        let r = transform::gaussian_blur(&transform::rotate(&img, deg.to_radians()), 0.8);
        // A third of the dynamic range.
        let mut rng = transform::Rng::new(21);
        let dim = transform::add_noise(
            &transform::brightness_contrast(&r, -20.0, 0.33),
            6.0,
            &mut rng,
        );
        assert_scans(&half(&dim), text, &format!("dim, half-res, {deg}°"));
        // Heavy sensor noise on reduced contrast.
        let grain = transform::add_noise(
            &transform::brightness_contrast(&r, -30.0, 0.6),
            16.0,
            &mut rng,
        );
        assert_scans(&grain, text, &format!("grain, full-res, {deg}°"));
        assert_scans(&half(&grain), text, &format!("grain, half-res, {deg}°"));
    }
    let text = text_of(30);
    let mut rng = transform::Rng::new(22);
    let img = transform::gaussian_blur(&framed(&text, 3, 30), 0.8);
    let dim = transform::add_noise(
        &transform::brightness_contrast(&img, -20.0, 0.33),
        6.0,
        &mut rng,
    );
    assert_scans(&dim, &text, "dim at 3 px/module");
}

#[test]
fn scans_motion_blur() {
    let text = "AZTEC LIVE";
    let img = framed(text, 5, 40);
    for deg in [0.0f32, 17.0, 45.0] {
        let mut rng = transform::Rng::new(31);
        let r = transform::gaussian_blur(&transform::rotate(&img, deg.to_radians()), 0.6);
        let moved = transform::add_noise(&motion_blur(&r, 5, 30.0), 4.0, &mut rng);
        assert_scans(&moved, text, &format!("5 px motion blur, {deg}°"));
        assert_scans(
            &half(&moved),
            text,
            &format!("5 px motion blur, half-res, {deg}°"),
        );
    }
}

#[test]
fn scans_tilted_half_resolution() {
    let text = "AZTEC LIVE";
    let img = transform::tilt_right(&framed(text, 5, 40), 0.3);
    for deg in [0.0f32, 45.0, 90.0] {
        let r = capture(&transform::rotate(&img, deg.to_radians()), 41);
        assert_scans(&r, text, &format!("30% tilt, {deg}°"));
        assert_scans(&half(&r), text, &format!("30% tilt, half-res, {deg}°"));
    }
}

#[test]
fn scans_large_symbols_through_perspective() {
    // The bullseye spans a small fraction of a large symbol: a map anchored on it
    // alone drifts by more than a module at the rim, so the fit has to be carried
    // outward over the data modules.
    for (len, scale) in [(300usize, 4usize), (700, 3), (1500, 4)] {
        let text = text_of(len);
        let img = framed(&text, scale, 30);
        assert_scans(&capture(&img, 51), &text, &format!("{len} chars, upright"));
        assert_scans(
            &capture(&transform::rotate(&img, 45f32.to_radians()), 52),
            &text,
            &format!("{len} chars at 45°"),
        );
        assert_scans(
            &capture(&transform::tilt_right(&img, 0.3), 53),
            &text,
            &format!("{len} chars, 30% tilt"),
        );
        let both = transform::rotate(&transform::tilt_right(&img, 0.25), 28f32.to_radians());
        assert_scans(
            &capture(&both, 54),
            &text,
            &format!("{len} chars, tilted and rotated"),
        );
    }
}

#[test]
fn scans_large_symbol_in_uneven_light() {
    // An 8:1 illumination falloff across a 75-module symbol: no single threshold
    // (not even the bullseye's own) splits dark from light at both ends.
    let text = text_of(700);
    let img = framed(&text, 4, 30);
    let mut rng = transform::Rng::new(61);
    let lit = transform::add_noise(
        &shade(&transform::gaussian_blur(&img, 1.0), 0.12),
        6.0,
        &mut rng,
    );
    assert_scans(&lit, &text, "8:1 lighting falloff");
}

#[test]
fn bullseye_lookalikes_do_not_read() {
    let (w, h) = (480usize, 320usize);
    let paint = |f: &dyn Fn(usize, usize) -> bool| {
        let mut img = GrayImage::filled(w, h, 230);
        for y in 0..h {
            for x in 0..w {
                if f(x, y) {
                    img.set(x, y, 30);
                }
            }
        }
        capture(&img, 71)
    };
    let cases: [(&str, GrayImage); 5] = [
        ("checkerboard", paint(&|x, y| (x / 6 + y / 6) % 2 == 0)),
        ("stripes", paint(&|x, _| (x / 4) % 2 == 0)),
        (
            "concentric circles",
            paint(&|x, y| {
                let (dx, dy) = (x as f32 - 240.0, y as f32 - 160.0);
                ((dx * dx + dy * dy).sqrt() / 5.0) as i32 % 2 == 0
            }),
        ),
        (
            // Bare bullseyes: perfect rings, but no orientation marks or mode ring.
            "sheet of bare bullseyes",
            paint(&|x, y| {
                let (cx, cy) = ((x % 64) as i32 - 32, (y % 64) as i32 - 32);
                let r = cx.abs().max(cy.abs());
                r < 18 && (r / 4) % 2 == 0
            }),
        ),
        ("bullseyes in random modules", {
            // Genuine cores (bullseye, orientation marks, mode message) whose data
            // region is noise: everything checks out except the payload's own RS.
            let enc = AztecEncoder::new();
            let m = enc.encode(&enc.build_text("AZTEC LIVE").unwrap()).unwrap();
            let sprite = render(&m, 4);
            let size = sprite.width() / 4;
            let c = (size / 2) as i32;
            let mut rng = transform::Rng::new(72);
            let mut img = GrayImage::filled(w, h, 230);
            for ty in 0..4 {
                for tx in 0..6 {
                    for my in 0..size {
                        for mx in 0..size {
                            let core = (mx as i32 - c).abs().max((my as i32 - c).abs()) <= 5;
                            let dark = if core {
                                sprite.get(mx * 4 + 1, my * 4 + 1) < 128
                            } else {
                                rng.next_f32() < 0.5
                            };
                            if dark {
                                for p in 0..16 {
                                    img.set(
                                        8 + tx * 78 + mx * 4 + p % 4,
                                        8 + ty * 76 + my * 4 + p / 4,
                                        30,
                                    );
                                }
                            }
                        }
                    }
                }
            }
            capture(&img, 73)
        }),
    ];
    for (name, img) in &cases {
        if let Ok(sym) = scan(&img.as_frame()) {
            panic!("{name} read as {:?} {:?}", sym.symbology, sym.text());
        }
    }
}
