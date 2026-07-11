//! GS1 DataBar end-to-end tests.
//!
//! Two kinds of check:
//! 1. **Independent reference vectors** — the exact module pattern for a known
//!    GTIN, taken from BWIPP (`postscriptbarcode`, an implementation independent
//!    of this crate's zint-derived algorithm), rendered at 1 pixel per module.
//!    `1` = dark module, `0` = light, left to right.
//! 2. **Lossless round-trip** — encode -> decode -> re-encode must reproduce the
//!    exact segments, metadata and module pattern.

use anyd::codes::databar::{DataBarDecoder, DataBarEncoder, DataBarMeta, DataBarVariant};
use anyd::output::Encoding;
use anyd::segment::{Mode, Segment};
use anyd::symbol::SymbolMeta;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

/// Render an encoding to a `1`/`0` module string (dark = `1`).
fn bits(encoding: &Encoding) -> String {
    let Encoding::Linear(p) = encoding else {
        panic!("expected a linear pattern");
    };
    p.modules
        .iter()
        .map(|&d| if d { '1' } else { '0' })
        .collect()
}

// ---- Independent reference vectors (BWIPP `databaromni`, 96 modules) ----

#[test]
fn omni_reference_vectors() {
    let enc = DataBarEncoder::new();
    // (GTIN-13 body, BWIPP-rendered 96-module pattern).
    let cases: &[(&[u8], &str)] = &[
        (
            b"0000000000000",
            "010101001000000001000111111110010111111100101010101010110000000101111111110111011111111011010101",
        ),
        (
            b"2001234567890",
            "010100011101000001001111111000010100110110111110110000010010100101100000000111000110110110001101",
        ),
        (
            b"0095011015300",
            "010100100000001001000100000000010100111011110110110100110111100101111111000111011110111101101101",
        ),
    ];
    for (gtin, expected) in cases {
        let symbol = enc.build_omni(gtin).unwrap();
        let encoding = enc.encode(&symbol).unwrap();
        assert_eq!(&bits(&encoding), expected, "omni mismatch for {gtin:?}");
    }
}

// ---- Independent reference vectors (BWIPP `databarlimited`, 79 modules) ----

#[test]
fn limited_reference_vectors() {
    let enc = DataBarEncoder::new();
    let cases: &[(&[u8], &str)] = &[
        (
            b"0000000000000",
            "0101010101010000001000000111010111010100100101010101010100000010000001110100000",
        ),
        (
            b"0015133531096",
            "0101010011000001000001011001010101011001100101011010110001011011000111110100000",
        ),
        (
            b"1234567890128",
            "0100110011110010100010011101011101010101000101001010100101100000111100010100000",
        ),
    ];
    for (gtin, expected) in cases {
        let symbol = enc.build_limited(gtin).unwrap();
        let encoding = enc.encode(&symbol).unwrap();
        assert_eq!(&bits(&encoding), expected, "limited mismatch for {gtin:?}");
    }
}

// ---- Canonical form: build stores a 14-digit GTIN with computed check digit ----

#[test]
fn build_appends_check_digit() {
    let enc = DataBarEncoder::new();
    let symbol = enc.build_omni(b"2001234567890").unwrap();
    assert_eq!(symbol.segments.len(), 1);
    assert_eq!(symbol.segments[0].mode, Mode::Numeric);
    assert_eq!(symbol.segments[0].data, b"20012345678909");
    assert_eq!(
        symbol.meta,
        SymbolMeta::DataBar(DataBarMeta::new(DataBarVariant::Omni))
    );
}

#[test]
fn build_accepts_full_14_digit_gtin() {
    let enc = DataBarEncoder::new();
    // Passing the full 14-digit GTIN (with correct check digit) is accepted and
    // yields the identical symbol as the 13-digit body.
    let a = enc.build_omni(b"2001234567890").unwrap();
    let b = enc.build_omni(b"20012345678909").unwrap();
    assert_eq!(a.segments, b.segments);
    assert_eq!(enc.encode(&a).unwrap(), enc.encode(&b).unwrap());
}

// ---- Lossless round-trips ----

fn assert_omni_roundtrip(gtin13: u64) {
    let enc = DataBarEncoder::new();
    let dec = DataBarDecoder::new();
    let body = format!("{gtin13:013}");
    let symbol = enc.build_omni(body.as_bytes()).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = dec.decode(&encoding).unwrap();
    assert_eq!(decoded.symbology, Symbology::DataBarOmni);
    assert_eq!(
        decoded.segments, symbol.segments,
        "segments differ ({body})"
    );
    assert_eq!(decoded.meta, symbol.meta, "meta differs ({body})");
    assert_eq!(
        enc.encode(&decoded).unwrap(),
        encoding,
        "re-encode not identical ({body})"
    );
}

