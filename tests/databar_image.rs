//! GS1 DataBar through the image pipeline: encode a symbol, draw it, and read it back
//! from the pixels alone with [`anyd::pipeline`].
//!
//! The structural decoder is given a module pattern and knows which variant it is
//! looking at; the pipeline has to find the symbol by its finder patterns, tell the
//! variants apart by where the halves or blocks lie, and still return the very symbol
//! that was encoded — symbology, payload and metadata — so that it re-encodes to the
//! same modules.
#![cfg(all(
    feature = "decode",
    feature = "encode",
    feature = "scan",
    feature = "databar",
    feature = "code128",
    feature = "ean"
))]

use anyd::Symbol;
use anyd::codes::code128::Code128Encoder;
use anyd::codes::databar::DataBarEncoder;
use anyd::codes::ean::EanEncoder;
use anyd::image::GrayImage;
use anyd::output::Encoding;
use anyd::pipeline::{scan_1d, scan_all};
use anyd::symbology::Symbology;
use anyd::traits::Encode;
use anyd::transform::{Rng, add_noise, gaussian_blur, rotate};

/// Light margin drawn around a symbol, in modules.
const MARGIN: usize = 8;

/// Draw `encoding` at `scale` pixels per module with `margin` light modules around
/// it. The abstract encoding makes every row one module tall; here they get their
/// printed heights: 33 modules for a linear symbol or a row of a Stacked
/// Omnidirectional or Expanded Stacked one, 5 and 7 for the two rows of a Stacked one,
/// and 1 for every separator row.
fn draw(symbol: &Symbol, encoding: &Encoding, scale: usize, margin: usize) -> GrayImage {
    let (width, rows): (usize, Vec<(Vec<bool>, usize)>) = match encoding {
        Encoding::Linear(p) => (p.modules.len(), vec![(p.modules.clone(), 33)]),
        Encoding::Matrix(m) => {
            let stacked = symbol.symbology == Symbology::DataBarStacked;
            let rows = (0..m.height())
                .map(|y| {
                    let row = (0..m.width()).map(|x| m.get(x, y)).collect();
                    let height = match (stacked, y) {
                        (true, 0) => 5,
                        (true, 2) => 7,
                        (false, y) if y % 4 == 0 => 33,
                        _ => 1,
                    };
                    (row, height)
                })
                .collect();
            (m.width(), rows)
        }
    };
    let height: usize = rows.iter().map(|(_, h)| h).sum();
    let mut img = GrayImage::filled(
        (width + 2 * margin) * scale,
        (height + 2 * margin) * scale,
        255,
    );
    let mut top = margin * scale;
    for (row, h) in &rows {
        for (x, _) in row.iter().enumerate().filter(|(_, dark)| **dark) {
            for py in top..top + h * scale {
                for px in (x + margin) * scale..(x + margin + 1) * scale {
                    img.set(px, py, 0);
                }
            }
        }
        top += h * scale;
    }
    img
}

fn drawn(symbol: &Symbol, scale: usize) -> GrayImage {
    let encoding = DataBarEncoder::new().encode(symbol).unwrap();
    draw(symbol, &encoding, scale, MARGIN)
}

/// Assert that `found` is exactly `symbol`, located inside `image`.
fn assert_reads(found: &[Symbol], symbol: &Symbol, image: &GrayImage, what: &str) {
    assert_eq!(found.len(), 1, "{what}: read {found:?}");
    let got = &found[0];
    assert_eq!(got.symbology, symbol.symbology, "{what}: symbology");
    assert_eq!(got.segments, symbol.segments, "{what}: segments");
    assert_eq!(got.meta, symbol.meta, "{what}: metadata");
    let enc = DataBarEncoder::new();
    assert_eq!(
        enc.encode(got).unwrap(),
        enc.encode(symbol).unwrap(),
        "{what}: re-encoding differs"
    );
    let location = got.location.as_ref().expect("a location");
    let (min, max) = location.outline.bounds();
    let (w, h) = (image.width() as f32, image.height() as f32);
    // The outline is a box along the reading axis, so on a rotated symbol its
    // corners may poke a little past the bars.
    let slack = 0.05 * w.max(h);
    assert!(
        min.x >= -slack && min.y >= -slack && max.x <= w + slack && max.y <= h + slack,
        "{what}: outline {location:?} outside the {w}x{h} image"
    );
    assert!(
        max.x - min.x > 0.3 * w.min(h) || max.y - min.y > 0.3 * w.min(h),
        "{what}: outline {location:?} does not span the symbol"
    );
    assert!(location.module_size.is_some_and(|m| m > 0.5));
}

