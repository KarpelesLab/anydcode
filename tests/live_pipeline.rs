//! End-to-end tests of the **live** path: locate on a reduced frame → crop the full
//! frame → decode the crop — what a camera front-end does on every frame.
//!
//! The per-symbology image tests hand each sampler a pristine render that fills the
//! frame. These instead plant a code in a *scene* — off-centre, rotated, on a dark
//! surface or amid print, unevenly lit, blurred and noisy — and require the pipeline to
//! find and read it, and to read nothing that is not there. `examples/liveeval.rs` is
//! the exploratory big brother of this file; the cases here pin what it measured.
#![cfg(all(feature = "encode", feature = "scan", feature = "all-codes"))]

use anyd::codes::code39::Code39Encoder;
use anyd::codes::code128::Code128Encoder;
use anyd::codes::ean::EanEncoder;
use anyd::codes::itf::ItfEncoder;
use anyd::codes::pdf417::{self, Pdf417Encoder};
use anyd::detect::{LocateOptions, locate};
use anyd::render::render;
use anyd::traits::Encode;
use anyd::transform::{Rng, add_noise, gaussian_blur};
use anyd::{Dimension, GrayImage, Symbol};

const W: usize = 1280;
const H: usize = 720;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Bg {
    /// Plain light page.
    Page,
    /// White label on a dark surface.
    Label,
    /// Light page covered in text-like print.
    Clutter,
}

/// Paste `sprite` rotated by `deg` about `(cx, cy)`.
fn paste(canvas: &mut GrayImage, sprite: &GrayImage, cx: f32, cy: f32, deg: f32) {
    let (s, c) = deg.to_radians().sin_cos();
    let (sw, sh) = (sprite.width() as f32, sprite.height() as f32);
    let reach = sw.hypot(sh) / 2.0 + 1.0;
    let y0 = (cy - reach).max(0.0) as usize;
    let y1 = ((cy + reach) as usize).min(canvas.height());
    let x0 = (cx - reach).max(0.0) as usize;
    let x1 = ((cx + reach) as usize).min(canvas.width());
    for y in y0..y1 {
        for x in x0..x1 {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let sx = c * dx + s * dy + sw / 2.0 - 0.5;
            let sy = -s * dx + c * dy + sh / 2.0 - 0.5;
            if sx < -0.5 || sy < -0.5 || sx > sw - 0.5 || sy > sh - 0.5 {
                continue;
            }
            let v = sprite
                .sample_bilinear(sx.clamp(0.0, sw - 1.0), sy.clamp(0.0, sh - 1.0))
                .unwrap_or(255.0);
            canvas.set(x, y, v.round() as u8);
        }
    }
}

/// Rows of glyph-sized stroke clusters: printed text as a locator sees it.
fn clutter(canvas: &mut GrayImage, rng: &mut Rng) {
    let mut y = 14;
    while y + 16 < canvas.height() {
        let mut x = 12;
        while x + 12 < canvas.width() {
            if rng.next_f32() < 0.16 {
                x += 14;
                continue;
            }
            let gw = 3 + (rng.next_f32() * 6.0) as usize;
            let gh = 8 + (rng.next_f32() * 6.0) as usize;
            for yy in y + (14 - gh)..y + 14 {
                for xx in x..x + gw {
                    if (xx - x) % 3 != 1 || (yy - y) % 5 == 0 {
                        canvas.set(xx, yy, 40);
                    }
                }
            }
            x += gw + 3;
        }
        y += 24;
    }
}

/// A camera-like frame with `sprite` planted in it.
fn scene(sprite: &GrayImage, bg: Bg, deg: f32, gradient: bool, seed: u64) -> GrayImage {
    let mut rng = Rng::new(seed);
    let mut canvas = GrayImage::filled(W, H, if bg == Bg::Label { 55 } else { 232 });
    if bg == Bg::Clutter {
        clutter(&mut canvas, &mut rng);
    }
    let margin = if bg == Bg::Label { 10 } else { 0 };
    let mut label = GrayImage::filled(
        sprite.width() + 2 * margin,
        sprite.height() + 2 * margin,
        255,
    );
    for y in 0..sprite.height() {
        for x in 0..sprite.width() {
            label.set(x + margin, y + margin, sprite.get(x, y));
        }
    }
    let cx = W as f32 * (0.42 + 0.16 * rng.next_f32());
    let cy = H as f32 * (0.44 + 0.12 * rng.next_f32());
    paste(&mut canvas, &label, cx, cy, deg);
    if gradient {
        for y in 0..H {
            for x in 0..W {
                let f = 0.45 + 0.55 * (x as f32 / W as f32);
                canvas.set(x, y, (canvas.get(x, y) as f32 * f) as u8);
            }
        }
    }
    add_noise(&gaussian_blur(&canvas, 0.8), 4.0, &mut rng)
}

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

