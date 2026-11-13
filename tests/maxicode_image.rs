//! MaxiCode image-sampling tests: render the hexagonal symbol → transform → scan
//! through the bullseye finder and the lattice fit.
//!
//! The envelope asserted here, on `render_hex` renders with a one-module quiet zone
//! set in a light margin:
//!
//! - every mode (2–6) at every rotation tried, in steps that cover all six 60°
//!   sectors and the angles between;
//! - module pitches from 4 px to 24 px;
//! - Gaussian blur to σ = 2 px, sensor noise to σ = 30, and a milder dose of both
//!   at once, at 8 px per module; a quarter of the contrast; a shadow across the
//!   symbol that no single threshold separates; ink spread that fuses neighbouring
//!   modules and runs the bullseye into them;
//! - perspective tilt to a 20 % foreshortening of one side, at any rotation;
//! - nothing read from frames without a MaxiCode — blank, noise, a bare bullseye,
//!   other 2D symbologies.
#![cfg(all(feature = "encode", feature = "scan", feature = "maxicode"))]

use anyd::codes::maxicode::{MaxiCodeEncoder, MaxiCodeMeta, render_hex, scan};
use anyd::output::Encoding;
use anyd::symbol::SymbolMeta;
use anyd::traits::Encode;
use anyd::{GrayImage, Symbol, transform};

const TEXT: &str = "MaxiCode image sampler 0123456789";

/// A symbol of each mode: 2 and 3 are Structured Carrier Messages.
fn symbol(mode: u8) -> Symbol {
    let enc = MaxiCodeEncoder::new();
    match mode {
        2 => enc.build_structured(2, "152382802", 840, 1, TEXT.as_bytes()),
        3 => enc.build_structured(3, "B1050", 56, 999, TEXT.as_bytes()),
        _ => enc.build_mode(TEXT.as_bytes(), mode),
    }
    .unwrap()
}

/// Render `sym` at `module_px` pixels per module in a light `margin`.
fn framed(sym: &Symbol, module_px: f32, margin: usize) -> GrayImage {
    let Encoding::Matrix(m) = MaxiCodeEncoder::new().encode(sym).unwrap() else {
        panic!("expected a matrix");
    };
    let img = render_hex(&m, module_px);
    let mut canvas = GrayImage::filled(img.width() + 2 * margin, img.height() + 2 * margin, 255);
    for y in 0..img.height() {
        for x in 0..img.width() {
            canvas.set(x + margin, y + margin, img.get(x, y));
        }
    }
    canvas
}

fn meta(sym: &Symbol) -> &MaxiCodeMeta {
    match &sym.meta {
        SymbolMeta::MaxiCode(m) => m,
        _ => panic!("expected MaxiCode meta"),
    }
}

/// Scan `img` and require exactly `want`: payload, mode, carrier, codewords.
fn assert_reads(img: &GrayImage, want: &Symbol, what: &str) {
    let got = scan(&img.as_frame()).unwrap_or_else(|e| panic!("{what}: {e:?}"));
    assert_eq!(got.payload_bytes(), want.payload_bytes(), "{what}");
    assert_eq!(meta(&got), meta(want), "{what}");
    assert!(got.location.is_some(), "{what}: no location");
}

#[test]
fn scans_every_mode_upright() {
    for mode in 2..=6 {
        let sym = symbol(mode);
        assert_reads(&framed(&sym, 8.0, 20), &sym, &format!("mode {mode}"));
    }
}

/// The sampled grid is the grid that was drawn, module for module.
#[test]
fn sampled_grid_reencodes_identically() {
    let enc = MaxiCodeEncoder::new();
    for mode in 2..=6 {
        let sym = symbol(mode);
        let got = scan(&framed(&sym, 6.0, 12).as_frame()).unwrap();
        assert_eq!(enc.encode(&got), enc.encode(&sym), "mode {mode}");
    }
}

#[test]
fn scans_every_mode_at_any_rotation() {
    for mode in 2..=6 {
        let sym = symbol(mode);
        let img = framed(&sym, 8.0, 20);
        for deg in [
            7.0f32, 30.0, 45.0, 60.0, 90.0, 118.0, 150.0, 180.0, 203.0, 240.0, 270.0, 299.0, 331.0,
        ] {
            let r = transform::rotate(&img, deg.to_radians());
            assert_reads(&r, &sym, &format!("mode {mode} at {deg}°"));
        }
    }
}