const GTINS: &[&[u8]] = &[
    b"0441234567890",
    b"0200123456789",
    b"0001234567890",
    b"9501101530003",
    b"1234567890123",
    b"0082193510642",
];

/// Reduced element strings: `(01)` with a weight, with a weight and a date, a batch
/// with a variable-length field before a date, the shortest symbol there is, letters,
/// a long symbol, and punctuation.
const ELEMENT_STRINGS: &[&[u8]] = &[
    b"0198898765432106310300175",
    b"0190012345678908310301223315991231",
    b"1012A\x1d15991231",
    b"101",
    b"10ABC123\x1d2198765",
    b"019889876543210615991231310300175010 12A",
    b"0112345678901231101098-1234\x1d17260101",
];

fn element_strings() -> impl Iterator<Item = &'static [u8]> {
    ELEMENT_STRINGS.iter().copied()
}

#[test]
fn omnidirectional_through_image() {
    let enc = DataBarEncoder::new();
    for gtin in GTINS {
        let symbol = enc.build_omni(gtin).unwrap();
        for scale in [2, 3, 5] {
            let image = drawn(&symbol, scale);
            let found = scan_all(&image.as_frame());
            let what = format!("omni {} x{scale}", String::from_utf8_lossy(gtin));
            assert_reads(&found, &symbol, &image, &what);
        }
    }
}

#[test]
fn stacked_through_image() {
    let enc = DataBarEncoder::new();
    for gtin in GTINS {
        let symbol = enc.build_stacked(gtin).unwrap();
        // The top row is five modules tall: it takes a few pixels per module for a
        // couple of scan lines to cross it.
        for scale in [6, 8] {
            let image = drawn(&symbol, scale);
            let found = scan_all(&image.as_frame());
            let what = format!("stacked {} x{scale}", String::from_utf8_lossy(gtin));
            assert_reads(&found, &symbol, &image, &what);
        }
    }
}

#[test]
fn stacked_omnidirectional_through_image() {
    let enc = DataBarEncoder::new();
    for gtin in GTINS {
        let symbol = enc.build_stacked_omni(gtin).unwrap();
        for scale in [2, 3, 5] {
            let image = drawn(&symbol, scale);
            let found = scan_all(&image.as_frame());
            let what = format!("stacked omni {} x{scale}", String::from_utf8_lossy(gtin));
            assert_reads(&found, &symbol, &image, &what);
        }
    }
}

#[test]
fn expanded_through_image() {
    let enc = DataBarEncoder::new();
    for gs1 in element_strings() {
        let symbol = enc.build_expanded(gs1).unwrap();
        for scale in [2, 3] {
            let image = drawn(&symbol, scale);
            let found = scan_all(&image.as_frame());
            let what = format!("expanded {:?} x{scale}", String::from_utf8_lossy(gs1));
            assert_reads(&found, &symbol, &image, &what);
        }
    }
}