fn assert_limited_roundtrip(gtin13: u64) {
    let enc = DataBarEncoder::new();
    let dec = DataBarDecoder::new();
    let body = format!("{gtin13:013}");
    let symbol = enc.build_limited(body.as_bytes()).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = dec.decode(&encoding).unwrap();
    assert_eq!(decoded.symbology, Symbology::DataBarLimited);
    assert_eq!(
        decoded.segments, symbol.segments,
        "segments differ ({body})"
    );
    assert_eq!(decoded.meta, symbol.meta, "meta differs ({body})");
    assert_eq!(
        enc.encode(&decoded).unwrap(),
        encoding,
        "re-encode not identical ({body})"
    );
}

#[test]
fn omni_roundtrip_boundaries() {
    // Values around the pair/character split boundaries and the extremes.
    for v in [
        0,
        1,
        1596,
        1597,
        1598,
        4_537_076,
        4_537_077,
        4_537_078,
        1_000_000_000_000,
        9_999_999_999_999,
    ] {
        assert_omni_roundtrip(v);
    }
}

#[test]
fn omni_roundtrip_sweep() {
    // A broad sweep exercises every group / finder combination indirectly: if
    // `get_value` were not an exact inverse, the decoded GTIN would differ.
    let mut v: u64 = 7;
    for _ in 0..4000 {
        assert_omni_roundtrip(v % 10_000_000_000_000);
        v = v.wrapping_mul(1_000_003).wrapping_add(97) % 9_999_999_999_999;
    }
}

#[test]
fn limited_roundtrip_boundaries() {
    for v in [
        0,
        1,
        2_013_570,
        2_013_571,
        2_013_572,
        1_000_000_000_000,
        1_999_999_999_999,
    ] {
        assert_limited_roundtrip(v);
    }
}

#[test]
fn limited_roundtrip_sweep() {
    let mut v: u64 = 3;
    for _ in 0..4000 {
        assert_limited_roundtrip(v % 2_000_000_000_000);
        v = v.wrapping_mul(1_000_003).wrapping_add(97) % 1_999_999_999_999;
    }
}

// ---- Error handling ----

#[test]
fn rejects_bad_check_digit() {
    let enc = DataBarEncoder::new();
    // 14 digits with a wrong check digit (correct is 9).
    assert!(enc.build_omni(b"20012345678900").is_err());
}

#[test]
fn rejects_wrong_length() {
    let enc = DataBarEncoder::new();
    assert!(enc.build_omni(b"123").is_err());
    assert!(enc.build_omni(b"123456789012345").is_err());
    assert!(enc.build_omni(b"abcdefghijklm").is_err());
}

#[test]
fn limited_rejects_high_indicator_digit() {
    let enc = DataBarEncoder::new();
    // Indicator digit must be 0 or 1 for Limited.
    assert!(enc.build_limited(b"2001234567890").is_err());
    assert!(enc.build_limited(b"1001234567890").is_ok());
}

#[test]
fn unsupported_variants_error() {
    let enc = DataBarEncoder::new();
    for sym in [
        Symbology::DataBarExpanded,
        Symbology::DataBarStacked,
        Symbology::DataBarStackedOmni,
        Symbology::DataBarExpandedStacked,
    ] {
        let symbol = anyd::symbol::Symbol::new(
            sym,
            vec![Segment::numeric(b"0000000000000".to_vec())],
            SymbolMeta::DataBar(DataBarMeta::new(DataBarVariant::Omni)),
        );
        assert!(matches!(
            enc.encode(&symbol),
            Err(anyd::error::Error::Unsupported { .. })
        ));
    }
}

#[test]
fn decode_rejects_corrupted_pattern() {
    let enc = DataBarEncoder::new();
    let dec = DataBarDecoder::new();
    let symbol = enc.build_omni(b"0095011015300").unwrap();
    let Encoding::Linear(mut p) = enc.encode(&symbol).unwrap() else {
        panic!("expected linear");
    };
    // Flip several interior modules to break the checksum / finder consistency.
    for i in [20, 21, 40, 41, 60] {
        p.modules[i] = !p.modules[i];
    }
    assert!(dec.decode(&Encoding::Linear(p)).is_err());
}