#[test]
fn reports_where_the_symbol_is() {
    let sym = symbol(4);
    let (px, margin) = (10.0f32, 30usize);
    let got = scan(&framed(&sym, px, margin).as_frame()).unwrap();
    let loc = got.location.unwrap();
    // The module area: one module of quiet zone inside the margin, 30 modules wide.
    let left = margin as f32 + px;
    let (lo, hi) = loc.outline.bounds();
    assert!((lo.x - left).abs() < 1.5, "left {}", lo.x);
    assert!((hi.x - (left + 30.0 * px)).abs() < 1.5, "right {}", hi.x);
    assert!((lo.y - left).abs() < 1.5, "top {}", lo.y);
    let height = (32.0 * 0.866 + 1.1547) * px;
    assert!((hi.y - (left + height)).abs() < 1.5, "bottom {}", hi.y);
    assert!((loc.module_size.unwrap() - px).abs() < 0.1);
    assert!(loc.rotation.unwrap().abs() < 0.01);

    // Turned a quarter: the rows run down the image.
    let r = transform::rotate(&framed(&sym, px, margin), 90f32.to_radians());
    let loc = scan(&r.as_frame()).unwrap().location.unwrap();
    assert!((loc.rotation.unwrap() - core::f32::consts::FRAC_PI_2).abs() < 0.01);
}

#[test]
fn scans_across_module_sizes() {
    let sym = symbol(4);
    for px in [4.0f32, 4.6, 5.3, 6.5, 9.0, 13.7, 24.0] {
        assert_reads(&framed(&sym, px, 16), &sym, &format!("{px} px/module"));
        let r = transform::rotate(&framed(&sym, px, 16), 23f32.to_radians());
        assert_reads(&r, &sym, &format!("{px} px/module at 23°"));
    }
    // Resampled rather than rendered at size.
    let img = framed(&sym, 8.0, 20);
    for factor in [0.6f32, 0.75, 1.3, 2.0] {
        let scaled = transform::scale(&img, factor);
        assert_reads(&scaled, &sym, &format!("scaled {factor}×"));
    }
}

#[test]
fn scans_blurred_and_noisy() {
    for mode in [2u8, 5] {
        let sym = symbol(mode);
        let img = framed(&sym, 8.0, 20);
        for sigma in [0.7f32, 1.3, 2.0] {
            let blurred = transform::gaussian_blur(&img, sigma);
            assert_reads(&blurred, &sym, &format!("mode {mode} blur σ={sigma}"));
        }
        for (seed, sigma) in [(1u64, 8.0f32), (2, 18.0), (3, 30.0)] {
            let mut rng = transform::Rng::new(seed);
            let noisy = transform::add_noise(&img, sigma, &mut rng);
            assert_reads(&noisy, &sym, &format!("mode {mode} noise σ={sigma}"));
        }
        // A realistic capture: turned, soft and grainy at once.
        for (seed, deg) in [(11u64, 14.0f32), (12, 77.0), (13, 195.0)] {
            let mut rng = transform::Rng::new(seed);
            let r = transform::rotate(&img, deg.to_radians());
            let shot = transform::add_noise(&transform::gaussian_blur(&r, 1.0), 10.0, &mut rng);
            assert_reads(&shot, &sym, &format!("mode {mode} capture at {deg}°"));
        }
    }
}

#[test]
fn scans_dim_and_unevenly_lit() {
    let sym = symbol(4);
    let img = framed(&sym, 8.0, 20);
    // Low contrast, dark and bright.
    for (brightness, contrast) in [(-50.0f32, 0.4f32), (40.0, 0.5), (0.0, 0.25)] {
        let dim = transform::brightness_contrast(&img, brightness, contrast);
        assert_reads(
            &dim,
            &sym,
            &format!("brightness {brightness} contrast {contrast}"),
        );
    }
    // A shadow falling across the symbol: the paper on one side is darker than the
    // ink on the other is light, so no single threshold separates the two.
    let (w, h) = (img.width(), img.height());
    let mut lit = GrayImage::filled(w, h, 0);
    for y in 0..h {
        for x in 0..w {
            let gain = 0.3 + 0.7 * x as f32 / w as f32;
            let v = 40.0 + gain * (f32::from(img.get(x, y)) * 0.8);
            lit.set(x, y, v as u8);
        }
    }
    assert_reads(&lit, &sym, "light gradient");
    let r = transform::rotate(&lit, 40f32.to_radians());
    assert_reads(&r, &sym, "light gradient at 40°");
}