#[test]
fn expanded_stacked_through_image() {
    let enc = DataBarEncoder::new();
    for gs1 in element_strings() {
        // Even column counts alternate the reading direction of their rows; odd
        // ones read every row left to right.
        for columns in [1, 2, 3, 4] {
            let symbol = enc.build_expanded_stacked(gs1, columns).unwrap();
            let encoding = enc.encode(&symbol).unwrap();
            let Encoding::Matrix(m) = &encoding else {
                panic!("stacked symbols are matrices");
            };
            // A symbol that fits one row *is* the linear symbol, and reads as one.
            let expect = if m.height() == 1 {
                enc.build_expanded(gs1).unwrap()
            } else {
                symbol.clone()
            };
            let image = draw(&symbol, &encoding, 3, MARGIN);
            let found = scan_all(&image.as_frame());
            let what = format!(
                "expanded stacked {:?} in {columns} columns",
                String::from_utf8_lossy(gs1)
            );
            assert_eq!(found.len(), 1, "{what}: read {found:?}");
            assert_eq!(found[0].symbology, expect.symbology, "{what}: symbology");
            assert_eq!(found[0].segments, expect.segments, "{what}: segments");
            assert_eq!(found[0].meta, expect.meta, "{what}: metadata");
            assert_eq!(
                enc.encode(&found[0]).unwrap(),
                enc.encode(&expect).unwrap(),
                "{what}: re-encoding differs"
            );
        }
    }
}

/// A photograph is neither level nor sharp nor clean.
#[test]
fn reads_tilted_blurred_noisy_captures() {
    let enc = DataBarEncoder::new();
    let symbols = [
        enc.build_omni(GTINS[0]).unwrap(),
        enc.build_stacked_omni(GTINS[1]).unwrap(),
        enc.build_expanded(ELEMENT_STRINGS[1]).unwrap(),
        enc.build_expanded_stacked(ELEMENT_STRINGS[1], 2).unwrap(),
    ];
    let mut rng = Rng::new(0xdb);
    for symbol in &symbols {
        for degrees in [-4.0f32, 3.0] {
            let image = drawn(symbol, 4);
            let image = rotate(&image, degrees.to_radians());
            let image = gaussian_blur(&image, 1.2);
            let image = add_noise(&image, 6.0, &mut rng);
            let found = scan_all(&image.as_frame());
            let what = format!("{:?} at {degrees}°", symbol.symbology);
            assert_reads(&found, symbol, &image, &what);
        }
    }
}

/// The reading axis is found by the pipeline: a symbol on its side or upside down
/// reads all the same.
#[test]
fn reads_at_any_orientation() {
    let enc = DataBarEncoder::new();
    let symbols = [
        enc.build_omni(GTINS[3]).unwrap(),
        enc.build_stacked_omni(GTINS[3]).unwrap(),
        enc.build_expanded(ELEMENT_STRINGS[0]).unwrap(),
        enc.build_expanded_stacked(ELEMENT_STRINGS[1], 2).unwrap(),
    ];
    for symbol in &symbols {
        for degrees in [90.0f32, 180.0, 270.0, 35.0] {
            let image = rotate(&drawn(symbol, 4), degrees.to_radians());
            let found = scan_1d(&image.as_frame());
            let what = format!("{:?} at {degrees}°", symbol.symbology);
            assert_reads(&found, symbol, &image, &what);
        }
    }
}

/// DataBar needs no quiet zone: a symbol cropped to its outermost bars still reads.
#[test]
fn reads_without_a_quiet_zone() {
    let enc = DataBarEncoder::new();
    let symbols = [
        enc.build_omni(GTINS[2]).unwrap(),
        enc.build_expanded(ELEMENT_STRINGS[0]).unwrap(),
        enc.build_expanded_stacked(ELEMENT_STRINGS[1], 2).unwrap(),
    ];
    for symbol in &symbols {
        let encoding = enc.encode(symbol).unwrap();
        let image = draw(symbol, &encoding, 3, 0);
        let found = scan_all(&image.as_frame());
        let what = format!("{:?} without margins", symbol.symbology);
        assert_eq!(found.len(), 1, "{what}: read {found:?}");
        assert_eq!(found[0].symbology, symbol.symbology, "{what}");
        assert_eq!(found[0].segments, symbol.segments, "{what}");
        assert_eq!(found[0].meta, symbol.meta, "{what}");
    }
}