fn crop(img: &GrayImage, b: [f32; 4]) -> Option<GrayImage> {
    let x0 = b[0].max(0.0) as usize;
    let y0 = b[1].max(0.0) as usize;
    let x1 = (b[2].ceil() as usize).min(img.width());
    let y1 = (b[3].ceil() as usize).min(img.height());
    if x1 < x0 + 12 || y1 < y0 + 12 {
        return None;
    }
    let mut out = GrayImage::filled(x1 - x0, y1 - y0, 0);
    for y in y0..y1 {
        for x in x0..x1 {
            out.set(x - x0, y - y0, img.get(x, y));
        }
    }
    Some(out)
}

/// What a live front-end reads off one frame through the locate → crop → decode path:
/// locate on the half-resolution grab, crop each candidate (+16 px) out of the full
/// frame, decode linear crops along their axis and matrix crops with everything.
fn live_reads(image: &GrayImage) -> Vec<Symbol> {
    let small = half(image);
    let opts = LocateOptions {
        downscale: 1,
        ..Default::default()
    };
    let mut out = Vec::new();
    for c in locate(&small.as_frame(), &opts) {
        let cs = c.location.outline.corners;
        let b = [
            cs.iter().map(|p| p.x).fold(f32::MAX, f32::min) * 2.0 - 16.0,
            cs.iter().map(|p| p.y).fold(f32::MAX, f32::min) * 2.0 - 16.0,
            cs.iter().map(|p| p.x).fold(f32::MIN, f32::max) * 2.0 + 16.0,
            cs.iter().map(|p| p.y).fold(f32::MIN, f32::max) * 2.0 + 16.0,
        ];
        let Some(cropped) = crop(image, b) else {
            continue;
        };
        let linear = c.symbology.map(|s| s.dimension()) != Some(Dimension::Matrix);
        out.extend(if linear {
            anyd::pipeline::scan_linear_at(
                &cropped.as_frame(),
                c.location.rotation.unwrap_or(f32::NAN),
            )
        } else {
            anyd::pipeline::scan_all(&cropped.as_frame())
        });
    }
    out
}

fn texts(syms: &[Symbol]) -> Vec<String> {
    syms.iter().map(|s| s.text().unwrap_or_default()).collect()
}

struct Linear {
    name: &'static str,
    text: &'static str,
    sprite: GrayImage,
}

fn linear_codes(scale: usize) -> Vec<Linear> {
    let ean = EanEncoder::new();
    let c128 = Code128Encoder::new();
    let c39 = Code39Encoder::new();
    let itf = ItfEncoder::new();
    vec![
        Linear {
            name: "ean13",
            text: "4901085663356",
            sprite: render(
                &ean.encode(&ean.build_ean13("4901085663356").unwrap())
                    .unwrap(),
                scale,
            ),
        },
        Linear {
            name: "code128",
            text: "ANYD-128",
            sprite: render(
                &c128.encode(&c128.build_text("ANYD-128").unwrap()).unwrap(),
                scale,
            ),
        },
        Linear {
            name: "code39",
            text: "ANYD39",
            sprite: render(
                &c39.encode(&c39.build(b"ANYD39", false, false).unwrap())
                    .unwrap(),
                scale,
            ),
        },
        Linear {
            name: "itf",
            text: "12345678901231",
            sprite: render(
                &itf.encode(&itf.build(b"1234567890123", true).unwrap())
                    .unwrap(),
                scale,
            ),
        },
    ]
}