/// Dark grown by one pixel all round, `rounds` times: ink spread.
fn spread(img: &GrayImage, rounds: usize) -> GrayImage {
    let mut cur = img.clone();
    for _ in 0..rounds {
        let mut next = cur.clone();
        for y in 1..cur.height() - 1 {
            for x in 1..cur.width() - 1 {
                let mut v = cur.get(x, y);
                for (dx, dy) in [(0, 1), (2, 1), (1, 0), (1, 2)] {
                    v = v.min(cur.get(x + dx - 1, y + dy - 1));
                }
                next.set(x, y, v);
            }
        }
        cur = next;
    }
    cur
}

/// Heavy print: the hexagons run into one another and into the bullseye's rim.
#[test]
fn scans_with_ink_spread() {
    for mode in [2u8, 4] {
        let sym = symbol(mode);
        // A dark ring half as wide again as it should be, a light one a third
        // narrower.
        for (px, rounds) in [(8.0f32, 1usize), (12.0, 2)] {
            let fat = spread(&framed(&sym, px, 20), rounds);
            let what = format!("mode {mode} at {px} px/module, spread {rounds} px");
            assert_reads(&fat, &sym, &what);
            let r = transform::rotate(&fat, 65f32.to_radians());
            assert_reads(&r, &sym, &format!("{what}, at 65°"));
        }
    }
}

#[test]
fn scans_tilted() {
    for mode in [3u8, 4] {
        let sym = symbol(mode);
        let img = framed(&sym, 8.0, 20);
        for amount in [0.1f32, 0.2] {
            assert_reads(
                &transform::tilt_right(&img, amount),
                &sym,
                &format!("mode {mode} tilt right {amount}"),
            );
            assert_reads(
                &transform::tilt_bottom(&img, amount),
                &sym,
                &format!("mode {mode} tilt bottom {amount}"),
            );
            for deg in [35.0f32, 100.0, 222.0, 290.0] {
                let r = transform::rotate(&img, deg.to_radians());
                assert_reads(
                    &transform::tilt_right(&r, amount),
                    &sym,
                    &format!("mode {mode} at {deg}° tilt right {amount}"),
                );
                assert_reads(
                    &transform::tilt_bottom(&r, amount),
                    &sym,
                    &format!("mode {mode} at {deg}° tilt bottom {amount}"),
                );
            }
        }
    }
}

/// The whole-frame pipeline returns the MaxiCode, and only it.
#[test]
fn pipeline_reads_it() {
    let sym = symbol(5);
    let img = transform::rotate(&framed(&sym, 7.0, 24), 52f32.to_radians());
    let found = anyd::pipeline::scan_2d(&img.as_frame());
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].symbology, anyd::Symbology::MaxiCode);
    assert_eq!(found[0].payload_bytes(), sym.payload_bytes());
}

/// Concentric rings as `render_hex` draws the bullseye, alone on a light frame.
fn bare_bullseye(size: usize, module_px: f32) -> GrayImage {
    let mut img = GrayImage::filled(size, size, 255);
    let c = size as f32 / 2.0;
    let edges = [0.5774f32, 1.3619, 2.1464, 2.9310, 3.7155, 4.5];
    for y in 0..size {
        for x in 0..size {
            let r = ((x as f32 + 0.5 - c).hypot(y as f32 + 0.5 - c)) / module_px;
            let ring = edges.iter().filter(|&&e| r >= e).count();
            if ring % 2 == 1 {
                img.set(x, y, 0);
            }
        }
    }
    img
}

