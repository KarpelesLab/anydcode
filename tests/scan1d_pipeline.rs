//! End-to-end 1D image pipeline: encode a linear symbol, rasterize it, then recover
//! it purely from pixels through the generic `scan1d` front-end feeding each
//! symbology's real decoder. This exercises the render → scan → decode path across
//! three independently-implemented modules, not just structural round-trips.
#![cfg(all(
    feature = "decode",
    feature = "encode",
    feature = "scan",
    feature = "code128",
    feature = "code39",
    feature = "ean"
))]

use anyd::codes::code39::{Code39Decoder, Code39Encoder};
use anyd::codes::code128::{Code128Decoder, Code128Encoder};
use anyd::codes::ean::{EanDecoder, EanEncoder};
use anyd::output::Encoding;
use anyd::render::render;
use anyd::scan1d::{ScanOptions, scan_lines, try_decode};
use anyd::traits::{Decode, Encode};

/// Render `encoding` at `scale` px/module, scan it with `scan1d`, and return the
/// first scanline candidate that `decoder` accepts.
fn scan_and_decode(encoding: &Encoding, scale: usize, decoder: &dyn Decode) -> Option<String> {
    let image = render(encoding, scale);
    let frame = image.as_frame();
    let candidates = scan_lines(&frame, &ScanOptions::default());
    for candidate in &candidates {
        if let Some(sym) = try_decode(candidate, decoder) {
            return Some(sym.text().unwrap_or_default());
        }
    }
    None
}

#[test]
fn code128_through_image() {
    let symbol = Code128Encoder::new().build_text("ABC-1234").unwrap();
    let encoding = Code128Encoder::new().encode(&symbol).unwrap();
    let decoded = scan_and_decode(&encoding, 6, &Code128Decoder::new());
    assert_eq!(decoded.as_deref(), Some("ABC-1234"));
}

#[test]
fn code39_through_image() {
    let enc = Code39Encoder::new();
    let symbol = enc.build(b"HELLO 123", false, false).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    // Code 39 has no in-band full-ascii/check flags; decode with the matching config.
    let decoded = scan_and_decode(&encoding, 6, &Code39Decoder::new());
    assert_eq!(decoded.as_deref(), Some("HELLO 123"));
}

#[test]
fn ean13_through_image() {
    let enc = EanEncoder::new();
    let symbol = enc.build_ean13("5901234123457").unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = scan_and_decode(&encoding, 6, &EanDecoder::new());
    // EAN payload is the full 13-digit GTIN incl. check digit.
    assert_eq!(decoded.as_deref(), Some("5901234123457"));
}

/// A picture cropped to the bars — no quiet zone, the first and last bars touching
/// the frame's edge — reads as a still image, and not as a crop of a live frame, whose
/// edge may have cut a longer symbol short.
#[test]
fn still_image_cropped_to_the_bars() {
    use anyd::codes::code39::Code39Encoder;
    use anyd::codes::code128::Code128Encoder;
    use anyd::output::Encoding;
    use anyd::traits::Encode;
    let c128 = Code128Encoder::new();
    let c39 = Code39Encoder::new();
    let symbols = [
        c128.encode(&c128.build_text("10064908").unwrap()).unwrap(),
        c39.encode(&c39.build(b"TEST-SHEET", false, false).unwrap())
            .unwrap(),
    ];
    for (encoding, text) in symbols.iter().zip(["10064908", "TEST-SHEET"]) {
        let Encoding::Linear(pattern) = encoding else {
            panic!("a linear symbol");
        };
        // Bars only: three pixels a module, forty rows, nothing around them.
        let modules = &pattern.modules;
        let (w, h) = (modules.len() * 3, 40);
        let mut pixels = vec![255u8; w * h];
        for y in 0..h {
            for x in 0..w {
                if modules[x / 3] {
                    pixels[y * w + x] = 0;
                }
            }
        }
        let frame = anyd::GrayFrame::new(&pixels, w, h).unwrap();
        assert!(
            anyd::pipeline::scan_all(&frame).is_empty(),
            "{text}: read as a crop"
        );
        let found = anyd::pipeline::scan_image(&frame);
        assert_eq!(found.len(), 1, "{text}");
        assert_eq!(found[0].text().as_deref(), Some(text));
        // The outline stays inside the picture.
        let outline = found[0].location.as_ref().expect("a location").outline;
        for p in outline.corners {
            assert!(
                p.x >= 0.0 && p.x < w as f32 && p.y >= 0.0 && p.y < h as f32,
                "{p:?}"
            );
        }
    }
}
