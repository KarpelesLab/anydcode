//! End-to-end 1D image pipeline: encode a linear symbol, rasterize it, then recover
//! it purely from pixels through the generic `scan1d` front-end feeding each
//! symbology's real decoder. This exercises the render → scan → decode path across
//! three independently-implemented modules, not just structural round-trips.

use anydcode::codes::code39::{Code39Decoder, Code39Encoder};
use anydcode::codes::code128::{Code128Decoder, Code128Encoder};
use anydcode::codes::ean::{EanDecoder, EanEncoder};
use anydcode::output::Encoding;
use anydcode::render::render;
use anydcode::scan1d::{ScanOptions, scan_lines, try_decode};
use anydcode::traits::{Decode, Encode};

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