#[test]
fn reads_nothing_where_there_is_no_maxicode() {
    // Blank, and flat grey.
    for fill in [255u8, 128, 0] {
        let img = GrayImage::filled(320, 240, fill);
        assert!(scan(&img.as_frame()).is_err(), "fill {fill}");
    }
    // Too small to hold a symbol.
    assert!(scan(&GrayImage::filled(8, 8, 255).as_frame()).is_err());
    // Noise, coarse and fine.
    for (seed, sigma) in [(5u64, 30.0f32), (6, 90.0)] {
        let mut rng = transform::Rng::new(seed);
        let img = transform::add_noise(&GrayImage::filled(400, 300, 128), sigma, &mut rng);
        assert!(scan(&img.as_frame()).is_err(), "noise σ={sigma}");
    }
    // A bullseye with no modules around it, alone and on noise.
    let eye = bare_bullseye(300, 9.0);
    assert!(scan(&eye.as_frame()).is_err(), "bare bullseye");
    let mut rng = transform::Rng::new(9);
    let noisy = transform::add_noise(&eye, 40.0, &mut rng);
    assert!(scan(&noisy.as_frame()).is_err(), "bare bullseye on noise");
    // Stripes: every row a run of equal bars.
    let mut stripes = GrayImage::filled(320, 240, 255);
    for y in 0..240 {
        for x in 0..320 {
            if (x / 5) % 2 == 0 {
                stripes.set(x, y, 0);
            }
        }
    }
    assert!(scan(&stripes.as_frame()).is_err(), "stripes");
    // The square-module rendering of a MaxiCode grid is not a MaxiCode.
    let enc = MaxiCodeEncoder::new();
    let squares = anyd::render::render(&enc.encode(&symbol(4)).unwrap(), 8);
    assert!(scan(&squares.as_frame()).is_err(), "square modules");
}

/// Other bullseye and finder symbologies are not mistaken for a MaxiCode.
#[cfg(all(feature = "aztec", feature = "qr"))]
#[test]
fn reads_nothing_from_other_symbologies() {
    use anyd::codes::aztec::AztecEncoder;
    use anyd::codes::qr::QrEncoder;
    let aztec = AztecEncoder::new();
    for text in [
        "AZTEC",
        "A longer Aztec message that needs a full-range symbol 0123456789",
    ] {
        let img = anyd::render::render(&aztec.encode(&aztec.build_text(text).unwrap()).unwrap(), 7);
        for deg in [0.0f32, 45.0] {
            let r = transform::rotate(&img, deg.to_radians());
            assert!(scan(&r.as_frame()).is_err(), "Aztec {text:?} at {deg}°");
        }
    }
    let qr = QrEncoder::new();
    let img = anyd::render::render(
        &qr.encode(
            &qr.build_text("QR CODE 0123456789", anyd::codes::qr::EcLevel::M)
                .unwrap(),
        )
        .unwrap(),
        7,
    );
    assert!(scan(&img.as_frame()).is_err(), "QR");
}

/// On any pixels — degenerate sizes, saturated frames, symbols clipped by the crop
/// edge anywhere, the bullseye included — the sampler returns, and what it reads is
/// what was drawn.
#[test]
fn never_panics_or_misreads_on_arbitrary_crops() {
    let mut seed = 0x4D41_5849_C0DE_2026u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for (w, h) in [
        (1, 1),
        (31, 400),
        (400, 31),
        (32, 32),
        (33, 1000),
        (1000, 33),
    ] {
        for fill in [0u8, 255] {
            assert!(scan(&GrayImage::filled(w, h, fill).as_frame()).is_err());
        }
        let mut img = GrayImage::filled(w, h, 0);
        for y in 0..h {
            for x in 0..w {
                img.set(x, y, next() as u8);
            }
        }
        assert!(scan(&img.as_frame()).is_err(), "noise {w}×{h}");
    }
    let sym = symbol(4);
    let mut rng = transform::Rng::new(77);
    let shots = [
        framed(&sym, 6.0, 30),
        transform::rotate(&framed(&sym, 9.0, 10), 33f32.to_radians()),
        transform::add_noise(&framed(&sym, 5.0, 40), 25.0, &mut rng),
    ];
    let mut read = 0;
    for img in &shots {
        for _ in 0..60 {
            let cw = 20 + next() as usize % (img.width() - 20);
            let ch = 20 + next() as usize % (img.height() - 20);
            let x0 = next() as usize % (img.width() - cw + 1);
            let y0 = next() as usize % (img.height() - ch + 1);
            let mut crop = GrayImage::filled(cw, ch, 0);
            for y in 0..ch {
                for x in 0..cw {
                    crop.set(x, y, img.get(x0 + x, y0 + y));
                }
            }
            if let Ok(got) = scan(&crop.as_frame()) {
                assert_eq!(got.payload_bytes(), sym.payload_bytes());
                read += 1;
            }
        }
    }
    // Reed–Solomon carries a symbol with a corner or an edge cut off.
    assert!(read > 0);
}