/// The headline: 1D codes are found and read at any rotation, on any background, under
/// uneven light — and nothing else is read. Before the live-path rework this grid read
/// under half its codes and produced dozens of wrong values.
#[test]
fn linear_codes_read_at_any_rotation_and_background() {
    let mut seed = 100u64;
    let (mut total, mut read) = (0u32, 0u32);
    let mut misses = Vec::new();
    for code in linear_codes(3) {
        for bg in [Bg::Page, Bg::Label, Bg::Clutter] {
            for deg in [0.0f32, 17.0, 45.0, 90.0] {
                seed += 1;
                let gradient = seed.is_multiple_of(2);
                let image = scene(&code.sprite, bg, deg, gradient, seed);
                let got = texts(&live_reads(&image));
                for t in &got {
                    assert_eq!(
                        t, code.text,
                        "{} {bg:?} rot={deg}: read a value that is not in the frame",
                        code.name
                    );
                }
                total += 1;
                if got.iter().any(|t| t == code.text) {
                    read += 1;
                } else {
                    misses.push(format!("{} {bg:?} rot={deg}", code.name));
                }
            }
        }
    }
    assert!(
        read * 100 >= total * 90,
        "live 1D read rate {read}/{total}; missed: {misses:?}"
    );
}

/// A scanline that leaves an EAN-13 half way reads its start guard and six left digits
/// — structurally a UPC-E, with a check digit that passes one time in ten. The width
/// reader must insist on the quiet zone a real UPC-E ends with.
#[test]
fn half_an_ean13_is_not_a_upce() {
    let ean = EanEncoder::new();
    // Every payload whose left half happens to satisfy the UPC-E check would do; this is
    // the one from the field capture that exposed it.
    let full = render(
        &ean.encode(&ean.build_ean13("4901085663356").unwrap())
            .unwrap(),
        4,
    );
    // Keep the left quiet zone, guard, six digits, the centre guard and a little more.
    let cut = full.width() * 58 / 100;
    let mut left = GrayImage::filled(cut + 40, full.height(), 255);
    for y in 0..full.height() {
        for x in 0..cut {
            left.set(x, y, full.get(x, y));
        }
    }
    // The cut leaves a partial right half butting against white: nothing here is a
    // complete symbol.
    let got = anyd::pipeline::scan_1d(&left.as_frame());
    assert!(
        got.is_empty(),
        "read {:?} from a truncated EAN-13",
        got.iter()
            .map(|s| (s.symbology, s.text()))
            .collect::<Vec<_>>()
    );
}

/// The flip side of [`half_an_ean13_is_not_a_upce`]: the stricter UPC-E acceptance must
/// not cost genuine UPC-E symbols, upright or turned.
#[test]
fn genuine_upce_still_reads() {
    let ean = EanEncoder::new();
    let sym = ean.build_upce("01234565").unwrap();
    let want = sym.text().unwrap();
    let sprite = render(&ean.encode(&sym).unwrap(), 3);
    for (deg, seed) in [(0.0f32, 21u64), (30.0, 22), (90.0, 23)] {
        let image = scene(&sprite, Bg::Label, deg, seed.is_multiple_of(2), seed);
        let got = live_reads(&image);
        assert!(
            got.iter()
                .any(|s| s.symbology == anyd::Symbology::UpcE && s.text().as_deref() == Some(&want)),
            "rot={deg}: got {:?}",
            got.iter().map(|s| (s.symbology, s.text())).collect::<Vec<_>>()
        );
    }
}

/// An ITF has no check character and no fixed length, so a scanline that leaves its bars
/// early still finds a start, some digit pairs and something stop-like. Steeply slanted
/// lines must not surface those fragments as readings.
#[test]
fn slanted_itf_yields_the_whole_symbol_or_nothing() {
    let itf = ItfEncoder::new();
    let sprite = render(
        &itf.encode(&itf.build(b"1234567890123", true).unwrap())
            .unwrap(),
        3,
    );
    for deg in [8.0f32, 12.0, 17.0, 24.0] {
        let mut canvas = GrayImage::filled(sprite.width() + 200, sprite.height() + 260, 255);
        let (cx, cy) = (canvas.width() as f32 / 2.0, canvas.height() as f32 / 2.0);
        paste(&mut canvas, &sprite, cx, cy, deg);
        // The plain horizontal-first search, with no axis hint.
        for t in texts(&anyd::pipeline::scan_1d(&canvas.as_frame())) {
            assert_eq!(t, "12345678901231", "rot={deg}: fragment read as a symbol");
        }
    }
}