fn is_databar(symbol: &Symbol) -> bool {
    matches!(
        symbol.symbology,
        Symbology::DataBarOmni
            | Symbology::DataBarLimited
            | Symbology::DataBarStacked
            | Symbology::DataBarStackedOmni
            | Symbology::DataBarExpanded
            | Symbology::DataBarExpandedStacked
    )
}

/// Half a symbol is not a symbol: with either half of an Omnidirectional symbol, or
/// any block of an Expanded one, painted over, nothing is read.
#[test]
fn incomplete_symbols_do_not_read() {
    let enc = DataBarEncoder::new();
    let scale = 4;
    let blank = |image: &GrayImage, from: usize, to: usize| {
        let mut image = image.clone();
        for y in 0..image.height() {
            for x in from * scale..to * scale {
                image.set(x, y, 255);
            }
        }
        image
    };
    let omni = enc.build_omni(GTINS[0]).unwrap();
    let image = drawn(&omni, scale);
    // 96 modules: the halves meet at 48.
    for (from, to) in [(MARGIN, MARGIN + 40), (MARGIN + 56, MARGIN + 96)] {
        let found = scan_all(&blank(&image, from, to).as_frame());
        assert!(found.is_empty(), "half an Omnidirectional read {found:?}");
    }
    let expanded = enc.build_expanded(ELEMENT_STRINGS[1]).unwrap();
    let image = drawn(&expanded, scale);
    let blocks = (image.width() / scale - 2 * MARGIN) / 49;
    assert!(blocks >= 3);
    for block in 0..blocks {
        let from = MARGIN + 2 + 49 * block;
        let found = scan_all(&blank(&image, from, from + 49).as_frame());
        assert!(
            found.is_empty(),
            "an Expanded symbol without block {block} read {found:?}"
        );
    }
}

/// Other bars are not DataBar: no DataBar symbol is reported beside a Code 128 or an
/// EAN-13, from bars drawn at random, or from a DataBar Limited symbol (which has no
/// finder pattern of this kind and is not read from images).
#[test]
fn other_bars_are_not_databar() {
    let mut images: Vec<(String, GrayImage)> = Vec::new();
    let c128 = Code128Encoder::new();
    for text in ["ABC-1234", "0123456789012345678901", "DataBar? No."] {
        let encoding = c128.encode(&c128.build_text(text).unwrap()).unwrap();
        images.push((
            format!("code128 {text}"),
            anyd::render::render(&encoding, 3),
        ));
    }
    let ean = EanEncoder::new();
    for digits in ["5901234123457", "4006381333931", "9780201379624"] {
        let encoding = ean.encode(&ean.build_ean13(digits).unwrap()).unwrap();
        images.push((
            format!("ean13 {digits}"),
            anyd::render::render(&encoding, 3),
        ));
    }
    let enc = DataBarEncoder::new();
    for gtin in GTINS.iter().filter(|g| g[0] <= b'1') {
        let symbol = enc.build_limited(gtin).unwrap();
        images.push((
            format!("limited {}", String::from_utf8_lossy(gtin)),
            drawn(&symbol, 3),
        ));
    }
    // Random bars one to nine modules wide, the widths DataBar itself uses.
    let mut rng = Rng::new(7);
    for n in 0..40 {
        let scale = 2 + n % 3;
        let mut image = GrayImage::filled(900, 120, 255);
        let (mut x, mut dark) = (10usize, true);
        while x < 880 {
            let w = scale * (1 + (rng.next_u64() % 9) as usize);
            if dark {
                for y in 10..110 {
                    for px in x..(x + w).min(890) {
                        image.set(px, y, 0);
                    }
                }
            }
            x += w;
            dark = !dark;
        }
        images.push((format!("random bars {n}"), image));
    }
    for (what, image) in &images {
        let found = scan_all(&image.as_frame());
        assert!(
            !found.iter().any(is_databar),
            "{what}: read {:?}",
            found.iter().filter(|s| is_databar(s)).collect::<Vec<_>>()
        );
    }
}