/// Dense print with no code in it reads as nothing, through every decoder.
#[test]
fn text_reads_as_nothing() {
    for seed in 1..=4u64 {
        let mut rng = Rng::new(seed);
        let mut canvas = GrayImage::filled(640, 360, 232);
        clutter(&mut canvas, &mut rng);
        let image = add_noise(&gaussian_blur(&canvas, 0.7), 3.0, &mut rng);
        let got = anyd::pipeline::scan_all(&image.as_frame());
        assert!(
            got.is_empty(),
            "seed {seed}: read {:?} from plain text",
            got.iter()
                .map(|s| (s.symbology, s.text()))
                .collect::<Vec<_>>()
        );
    }
}

/// A PDF417 is rows of bars, so the locator reports it as a *linear* region; the
/// linear-region decode must therefore try the stacked samplers, derotating by the
/// locator's axis when the symbol is turned beyond their own tolerance.
#[test]
fn pdf417_reads_from_a_linear_region_when_rotated() {
    let enc = Pdf417Encoder::new();
    let sym = enc
        .build_text("PDF417 LIVE", pdf417::EcLevel::new(2).unwrap())
        .unwrap();
    let sprite = render(&enc.encode(&sym).unwrap(), 4);
    for (deg, seed) in [(0.0f32, 7u64), (30.0, 8), (90.0, 9)] {
        let image = scene(&sprite, Bg::Label, deg, false, seed);
        let got = texts(&live_reads(&image));
        assert!(
            got.iter().any(|t| t == "PDF417 LIVE"),
            "rot={deg}: got {got:?}"
        );
    }
}

/// The locator reports a rotated barcode as linear, with its reading axis, and keeps
/// finding it in the dim half of an unevenly lit frame.
#[test]
fn locator_reports_the_reading_axis() {
    let code = &linear_codes(3)[0];
    for deg in [-60.0f32, -20.0, 0.0, 33.0, 75.0] {
        let image = scene(&code.sprite, Bg::Label, deg, true, 40 + deg.abs() as u64);
        let small = half(&image);
        let opts = LocateOptions {
            downscale: 1,
            ..Default::default()
        };
        let cands = locate(&small.as_frame(), &opts);
        let axis = cands
            .iter()
            .filter(|c| c.symbology.map(|s| s.dimension()) == Some(Dimension::Linear))
            .filter_map(|c| c.location.rotation)
            .map(f32::to_degrees)
            .next()
            .unwrap_or_else(|| panic!("rot={deg}: no linear candidate among {}", cands.len()));
        let diff = (axis - deg).rem_euclid(180.0);
        let diff = diff.min(180.0 - diff);
        assert!(diff <= 7.0, "rot={deg}: reported axis {axis:.1}°");
    }
}

/// Nothing on the live path may panic, whatever the pixels: a panic in a wasm worker
/// takes the decoder down for the session.
#[test]
fn degenerate_frames_do_not_panic() {
    let mut rng = Rng::new(0xD1CE);
    let mut frames: Vec<GrayImage> = Vec::new();
    for (w, h) in [
        (1, 1),
        (2, 9),
        (9, 2),
        (7, 7),
        (8, 8),
        (16, 3),
        (33, 17),
        (64, 64),
        (97, 61),
        (160, 120),
    ] {
        // Flat, saturated, noise, stripes at the pixel limit, a hard diagonal.
        frames.push(GrayImage::filled(w, h, 0));
        frames.push(GrayImage::filled(w, h, 255));
        let mut noise = GrayImage::filled(w, h, 0);
        let mut stripes = GrayImage::filled(w, h, 255);
        let mut diag = GrayImage::filled(w, h, 255);
        for y in 0..h {
            for x in 0..w {
                noise.set(x, y, (rng.next_u64() & 0xFF) as u8);
                if x % 2 == 0 {
                    stripes.set(x, y, 0);
                }
                if (x + y) % 5 < 2 {
                    diag.set(x, y, 0);
                }
            }
        }
        frames.extend([noise, stripes, diag]);
    }
    for img in &frames {
        let f = img.as_frame();
        for downscale in [0, 1, 2, 5] {
            let opts = LocateOptions {
                downscale,
                ..Default::default()
            };
            let _ = locate(&f, &opts);
        }
        let _ = anyd::pipeline::scan_all(&f);
        for angle in [0.0, 1.0, -1.5, 3.2, f32::NAN, f32::INFINITY, 1e9] {
            let _ = anyd::pipeline::scan_linear_at(&f, angle);
        }
        let _ = anyd::scan1d::refine_axis(&f, 45.0);
    }
}
